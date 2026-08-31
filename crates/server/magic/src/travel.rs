//! Travel: where a rune may be marked, and where its owner may go.
//!
//! The permission model first, because it is the part with a reference behind
//! it. ServUO's `SpellHelper.CheckTravel` is a `bool[7,24]` matrix — seven kinds
//! of travel against twenty-four named areas of Britannia, each cell saying
//! whether that kind is allowed there. Almost none of those areas exist here,
//! and the ones that matter are already flags on a region, so the matrix
//! collapses onto two booleans. What survives the collapse is the *shape* of the
//! rule, which is the part worth keeping:
//!
//! - **Both ends are checked.** A jail one can cast out of is not a jail, which
//!   is the same argument `WorldState::may_teleport` makes for `no_teleport`.
//! - **Recall out is the permissive direction.** ServUO's `RecallFrom` row is
//!   the only one with `true` in it; `RecallTo`, `GateFrom`, `GateTo` and `Mark`
//!   are barred everywhere it bars anything. So a dungeon you can recall *out*
//!   of is still one nobody can recall *into*, gate either way, or mark in.
//!
//! Sphere draws the line finer — `REGION_ANTIMAGIC_RECALL_IN`, `RECALL_OUT`,
//! `GATE` and `TELEPORT` are four separate bits (`CRegion.cpp`), so a shard
//! there can bar gating without barring recall. Our single `no_recall` is what
//! collapses those four, and this is where to widen it if that is ever wanted.

use openshard_entities::EntityId;
use openshard_protocol::world::{
    Facet,
    Point,
};
use openshard_state::WorldState;
use openshard_state::components::{
    Position,
    RuneMark,
};

/// What a mobile is trying to do, and at which end.
///
/// The seven of ServUO's `TravelCheckType` that this engine can distinguish. Its
/// `TeleportFrom`/`TeleportTo` are absent on purpose: teleport has its own
/// predicate (`WorldState::may_teleport`, reading `no_teleport`) and folding the
/// two would mean one flag doing two jobs.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TravelKind {
    /// Recalling away from here.
    RecallFrom,
    /// Recalling to here.
    RecallTo,
    /// Opening a gate here.
    GateFrom,
    /// Opening a gate whose far end is here.
    GateTo,
    /// Marking a rune here.
    Mark,
}

impl TravelKind {
    /// Whether a region's `no_recall` bars this kind.
    ///
    /// Everything but `RecallFrom`, which is ServUO's permissive row: you may
    /// leave a place you may not enter, gate from, gate to or mark in.
    const fn barred_by_no_recall(self) -> bool {
        !matches!(self, Self::RecallFrom)
    }

    /// The line the client is shown when this kind is refused — ServUO's
    /// `SendInvalidMessage`, which distinguishes arriving from everything else.
    pub const fn refusal(self) -> &'static str {
        match self {
            Self::RecallTo | Self::GateTo => "You are not allowed to travel there.",
            _ => "Thy spell doth not appear to work.",
        }
    }
}

/// Whether `mobile` may travel of this kind, at the one end `at` on `facet`.
///
/// **One end, one kind.** The kinds are directional — `RecallFrom` is what you
/// may do where you stand, `RecallTo` what you may do where you are going — so a
/// caller checks both ends with the *matching* kind for each, exactly as
/// ServUO's `Recall.Effect` calls `CheckTravel` twice. Folding the two into one
/// call and testing both tiles against a single kind reads tidier and is wrong
/// in a way that is easy to miss: it asks whether you may *arrive* where you are
/// already standing, so a dungeon nobody can recall into becomes one nobody can
/// recall out of either — a one-way trap that no test of the arriving direction
/// would catch.
///
/// Staff pass, through the one exemption door `is_staff`.
#[must_use]
pub fn may_travel(state: &WorldState, mobile: EntityId, kind: TravelKind, facet: Facet, at: Point) -> bool {
    if state.is_staff(mobile) {
        return true;
    }
    state.region_at(facet, at).is_none_or(|region| {
        // `no_teleport` bars every kind at either end — a jail is a jail.
        // `no_recall` bars all but leaving.
        !region.flags.no_teleport && !(kind.barred_by_no_recall() && region.flags.no_recall)
    })
}

/// Where a mobile is standing, as the pair [`may_travel`] wants — the departure
/// end of any travel it is about to attempt.
#[must_use]
pub fn standing_at(state: &WorldState, mobile: EntityId) -> Option<(Facet, Point)> {
    state
        .registry
        .get::<Position>(mobile)
        .map(|at| (state.facet_of(mobile), at.0))
}

/// Where a marked rune points, if it is marked at all.
///
/// A blank rune has no [`RuneMark`] rather than a marked flag set false, so this
/// is the whole of "is this rune usable".
#[must_use]
pub fn destination_of(state: &WorldState, rune: EntityId) -> Option<(Facet, Point)> {
    state
        .registry
        .get::<RuneMark>(rune)
        .map(|mark| (mark.facet, mark.destination))
}

/// What to call a marked spot: the region's name, or its coordinates.
///
/// ServUO generates the same thing in `RecallRune.CalculateDescription`, and for
/// the same reason — a rune that reads "1495, 1629" in a list of sixteen is a
/// rune nobody can find twice.
#[must_use]
pub fn describe(state: &WorldState, facet: Facet, at: Point) -> String {
    state.region_at(facet, at).map_or_else(
        || format!("{}, {} (facet {facet})", at.x, at.y),
        |region| region.name.clone(),
    )
}

/// One of the city moongates that always stand — ServUO's `PMEntry`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PublicGate {
    /// What the destination list calls it.
    pub name:  &'static str,
    /// Which facet it stands on.
    pub facet: Facet,
    /// The tile it stands on, which is also the tile it takes you to.
    pub at:    Point,
}

/// Felucca's nine public moongates — ServUO's `PMList.Felucca`, verbatim.
///
/// Core data, like [`crate::MAGERY`] and the craft recipes: a bare shard has to
/// be able to travel. The pack's own `Moongates` region covers only eight of
/// these — Buccaneer's Den has no rectangle — which costs those tiles the
/// `guarded` flag and nothing else, so the ninth gate ships rather than being
/// silently dropped to match.
pub static PUBLIC_MOONGATES: &[PublicGate] = &[
    gate("Moonglow", Facet(0), Point::new(4467, 1283, 5)),
    gate("Britain", Facet(0), Point::new(1336, 1997, 5)),
    gate("Jhelom", Facet(0), Point::new(1499, 3771, 5)),
    gate("Yew", Facet(0), Point::new(771, 752, 5)),
    gate("Minoc", Facet(0), Point::new(2701, 692, 5)),
    gate("Trinsic", Facet(0), Point::new(1828, 2948, -20)),
    gate("Skara Brae", Facet(0), Point::new(643, 2067, 5)),
    gate("Magincia", Facet(0), Point::new(3563, 2139, 5)),
    gate("Buccaneer's Den", Facet(0), Point::new(2711, 2234, 0)),
];

/// One row, kept terse so the nine read at a glance.
const fn gate(name: &'static str, facet: Facet, at: Point) -> PublicGate {
    PublicGate { name, facet, at }
}

/// Whether a tile is one of the city moongates.
///
/// A **read-site derivation**, the shape `equipped_weapon` set: a public gate
/// carries no component at all, so it is saved and restored as ordinary
/// decoration and its meaning is re-derived every boot for nothing. That is what
/// keeps it out of the schema, and what makes a restored one correct with no
/// restore hook to forget.
#[must_use]
pub fn public_gate_at(facet: Facet, at: Point) -> Option<&'static PublicGate> {
    PUBLIC_MOONGATES
        .iter()
        .find(|gate| gate.facet == facet && gate.at.x == at.x && gate.at.y == at.y)
}
