//! The client-owned map-editor workspace.
//!
//! `openshard-client-editor` owns the UI-independent catalogue, tools and draft;
//! this module joins them to egui and to the wire without letting either own the
//! authoritative map. The draft stays a sparse projection over the current
//! snapshot until the shard accepts it and the accepted revision arrives.

use std::path::Path;

use openshard_client_editor::draft::Draft;
use openshard_client_editor::tools::{
    Brush, BrushRadius, BrushShape, HeightStrength, StaticHeight, StaticPlacement, TargetHeight, TilePoint,
    Tool,
};
use openshard_client_editor::{AssetId, Catalog, KindFilter, PaletteState};
use openshard_map::map::WorldMap;
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
}

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

pub(crate) struct MapEditor {
    active: bool,
    catalogue: Option<Catalog>,
    catalogue_error: Option<String>,
    palette: PaletteState,
    matches: Vec<AssetId>,
    preview: Option<(AssetId, egui::TextureHandle)>,
    tool: ActiveTool,
    brush: Brush,
    strength: u8,
    flatten: i8,
    draft: Option<Draft>,
    commit: CommitState,
    message: Option<String>,
}

impl MapEditor {
    /// Open the install-backed catalogue and start in ordinary play mode.
    pub(crate) fn open(client_dir: &Path) -> Self {
        match Catalog::open(client_dir) {
            Ok(catalogue) => Self::with_catalogue(Some(catalogue), None),
            Err(error) => Self::with_catalogue(None, Some(error.to_string())),
        }
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
            preview: None,
            tool: ActiveTool::PaintLand,
            brush: Brush::default(),
            strength: 1,
            flatten: 0,
            draft: None,
            commit: CommitState::Ready,
            message: None,
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
        matches!(self.tool, ActiveTool::RemoveStatic)
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
                self.commit = CommitState::Ready;
                self.message = None;
            }
            Err(error) => self.message = Some(error.to_string()),
        }
    }

    fn tool(&mut self, static_id: Option<StaticId>) -> Option<Tool> {
        match self.tool {
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
                ] {
                    ui.selectable_value(&mut self.tool, tool, label);
                }
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
                if matches!(self.tool, ActiveTool::Raise | ActiveTool::Lower) {
                    ui.add(egui::Slider::new(&mut self.strength, 1..=16).text("Strength"));
                }
                if self.tool == ActiveTool::Flatten {
                    ui.add(egui::Slider::new(&mut self.flatten, i8::MIN..=i8::MAX).text("Height"));
                }

                ui.separator();
                self.catalogue(ui);
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
            });
        commit_pressed
    }

    fn status(&self, ui: &mut egui::Ui) {
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
        let mut selected = None;
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .show_rows(ui, 20.0, self.matches.len(), |ui, rows| {
                for id in &self.matches[rows] {
                    let entry = catalogue.entry(*id);
                    let kind = match id {
                        AssetId::Land(_) => "land",
                        AssetId::Static(_) => "static",
                    };
                    let label = format!("{kind} {:#06x}  {}", id.raw(), entry.name.unwrap_or("(unnamed)"));
                    if ui
                        .selectable_label(self.palette.selected() == Some(*id), label)
                        .clicked()
                    {
                        selected = Some(*id);
                    }
                }
            });
        if let Some(id) = selected {
            self.palette.select(id);
            self.preview = None;
            match catalogue.preview(id) {
                Ok(Some(image)) => {
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
                    let texture = ui.ctx().load_texture(
                        format!("map-editor-{:?}-{:#06x}", id.kind(), id.raw()),
                        egui::ColorImage::new(size, pixels),
                        egui::TextureOptions::NEAREST,
                    );
                    self.preview = Some((id, texture));
                    self.message = None;
                }
                Ok(None) => self.message = Some("this catalogue entry has no art".to_owned()),
                Err(error) => self.message = Some(error.to_string()),
            }
        }
        if let Some(id) = self.palette.selected() {
            let entry = catalogue.entry(id);
            ui.small(format!(
                "Selected {:#06x}: {}",
                id.raw(),
                entry.name.unwrap_or("unnamed")
            ));
            if let Some((previewed, texture)) = &self.preview {
                if *previewed == id {
                    ui.add(egui::Image::from_texture(texture).max_size(egui::vec2(128.0, 128.0)));
                }
            }
        }
    }
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
    use openshard_protocol::access::AccessLevel;

    use super::MapEditor;

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
}
