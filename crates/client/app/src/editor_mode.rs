//! The client-owned map-editor workspace.
//!
//! `openshard-client-editor` owns the UI-independent catalogue, tools and draft;
//! this module joins them to egui and to the wire without letting either own the
//! authoritative map. The draft stays a sparse projection over the current
//! snapshot until the shard accepts it and the accepted revision arrives.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use openshard_client_editor::draft::Draft;
use openshard_client_editor::tools::{
    Brush, BrushRadius, BrushShape, HeightStrength, StaticHeight, StaticPlacement, TargetHeight, TilePoint,
    Tool,
};
use openshard_client_editor::{AssetId, Catalog, KindFilter, PaletteState};
use openshard_map::map::{StaticItem, WorldMap};
use openshard_map::patch::{PatchOp, StaticId};
use openshard_map::snapshot::MapRevision;
use openshard_protocol::access::AccessLevel;
use openshard_protocol::chunks::WorldRevision;
use openshard_protocol::mapedit::{
    EditLandTile, EditStaticId, EditTile, EditX, EditY, EditZ, MAX_EDIT_OPS, MapEditOp, MapEditOutcome,
    MapEditRefusal, MapEditReply, MapEditRequest,
};
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::Facet;
use openshard_uofiles::multi::{Component, Multis};
use serde::Deserialize;

/// The authority required before this client offers map editing controls.
pub(crate) const REQUIRED_AUTHORITY: AccessLevel = AccessLevel::GameMaster;

/// Whether the application is currently treating the world as an editable map.
///
/// Kept on [`crate::app::App`] rather than in egui memory: future brushes and
/// drafts must read the same mode as the panel, and closing a panel must not
/// silently end or begin an editing session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActiveTool {
    PaintLand,
    PlaceStatic,
    RemoveStatic,
    Raise,
    Lower,
    Flatten,
    PlaceHouse,
    RemoveHouse,
}

/// The two origins a house preview can have in the editor.
///
/// A classic multi is resolved by id from the client install.  A custom
/// template is already the list of components the installed art files draw;
/// keeping those variants distinct stops a locally loaded template from ever
/// being sent as an invented `multi_id` to the shard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HousePreview {
    Multi(u16),
    Design(Arc<[Component]>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HouseSelection {
    Multi(u16),
    Template(usize),
}

struct HouseTemplate {
    /// Stable server catalogue key: exactly the JSON file stem.
    key: String,
    /// Readable label made from that key for the editor list.
    name: String,
    components: Arc<[Component]>,
}

#[derive(Deserialize)]
struct TemplateFile {
    format: String,
    components: Vec<TemplateComponent>,
}

#[derive(Deserialize)]
struct TemplateComponent {
    graphic: u16,
    dx: i16,
    dy: i16,
    dz: i16,
    flags: u64,
}

/// The directory an operator populates with [`wsc_to_design`]'s JSON output.
///
/// It belongs beside the client files because map-editor catalogues are local
/// operator tools today; templates become shard-owned data only when the
/// placement/catalogue protocol is added.
const CUSTOM_HOUSE_DIRECTORY: &str = "openshard-houses";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommitState {
    Ready,
    Pending,
    Accepted(WorldRevision),
    Refused(MapEditRefusal),
}

/// One dirty tile as the world overlay draws it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PreviewTile {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) corners: [i8; 4],
}

struct PreviewTexture {
    image: egui::ColorImage,
    panel: egui::TextureHandle,
    overlay: Option<egui::TextureHandle>,
}

pub(crate) struct MapEditor {
    active: bool,
    catalogue: Option<Catalog>,
    catalogue_error: Option<String>,
    palette: PaletteState,
    matches: Vec<AssetId>,
    previews: BTreeMap<AssetId, Option<PreviewTexture>>,
    draft_static_assets: BTreeSet<AssetId>,
    tool: Option<ActiveTool>,
    brush: Brush,
    strength: u8,
    flatten: i8,
    draft: Option<Draft>,
    commit: CommitState,
    message: Option<String>,
    multis: Option<std::sync::Arc<Multis>>,
    house_search: String,
    house_matches: Vec<u16>,
    selected_house: Option<HouseSelection>,
    house_previews: BTreeMap<u16, Option<egui::TextureHandle>>,
    templates: Vec<HouseTemplate>,
    template_error: Option<String>,
    template_previews: BTreeMap<usize, Option<egui::TextureHandle>>,
}

impl MapEditor {
    /// Open the install-backed catalogue and start in ordinary play mode.
    pub(crate) fn open(client_dir: &Path, multis: Option<std::sync::Arc<Multis>>) -> Self {
        let mut editor = match Catalog::open(client_dir) {
            Ok(catalogue) => Self::with_catalogue(Some(catalogue), None),
            Err(error) => Self::with_catalogue(None, Some(error.to_string())),
        };
        editor.multis = multis;
        match load_house_templates(&client_dir.join(CUSTOM_HOUSE_DIRECTORY)) {
            Ok(templates) => editor.templates = templates,
            Err(error) => editor.template_error = Some(error),
        }
        editor.refresh_house_matches();
        editor
    }

    fn with_catalogue(catalogue: Option<Catalog>, catalogue_error: Option<String>) -> Self {
        let palette = PaletteState::default();
        let matches = catalogue
            .as_ref()
            .map_or_else(Vec::new, |catalogue| catalogue.matching(&palette));
        Self {
            active: false,
            catalogue,
            catalogue_error,
            palette,
            matches,
            previews: BTreeMap::new(),
            draft_static_assets: BTreeSet::new(),
            tool: Some(ActiveTool::PaintLand),
            brush: Brush::default(),
            strength: 1,
            flatten: 0,
            draft: None,
            commit: CommitState::Ready,
            message: None,
            multis: None,
            house_search: String::new(),
            house_matches: Vec::new(),
            selected_house: None,
            house_previews: BTreeMap::new(),
            templates: Vec::new(),
            template_error: None,
            template_previews: BTreeMap::new(),
        }
    }

    /// Whether `authority` is allowed to see and enter the editor.
    pub(crate) fn available_to(authority: AccessLevel) -> bool {
        authority.allows(REQUIRED_AUTHORITY)
    }

    /// Whether editor mode is active.
    pub(crate) const fn active(&self) -> bool {
        self.active
    }

    /// Apply a UI request under the shard's current authority.
    ///
    /// Refusing an activation locally is only presentation hardening; the
    /// server must independently authorize every eventual map commit.
    pub(crate) fn set_active(&mut self, requested: bool, authority: AccessLevel) {
        self.active = requested && Self::available_to(authority);
    }

    /// Leave editor mode immediately when the authoritative view no longer
    /// grants it. A missing view is represented by `Player` at the call site,
    /// so login, disconnect and authority loss all fail in the same direction.
    pub(crate) fn reconcile(&mut self, authority: AccessLevel) {
        if !Self::available_to(authority) {
            self.active = false;
        }
    }

    /// Whether the active tool needs a static rather than a ground coordinate.
    pub(crate) const fn removes_static(&self) -> bool {
        matches!(self.tool, Some(ActiveTool::RemoveStatic))
    }

    /// The classic multi the next editor click places as a real house entity.
    #[must_use]
    pub(crate) const fn selected_multi(&self) -> Option<u16> {
        match (self.tool, self.selected_house) {
            (Some(ActiveTool::PlaceHouse), Some(HouseSelection::Multi(multi))) => Some(multi),
            _ => None,
        }
    }

    /// What the editor should draw under the pointer for its current house
    /// selection.  Template geometry is reference-counted: rebuilding a ghost
    /// every frame must not clone a thousand-piece imported building.
    pub(crate) fn selected_house_preview(&self) -> Option<HousePreview> {
        if self.tool != Some(ActiveTool::PlaceHouse) {
            return None;
        }
        match self.selected_house? {
            HouseSelection::Multi(multi) => Some(HousePreview::Multi(multi)),
            HouseSelection::Template(index) => self
                .templates
                .get(index)
                .map(|template| HousePreview::Design(Arc::clone(&template.components))),
        }
    }

    /// The imported template the next editor click asks the shard to place.
    ///
    /// Its file stem is an operator-controlled, command-safe catalogue key; it
    /// is never conflated with a `multi.mul` id.
    #[must_use]
    pub(crate) fn selected_template_name(&self) -> Option<&str> {
        match (self.tool, self.selected_house) {
            (Some(ActiveTool::PlaceHouse), Some(HouseSelection::Template(index))) => {
                self.templates.get(index).map(|template| template.key.as_str())
            }
            _ => None,
        }
    }

    /// Whether the next editor click should name a house entity for demolition.
    pub(crate) const fn removes_house(&self) -> bool {
        matches!(self.tool, Some(ActiveTool::RemoveHouse))
    }

    /// Put away the current tool while leaving map-editor mode and its draft
    /// open. Returns whether there was a tool to put away.
    pub(crate) fn cancel_tool(&mut self) -> bool {
        self.tool.take().is_some()
    }

    fn refresh_house_matches(&mut self) {
        let query = self.house_search.trim().to_ascii_lowercase();
        self.house_matches = self.multis.as_deref().map_or_else(Vec::new, |multis| {
            multis
                .iter()
                // `multi.mul` has no semantic kind. Its low ids are ships;
                // 0x64 is the first classic house and the housing examples'
                // own boundary.
                .filter(|multi| multi.id >= 0x0064)
                .filter(|multi| {
                    query.is_empty()
                        || format!("{:#06x}", multi.id).contains(&query)
                        || multi.id.to_string().contains(&query)
                })
                .map(|multi| multi.id)
                .collect()
        });
    }

    fn select_asset(&mut self, id: AssetId) {
        self.palette.select(id);
        self.tool = Some(match id {
            AssetId::Land(_) => ActiveTool::PaintLand,
            AssetId::Static(_) => ActiveTool::PlaceStatic,
        });
        self.message = None;
    }

    /// Resolve the topmost visible matching static to its preview ordinal.
    pub(crate) fn static_ordinal(
        &self,
        map: &WorldMap,
        x: u16,
        y: u16,
        graphic: Graphic,
        z: i8,
    ) -> Option<StaticId> {
        let standing = self.draft.as_ref().map_or_else(
            || map.statics_at(x, y).copied().collect(),
            |draft| draft.statics_at(map, x, y),
        );
        standing
            .iter()
            .rposition(|item| item.tile == graphic && item.z == z)
            .and_then(|index| u16::try_from(index).ok())
            .map(StaticId)
    }

    /// Apply one click through the active tool into the local draft.
    pub(crate) fn apply_at(
        &mut self,
        map: &WorldMap,
        facet: Facet,
        revision: MapRevision,
        at: TilePoint,
        static_id: Option<StaticId>,
    ) {
        if matches!(self.commit, CommitState::Pending | CommitState::Accepted(_)) {
            self.message = Some("wait for the current commit to finish".to_owned());
            return;
        }
        let Some(tool) = self.tool(static_id) else {
            return;
        };
        let draft = self.draft.get_or_insert_with(|| Draft::new(facet, revision));
        if let Some(conflict) = draft.conflict_with(facet, revision) {
            self.message = Some(conflict.to_string());
            return;
        }
        let mut gesture = draft.gesture(map);
        if let Err(error) = gesture.apply(tool, self.brush, at) {
            self.message = Some(error.to_string());
            return;
        }
        match draft.apply_gesture(map, gesture.finish()) {
            Ok(_) => {
                if let Tool::PlaceStatic(placement) = tool {
                    self.draft_static_assets.insert(AssetId::Static(placement.tile));
                }
                self.commit = CommitState::Ready;
                self.message = None;
            }
            Err(error) => self.message = Some(error.to_string()),
        }
    }

    fn tool(&mut self, static_id: Option<StaticId>) -> Option<Tool> {
        match self.tool? {
            ActiveTool::PaintLand => match self.palette.selected() {
                Some(AssetId::Land(tile)) => Some(Tool::PaintLand(tile)),
                _ => {
                    self.message = Some("select a land tile first".to_owned());
                    None
                }
            },
            ActiveTool::PlaceStatic => match self.palette.selected() {
                Some(AssetId::Static(tile)) => Some(Tool::PlaceStatic(StaticPlacement {
                    tile,
                    height: StaticHeight::OnGround,
                    hue: Hue::NONE,
                })),
                _ => {
                    self.message = Some("select a static first".to_owned());
                    None
                }
            },
            ActiveTool::RemoveStatic => match static_id {
                Some(which) => Some(Tool::RemoveStatic(which)),
                None => {
                    self.message = Some("point at a map static to remove it".to_owned());
                    None
                }
            },
            ActiveTool::Raise => Some(Tool::Raise(
                HeightStrength::new(self.strength).expect("the editor slider excludes zero"),
            )),
            ActiveTool::Lower => Some(Tool::Lower(
                HeightStrength::new(self.strength).expect("the editor slider excludes zero"),
            )),
            ActiveTool::Flatten => Some(Tool::Flatten(TargetHeight(self.flatten))),
            ActiveTool::PlaceHouse | ActiveTool::RemoveHouse => None,
        }
    }

    /// Build the bounded wire request and enter the pending state.
    pub(crate) fn commit_request(
        &mut self,
        map: &WorldMap,
        facet: Facet,
        revision: MapRevision,
    ) -> Option<MapEditRequest> {
        let Some(draft) = self.draft.as_ref() else {
            self.message = Some("the draft is empty".to_owned());
            return None;
        };
        if let Some(conflict) = draft.conflict_with(facet, revision) {
            self.message = Some(conflict.to_string());
            return None;
        }
        let ops = draft.commit_ops(map);
        if ops.is_empty() {
            self.message = Some("the draft is empty".to_owned());
            return None;
        }
        if ops.len() > usize::from(MAX_EDIT_OPS) {
            self.message = Some(format!(
                "the draft has {} operations; one commit is limited to {MAX_EDIT_OPS}",
                ops.len()
            ));
            return None;
        }
        let ops = ops.into_iter().map(wire_op).collect();
        self.commit = CommitState::Pending;
        self.message = None;
        Some(MapEditRequest {
            facet,
            parent: WorldRevision(revision.get()),
            ops,
        })
    }

    /// Fold the shard's one answer to the pending commit.
    pub(crate) fn on_reply(&mut self, reply: MapEditReply) {
        let Some(draft) = self.draft.as_ref() else {
            return;
        };
        if reply.facet != draft.facet() {
            return;
        }
        self.commit = match reply.outcome {
            MapEditOutcome::Accepted => CommitState::Accepted(reply.revision),
            MapEditOutcome::Refused(reason) => CommitState::Refused(reason),
        };
    }

    /// Retire an accepted draft once the authoritative ground reaches it.
    pub(crate) fn ground_at(&mut self, facet: Facet, revision: MapRevision) {
        if matches!(self.commit, CommitState::Accepted(accepted) if accepted.0 <= revision.get())
            && self.draft.as_ref().is_some_and(|draft| draft.facet() == facet)
        {
            self.draft = None;
            self.draft_static_assets.clear();
            self.commit = CommitState::Ready;
            self.message = Some(format!("committed revision {}", revision.get()));
        } else if self
            .draft
            .as_ref()
            .is_some_and(|draft| draft.conflict_with(facet, revision).is_some())
        {
            self.commit = CommitState::Refused(MapEditRefusal::Conflict);
            self.message = Some("the shard moved; discard or rebuild the draft".to_owned());
        }
    }

    /// Geometry for the sparse preview overlay.
    pub(crate) fn preview_tiles(&self, map: &WorldMap) -> Vec<PreviewTile> {
        if !self.active {
            return Vec::new();
        }
        let Some(draft) = self.draft.as_ref() else {
            return Vec::new();
        };
        draft
            .dirty_tiles()
            .iter()
            .map(|at| {
                let own = draft
                    .land(map, at.x, at.y)
                    .expect("a dirty tile is on the preview")
                    .z;
                let z = |x: u16, y: u16| draft.land(map, x, y).map_or(own, |cell| cell.z);
                PreviewTile {
                    x: at.x,
                    y: at.y,
                    corners: [
                        own,
                        at.x.checked_add(1).map_or(own, |x| z(x, at.y)),
                        at.y.checked_add(1).map_or(own, |y| z(at.x, y)),
                        match (at.x.checked_add(1), at.y.checked_add(1)) {
                            (Some(x), Some(y)) => z(x, y),
                            _ => own,
                        },
                    ],
                }
            })
            .collect()
    }

    /// Where the selected static would be placed by the next world click.
    pub(crate) fn static_preview_at(
        &self,
        map: &WorldMap,
        pick: &crate::diagnostics::Pick,
    ) -> Option<(openshard_protocol::world::Point, Graphic)> {
        if !self.active || self.tool != Some(ActiveTool::PlaceStatic) {
            return None;
        }
        let Some(AssetId::Static(graphic)) = self.palette.selected() else {
            return None;
        };
        let (x, y) = pick.static_.map_or_else(
            || pick.tile.as_ref().map(|tile| (tile.at.x, tile.at.y)),
            |picked| Some((picked.at.x, picked.at.y)),
        )?;
        let z = self
            .draft
            .as_ref()
            .and_then(|draft| draft.land(map, x, y))
            .or_else(|| map.land(x, y))?
            .z;
        Some((openshard_protocol::world::Point::new(x, y, z), graphic))
    }

    /// Every unpublished static that is not already drawn by the base map.
    pub(crate) fn static_draft_previews(&self, map: &WorldMap) -> Vec<StaticItem> {
        if !self.active {
            return Vec::new();
        }
        self.draft
            .as_ref()
            .map_or_else(Vec::new, |draft| draft.added_statics(map))
    }

    /// Texture owned by the world-overlay egui context for one preview static.
    pub(crate) fn static_preview_texture(
        &mut self,
        context: &egui::Context,
        id: Graphic,
    ) -> Option<&egui::TextureHandle> {
        let preview = self.previews.get_mut(&AssetId::Static(id))?.as_mut()?;
        if preview.overlay.is_none() {
            preview.overlay = Some(context.load_texture(
                format!("map-editor-overlay-static-{:#06x}", id.0),
                preview.image.clone(),
                egui::TextureOptions::NEAREST,
            ));
        }
        preview.overlay.as_ref()
    }

    /// Draw and directly update local editor controls. Returns whether Commit
    /// was pressed; the delayed application boundary rechecks authority.
    pub(crate) fn panel(&mut self, root: &mut egui::Ui) -> bool {
        let mut commit_pressed = false;
        egui::Panel::left("map editor tools")
            .resizable(true)
            .default_size(270.0)
            .show(root, |ui| {
                ui.heading("Map editor");
                self.status(ui);
                ui.separator();
                ui.label("Tool");
                for (tool, label) in [
                    (ActiveTool::PaintLand, "Paint land"),
                    (ActiveTool::PlaceStatic, "Place static"),
                    (ActiveTool::RemoveStatic, "Remove static"),
                    (ActiveTool::Raise, "Raise"),
                    (ActiveTool::Lower, "Lower"),
                    (ActiveTool::Flatten, "Flatten"),
                    (ActiveTool::PlaceHouse, "Place house"),
                    (ActiveTool::RemoveHouse, "Remove house"),
                ] {
                    ui.selectable_value(&mut self.tool, Some(tool), label);
                }
                if self.tool == Some(ActiveTool::PlaceHouse) {
                    self.house_catalogue(ui);
                } else if self.tool != Some(ActiveTool::RemoveHouse) {
                    ui.horizontal(|ui| {
                        ui.label("Shape");
                        ui.selectable_value(&mut self.brush.shape, BrushShape::Circle, "Circle");
                        ui.selectable_value(&mut self.brush.shape, BrushShape::Square, "Square");
                    });
                    let mut radius = self.brush.radius.get();
                    if ui
                        .add(egui::Slider::new(&mut radius, 0..=16).text("Radius"))
                        .changed()
                    {
                        self.brush.radius = BrushRadius::new(radius);
                    }
                    if matches!(self.tool, Some(ActiveTool::Raise | ActiveTool::Lower)) {
                        ui.add(egui::Slider::new(&mut self.strength, 1..=16).text("Strength"));
                    }
                    if self.tool == Some(ActiveTool::Flatten) {
                        ui.add(egui::Slider::new(&mut self.flatten, i8::MIN..=i8::MAX).text("Height"));
                    }
                }

                ui.separator();
                if !matches!(self.tool, Some(ActiveTool::PlaceHouse | ActiveTool::RemoveHouse)) {
                    self.catalogue(ui);
                }
                if !matches!(self.tool, Some(ActiveTool::PlaceHouse | ActiveTool::RemoveHouse)) {
                    ui.separator();
                    ui.horizontal(|ui| {
                        let undo = self.draft.as_ref().is_some_and(Draft::can_undo);
                        if ui.add_enabled(undo, egui::Button::new("Undo")).clicked() {
                            self.draft.as_mut().unwrap().undo();
                            self.commit = CommitState::Ready;
                        }
                        let redo = self.draft.as_ref().is_some_and(Draft::can_redo);
                        if ui.add_enabled(redo, egui::Button::new("Redo")).clicked() {
                            self.draft.as_mut().unwrap().redo();
                            self.commit = CommitState::Ready;
                        }
                        let dirty = self.draft.as_ref().is_some_and(Draft::is_dirty);
                        if ui.add_enabled(dirty, egui::Button::new("Discard")).clicked() {
                            self.draft = None;
                            self.draft_static_assets.clear();
                            self.commit = CommitState::Ready;
                            self.message = None;
                        }
                    });
                    let dirty = self.draft.as_ref().is_some_and(Draft::is_dirty);
                    commit_pressed = ui
                        .add_enabled(
                            dirty && !matches!(self.commit, CommitState::Pending | CommitState::Accepted(_)),
                            egui::Button::new("Commit draft"),
                        )
                        .clicked();
                }
            });
        commit_pressed
    }

    fn house_catalogue(&mut self, ui: &mut egui::Ui) {
        if self.multis.is_none() {
            ui.colored_label(egui::Color32::LIGHT_RED, "multi table is unavailable");
            return;
        }
        if ui
            .add(egui::TextEdit::singleline(&mut self.house_search).hint_text("multi id: 0x64 or 100"))
            .changed()
        {
            self.refresh_house_matches();
        }
        ui.small("Classic multis from the client install");
        let mut chosen = None;
        let multis = self.multis.as_deref().expect("checked above");
        // Leave the selected multi's picture visible below the catalogue even
        // on an ordinary-height window; the static-art palette can spend more
        // height because it has no second building-sized card under its rows.
        egui::ScrollArea::vertical()
            .id_salt("map-editor-classic-houses")
            .max_height(180.0)
            .show_rows(ui, 24.0, self.house_matches.len(), |ui, rows| {
                for id in &self.house_matches[rows] {
                    let Some(multi) = multis.get(*id) else { continue };
                    let label = format!(
                        "{:#06x}  {}x{}  {} pieces",
                        id,
                        multi.size.0,
                        multi.size.1,
                        multi.drawn().count()
                    );
                    if ui
                        .selectable_label(self.selected_house == Some(HouseSelection::Multi(*id)), label)
                        .clicked()
                    {
                        chosen = Some(*id);
                    }
                }
            });
        if let Some(id) = chosen {
            self.selected_house = Some(HouseSelection::Multi(id));
        }

        ui.separator();
        ui.label("Custom templates");
        if let Some(error) = &self.template_error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        } else if self.templates.is_empty() {
            ui.small(format!(
                "Export .wsc files to {CUSTOM_HOUSE_DIRECTORY}/ beside this client install."
            ));
        } else {
            let mut chosen = None;
            egui::ScrollArea::vertical()
                .id_salt("map-editor-custom-houses")
                .max_height(120.0)
                .show_rows(ui, 24.0, self.templates.len(), |ui, rows| {
                    for index in rows {
                        let template = &self.templates[index];
                        let label = format!("{}  {} pieces", template.name, template.components.len());
                        if ui
                            .selectable_label(
                                self.selected_house == Some(HouseSelection::Template(index)),
                                label,
                            )
                            .clicked()
                        {
                            chosen = Some(index);
                        }
                    }
                });
            if let Some(index) = chosen {
                self.selected_house = Some(HouseSelection::Template(index));
            }
        }

        match self.selected_house {
            Some(HouseSelection::Multi(id)) => {
                ui.small(format!("Selected house {id:#06x}. Click the world to place it."));
                self.house_previews.retain(|cached, _| *cached == id);
                if !self.house_previews.contains_key(&id) {
                    let preview = self
                        .catalogue
                        .as_ref()
                        .zip(self.multis.as_deref().and_then(|multis| multis.get(id)))
                        .map(|(catalogue, multi)| {
                            components_preview_texture(
                                catalogue,
                                ui.ctx(),
                                &format!("map-editor-house-{id:#06x}"),
                                multi.components.as_slice(),
                            )
                        })
                        .transpose()
                        .map(Option::flatten);
                    match preview {
                        Ok(texture) => {
                            self.house_previews.insert(id, texture);
                        }
                        Err(error) => {
                            self.house_previews.insert(id, None);
                            self.message = Some(error.to_string());
                        }
                    }
                }
                match self.house_previews.get(&id).and_then(Option::as_ref) {
                    Some(texture) => {
                        egui::Frame::canvas(ui.style()).show(ui, |ui| {
                            ui.add(
                                egui::Image::from_texture(texture)
                                    .max_size(egui::vec2(220.0, 170.0))
                                    .maintain_aspect_ratio(true),
                            );
                        });
                    }
                    None => {
                        ui.small("No drawable preview for this multi.");
                    }
                }
            }
            Some(HouseSelection::Template(index)) => {
                let Some(template) = self.templates.get(index) else {
                    self.selected_house = None;
                    return;
                };
                ui.small(format!(
                    "Selected template {}. Click the world to place it.",
                    template.name
                ));
                self.template_previews.retain(|cached, _| *cached == index);
                if !self.template_previews.contains_key(&index) {
                    let preview = self
                        .catalogue
                        .as_ref()
                        .map(|catalogue| {
                            components_preview_texture(
                                catalogue,
                                ui.ctx(),
                                &format!("map-editor-template-{index}"),
                                &template.components,
                            )
                        })
                        .transpose()
                        .map(Option::flatten);
                    match preview {
                        Ok(texture) => {
                            self.template_previews.insert(index, texture);
                        }
                        Err(error) => {
                            self.template_previews.insert(index, None);
                            self.message = Some(error.to_string());
                        }
                    }
                }
                match self.template_previews.get(&index).and_then(Option::as_ref) {
                    Some(texture) => {
                        egui::Frame::canvas(ui.style()).show(ui, |ui| {
                            ui.add(
                                egui::Image::from_texture(texture)
                                    .max_size(egui::vec2(220.0, 170.0))
                                    .maintain_aspect_ratio(true),
                            );
                        });
                    }
                    None => {
                        ui.small("No drawable preview for this template.");
                    }
                }
            }
            None => {
                ui.small("Select a house, then point at the world.");
            }
        };
    }

    fn status(&self, ui: &mut egui::Ui) {
        if self.tool == Some(ActiveTool::PlaceHouse) {
            ui.label("House placement is immediate and uses the shard's housing rules.");
            return;
        }
        if self.tool == Some(ActiveTool::RemoveHouse) {
            ui.label("Click any visible part of a house to remove the whole house.");
            return;
        }
        let (tiles, ops) = self.draft.as_ref().map_or((0, 0), |draft| {
            (draft.dirty_tiles().len(), draft.dirty_tiles().len())
        });
        ui.label(format!("Draft: {tiles} dirty tile(s), {ops}+ operation(s)"));
        let status = match self.commit {
            CommitState::Ready => "ready",
            CommitState::Pending => "commit pending",
            CommitState::Accepted(_) => "accepted; fetching changed chunks",
            CommitState::Refused(reason) => refusal(reason),
        };
        ui.small(status);
        if let Some(message) = &self.message {
            ui.colored_label(egui::Color32::LIGHT_RED, message);
        }
    }

    fn catalogue(&mut self, ui: &mut egui::Ui) {
        let Some(catalogue) = self.catalogue.as_ref() else {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                self.catalogue_error
                    .as_deref()
                    .unwrap_or("asset catalogue is unavailable"),
            );
            return;
        };
        let mut changed = ui.text_edit_singleline(self.palette.search_mut()).changed();
        ui.horizontal(|ui| {
            for (filter, label) in [
                (KindFilter::All, "All"),
                (KindFilter::Land, "Land"),
                (KindFilter::Static, "Statics"),
            ] {
                if ui
                    .selectable_label(self.palette.filter() == filter, label)
                    .clicked()
                {
                    self.palette.set_filter(filter);
                    changed = true;
                }
            }
        });
        if changed {
            self.matches = catalogue.matching(&self.palette);
        }
        ui.horizontal(|ui| {
            ui.add_sized([88.0, 20.0], egui::Label::new(egui::RichText::new("ID").strong()));
            ui.label(egui::RichText::new("Art").strong());
        });
        let mut selected = None;
        let mut visible = Vec::new();
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .show_rows(ui, 56.0, self.matches.len(), |ui, rows| {
                egui::Grid::new("map-editor-catalogue-table")
                    .num_columns(2)
                    .striped(true)
                    .min_row_height(48.0)
                    .min_col_width(88.0)
                    .show(ui, |ui| {
                        for id in &self.matches[rows] {
                            visible.push(*id);
                            let entry = catalogue.entry(*id);
                            let prefix = match id {
                                AssetId::Land(_) => "L",
                                AssetId::Static(_) => "S",
                            };
                            let label = format!("{prefix} {:#06x}", id.raw());
                            let id_clicked = ui
                                .selectable_label(self.palette.selected() == Some(*id), label)
                                .on_hover_text(entry.name.unwrap_or("unnamed"))
                                .clicked();

                            if !self.previews.contains_key(id) {
                                match preview_texture(catalogue, ui.ctx(), *id) {
                                    Ok(texture) => {
                                        self.previews.insert(*id, texture);
                                    }
                                    Err(error) => {
                                        self.previews.insert(*id, None);
                                        self.message = Some(error.to_string());
                                    }
                                }
                            }
                            let art_clicked = match self.previews.get(id).and_then(Option::as_ref) {
                                Some(texture) => ui
                                    .add(
                                        egui::Image::from_texture(&texture.panel)
                                            .max_size(egui::vec2(44.0, 44.0))
                                            .sense(egui::Sense::click()),
                                    )
                                    .on_hover_text(entry.name.unwrap_or("unnamed"))
                                    .clicked(),
                                None => {
                                    ui.add_sized([44.0, 44.0], egui::Label::new("—"));
                                    false
                                }
                            };
                            if id_clicked || art_clicked {
                                selected = Some(*id);
                            }
                            ui.end_row();
                        }
                    });
            });
        let selected_before_click = self.palette.selected();
        self.previews.retain(|id, _| {
            visible.contains(id)
                || selected_before_click == Some(*id)
                || self.draft_static_assets.contains(id)
        });
        if let Some(id) = selected {
            self.select_asset(id);
        }
        let catalogue = self
            .catalogue
            .as_ref()
            .expect("the unavailable catalogue returned at the top of the panel");
        if let Some(id) = self.palette.selected() {
            let entry = catalogue.entry(id);
            ui.small(format!(
                "Selected {:#06x}: {}",
                id.raw(),
                entry.name.unwrap_or("unnamed")
            ));
            ui.small(match id {
                AssetId::Land(_) => "Click a world tile to paint it.",
                AssetId::Static(_) => "Click a world tile to place it.",
            });
            if let Some(texture) = self.previews.get(&id).and_then(Option::as_ref) {
                ui.add(egui::Image::from_texture(&texture.panel).max_size(egui::vec2(128.0, 128.0)));
            }
        }
    }
}

fn preview_texture(
    catalogue: &Catalog,
    context: &egui::Context,
    id: AssetId,
) -> Result<Option<PreviewTexture>, openshard_uofiles::art::ArtError> {
    let Some(image) = catalogue.preview(id)? else {
        return Ok(None);
    };
    let size = [usize::from(image.width()), usize::from(image.height())];
    let pixels = image
        .pixels()
        .iter()
        .map(|pixel| {
            let color = pixel.rgb8();
            if matches!(id, AssetId::Static(_)) && pixel.is_transparent() {
                egui::Color32::TRANSPARENT
            } else {
                egui::Color32::from_rgb(color.red, color.green, color.blue)
            }
        })
        .collect();
    let image = egui::ColorImage::new(size, pixels);
    let panel = context.load_texture(
        format!("map-editor-{:?}-{:#06x}", id.kind(), id.raw()),
        image.clone(),
        egui::TextureOptions::NEAREST,
    );
    Ok(Some(PreviewTexture {
        image,
        panel,
        overlay: None,
    }))
}

fn load_house_templates(directory: &Path) -> Result<Vec<HouseTemplate>, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("could not read {}: {error}", directory.display())),
    };
    let mut paths = entries
        .map(|entry| entry.map(|entry| entry.path()).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    paths
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        })
        .map(|path| {
            let source = fs::read_to_string(&path)
                .map_err(|error| format!("could not read {}: {error}", path.display()))?;
            decode_house_template(&path, &source)
        })
        .collect()
}

fn decode_house_template(path: &Path, source: &str) -> Result<HouseTemplate, String> {
    let template: TemplateFile = serde_json::from_str(source)
        .map_err(|error| format!("could not decode {}: {error}", path.display()))?;
    if template.format != "openshard-house-design/v1" {
        return Err(format!("{} has an unknown house-template format", path.display()));
    }
    if template.components.is_empty() {
        return Err(format!("{} has no components", path.display()));
    }
    let components = template
        .components
        .into_iter()
        .map(|component| {
            if i8::try_from(component.dx).is_err()
                || i8::try_from(component.dy).is_err()
                || i8::try_from(component.dz).is_err()
            {
                return Err(format!(
                    "{} has a component the custom-house wire format cannot carry",
                    path.display()
                ));
            }
            Ok(Component {
                graphic: Graphic(component.graphic),
                dx: component.dx,
                dy: component.dy,
                dz: component.dz,
                flags: component.flags,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let key = path
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| format!("{} has no UTF-8 file name", path.display()))?
        .to_owned();
    Ok(HouseTemplate {
        name: key.replace(['_', '-'], " "),
        key,
        components: components.into(),
    })
}

/// Flatten a multi's component art into the same south-east isometric view as
/// the world, for the selected-house card in the editor panel.
fn components_preview_texture(
    catalogue: &Catalog,
    context: &egui::Context,
    texture_name: &str,
    components: &[Component],
) -> Result<Option<egui::TextureHandle>, openshard_uofiles::art::ArtError> {
    const HALF_TILE: i32 = 22;
    const Z_STEP: i32 = 4;
    const MAX_SOURCE_SIDE: usize = 2048;
    const THUMBNAIL_SIDE: usize = 256;

    let mut layers = Vec::new();
    let mut bounds: Option<(i32, i32, i32, i32)> = None;
    for component in components.iter().copied().filter(|component| component.drawn()) {
        let Some(image) = catalogue.preview(AssetId::Static(component.graphic))? else {
            continue;
        };
        let anchor_x = (i32::from(component.dx) - i32::from(component.dy)) * HALF_TILE;
        let anchor_y = (i32::from(component.dx) + i32::from(component.dy)) * HALF_TILE
            - i32::from(component.dz) * Z_STEP;
        let left = anchor_x - (i32::from(image.width()) >> 1);
        let top = anchor_y + HALF_TILE - i32::from(image.height());
        let right = left + i32::from(image.width());
        let bottom = top + i32::from(image.height());
        bounds = Some(
            bounds.map_or((left, top, right, bottom), |(min_x, min_y, max_x, max_y)| {
                (
                    min_x.min(left),
                    min_y.min(top),
                    max_x.max(right),
                    max_y.max(bottom),
                )
            }),
        );
        layers.push((component, image, left, top));
    }
    let Some((min_x, min_y, max_x, max_y)) = bounds else {
        return Ok(None);
    };
    let Ok(width) = usize::try_from(max_x - min_x) else {
        return Ok(None);
    };
    let Ok(height) = usize::try_from(max_y - min_y) else {
        return Ok(None);
    };
    if width == 0 || height == 0 || width > MAX_SOURCE_SIDE || height > MAX_SOURCE_SIDE {
        return Ok(None);
    }

    // The world depth key is tile x+y, then z. File order remains the stable
    // tie-breaker for two components occupying the same cell and height.
    layers.sort_by_key(|(component, _, _, _)| {
        (
            i32::from(component.dx) + i32::from(component.dy),
            i32::from(component.dz),
        )
    });
    let mut pixels = vec![egui::Color32::TRANSPARENT; width * height];
    for (_, image, left, top) in layers {
        let offset_x = usize::try_from(left - min_x).expect("the layer lies inside its derived bounds");
        let offset_y = usize::try_from(top - min_y).expect("the layer lies inside its derived bounds");
        for y in 0..usize::from(image.height()) {
            for x in 0..usize::from(image.width()) {
                let pixel = image.pixels()[y * usize::from(image.width()) + x];
                if pixel.is_transparent() {
                    continue;
                }
                let color = pixel.rgb8();
                pixels[(offset_y + y) * width + offset_x + x] =
                    egui::Color32::from_rgb(color.red, color.green, color.blue);
            }
        }
    }

    let divisor = width.max(height).div_ceil(THUMBNAIL_SIDE).max(1);
    let thumb_width = width.div_ceil(divisor);
    let thumb_height = height.div_ceil(divisor);
    let pixels = if divisor == 1 {
        pixels
    } else {
        (0..thumb_height)
            .flat_map(|y| {
                let pixels = &pixels;
                (0..thumb_width).map(move |x| pixels[(y * divisor) * width + x * divisor])
            })
            .collect()
    };
    let image = egui::ColorImage::new([thumb_width, thumb_height], pixels);
    Ok(Some(context.load_texture(
        texture_name,
        image,
        egui::TextureOptions::NEAREST,
    )))
}

fn wire_op(op: PatchOp) -> MapEditOp {
    let at = |x, y| EditTile {
        x: EditX(x),
        y: EditY(y),
    };
    match op {
        PatchOp::SetLand { x, y, now, .. } => MapEditOp::SetLand {
            at: at(x, y),
            tile: EditLandTile::from_wire(now.tile.0).expect("LandTileId is inside tiledata"),
            z: EditZ(now.z),
        },
        PatchOp::AddStatic { item } => MapEditOp::AddStatic {
            at: at(item.x, item.y),
            graphic: item.tile,
            z: EditZ(item.z),
            hue: item.hue,
        },
        PatchOp::RemoveStatic { which, was } => MapEditOp::RemoveStatic {
            at: at(was.x, was.y),
            which: EditStaticId(which.0),
        },
    }
}

fn refusal(reason: MapEditRefusal) -> &'static str {
    match reason {
        MapEditRefusal::NotAuthorized => "refused: Game Master authority required",
        MapEditRefusal::UnknownFacet => "refused: facet is not loaded",
        MapEditRefusal::NoGround => "refused: facet has no ground",
        MapEditRefusal::EmptyDraft => "refused: draft is empty",
        MapEditRefusal::Conflict => "conflict: the shard moved; discard or rebuild the draft",
        MapEditRefusal::OffMap => "refused: an operation is outside the facet",
        MapEditRefusal::NoSuchStatic => "refused: a static ordinal is stale",
        MapEditRefusal::NotOurWorld => "refused: the facet has no OpenShard base set",
        MapEditRefusal::Storage => "refused: the patch log could not be written",
    }
}

impl crate::app::App {
    /// Turn the already-drawn cursor answer into one editor dab.
    pub(crate) fn apply_map_editor_click(&mut self, camera: openshard_client_render::camera::Camera) {
        if self.map_editor.removes_house() {
            if let (Some(link), Some(item)) = (self.world.shard.link(), self.picking.hover.item) {
                link.say(
                    format!(".hdemolish {}", item.serial),
                    openshard_protocol::speech::TalkMode::Regular,
                );
            }
            return;
        }
        let removing_static = self.map_editor.removes_static();
        let picked = self.picking.hover.static_;
        let at = picked
            .map(|picked| TilePoint::new(picked.at.x, picked.at.y))
            .or_else(|| {
                self.pick_tile(camera)
                    .map(|tile| TilePoint::new(tile.at.x, tile.at.y))
            });
        let Some(at) = at else {
            return;
        };
        if let Some(template) = self.map_editor.selected_template_name() {
            let tile = self
                .pick_tile(camera)
                .map(|tile| openshard_protocol::world::Point::new(tile.at.x, tile.at.y, tile.stand_z.0));
            if let (Some(link), Some(at)) = (self.world.shard.link(), tile) {
                link.say(
                    format!(".house @{template} {} {} {}", at.x, at.y, at.z),
                    openshard_protocol::speech::TalkMode::Regular,
                );
            }
            return;
        }
        if let Some(multi) = self.map_editor.selected_multi() {
            let tile = self
                .pick_tile(camera)
                .map(|tile| openshard_protocol::world::Point::new(tile.at.x, tile.at.y, tile.stand_z.0));
            if let (Some(link), Some(at)) = (self.world.shard.link(), tile) {
                link.say(
                    format!(".house {multi:#06x} {} {} {}", at.x, at.y, at.z),
                    openshard_protocol::speech::TalkMode::Regular,
                );
            }
            return;
        }
        let snapshot = self
            .resources
            .ground
            .snapshot()
            .expect("world input is gated until ground arrives");
        let static_id = removing_static.then_some(picked).flatten().and_then(|picked| {
            self.map_editor.static_ordinal(
                snapshot.map(),
                picked.at.x,
                picked.at.y,
                picked.graphic,
                picked.at.z,
            )
        });
        self.map_editor.apply_at(
            snapshot.map(),
            snapshot.facet(),
            snapshot.revision(),
            at,
            static_id,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use openshard_protocol::access::AccessLevel;
    use openshard_protocol::wire::Graphic;
    use openshard_tiles::LandTileId;
    use openshard_uofiles::multi::{Component, Multi, Multis};

    use super::{ActiveTool, AssetId, HousePreview, HouseSelection, MapEditor, decode_house_template};

    #[test]
    fn a_player_cannot_enter_editor_mode() {
        let mut editor = MapEditor::with_catalogue(None, None);

        editor.set_active(true, AccessLevel::Player);

        assert!(!editor.active());
    }

    #[test]
    fn both_staff_levels_may_enter_editor_mode() {
        for authority in [AccessLevel::GameMaster, AccessLevel::Administrator] {
            let mut editor = MapEditor::with_catalogue(None, None);

            editor.set_active(true, authority);

            assert!(editor.active());
        }
    }

    #[test]
    fn losing_authority_closes_an_active_editor() {
        let mut editor = MapEditor::with_catalogue(None, None);
        editor.set_active(true, AccessLevel::GameMaster);

        editor.reconcile(AccessLevel::Player);

        assert!(!editor.active());
    }

    #[test]
    fn an_authorized_user_can_leave_editor_mode() {
        let mut editor = MapEditor::with_catalogue(None, None);
        editor.set_active(true, AccessLevel::Administrator);

        editor.set_active(false, AccessLevel::Administrator);

        assert!(!editor.active());
    }

    #[test]
    fn selecting_static_art_arms_the_placement_tool() {
        let mut editor = MapEditor::with_catalogue(None, None);

        editor.select_asset(AssetId::Static(Graphic(0x0eed)));

        assert_eq!(editor.tool, Some(ActiveTool::PlaceStatic));
    }

    #[test]
    fn selecting_land_art_arms_the_paint_tool() {
        let mut editor = MapEditor::with_catalogue(None, None);

        editor.select_asset(AssetId::Land(LandTileId(3)));

        assert_eq!(editor.tool, Some(ActiveTool::PaintLand));
    }

    #[test]
    fn a_house_selection_only_arms_house_placement_in_its_tool() {
        let mut editor = MapEditor::with_catalogue(None, None);
        editor.multis = Some(Arc::new(Multis::of([Multi::new(
            0x64,
            vec![Component {
                graphic: Graphic(1),
                dx: 0,
                dy: 0,
                dz: 0,
                flags: 1,
            }],
        )])));
        editor.refresh_house_matches();
        editor.selected_house = Some(HouseSelection::Multi(0x64));

        assert_eq!(editor.selected_multi(), None);
        editor.tool = Some(ActiveTool::PlaceHouse);
        assert_eq!(editor.selected_multi(), Some(0x64));
        assert_eq!(editor.house_matches, vec![0x64]);
    }

    #[test]
    fn an_exported_wsc_design_appears_as_a_custom_template_preview() {
        let template = decode_house_template(
            Path::new("Marble-Bungalow.json"),
            r#"{
                "format": "openshard-house-design/v1",
                "revision": 1,
                "components": [
                    { "graphic": 1442, "dx": 3, "dy": 1, "dz": 20, "flags": 1 }
                ]
            }"#,
        )
        .expect("the exporter format is accepted");
        assert_eq!(template.name, "Marble Bungalow");

        let mut editor = MapEditor::with_catalogue(None, None);
        editor.templates.push(template);
        editor.selected_house = Some(HouseSelection::Template(0));
        editor.tool = Some(ActiveTool::PlaceHouse);

        assert_eq!(
            editor.selected_multi(),
            None,
            "a template is not an invented multi id"
        );
        assert_eq!(editor.selected_template_name(), Some("Marble-Bungalow"));
        assert!(matches!(
            editor.selected_house_preview(),
            Some(HousePreview::Design(components)) if components.as_ref() == [Component {
                graphic: Graphic(1442), dx: 3, dy: 1, dz: 20, flags: 1
            }]
        ));
    }

    #[test]
    fn a_template_that_cannot_cross_the_house_design_wire_is_rejected() {
        let source = r#"{
            "format": "openshard-house-design/v1",
            "components": [
                { "graphic": 1442, "dx": 128, "dy": 0, "dz": 0, "flags": 1 }
            ]
        }"#;
        assert!(
            decode_house_template(Path::new("too-wide.json"), source).is_err(),
            "the preview must not bless a template the shard would drop on the wire"
        );
    }

    #[test]
    fn house_removal_is_only_armed_in_its_tool() {
        let mut editor = MapEditor::with_catalogue(None, None);

        assert!(!editor.removes_house());
        editor.tool = Some(ActiveTool::RemoveHouse);
        assert!(editor.removes_house());
        assert_eq!(editor.selected_multi(), None);
    }

    #[test]
    fn cancelling_a_tool_leaves_the_editor_active_but_disarms_map_and_house_tools() {
        let mut editor = MapEditor::with_catalogue(None, None);
        editor.active = true;

        assert!(editor.cancel_tool());
        assert!(editor.active());
        assert!(!editor.removes_static());
        assert_eq!(editor.selected_multi(), None);
        assert!(!editor.cancel_tool());

        editor.tool = Some(ActiveTool::PlaceHouse);
        assert!(editor.cancel_tool());
        assert_eq!(editor.selected_multi(), None);
        assert!(!editor.removes_house());
    }
}
