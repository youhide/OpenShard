//! What the shard — or its absence — has said: [`WorldState`].
//!
//! Every field here is a projection of the last `WorldView` the connection
//! produced, or of the clocks that age it, and none of it is read from disk —
//! see [`crate::resources::Resources`] for that half, and
//! [`crate::graphics::GraphicsSettings`] for the person's own view of it.
//! Pulled out of [`crate::App`] for the same reason both of those were: the
//! fields here change together, on every `Update::World`, and a method that
//! only touches this half can be written and tested against it alone.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::time::{Duration, Instant};

use openshard_client_net::view::WorldView;
use openshard_client_render::animate::StaticAnimations;
use openshard_client_render::camera::Camera;
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::follow::Gaze;
use openshard_client_render::items::GroundItem;
use openshard_client_render::mobiles::Mobile;
use openshard_client_render::statics::StaticGeometry;
use openshard_protocol::direction::Facing;
use openshard_protocol::serial::Serial;
use openshard_protocol::wire::Hue;
use openshard_protocol::world::Point;
use openshard_uofiles::map::Map;
use openshard_uofiles::tiledata::TileData;

use crate::crowd::{Crowd, Who};
use crate::{clutter, link, resources};

/// How long a damage number remains over the mobile it struck.
pub const DAMAGE_NUMBER_HOLD: Duration = Duration::from_secs(1);
/// Vertical distance, in world pixels, a damage number travels during its hold.
pub const DAMAGE_NUMBER_RISE: i32 = 28;
/// How far apart stacked overhead lines sit, in world pixels.
///
/// A constant rather than the drawn line's own height: the text is measured only
/// once it reaches the atlas, three stages after the position is decided, and a
/// spacing that varied with the glyphs would make a mobile's lines jump as one
/// of them expires.
pub const SPEECH_LINE_HEIGHT: i32 = 15;
/// How long a health estimate takes to settle on a newly confirmed value.
pub const HEALTH_ESTIMATE_LAG: Duration = Duration::from_millis(450);

/// A short-lived combat number shown over a mobile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DamageNumber {
    pub serial: Serial,
    pub amount: DamageAmount,
    /// Colour distinguishes damage received by the local player from damage
    /// shown over another mobile.
    pub hue: Hue,
    pub elapsed: Duration,
}

/// The delayed part of an overhead health bar. The shard's current health is
/// never replaced by this value; it only leaves a readable red/green trail for
/// a hit, a heal, and a sequence of DoT ticks.
#[derive(Clone, Copy, Debug)]
pub struct HealthEstimate {
    from: u16,
    target: u16,
    elapsed: Duration,
}

impl HealthEstimate {
    fn shown(self) -> u16 {
        let progress = (self.elapsed.as_secs_f32() / HEALTH_ESTIMATE_LAG.as_secs_f32()).min(1.0);
        (f32::from(self.from) + (f32::from(self.target) - f32::from(self.from)) * progress)
            .round()
            .clamp(0.0, f32::from(u16::MAX)) as u16
    }

    fn settled(self) -> bool {
        self.elapsed >= HEALTH_ESTIMATE_LAG
    }
}

/// Damage in the health-bar scale, kept distinct from other wire `u16`s.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DamageAmount(u16);

impl DamageAmount {
    pub const fn new(value: u16) -> Self {
        Self(value)
    }
}

impl fmt::Display for DamageAmount {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(out)
    }
}

/// What the connection has told this client the world looks like — see the
/// module docs.
pub struct WorldState {
    /// The server's last complete word, and only that. It is owned and mutated
    /// on the application thread; render data is rebuilt from it below rather
    /// than sharing or mutating this record from another thread.
    pub authoritative: AuthoritativeWorld,
    /// The sole app-thread owner of player movement.  Presentation, planning,
    /// camera following, and the HUD query this rather than maintaining their
    /// own position records.
    pub motion: PlayerMotion,
    /// The renderer-facing projection rebuilt from authoritative state and
    /// prediction before a frame is drawn.
    pub presentation: PresentationWorld,
    /// Whether a world picture is safe to show. The offline viewer starts
    /// ready; a connected client becomes ready only when the shard has sent its
    /// first complete [`WorldView`]. Until then the presentation's placeholder
    /// is state for startup mechanics, not a picture for the player.
    pub render_ready: bool,
    /// What the connection is doing, for the status strip.
    pub connection: String,
    /// The shard: whether there is one, and — if there is not — whether there
    /// ever was.
    pub shard: Shard,
}

/// What is on the other end of this client, in the one field that decides it.
///
/// # Why three states and not an `Option`
///
/// Because two of them are *not having a shard* and they mean opposite things.
/// A client with no shard walks the body itself — that is the map viewer, and
/// the whole reason `App::walk` has an offline arm. A client that has **lost**
/// one must not: the body's position is the shard's fact, the last one it
/// stated, and a client that keeps stepping it draws a character somewhere
/// nobody ever put it.
///
/// This was an `Option<Link>`, and the missing distinction is what let a
/// disconnect pass for a game that had gone strange: the socket died on an
/// unframable packet, the link went to `None`, and the arrows kept working
/// because `None` was also how a map viewer says it is a map viewer. Everything
/// else quietly stopped — the paperdoll returned without asking, speech went to
/// the log — so the one thing that still answered was the one thing written
/// twice. A second `lost: bool` beside the `Option` would put the same state in
/// two shapes and let them disagree; this is one field with one answer.
pub enum Shard {
    /// No shard was ever dialled: this run is the offline map viewer.
    Viewer,
    /// Connected. What the keyboard asks: a step is a `0x02` when there is
    /// somebody to send it to.
    Live(link::Link),
    /// There was one and the connection ended, with the reason it ended for.
    /// Nothing is sent from here on, and nothing local moves in a shard's
    /// place — see [`WorldView::shard_lost`](openshard_client_net::view::WorldView::shard_lost)
    /// for the other half, which puts the world it described out.
    Lost(String),
}

impl Shard {
    /// Somewhere to send to, or `None` — the question every action asks, and
    /// the only one that does not care *why* there is nobody.
    #[must_use]
    pub fn link(&self) -> Option<&link::Link> {
        match self {
            Self::Live(link) => Some(link),
            Self::Viewer | Self::Lost(_) => None,
        }
    }

    /// Whether this client is the map viewer — the one state in which it is
    /// this end's business to move the body. Deliberately not `!is_live()`:
    /// see the type's docs for what that conflation cost.
    #[must_use]
    pub fn is_viewer(&self) -> bool {
        matches!(self, Self::Viewer)
    }
}

/// Render-facing data rebuilt from the authoritative view and local prediction.
/// This is the only `WorldState` section a frame reads.
pub struct PresentationWorld {
    /// Static animation state and its flame clock, advanced whenever
    /// presentation time progresses.
    pub tile_animations: StaticAnimations,
    pub flame_clock: Duration,
    /// The player's rendered body. Its position and facing are projected from
    /// [`PlayerMotion`]; body, hue and equipment come from the shard.
    pub player: Mobile,
    /// The guarded cutaway tile. It may deliberately lag a doomed prediction.
    pub cutaway_at: Point,
    /// Persistent opacity for world objects moving into or out of cutaway.
    pub cutaway_fades: openshard_client_render::cutaway::Fades,
    /// Last reusable map-static collection. Dynamic server items remain live.
    pub static_geometry_cache: Option<StaticGeometryCache>,
    /// Per-block and per-building interior topology. It is changed only while
    /// a frame resolves its immutable picture policy.
    pub interior_cache: RefCell<InteriorCache>,
    /// Render mobiles beside the identity their animation clocks use.
    pub others: Vec<(Who, Mobile)>,
    /// Item corpses projected through the mobile renderer. Their serial remains
    /// an item serial, so double-clicking one still opens its loot container.
    pub corpses: Vec<(Who, Mobile)>,
    /// Ground-item render data and the parallel wire serials used for picks.
    pub items: Vec<GroundItem>,
    pub item_serials: Vec<Serial>,
    /// The house a `0x99` cursor is drawing under the pointer, expanded into its
    /// pieces. Empty whenever no multi cursor is up.
    ///
    /// **Deliberately not in [`items`](Self::items)**, and the two reasons are
    /// different from each other. It has no serial, so appending it would desync
    /// `item_serials`, which picking indexes by position — and it must not be
    /// pickable in any case, because it is not a thing in the world. And it
    /// moves with the *pointer* rather than with the item list, so it is
    /// rebuilt on a different clock from everything beside it.
    ///
    /// It is chained onto `items` at the one call that hands them to
    /// `frame::Inputs`, so the renderer never learns there were two lists.
    pub multi_preview: Vec<GroundItem>,
    /// Damage numbers are presentation events, not authoritative world state.
    pub damage_numbers: Vec<DamageNumber>,
    /// Presentation-only delayed health, keyed by the mobile the shard named.
    pub health_estimates: BTreeMap<Serial, HealthEstimate>,
    /// The corresponding transient obstacles used by local movement.
    pub clutter: clutter::Clutter,
    /// Animation and glide history, which belongs to presentation rather than
    /// authoritative state.
    pub crowd: Crowd,
}

/// Every input that can change cached map-static geometry.
///
/// This is deliberately one value: adding a drawing predicate must mean adding
/// one field here, rather than remembering to extend two parallel argument
/// lists in [`StaticGeometryCache`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct StaticGeometryCacheKey {
    camera: Camera,
    cutaway: Cutaway,
    interior: Option<u64>,
    atlas_revision: u64,
    player_mask: Option<u64>,
    has_occlusion: bool,
    animation_tick: u128,
    items_fingerprint: u64,
}

impl StaticGeometryCacheKey {
    pub const fn new(
        camera: Camera,
        cutaway: Cutaway,
        interior: Option<u64>,
        atlas_revision: u64,
        player_mask: Option<u64>,
        has_occlusion: bool,
        animation_tick: u128,
        items_fingerprint: u64,
    ) -> Self {
        Self {
            camera,
            cutaway,
            interior,
            atlas_revision,
            player_mask,
            has_occlusion,
            animation_tick,
            items_fingerprint,
        }
    }
}

/// A map-static result whose every input that can alter opaque/cutaway pixels
/// is recorded. It is intentionally conservative: callers only populate it
/// for non-animated, fully opaque collections with no fade in progress.
#[derive(Debug)]
pub struct StaticGeometryCache {
    key: StaticGeometryCacheKey,
    geometry: StaticGeometry,
}

/// The durable map work behind one building picture.
#[derive(Default, Debug)]
pub struct InteriorCache {
    pub index: openshard_client_render::interiors::Index,
    pub buildings: BTreeMap<u32, InteriorBuilding>,
}

/// A complete, immutable room/floor graph for one label in the facet artifact.
#[derive(Debug)]
pub struct InteriorBuilding {
    pub rooms: openshard_client_render::interiors::StitchedRooms,
    pub buildings: openshard_client_render::interiors::Buildings,
}

impl StaticGeometryCache {
    pub fn new(key: StaticGeometryCacheKey, geometry: StaticGeometry) -> Self {
        Self { key, geometry }
    }

    pub fn matches(&self, key: StaticGeometryCacheKey) -> bool {
        self.key == key
    }

    pub fn geometry(&self) -> &StaticGeometry {
        &self.geometry
    }
}

impl PresentationWorld {
    /// Advance every clock that changes a presentation independently of a
    /// newly received world view.
    ///
    /// A packet can arrive between frames. It has to age the complete picture
    /// before its mutation is projected, or moving `last_advance` there would
    /// silently discard that span from static and flame animation.
    pub(crate) fn advance(&mut self, elapsed: Duration) {
        self.crowd.advance(elapsed);
        self.tile_animations.advance(elapsed);
        self.flame_clock += elapsed;
        for number in &mut self.damage_numbers {
            number.elapsed += elapsed;
        }
        self.damage_numbers
            .retain(|number| number.elapsed < DAMAGE_NUMBER_HOLD);
        for estimate in self.health_estimates.values_mut() {
            estimate.elapsed += elapsed;
        }
        self.health_estimates.retain(|_, estimate| !estimate.settled());
    }

    /// Show the damage the last health update established for one mobile.
    pub(crate) fn damage(&mut self, serial: Serial, amount: u16, hue: Hue) {
        if amount > 0 {
            self.damage_numbers.push(DamageNumber {
                serial,
                amount: DamageAmount::new(amount),
                hue,
                elapsed: Duration::ZERO,
            });
        }
    }

    /// Start (or retarget) the fake-health interpolation at a newly confirmed
    /// value. Retargeting from the value currently on screen preserves every
    /// individual DoT tick instead of making a busy fight visibly jump.
    pub(crate) fn health_changed(&mut self, serial: Serial, previous: u16, current: u16) {
        let from = self
            .health_estimates
            .get(&serial)
            .copied()
            .map_or(previous, HealthEstimate::shown);
        if from != current {
            self.health_estimates.insert(
                serial,
                HealthEstimate {
                    from,
                    target: current,
                    elapsed: Duration::ZERO,
                },
            );
        }
    }

    pub(crate) fn estimated_health(&self, serial: Serial, current: u16) -> u16 {
        self.health_estimates
            .get(&serial)
            .copied()
            .map_or(current, HealthEstimate::shown)
    }
}

/// Move a presentation to one measured instant.
///
/// The App calls this before applying a network update as well as before it
/// prepares a frame. Keeping the instant beside the clock update makes it
/// impossible for one caller to move `last_advance` while forgetting a clock.
pub(crate) fn advance_presentation_to(
    presentation: &mut PresentationWorld,
    motion: &mut PlayerMotion,
    last_advance: &mut Instant,
    now: Instant,
) {
    let elapsed = now.saturating_duration_since(*last_advance);
    motion.advance_with_ease(elapsed, presentation.crowd.ease());
    presentation.advance(elapsed);
    *last_advance = now;
}

/// The one authoritative record the shard updates, kept apart from the
/// presentation projection and local prediction state in [`WorldState`].
pub struct AuthoritativeWorld {
    /// The last thing the server said, whole. Kept for the HUD and as the sole
    /// source from which this app rebuilds render projections.
    pub view: Option<Box<WorldView>>,
    /// Whether the shard's facet has been compared with the one loaded. See
    /// `App::entered`: once, because it cannot change without a `0xBF 0x08`
    /// nothing here reads yet.
    pub facet_checked: bool,
    /// The shape of every **designed** house this client has been sent, by the
    /// house's own serial.
    ///
    /// It is here rather than in `WorldView` because a design is a list of
    /// `Component`s — a client-file type — and `client/net` is the wire and has
    /// never depended on `openshard-uofiles`. The view holds the *revision* the
    /// shard named; this holds what was made of it, and the two together are
    /// what says whether to ask again.
    ///
    /// Empty on every shard where nobody has designed a house, which is every
    /// shard today.
    pub designs: std::collections::HashMap<Serial, HouseShape>,
}

/// A designed house's picture, as this client holds it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HouseShape {
    /// The revision the `0xD8` that filled this carried. Compared against the
    /// one `WorldView::designs` holds to decide whether to ask again.
    pub revision: u32,
    /// The tiles the house draws as.
    pub components: Vec<openshard_uofiles::multi::Component>,
}

/// The player movement coordinator.
///
/// It deliberately joins, but does not merge, two independently valid cores:
/// [`NetworkMotion`] is the integer-grid protocol state, while [`GameMotion`]
/// is the continuous pose a frame draws.  `Crowd` may animate the player's
/// body, but is never a source of either position.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerMotion {
    network: NetworkMotion,
    game: GameMotion,
}

/// Discrete, server-synchronised movement state.
///
/// Every point in here is a tile the protocol can name.  It has no clock and
/// no fractional coordinate, so a normal world packet cannot accidentally
/// advance the drawn player.
#[derive(Clone, Debug, PartialEq)]
struct NetworkMotion {
    /// The latest position explicitly confirmed by the shard.
    pub confirmed: openshard_client_net::walk::Predicted,
    /// The end of the locally accepted step chain.
    pub predicted: openshard_client_net::walk::Predicted,
    pending: VecDeque<PendingStep>,
    /// The transition currently drawn, followed by transitions accepted while
    /// the application was busy.  Predictions have protocol identity and must
    /// not be collapsed into one longer, faster glide.
    transitions: VecDeque<MotionTransition>,
    corrected: bool,
}

/// A local step waiting for the one protocol outcome that can retire it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingStep {
    sequence: openshard_protocol::world::StepSequence,
}

/// The logical endpoints that the network core gave to the game core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MotionTransition {
    from: openshard_client_net::walk::Predicted,
    to: openshard_client_net::walk::Predicted,
}

/// Continuous player pose, separate from the protocol's integer-grid state.
/// The camera and player sprite read this core; `Crowd` supplies animation only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GameMotion {
    /// The exact pose produced by the movement clock.  It remains separate
    /// from `drawn`: the next step must begin on schedule even while the
    /// picture deliberately eases a few pixels behind it.
    walked: Gaze,
    drawn: Gaze,
    transition: Option<GameTransition>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct GameTransition {
    from: Gaze,
    to: Gaze,
    elapsed: Duration,
    takes: Duration,
}

/// Named values for interfaces that report movement without inspecting a
/// `Mobile` or interpolation clock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HudMotionState {
    pub confirmed: openshard_client_net::walk::Predicted,
    pub predicted: openshard_client_net::walk::Predicted,
    pub route_origin: Point,
    pub pending_steps: usize,
}

/// The complete movement input required to project the player into a renderer.
/// It is intentionally independent of `Mobile`: appearance is not movement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MotionRenderState {
    /// The discrete endpoint of the transition currently being rendered.  It
    /// is intentionally not necessarily the newest local prediction: later
    /// numbered steps may be queued behind this one.
    pub rendered: openshard_client_net::walk::Predicted,
    pub predicted: openshard_client_net::walk::Predicted,
    pub transition: Option<(Point, Point)>,
    pub corrected: bool,
}

/// One internally consistent motion observation for diagnostics.  Keeping the
/// trace input as a value prevents diagnostic code from accidentally comparing
/// a fresh logical prediction with a stale presentation clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionSnapshot {
    pub confirmed: openshard_client_net::walk::Predicted,
    pub predicted: openshard_client_net::walk::Predicted,
    pub rendered: Gaze,
    pub route_origin: Point,
    pub pending_steps: usize,
    pub transition: Option<(Point, Point)>,
}

impl PlayerMotion {
    pub fn new(at: Point, facing: Facing) -> Self {
        let standing = openshard_client_net::walk::Predicted { position: at, facing };
        Self {
            network: NetworkMotion {
                confirmed: standing,
                predicted: standing,
                pending: VecDeque::new(),
                transitions: VecDeque::new(),
                corrected: false,
            },
            game: GameMotion {
                walked: Gaze::on(at),
                drawn: Gaze::on(at),
                transition: None,
            },
        }
    }

    /// Atomically accept one local protocol step and name the transition it
    /// starts.  The sequence survives until its matching acknowledgement.
    pub fn accept_local(&mut self, body: link::Body, sequence: openshard_protocol::world::StepSequence) {
        let from = self.network.predicted;
        self.network.predicted = body.predicted;
        self.network.corrected = false;
        self.network.pending.push_back(PendingStep { sequence });
        self.start_transition(from, self.network.predicted);
        self.debug_assert_valid();
    }

    /// Incorporate a fact delivered by a packet.  A non-movement packet is a
    /// no-op by construction; it cannot alter either movement anchor.
    pub fn accept_network(&mut self, movement: Option<link::Movement>) {
        let Some(movement) = movement else {
            return;
        };
        self.network.confirmed = movement.confirmed();
        self.network.corrected = matches!(
            movement,
            link::Movement::Reject { .. } | link::Movement::Relocation { .. }
        );
        match movement {
            link::Movement::Ack { sequence, .. } => {
                let pending = self.network.pending.pop_front().map(|step| step.sequence);
                debug_assert_eq!(
                    pending,
                    Some(sequence),
                    "walk acknowledgement must retire its own pending step"
                );
            }
            link::Movement::Reject { sequence, confirmed } => {
                debug_assert_eq!(
                    self.network.pending.front().map(|step| step.sequence),
                    Some(sequence),
                    "walk rejection must name the oldest pending step"
                );
                self.network.predicted = confirmed;
                self.network.pending.clear();
                self.network.transitions.clear();
                self.game.snap(confirmed.position);
            }
            link::Movement::Relocation { confirmed } => {
                self.network.predicted = confirmed;
                self.network.pending.clear();
                self.network.transitions.clear();
                self.game.snap(confirmed.position);
            }
            link::Movement::Turn { confirmed } => {
                // A combat turn is emitted independently of the walk
                // handshake.  Preserve an in-flight walk, but show the new
                // direction immediately when standing still.
                if self.network.pending.is_empty() {
                    self.network.predicted = confirmed;
                }
            }
        }
        self.debug_assert_valid();
    }

    /// Put down an offline/replay movement which has no protocol identity.
    pub fn set_local(&mut self, at: Point, facing: Facing) {
        let standing = openshard_client_net::walk::Predicted { position: at, facing };
        self.network.confirmed = standing;
        self.network.predicted = standing;
        self.network.pending.clear();
        self.network.transitions.clear();
        self.game.snap(at);
        self.network.corrected = false;
        self.debug_assert_valid();
    }

    /// Accept a step from a source that is authoritative immediately, such as
    /// the offline map viewer.  It shares the online path's transition
    /// ownership without inventing a protocol-pending step.
    pub fn accept_trusted_step(&mut self, at: Point, facing: Facing) {
        let from = self.network.predicted;
        let to = openshard_client_net::walk::Predicted { position: at, facing };
        self.network.confirmed = to;
        self.network.predicted = to;
        self.network.pending.clear();
        self.network.corrected = false;
        self.start_transition(from, to);
        self.debug_assert_valid();
    }

    /// Seed a newly-entered world from the server's initial position.
    pub fn reset(&mut self, body: link::Body) {
        self.network.confirmed = body.predicted;
        self.network.predicted = body.predicted;
        self.network.pending.clear();
        self.network.transitions.clear();
        self.game.snap(body.predicted.position);
        self.network.corrected = body.corrected;
        self.debug_assert_valid();
    }

    /// The tile from which the current route should be drawn or extended.
    pub fn route_origin(&self) -> Point {
        match self.network.transitions.front() {
            Some(transition) => transition.from.position,
            None => self.network.predicted.position,
        }
    }

    /// The authoritative starting state for the next movement decision.
    pub const fn planning_state(&self) -> openshard_client_net::walk::Predicted {
        self.network.predicted
    }

    /// The last player position the shard explicitly confirmed.
    #[cfg(test)]
    pub const fn confirmed_state(&self) -> openshard_client_net::walk::Predicted {
        self.network.confirmed
    }

    /// The stable movement snapshot for HUDs and diagnostics.
    pub fn hud_state(&self) -> HudMotionState {
        HudMotionState {
            confirmed: self.network.confirmed,
            predicted: self.network.predicted,
            route_origin: self.route_origin(),
            pending_steps: self.network.pending.len(),
        }
    }

    /// The renderer's discrete projection. Fractional placement comes only
    /// from `GameMotion::drawn`.
    pub fn render_state(&self) -> MotionRenderState {
        let active = self.network.transitions.front().copied();
        MotionRenderState {
            rendered: active.map_or(self.network.predicted, |transition| transition.to),
            predicted: self.network.predicted,
            transition: active.map(|transition| (transition.from.position, transition.to.position)),
            corrected: self.network.corrected,
        }
    }

    /// Capture all diagnostic movement values from this one state owner.
    pub fn snapshot(&self) -> MotionSnapshot {
        MotionSnapshot {
            confirmed: self.network.confirmed,
            predicted: self.network.predicted,
            rendered: self.game.drawn,
            route_origin: self.route_origin(),
            pending_steps: self.network.pending.len(),
            transition: self
                .network
                .transitions
                .front()
                .map(|transition| (transition.from.position, transition.to.position)),
        }
    }

    #[cfg(test)]
    pub fn advance(&mut self, elapsed: Duration) {
        self.advance_with_ease(elapsed, crate::crowd::Ease::NONE);
    }

    /// Advance the movement core and apply the current presentation policy to
    /// its pose.  `Crowd` supplies the user-selected policy, but never the
    /// local player's position: that remains owned by this core.
    pub fn advance_with_ease(&mut self, elapsed: Duration, ease: crate::crowd::Ease) {
        let mut remaining = elapsed;
        while self.game.transition.is_some() {
            remaining = self.game.advance(remaining);
            if self.game.transition.is_some() {
                break;
            }
            self.network.transitions.pop_front();
            let Some(next) = self.network.transitions.front().copied() else {
                break;
            };
            self.game
                .start(next.from.position, next.to.position, next.to.facing.running);
            if remaining.is_zero() {
                break;
            }
        }
        self.game.ease(ease, elapsed);
        self.debug_assert_valid();
    }

    pub const fn drawn(&self) -> Gaze {
        self.game.drawn
    }

    /// Whether the local movement core needs display-rate frames.  This does
    /// not consult `Crowd`, which may be rebased after a stalled frame.
    pub const fn is_gliding(&self) -> bool {
        self.game.transition.is_some()
    }

    /// Whether the last accepted movement event was a server correction or
    /// relocation and must be rendered as a snap.
    pub const fn corrected(&self) -> bool {
        self.network.corrected
    }

    #[cfg(test)]
    pub fn pending_steps(&self) -> usize {
        self.network.pending.len()
    }

    pub fn transition_from(&self) -> Option<Point> {
        self.network
            .transitions
            .front()
            .map(|transition| transition.from.position)
    }

    #[cfg(test)]
    pub fn transition_to(&self) -> Option<Point> {
        self.network
            .transitions
            .front()
            .map(|transition| transition.to.position)
    }

    fn start_transition(
        &mut self,
        from: openshard_client_net::walk::Predicted,
        to: openshard_client_net::walk::Predicted,
    ) {
        // A command can arrive at any point between two presentation frames.
        // It is allowed to extend the local chain, but never to rewrite the
        // transition that is already on screen: doing that would visibly cut a
        // stride short whenever a player changes direction quickly.
        let game_before = self.game;
        if from.position == to.position {
            // A turn has no interpolation.  In particular it must not snap an
            // easing tail after the exact crossing finished: changing facing
            // while rapidly circling the cursor used to move the body and the
            // locked camera several pixels in one frame.
            assert_eq!(
                self.game, game_before,
                "a turn command must not interrupt or snap the current visual pose"
            );
            return;
        }
        self.network.transitions.push_back(MotionTransition { from, to });
        if self.game.transition.is_none() {
            self.game.start(from.position, to.position, to.facing.running);
        } else {
            assert_eq!(
                self.game, game_before,
                "a new movement command must queue behind, not interrupt, the active animation"
            );
        }
    }

    /// Invariants that should hold at every app-thread movement boundary.
    /// This checks the boundary between the discrete and continuous cores.
    pub fn debug_assert_valid(&self) {
        if let Some(transition) = self.network.transitions.front() {
            debug_assert_ne!(transition.from.position, transition.to.position);
            let game = self
                .game
                .transition
                .expect("network transition needs game transition");
            debug_assert_eq!(game.to, Gaze::on(transition.to.position));
        }
        debug_assert_eq!(
            self.game.transition.is_some(),
            !self.network.transitions.is_empty()
        );
        if self.network.transitions.is_empty() {
            debug_assert_eq!(
                self.game.walked,
                Gaze::on(self.network.predicted.position),
                "a settled movement clock must be at its standing tile"
            );
        }
        if let Some(last) = self.network.transitions.back() {
            debug_assert_eq!(last.to.position, self.network.predicted.position);
        }
        if self.network.corrected {
            debug_assert!(self.network.pending.is_empty());
            debug_assert!(self.network.transitions.is_empty());
            debug_assert_eq!(self.network.confirmed, self.network.predicted);
            debug_assert_eq!(self.game.walked, Gaze::on(self.network.predicted.position));
            debug_assert_eq!(self.game.drawn, Gaze::on(self.network.predicted.position));
        }
    }
}

impl GameMotion {
    fn start(&mut self, from: Point, to: Point, running: bool) {
        if from == to {
            self.snap(to);
            return;
        }
        self.transition = Some(GameTransition {
            // Starting from the last continuous pose preserves continuity if a
            // trusted source supplies consecutive steps faster than a frame.
            from: self.walked,
            to: Gaze::on(to),
            elapsed: Duration::ZERO,
            takes: openshard_movement::step_hold(running),
        });
    }

    fn snap(&mut self, at: Point) {
        self.walked = Gaze::on(at);
        self.drawn = Gaze::on(at);
        self.transition = None;
    }

    /// Advance one transition and return the part of `elapsed` that belongs to
    /// a queued successor.  A delayed frame may legitimately finish several
    /// whole steps, but it may never compress them into one glide.
    fn advance(&mut self, elapsed: Duration) -> Duration {
        let Some(mut transition) = self.transition else {
            return elapsed;
        };
        let left_to_run = transition.takes.saturating_sub(transition.elapsed);
        let used = elapsed.min(left_to_run);
        transition.elapsed += used;
        let left = 1.0 - openshard_movement::step_progress(transition.elapsed, transition.takes);
        self.walked = transition.to.back_towards(transition.from, left);
        self.transition = (transition.elapsed < transition.takes).then_some(transition);
        elapsed.saturating_sub(used)
    }

    /// Smooth the visual pose after the exact walk has moved.  Keeping this
    /// state beside the authoritative local clock restores the old body ease
    /// without making `Crowd` a second source of local coordinates.
    fn ease(&mut self, ease: crate::crowd::Ease, elapsed: Duration) {
        self.drawn = self.drawn.eased_towards(self.walked, ease.tau, elapsed);
    }
}

impl WorldState {
    /// Who the crowd knows our own body as.
    ///
    /// Our serial once a shard has named us, and `None` for the offline
    /// placeholder — see [`Who`].
    pub fn me(&self) -> Who {
        self.authoritative.view.as_ref().map(|view| view.player.serial)
    }

    /// Where the body is drawn this instant, wherever game motion has it —
    /// [`crate::App::follow_player`]'s reason for calling this every frame.
    pub fn drawn_player(&self) -> Gaze {
        self.motion.drawn()
    }

    /// Whether any presentation movement needs a display-rate redraw.
    pub fn anyone_gliding(&self) -> bool {
        self.motion.is_gliding() || self.presentation.crowd.anyone_gliding()
    }
}

/// The client's own map, unclutted by anything the shard has stood on it —
/// [`cluttered`] is what a step decision should actually ask.
///
/// A facade over [`resources::Resources::map`] and
/// [`resources::Resources::tiledata`], which travel together in every caller
/// that wants either: the split that put them in `Resources` and
/// [`WorldState::clutter`] in `WorldState` is about what changes
/// together (disk-read once against updated every packet), not about who
/// asks for them together, and this is the seam that reunites the two.
///
/// **A free function taking `&Resources`, not an `App` method taking
/// `&self`.** A method on `App` borrows the whole of it, so a caller that
/// also holds `&mut self.steer` beside the terrain — every arrow key and
/// every replanned step does — could no longer compile: the borrow checker
/// sees disjoint *fields* through a chain of `.` projections but not through
/// a method call, which is opaque to it. Passing `&self.resources` here is
/// the same projection the field access always was, just wrapped.
pub(crate) fn terrain(resources: &resources::Resources) -> openshard_movement::MapTerrain<&Map, &TileData> {
    openshard_movement::MapTerrain::new(resources.map.map(), &resources.tiledata)
}

/// [`terrain`] with the shard's own items laid over it — what every step
/// decision on this end should actually ask. See [`clutter::Clutter::over`],
/// and `terrain`'s own docs for why this takes references rather than being
/// an `App` method.
pub(crate) fn cluttered<'a>(
    world: &'a WorldState,
    resources: &'a resources::Resources,
) -> clutter::Cluttered<'a, openshard_movement::MapTerrain<&'a Map, &'a TileData>> {
    world
        .presentation
        .clutter
        .over(resources.map.map(), &resources.tiledata)
}

/// The same, read as though every shut door on it stood open: what a route
/// may be *planned* through, and never what decides a step. See
/// [`clutter::Clutter::over_with_doors_open`].
pub(crate) fn cluttered_with_doors_open<'a>(
    world: &'a WorldState,
    resources: &'a resources::Resources,
) -> clutter::Cluttered<'a, openshard_movement::MapTerrain<&'a Map, &'a TileData>> {
    world
        .presentation
        .clutter
        .over_with_doors_open(resources.map.map(), &resources.tiledata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_client_render::mobiles::Mobile;
    use openshard_protocol::direction::Direction;
    use openshard_protocol::wire::Hue;
    use openshard_uofiles::anim::BodyKind;

    #[test]
    fn presentation_clocks_retain_the_interval_delivered_with_an_update() {
        let at = Point::new(10, 10, 0);
        let mut presentation = PresentationWorld {
            tile_animations: StaticAnimations::default(),
            flame_clock: Duration::ZERO,
            player: Mobile {
                at,
                body: openshard_protocol::wire::Graphic(400),
                group: BodyKind::of(openshard_protocol::wire::Graphic(400)).standing(),
                facing: Direction::SouthEast,
                frame: openshard_uofiles::anim::AnimationFrameIndex(0),
                from: None,
                hue: Hue::NONE,
                drawn: Gaze::on(at),
                equipment: Vec::new().into(),
            },
            cutaway_at: at,
            cutaway_fades: openshard_client_render::cutaway::Fades::default(),
            static_geometry_cache: None,
            interior_cache: RefCell::default(),
            others: Vec::new(),
            corpses: Vec::new(),
            items: Vec::new(),
            item_serials: Vec::new(),
            multi_preview: Vec::new(),
            damage_numbers: Vec::new(),
            health_estimates: BTreeMap::new(),
            clutter: clutter::Clutter::default(),
            crowd: Crowd::default(),
        };
        let update_interval = Duration::from_millis(750);
        let mut last_advance = Instant::now();
        let update_arrived = last_advance + update_interval;

        let mut motion = PlayerMotion::new(at, Facing::walking(Direction::South));
        advance_presentation_to(&mut presentation, &mut motion, &mut last_advance, update_arrived);

        assert_eq!(presentation.tile_animations.elapsed(), update_interval);
        assert_eq!(presentation.flame_clock, update_interval);
        assert_eq!(last_advance, update_arrived);

        let serial = Serial::new(7).unwrap();
        presentation.damage(serial, 12, Hue::SKILL_CHANGED);
        presentation.advance(DAMAGE_NUMBER_HOLD / 2);
        assert_eq!(presentation.damage_numbers.len(), 1);
        assert_eq!(presentation.damage_numbers[0].amount, DamageAmount::new(12));
        assert_eq!(presentation.damage_numbers[0].hue, Hue::SKILL_CHANGED);
        presentation.advance(DAMAGE_NUMBER_HOLD / 2);
        assert!(presentation.damage_numbers.is_empty());
    }

    #[test]
    fn prediction_keeps_its_position_outside_the_authoritative_view() {
        let mut motion = PlayerMotion::new(
            Point::new(100, 100, 0),
            Facing::walking(openshard_protocol::direction::Direction::North),
        );
        motion.accept_network(Some(link::Movement::Relocation {
            confirmed: openshard_client_net::walk::Predicted {
                position: Point::new(101, 100, 7),
                facing: Facing::running(openshard_protocol::direction::Direction::East),
            },
        }));

        assert_eq!(motion.network.predicted.position, Point::new(101, 100, 7));
        assert_eq!(
            motion.network.predicted.facing,
            Facing::running(openshard_protocol::direction::Direction::East)
        );
    }

    #[test]
    fn an_ordinary_packet_cannot_change_motion() {
        let mut motion = PlayerMotion::new(
            Point::new(100, 100, 0),
            Facing::walking(openshard_protocol::direction::Direction::North),
        );
        motion.accept_local(
            link::Body {
                predicted: openshard_client_net::walk::Predicted {
                    position: Point::new(101, 100, 0),
                    facing: Facing::walking(openshard_protocol::direction::Direction::East),
                },
                corrected: false,
            },
            openshard_protocol::world::StepSequence(7),
        );
        let before = motion.clone();

        // This is the value a generic mutation still carries for rendering,
        // but without a `Movement` fact it is deliberately ignored.
        motion.accept_network(None);

        assert_eq!(motion.network.confirmed, before.network.confirmed);
        assert_eq!(motion.network.predicted, before.network.predicted);
        assert_eq!(motion.pending_steps(), 1);
        assert_eq!(motion.transition_from(), Some(Point::new(100, 100, 0)));
    }

    #[test]
    fn acknowledgement_retires_only_its_matching_step_without_restarting_motion() {
        let north = Facing::walking(openshard_protocol::direction::Direction::North);
        let east = Facing::walking(openshard_protocol::direction::Direction::East);
        let mut motion = PlayerMotion::new(Point::new(100, 100, 0), north);
        motion.accept_local(
            link::Body {
                predicted: openshard_client_net::walk::Predicted {
                    position: Point::new(101, 100, 0),
                    facing: east,
                },
                corrected: false,
            },
            openshard_protocol::world::StepSequence(1),
        );
        motion.accept_local(
            link::Body {
                predicted: openshard_client_net::walk::Predicted {
                    position: Point::new(102, 100, 0),
                    facing: east,
                },
                corrected: false,
            },
            openshard_protocol::world::StepSequence(2),
        );

        motion.accept_network(Some(link::Movement::Ack {
            sequence: openshard_protocol::world::StepSequence(1),
            confirmed: openshard_client_net::walk::Predicted {
                position: Point::new(101, 100, 0),
                facing: east,
            },
        }));

        assert_eq!(motion.network.confirmed.position, Point::new(101, 100, 0));
        assert_eq!(motion.network.predicted.position, Point::new(102, 100, 0));
        assert_eq!(motion.pending_steps(), 1);
        assert_eq!(motion.transition_to(), Some(Point::new(101, 100, 0)));
    }

    #[test]
    fn rejection_discards_every_pending_step_and_transition() {
        let north = Facing::walking(openshard_protocol::direction::Direction::North);
        let east = Facing::walking(openshard_protocol::direction::Direction::East);
        let mut motion = PlayerMotion::new(Point::new(100, 100, 0), north);
        motion.accept_local(
            link::Body {
                predicted: openshard_client_net::walk::Predicted {
                    position: Point::new(101, 100, 0),
                    facing: east,
                },
                corrected: false,
            },
            openshard_protocol::world::StepSequence(1),
        );
        motion.accept_local(
            link::Body {
                predicted: openshard_client_net::walk::Predicted {
                    position: Point::new(102, 100, 0),
                    facing: east,
                },
                corrected: false,
            },
            openshard_protocol::world::StepSequence(2),
        );
        let rejected = openshard_client_net::walk::Predicted {
            position: Point::new(100, 100, 0),
            facing: north,
        };
        motion.accept_network(Some(link::Movement::Reject {
            sequence: openshard_protocol::world::StepSequence(1),
            confirmed: rejected,
        }));

        assert_eq!(motion.network.confirmed, rejected);
        assert_eq!(motion.network.predicted, rejected);
        assert_eq!(motion.pending_steps(), 0);
        assert_eq!(motion.transition_from(), None);
        assert_eq!(motion.route_origin(), rejected.position);
        assert_eq!(motion.hud_state().route_origin, rejected.position);
        assert_eq!(motion.render_state().rendered, rejected);
        assert_eq!(motion.drawn(), Gaze::on(rejected.position));
    }

    #[test]
    fn a_trusted_step_confirms_and_projects_one_transition_without_pending_protocol_work() {
        let north = Facing::walking(openshard_protocol::direction::Direction::North);
        let east = Facing::walking(openshard_protocol::direction::Direction::East);
        let mut motion = PlayerMotion::new(Point::new(100, 100, 0), north);

        motion.accept_trusted_step(Point::new(101, 100, 0), east);

        assert_eq!(motion.network.confirmed, motion.network.predicted);
        assert_eq!(motion.pending_steps(), 0);
        assert_eq!(motion.transition_from(), Some(Point::new(100, 100, 0)));
        assert_eq!(motion.transition_to(), Some(Point::new(101, 100, 0)));
    }

    #[test]
    fn game_motion_advances_the_drawn_body_without_a_crowd_clock() {
        let east = Facing::walking(openshard_protocol::direction::Direction::East);
        let start = Point::new(100, 100, 0);
        let end = Point::new(101, 100, 0);
        let mut motion = PlayerMotion::new(start, east);

        motion.accept_trusted_step(end, east);
        motion.advance(openshard_movement::WALK_HOLD / 2);

        assert_ne!(motion.drawn(), Gaze::on(start));
        assert_ne!(motion.drawn(), Gaze::on(end));
        assert_eq!(motion.transition_from(), Some(start));

        motion.advance(openshard_movement::WALK_HOLD / 2);
        assert_eq!(motion.drawn(), Gaze::on(end));
        assert_eq!(motion.transition_from(), None);
    }

    #[test]
    fn local_motion_keeps_the_body_ease_without_delegating_its_position_to_crowd() {
        let east = Facing::walking(openshard_protocol::direction::Direction::East);
        let start = Point::new(100, 100, 0);
        let end = Point::new(101, 100, 0);
        let mut linear = PlayerMotion::new(start, east);
        let mut eased = PlayerMotion::new(start, east);

        linear.accept_trusted_step(end, east);
        eased.accept_trusted_step(end, east);
        linear.advance(openshard_movement::WALK_HOLD / 2);
        eased.advance_with_ease(openshard_movement::WALK_HOLD / 2, crate::crowd::Ease::WALK);

        assert!(linear.is_gliding());
        assert!(eased.is_gliding());
        assert_ne!(
            linear.drawn(),
            eased.drawn(),
            "the configured body ease affects the local pose"
        );
        assert!(
            eased.drawn().x < linear.drawn().x,
            "the eased picture follows the exact local walk instead of snapping to it"
        );
        assert_eq!(
            eased.game.walked,
            linear.drawn(),
            "the movement clock itself stays exact"
        );
    }

    #[test]
    fn local_motion_arms_display_rate_frames_even_if_crowd_is_rebased() {
        let east = Facing::walking(openshard_protocol::direction::Direction::East);
        let start = Point::new(100, 100, 0);
        let end = Point::new(101, 100, 0);
        let mut motion = PlayerMotion::new(start, east);

        assert!(!motion.is_gliding());
        motion.accept_trusted_step(end, east);
        assert!(motion.is_gliding());
        motion.advance(openshard_movement::WALK_HOLD);
        assert!(!motion.is_gliding());
    }

    #[test]
    fn rapid_direction_changes_cannot_interrupt_a_mid_frame_animation() {
        let north = Facing::walking(openshard_protocol::direction::Direction::North);
        let east = Facing::walking(openshard_protocol::direction::Direction::East);
        let start = Point::new(100, 100, 0);
        let first = Point::new(101, 100, 0);
        let second = Point::new(101, 99, 0);
        let mut motion = PlayerMotion::new(start, east);
        let local = |position, facing| link::Body {
            predicted: openshard_client_net::walk::Predicted { position, facing },
            corrected: false,
        };

        motion.accept_local(local(first, east), openshard_protocol::world::StepSequence(1));
        motion.advance(openshard_movement::WALK_HOLD / 2);
        let active = motion.game;

        // The direction change is a turn at the predicted endpoint, followed
        // by a step in that new direction. Both may arrive before another
        // frame, but neither may reset the eastbound crossing in progress.
        motion.accept_local(local(first, north), openshard_protocol::world::StepSequence(2));
        motion.accept_local(local(second, north), openshard_protocol::world::StepSequence(3));

        assert_eq!(motion.game, active);
        assert_eq!(motion.transition_from(), Some(start));
        assert_eq!(motion.drawn(), active.drawn);

        motion.advance(openshard_movement::WALK_HOLD / 2);
        assert_eq!(motion.drawn(), Gaze::on(first));
        assert_eq!(motion.transition_from(), Some(first));
        assert_eq!(motion.render_state().rendered.position, second);
    }

    #[test]
    fn a_turn_cannot_snap_the_easing_tail_after_a_step_lands() {
        let north = Facing::walking(openshard_protocol::direction::Direction::North);
        let east = Facing::walking(openshard_protocol::direction::Direction::East);
        let start = Point::new(100, 100, 0);
        let end = Point::new(101, 100, 0);
        let mut motion = PlayerMotion::new(start, east);
        let local = |position, facing| link::Body {
            predicted: openshard_client_net::walk::Predicted { position, facing },
            corrected: false,
        };

        motion.accept_local(local(end, east), openshard_protocol::world::StepSequence(1));
        // At display cadence the eased body deliberately trails the exact
        // crossing by several pixels. A hard camera follows this same gaze, so
        // snapping it here would be a camera jump of the same size.
        for _ in 0..25 {
            motion.advance_with_ease(openshard_movement::WALK_HOLD / 25, crate::crowd::Ease::WALK);
        }
        assert!(!motion.is_gliding(), "the exact crossing has landed");
        assert_ne!(motion.drawn(), Gaze::on(end), "the visual ease is still settling");
        let tail = (motion.drawn().x - Gaze::on(end).x).hypot(motion.drawn().y - Gaze::on(end).y);
        assert!(
            tail > 2.0,
            "the turn would have jumped the camera {tail:.1} pixels"
        );
        let visual_before_turn = motion.game;

        motion.accept_local(local(end, north), openshard_protocol::world::StepSequence(2));

        assert_eq!(
            motion.game, visual_before_turn,
            "turning in place must preserve the body and camera's continuous target"
        );
        assert_eq!(motion.drawn(), visual_before_turn.drawn);
    }

    #[test]
    fn a_new_grid_step_keeps_the_continuous_pose_when_the_previous_one_is_mid_frame() {
        let east = Facing::walking(openshard_protocol::direction::Direction::East);
        let start = Point::new(100, 100, 0);
        let first = Point::new(101, 100, 0);
        let second = Point::new(102, 100, 0);
        let mut motion = PlayerMotion::new(start, east);

        motion.accept_trusted_step(first, east);
        motion.advance(openshard_movement::WALK_HOLD / 2);
        let mid_first = motion.drawn();

        motion.accept_trusted_step(second, east);
        assert_eq!(
            motion.drawn(),
            mid_first,
            "a network-grid event cannot reset the fractional game pose"
        );
        motion.advance(openshard_movement::WALK_HOLD / 2);
        assert_eq!(motion.drawn(), Gaze::on(first));
        assert_eq!(motion.transition_from(), Some(first));
        motion.advance(openshard_movement::WALK_HOLD / 4);
        assert_ne!(motion.drawn(), Gaze::on(first));
        assert_ne!(motion.drawn(), Gaze::on(second));
    }

    #[test]
    fn queued_predictions_each_keep_their_own_walk_hold() {
        let east = Facing::walking(openshard_protocol::direction::Direction::East);
        let start = Point::new(100, 100, 0);
        let first = Point::new(101, 100, 0);
        let second = Point::new(102, 100, 0);
        let mut motion = PlayerMotion::new(start, east);

        for (position, sequence) in [(first, 1), (second, 2)] {
            motion.accept_local(
                link::Body {
                    predicted: openshard_client_net::walk::Predicted {
                        position,
                        facing: east,
                    },
                    corrected: false,
                },
                openshard_protocol::world::StepSequence(sequence),
            );
        }

        assert_eq!(motion.planning_state().position, second);
        assert_eq!(motion.render_state().rendered.position, first);
        assert_eq!(motion.route_origin(), start);

        motion.advance(openshard_movement::WALK_HOLD);
        assert_eq!(motion.drawn(), Gaze::on(first));
        assert_eq!(motion.transition_from(), Some(first));
        assert_eq!(motion.render_state().rendered.position, second);

        motion.advance(openshard_movement::WALK_HOLD / 2);
        assert_ne!(motion.drawn(), Gaze::on(first));
        assert_ne!(motion.drawn(), Gaze::on(second));
    }

    #[test]
    fn online_offline_and_replay_sources_share_the_same_settled_motion_snapshot() {
        let east = Facing::walking(openshard_protocol::direction::Direction::East);
        let start = Point::new(100, 100, 0);
        let end = Point::new(101, 100, 0);
        let destination = openshard_client_net::walk::Predicted {
            position: end,
            facing: east,
        };

        let mut online = PlayerMotion::new(start, east);
        online.accept_local(
            link::Body {
                predicted: destination,
                corrected: false,
            },
            openshard_protocol::world::StepSequence(17),
        );
        online.accept_network(Some(link::Movement::Ack {
            sequence: openshard_protocol::world::StepSequence(17),
            confirmed: destination,
        }));

        let mut offline = PlayerMotion::new(start, east);
        offline.accept_trusted_step(end, east);
        let mut replay = PlayerMotion::new(start, east);
        replay.accept_trusted_step(end, east);

        assert_eq!(online.snapshot(), offline.snapshot());
        assert_eq!(offline.snapshot(), replay.snapshot());
    }

    #[test]
    fn health_estimate_moves_from_confirmed_damage_to_the_new_health() {
        let mut estimate = HealthEstimate {
            from: 80,
            target: 50,
            elapsed: Duration::ZERO,
        };
        assert_eq!(estimate.shown(), 80);
        estimate.elapsed = HEALTH_ESTIMATE_LAG / 2;
        assert_eq!(estimate.shown(), 65);
        estimate.elapsed = HEALTH_ESTIMATE_LAG;
        assert_eq!(estimate.shown(), 50);
        assert!(estimate.settled());
    }
}
