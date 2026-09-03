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
use openshard_protocol::wire::{
    Graphic,
    Hue,
};
use openshard_protocol::world::RangedRange;

use crate::graphics::{
    HighlightStyle,
    HighlightTarget,
};

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
    pub at:           openshard_map::grid::Tile,
    pub land:         Option<Graphic>,
    pub land_z:       Height,
    pub stand_z:      Height,
    pub corners:      [Height; 4],
    pub levels:       Vec<(Height, bool)>,
    pub ceiling:      Option<Height>,
    pub statics:      Vec<(Graphic, Height, Hue, PriorityZ)>,
    pub items:        Vec<(Graphic, Height, Hue, PriorityZ)>,
    pub tile_depth:   TileDepth,
    pub mobile_order: Option<openshard_client_render::depth::Order>,
}

/// A mobile identified by a click and resolved afresh for each frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PickedMobile {
    pub you:    bool,
    pub serial: Option<Serial>,
    pub body:   Graphic,
    pub hue:    Hue,
    pub at:     openshard_protocol::world::Point,
    pub order:  openshard_client_render::depth::Order,
}

/// A server-owned ground item identified by a click.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PickedItem {
    pub serial:     Serial,
    pub graphic:    Graphic,
    pub hue:        Hue,
    pub at:         openshard_protocol::world::Point,
    pub priority_z: PriorityZ,
}

/// What a left click landed on, with dynamic objects resolved from identity.
pub enum Selection {
    Tile(PickedTile),
    Static {
        static_: openshard_client_render::statics::PickedStatic,
        tile:    PickedTile,
        prism:   Option<Prism>,
    },
    Mobile(Option<(PickedMobile, PickedTile)>),
    Item(Option<(PickedItem, PickedTile)>),
}

impl Selection {
    /// The tile column associated with the selected subject, if it remains in
    /// the current presentation.
    pub fn tile(&self) -> Option<&PickedTile> {
        match self {
            Self::Tile(tile) | Self::Static { tile, .. } => Some(tile),
            Self::Mobile(live) => live.as_ref().map(|(_, tile)| tile),
            Self::Item(live) => live.as_ref().map(|(_, tile)| tile),
        }
    }
}

/// The walkability of the tiles currently in view.
pub struct TerrainOverlay {
    pub open:    Vec<openshard_protocol::world::Point>,
    pub blocked: Vec<openshard_protocol::world::Point>,
}

/// One cell in the interior-index diagnostic overlay.
///
/// This is a picture of R1's map fact only. Ordinary geometry does not consult
/// it yet: the index must be inspectable before R2 can let it hide a pixel.
#[derive(Clone, Copy)]
pub struct InteriorCell {
    pub at:    openshard_protocol::world::Point,
    /// The structural-floor ordinal inside its indexed building.
    pub floor: u32,
    /// A deterministic positive-space label from the facet bake.  In the first
    /// pass it names a whole house; the room pass will refine it without making
    /// the value depend on this camera frame.
    pub room:  u32,
    /// Reserved for the future door-reachability pass. Whole-house labels are
    /// all shown in the current diagnostic.
    pub shown: bool,
}

/// One door leaf in the interior-index diagnostic overlay.
#[derive(Clone, Copy)]
pub struct InteriorDoor {
    pub at:    openshard_protocol::world::Point,
    /// The live leaf state: `true` after it has swung open, `false` while shut.
    pub shown: bool,
}

/// One height-changing transition in the building graph.
#[derive(Clone, Copy)]
pub struct InteriorStair {
    pub from: openshard_protocol::world::Point,
    pub to:   openshard_protocol::world::Point,
}

/// The visible map region's current interior-index reading.
pub struct InteriorOverlay {
    pub cells:     Vec<InteriorCell>,
    pub doors:     Vec<InteriorDoor>,
    pub stairs:    Vec<InteriorStair>,
    pub buildings: usize,
}

/// One occlusion surface in the painter order the wireframe needs.
#[derive(Clone, Copy)]
pub struct OccluderSurface {
    pub x:     i32,
    pub y:     i32,
    pub solid: openshard_client_render::occlusion::Solid,
}

/// A planned route, split at the first obstacle the path cannot cross.
pub struct Route {
    pub open:    Vec<openshard_protocol::world::Point>,
    pub barred:  Vec<openshard_protocol::world::Point>,
    /// Why this route does not end on the destination, when it does not.
    ///
    /// A walk *toward* an unreachable place and a walk *to* a reachable one are
    /// the same list of steps, and drawing them the same way is what makes a
    /// body walking into a wall look like a client that has lost its mind. See
    /// [`crate::steer::Refusal`], and `draw_route`, which dashes the line and
    /// marks where it gives up.
    pub refusal: Option<crate::steer::Refusal>,
}

/// A look the shard's own rule was asked for, ready to draw.
///
/// **The shard's rule, not a picture of it.** The trace comes out of
/// `openshard_movement::sight::trace` — the same call `sight_clear` is, and the
/// same call a shot flies along — over this client's own map and its own
/// live overlay. See `docs/combat/design_sight.md`'s D1 and D3, and the limit D3 names: the
/// live half is what the shard has told this client about.
pub struct SightLine {
    /// The walk itself: every tile crossed, the ray's height over each, and
    /// where it stopped.
    pub trace:     openshard_movement::sight::SightTrace,
    /// Whether this is the look at the mobile the shard says we are attacking,
    /// rather than at the tile under the cursor. The first is the question the
    /// shard is really asking; the second is a person surveying a piece of map.
    pub at_quarry: bool,
    /// The reach the picture draws its limit at — the knob's number, not the
    /// shard's, because the shard does not send one. See
    /// [`GraphicsSettings::sight_reach`](crate::graphics::GraphicsSettings::sight_reach).
    ///
    /// It rides here rather than being read off the settings at draw time so
    /// that the drawn picture and the words beside it are one snapshot: a knob
    /// turned between them would put a line and a verdict from two different
    /// reaches on the same frame.
    pub reach:     RangedRange,
}

impl SightLine {
    /// How far the aim is, in tiles — the same Chebyshev count the shard's reach
    /// test is decided by.
    #[must_use]
    pub fn distance(&self) -> u32 {
        self.trace.from.distance(self.trace.to)
    }

    /// Whether the aim is inside [`reach`](Self::reach).
    ///
    /// **The other half of a refusal.** A shot is barred by this or by the ray,
    /// and the ray is all the trace knows about: a clear line to something
    /// fourteen tiles from a bow that reaches ten is a look that gets there and
    /// an arrow that does not.
    #[must_use]
    pub fn within_reach(&self) -> bool {
        self.distance() <= u32::from(self.reach.get())
    }

    /// How many of [`trace`](Self::trace)'s steps stand inside the reach.
    ///
    /// The count, and so also the index of the first step outside it: what the
    /// picture needs to know where to change colour. Every step is *measured*
    /// from the archer rather than counted off along the line, so that the place
    /// the colour changes is the same arithmetic the shard's refusal is, and not
    /// an assumption about how Bresenham's walk advances.
    #[must_use]
    pub fn steps_within_reach(&self) -> usize {
        let reach = u32::from(self.reach.get());
        self.trace
            .steps
            .iter()
            .take_while(|step| {
                let tile = openshard_protocol::world::Point {
                    x: step.tile.x,
                    y: step.tile.y,
                    z: self.trace.from.z,
                };
                self.trace.from.distance(tile) <= reach
            })
            .count()
    }
}

/// One overhead health line, anchored in world-viewport pixels.
///
/// Its colour remains a presentation decision: the query returns the wire's
/// notoriety, and an adapter such as egui resolves that fact for its palette.
pub struct HealthBar {
    /// Top-centre of the body sprite, in the world's viewport.
    pub anchor:    ViewPixel,
    /// Current hit points in the same scale as [`max`](Self::max).
    pub current:   HealthPoints,
    /// The short-lived visual estimate, which follows [`current`](Self::current)
    /// after damage or healing rather than snapping with the wire packet.
    pub estimated: HealthPoints,
    /// Mana is available only for the local player: the wire does not disclose
    /// other mobiles' mana in the ordinary world update.
    pub mana:      Option<ResourceBar>,
    /// Maximum hit points in the scale the shard chose for this body.
    pub max:       HealthPoints,
    /// The wire fact the presentation uses to choose the bar colour.
    pub notoriety: Notoriety,
    /// Whether this body is the attack target the shard settled on.
    pub targeted:  bool,
}

/// One overhead preparation bar, anchored in world-viewport pixels.
///
/// `docs/combat/evidence/2026-08-27-the-action-phases.md`'s Ф4. The pair of packets
/// Ф1 put on the wire finally
/// reaches a screen here: what a fighter is committed to, how far into it they
/// are, and — for a moment after it is over — how it ended. The colour and the
/// glyph stay presentation decisions, the same division [`HealthBar`] makes with
/// notoriety: this carries the wire's facts and an adapter such as egui resolves
/// them for its palette.
pub struct ActionBar {
    /// Top-centre of the body sprite, in the world's viewport. The same anchor
    /// [`HealthBar`] uses, so the two stack rather than being placed twice.
    pub anchor:   ViewPixel,
    /// What this body is doing about fighting, and how far into it.
    pub progress: crate::crowd::ActionProgress,
}

/// A current/max resource in the health-bar scale.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ResourceBar {
    pub current: HealthPoints,
    pub max:     HealthPoints,
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
    pub tile:       Option<PickedTile>,
    /// The eight tiles around [`Pick::tile`], for its wireframe ring.
    pub neighbours: Vec<PickedTile>,
    /// The map static under the cursor when no mobile or item is nearer.
    pub static_:    Option<PickedStatic>,
    /// The highlighted mobile's transient frame index.
    pub mobile:     Option<openshard_client_render::mobiles::MobileIndex>,
    /// The highlighted ground item's transient frame index.
    pub item:       Option<openshard_client_render::items::ItemIndex>,
}

/// What this client has by way of a coarse navigation graph, and what it is
/// doing about it.
///
/// Three states and no fourth: a client either has one, is building one, or has
/// none. It is a diagnostic type because the *only* thing that reads it is the
/// HUD — a route asks `Resources::coarse` and gets a graph or `None`, which is
/// all a search can do anything with — but it is not a diagnostic *detail*: with
/// no graph, a click that has to leave a building is refused, and a person is
/// owed the difference between "there is no way there" and "this client cannot
/// see that far yet".
#[derive(Clone, Debug)]
pub enum Navigation {
    /// Nothing baked beside this world, and nothing building one.
    ///
    /// The ordinary state of a client on an install with no artifact beside it,
    /// and of one whose bake failed — the terminal says which.
    Absent,
    /// A worker is building one, since this instant.
    ///
    /// Held as the start rather than as an elapsed time because the HUD is what
    /// counts it up, one frame at a time, and a duration written here would be
    /// as old as the last update that carried it.
    Baking { since: std::time::Instant },
    /// One is loaded: its size, and the file it came out of or was kept in.
    Ready {
        regions: usize,
        nodes:   usize,
        edges:   usize,
        path:    std::path::PathBuf,
    },
}

/// A read-only frame snapshot for the development HUD or another inspector.
///
/// This deliberately sits outside the egui adapter: its facts can equally be
/// sent to a frame dump or a future remote inspector.
pub struct Hud {
    pub locked: bool,
    pub rig: Rig,
    /// Last `Walk` request-to-acknowledgement transport round trip.
    pub ping: Option<std::time::Duration>,
    /// Time that acknowledgement waited for the window event loop.
    pub ping_app_delivery: Option<std::time::Duration>,
    pub perf: crate::frames::Perf,
    pub scripts: Vec<&'static str>,
    pub replay: Option<(&'static str, f32)>,
    pub pick: Pick,
    pub hover_lit: bool,
    pub highlight: HighlightTarget,
    pub highlight_style: HighlightStyle,
    pub selected: Option<Selection>,
    /// Tiles changed by the local, uncommitted map-editor draft.
    pub editor_preview: Vec<crate::editor_mode::PreviewTile>,
    /// Where the next static-placement click would anchor its sprite.
    pub editor_static_preview: Option<(openshard_protocol::world::Point, Graphic)>,
    /// Unpublished statics already placed in the local draft.
    pub editor_static_draft: Vec<openshard_map::map::StaticItem>,
    pub health_bars: Vec<HealthBar>,
    /// What each visible body is part way through committing, and how the last
    /// one ended. Every watcher's, not only the player's: the packets are
    /// broadcast, and an archer at full draw across the street is the picture
    /// the telegraph was built for.
    pub action_bars: Vec<ActionBar>,
    pub draw: openshard_client_render::frame::Draw,
    pub cutaway_disabled: bool,
    pub body_overlap_transparency_disabled: bool,
    /// Whether the server's time of day controls ambient light this frame.
    pub time_of_day: bool,
    /// Whether the local night-lighting comparison is on this frame.
    pub night: bool,
    pub show_terrain: bool,
    pub terrain: Option<Arc<TerrainOverlay>>,
    pub show_interiors: bool,
    pub interiors: Option<Arc<InteriorOverlay>>,
    pub buildings: bool,
    pub z_slice: bool,
    pub z_slice_view: openshard_client_render::interiors::ZSliceView,
    pub floor_view: openshard_client_render::interiors::FloorView,
    pub route: Option<Arc<Route>>,
    /// Whether the sight overlay is on this frame.
    pub show_sight: bool,
    /// The look it draws, when it is on and there is something to look at.
    pub sight: Option<Arc<SightLine>>,
    /// The reach its knob presently names, so the strip can draw the knob at the
    /// number the picture was built with.
    pub sight_reach: RangedRange,
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
    /// The radar's chosen levels, fallback tally and three budgets.
    pub radar: RadarTelemetry,
    /// Whether this frame draws the supplied TrueType face rather than
    /// `fonts.mul`.
    pub ttf_active: bool,
    /// Whether F1 can choose the supplied TrueType face at all.
    pub ttf_available: bool,
    /// The coarse graph: one of it, building, or none — see [`Navigation`].
    pub navigation: Navigation,
    /// Why the standing move order is not reaching its destination, if it is
    /// not — see [`crate::steer::Refusal`].
    ///
    /// The journal has the player's copy of this and is said once; this is the
    /// one that stands, for somebody reading the strip while the body walks at
    /// a wall.
    pub refusal: Option<crate::steer::Refusal>,
}

/// What one frame's radar demand and production resolved to.
///
/// Written by the radar block of `App::draw_from` and read by `App::hud` on
/// the **next** frame: the HUD is assembled near the top of a frame, before
/// the views are built and long before the producer runs, so reading these
/// live would report a frame's worth of nothing. Kept whole rather than as
/// parallel fields on `App` so that the level, the tally and the cost a reader
/// compares are all from the same frame.
#[derive(Clone, Default)]
pub struct RadarFrame {
    /// The level each open radar window chose, and the `tiles_per_pixel` it
    /// chose it from. The input is reported beside the output because the
    /// selection has a 10% dead band on each boundary: without it, a view
    /// sitting one notch inside a band is indistinguishable from a selector
    /// that has stopped responding.
    pub levels: Vec<(
        crate::windows::WindowSubject,
        openshard_client_render::radar::RadarLod,
        f32,
    )>,
    /// How every requested chunk was answered. Counted over the *protected*
    /// set — every key every open view's region names at that view's chosen
    /// level — which is the set `draw_radar_view` looks up a moment later, so
    /// this tallies the picture rather than a second opinion about it.
    pub demand: openshard_client_render::radar::RadarDemand,
    /// What this frame's producer turn spent walking the map and colouring
    /// tiles. The whole of the radar's synchronous CPU cost.
    pub raster: std::time::Duration,
    /// Chunks that turn actually published.
    pub built:  usize,
}

/// Every radar counter the development HUD reads, gathered in one place.
///
/// Three of the four come from live state and one — [`Self::frame`] — is the
/// previous frame's, for the reason that type's own doc gives. They are
/// presented together anyway: the question this panel exists to answer is
/// whether a level, a fallback tally and a budget are consistent with one
/// another, and a frame's lag does not move any of them far enough to change
/// that reading.
#[derive(Clone, Default)]
pub struct RadarTelemetry {
    pub frame: RadarFrame,
    pub cache: openshard_client_render::radar::RadarCacheCounters,
    pub queue: openshard_client_render::radar::RadarWorkCounters,
    pub pages: openshard_client_render::radar_pass::RadarPageCounters,
}

/// Read-only map-composite producer and cache counters for the development HUD.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompositeTelemetry {
    pub ready:             usize,
    pub pending:           usize,
    pub prepared:          usize,
    pub in_flight:         usize,
    pub gpu_bytes:         u64,
    pub gpu_budget_bytes:  u64,
    /// Blocks deliberately held at the safe direct LOD0 path this session.
    pub quarantined:       usize,
    /// Most recent block/key/source-owner proof and its safety reason.
    pub latest_quarantine: Option<openshard_client_render::composite::CompositeQuarantine>,
}
