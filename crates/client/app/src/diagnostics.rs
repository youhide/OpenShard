//! Read-only diagnostic facts about the world.
//!
//! These values sit between the application queries and the dev HUD.  They do
//! not depend on egui: a panel, a frame dump, or a future remote inspector can
//! all consume the same answer without the query layer depending on its view.

use std::sync::Arc;

use openshard_client_render::camera::ViewPixel;
use openshard_client_render::facing::Prism;
use openshard_client_render::follow::Rig;
use openshard_client_render::solid::Cut;
use openshard_client_render::statics::PickedStatic;
use openshard_protocol::mobile::Notoriety;
use openshard_protocol::serial::Serial;
use openshard_protocol::wire::{Graphic, Hue};

use crate::graphics::{HighlightStyle, HighlightTarget};

/// A z-height in the wire's own unit.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Height(pub i8);

/// A draw-order key's tile component alone.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct TileDepth(pub i32);

/// A static's draw-order priority within its tile.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct PriorityZ(pub i32);

/// Hit points in the shard's health-bar scale.
///
/// Keeping current and maximum values in this domain type prevents unrelated
/// `u16` quantities (such as map coordinates or item amounts) from being
/// accidentally mixed into a health bar.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct HealthPoints(u16);

impl HealthPoints {
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Everything the client knows about one map tile for inspection.
#[derive(Clone)]
pub struct PickedTile {
    pub at: openshard_movement::Tile,
    pub land: Option<Graphic>,
    pub land_z: Height,
    pub stand_z: Height,
    pub corners: [Height; 4],
    pub levels: Vec<(Height, bool)>,
    pub ceiling: Option<Height>,
    pub statics: Vec<(Graphic, Height, Hue, PriorityZ)>,
    pub items: Vec<(Graphic, Height, Hue, PriorityZ)>,
    pub tile_depth: TileDepth,
    pub mobile_order: Option<openshard_client_render::depth::Order>,
}

/// A mobile identified by a click and resolved afresh for each frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PickedMobile {
    pub you: bool,
    pub serial: Option<Serial>,
    pub body: Graphic,
    pub hue: Hue,
    pub at: openshard_protocol::world::Point,
    pub order: openshard_client_render::depth::Order,
}

/// A server-owned ground item identified by a click.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PickedItem {
    pub serial: Serial,
    pub graphic: Graphic,
    pub hue: Hue,
    pub at: openshard_protocol::world::Point,
    pub priority_z: PriorityZ,
}

/// What a left click landed on, with dynamic objects resolved from identity.
pub enum Selection {
    Tile(PickedTile),
    Static {
        static_: openshard_client_render::statics::PickedStatic,
        tile: PickedTile,
        prism: Option<Prism>,
    },
    Mobile(Option<(PickedMobile, PickedTile)>),
    Item(Option<(PickedItem, PickedTile)>),
}

impl Selection {
    /// The tile column associated with the selected subject, if it remains in
    /// the current presentation.
    pub fn tile(&self) -> Option<&PickedTile> {
        match self {
            Self::Tile(tile) => Some(tile),
            Self::Static { tile, .. } => Some(tile),
            Self::Mobile(live) => live.as_ref().map(|(_, tile)| tile),
            Self::Item(live) => live.as_ref().map(|(_, tile)| tile),
        }
    }
}

/// The walkability of the tiles currently in view.
pub struct TerrainOverlay {
    pub open: Vec<openshard_protocol::world::Point>,
    pub blocked: Vec<openshard_protocol::world::Point>,
}

/// One cell in the interior-index diagnostic overlay.
///
/// This is a picture of R1's map fact only. Ordinary geometry does not consult
/// it yet: the index must be inspectable before R2 can let it hide a pixel.
#[derive(Clone, Copy)]
pub struct InteriorCell {
    pub at: openshard_protocol::world::Point,
    /// The structural-floor ordinal inside its indexed building.
    pub floor: u32,
    /// A deterministic positive-space label from the facet bake.  In the first
    /// pass it names a whole house; the room pass will refine it without making
    /// the value depend on this camera frame.
    pub room: u32,
    /// Reserved for the future door-reachability pass. Whole-house labels are
    /// all shown in the current diagnostic.
    pub shown: bool,
}

/// One closed-door portal in the interior-index diagnostic overlay.
#[derive(Clone, Copy)]
pub struct InteriorDoor {
    pub at: openshard_protocol::world::Point,
    /// Whether both rooms at the portal are visible in this frame.
    pub shown: bool,
}

/// One height-changing transition in the building graph.
#[derive(Clone, Copy)]
pub struct InteriorStair {
    pub from: openshard_protocol::world::Point,
    pub to: openshard_protocol::world::Point,
}

/// The visible map region's current interior-index reading.
pub struct InteriorOverlay {
    pub cells: Vec<InteriorCell>,
    pub doors: Vec<InteriorDoor>,
    pub stairs: Vec<InteriorStair>,
    pub buildings: usize,
}

/// One occlusion surface in the painter order the wireframe needs.
#[derive(Clone, Copy)]
pub struct OccluderSurface {
    pub x: i32,
    pub y: i32,
    pub solid: openshard_client_render::occlusion::Solid,
}

/// A planned route, split at the first obstacle the path cannot cross.
pub struct Route {
    pub open: Vec<openshard_protocol::world::Point>,
    pub barred: Vec<openshard_protocol::world::Point>,
}

/// One overhead health line, anchored in world-viewport pixels.
///
/// Its colour remains a presentation decision: the query returns the wire's
/// notoriety, and an adapter such as egui resolves that fact for its palette.
pub struct HealthBar {
    /// Top-centre of the body sprite, in the world's viewport.
    pub anchor: ViewPixel,
    /// Current hit points in the same scale as [`max`](Self::max).
    pub current: HealthPoints,
    /// The short-lived visual estimate, which follows [`current`](Self::current)
    /// after damage or healing rather than snapping with the wire packet.
    pub estimated: HealthPoints,
    /// Mana is available only for the local player: the wire does not disclose
    /// other mobiles' mana in the ordinary world update.
    pub mana: Option<ResourceBar>,
    /// Maximum hit points in the scale the shard chose for this body.
    pub max: HealthPoints,
    /// The wire fact the presentation uses to choose the bar colour.
    pub notoriety: Notoriety,
    /// Whether this body is the attack target the shard settled on.
    pub targeted: bool,
}

/// A current/max resource in the health-bar scale.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ResourceBar {
    pub current: HealthPoints,
    pub max: HealthPoints,
}

#[cfg(test)]
mod tests {
    use super::HealthPoints;

    #[test]
    fn health_points_round_trip_without_wire_conversion() {
        let points = HealthPoints::new(u16::MAX);
        assert_eq!(points.get(), u16::MAX);
    }
}

/// Everything this frame's cursor is over, answered once and carried whole to
/// every diagnostic consumer rather than unpacked into parallel HUD fields.
#[derive(Clone)]
pub struct Pick {
    /// The ground tile under the cursor, whether or not an object took the
    /// highlight this frame.
    pub tile: Option<PickedTile>,
    /// The eight tiles around [`Pick::tile`], for its wireframe ring.
    pub neighbours: Vec<PickedTile>,
    /// The map static under the cursor when no mobile or item is nearer.
    pub static_: Option<PickedStatic>,
    /// The highlighted mobile's transient frame index.
    pub mobile: Option<openshard_client_render::mobiles::MobileIndex>,
    /// The highlighted ground item's transient frame index.
    pub item: Option<openshard_client_render::items::ItemIndex>,
}

/// A read-only frame snapshot for the development HUD or another inspector.
///
/// This deliberately sits outside the egui adapter: its facts can equally be
/// sent to a frame dump or a future remote inspector.
pub struct Hud {
    pub locked: bool,
    pub rig: Rig,
    pub perf: crate::frames::Perf,
    pub scripts: Vec<&'static str>,
    pub replay: Option<(&'static str, f32)>,
    pub pick: Pick,
    pub hover_lit: bool,
    pub highlight: HighlightTarget,
    pub highlight_style: HighlightStyle,
    pub selected: Option<Selection>,
    pub health_bars: Vec<HealthBar>,
    pub draw: openshard_client_render::frame::Draw,
    pub cutaway_disabled: bool,
    pub body_overlap_transparency_disabled: bool,
    pub show_terrain: bool,
    pub terrain: Option<Arc<TerrainOverlay>>,
    pub show_interiors: bool,
    pub interiors: Option<Arc<InteriorOverlay>>,
    pub route: Option<Arc<Route>>,
    pub show_occluders: bool,
    pub show_solids: bool,
    pub solids_only: bool,
    pub solids_opaque: bool,
    pub solid_cut: Cut,
    pub solids: (usize, usize),
    pub occluders: Option<Arc<[OccluderSurface]>>,
    pub goal: Option<PickedTile>,
    /// Producer queue and immutable map-composite cache pressure.
    pub composites: CompositeTelemetry,
    /// Whether `App::ttf_font` is set — which of the Chat tab's two size
    /// sliders, `ChatScale`'s or `TtfScale`'s, is the one actually drawing
    /// anything this run. See `desk::TtfScale`'s own doc.
    pub ttf_active: bool,
}

/// Read-only map-composite producer and cache counters for the development HUD.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompositeTelemetry {
    pub ready: usize,
    pub pending: usize,
    pub prepared: usize,
    pub in_flight: usize,
    pub gpu_bytes: u64,
    pub gpu_budget_bytes: u64,
    /// Blocks deliberately held at the safe direct LOD0 path this session.
    pub quarantined: usize,
    /// Most recent block/key/source-owner proof and its safety reason.
    pub latest_quarantine: Option<openshard_client_render::composite::CompositeQuarantine>,
}
