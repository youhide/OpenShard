//! The ground one body's step is decided against.
//!
//! # Four things, and the fourth arrived late
//!
//! A step needs the map, what the live world has laid over it, which way the
//! shut doors are being read, and **who else is standing on the ground** — and
//! this is those four carried together because they travel together, through
//! `find_path`, through the detour rule, through every hop a long route is
//! refined by.
//!
//! The fourth is [`Bodies`], and it was a caller-side afterthought before it was
//! a field. The shard used to ask `WorldState::mobile_occupies` *after*
//! [`can_step`](crate::can_step) had already answered, in the two places a step
//! is actually taken — so a step was refused by a body and a **route** was
//! planned straight through one. A creature whose quarry stood behind a
//! bystander walked into the bystander, was refused, re-decided the same
//! direction next beat, and never went round. One rule in one place is the whole
//! of the fix: `walk::landing` asks — the private function each of
//! [`can_step`](crate::can_step)'s and `steps_out_of`'s answers is — so every
//! caller asks, flanks included.
//!
//! # Why this is not the trait it replaces
//!
//! There used to be a `Terrain` trait here with six implementors, five of which
//! were an *action over* a terrain rather than a terrain: a mask of what the
//! live world put in the way, a rectangle to stay inside, a memo table, the
//! absence of a map. Each was a kind of terrain because the seam was a trait,
//! and each one being a kind of terrain was then the argument for the seam
//! being a trait. See `docs/map/terrain_seam.md`.
//!
//! This is not that, and three properties are what keep it from becoming that:
//!
//! - **Nothing implements it.** It is a struct with four public fields, so
//!   there is no "another kind of footing" to write.
//! - **It has no methods.** Every rule over it is a free function that reads
//!   its fields — [`can_step`](crate::can_step), [`step_allowed`](crate::step_allowed),
//!   [`find_path`](crate::find_path). A decorator's failure mode was forwarding
//!   nine methods by hand and silently forgetting one; there is nothing here to
//!   forward.
//! - **A caller that wants less takes less.** Baking a navigation graph wants
//!   the bare map and takes a [`MapTerrain`]; a caller that wants to know what
//!   is in the way takes an [`Overlay`]. Only a *step* takes all four, because
//!   only a step needs all four — and the fourth defaults to
//!   [`Bodies::nobody`], which is the reading a bake, a survey and a coarse
//!   corridor all want.

use openshard_map::overlay::{
    Doors,
    Overlay,
};
use openshard_protocol::world::Point;
use openshard_tiles::TileData;

use crate::ground::Ground;
use crate::terrain::MapTerrain;

/// How far apart two bodies' feet must be before neither is in the other's way.
///
/// ServUO's mobile overlap, character for character: `(mob.Z + 15) > newZ &&
/// (newZ + 15) > mob.Z` (`Scripts/Services/Pathing/Movement.cs:352`). **Fifteen
/// and not [`PLAYER_HEIGHT`](crate::PLAYER_HEIGHT)**, which is sixteen and is
/// what the same file calls `PersonHeight` — the one place ServUO measures a
/// body against another body rather than against the ceiling, it measures it a
/// unit shorter. Kept as its own constant rather than folded into the other,
/// because the two are different questions that happen to be one apart: a
/// stairwell that fits a body and a tile that fits two.
const MOBILE_OVERLAP: i32 = 15;

/// The other bodies a step has to get past.
///
/// # Borrowed for the question, and never an index
///
/// The same bargain [`MapTerrain`] makes: a slice the caller owns for the length
/// of one answer, with no identity in it and nothing here to keep in step. The
/// shard builds one out of its sector grid — which is already the authority from
/// tile to entity, and is already kept honest by the step itself — so there is
/// no second copy of `Position` to fall out of step, and no `unblock` anybody
/// can forget. That was the whole of the argument against registering mobiles in
/// the obstruction index; see `docs/roadmap.md`'s *a mobile is not an
/// obstacle*.
///
/// **Both ends of the wire build one**, which is the same fact
/// [`Footing::guide`] records about the bare map: the shard's
/// `WorldState::crowd_near` off its sector grid, and the client's
/// `clutter::crowd` off the mobiles in its view. A client that decided this
/// question differently would ask for steps the shard refuses — a rubber-band a
/// hold at a time — so the two answers are held to each other on purpose, and
/// the exemptions the client cannot derive for itself are sent to it in
/// `StatusFlags::IGNORE_MOBILES`.
///
/// **When each end builds it is not the same, and the reason is in the
/// arguments.** `crowd_near` takes a mover, a centre and a reach, so its answer
/// is different for every asker and cannot be anything but built at the
/// question. The client's is a function of the *view* alone — the mover is
/// always this connection's own body and the reach is whatever the shard has
/// shown it — so it is projected once when the view moves, beside the two other
/// projections that view already feeds (`clutter::project`). Neither is an
/// index: both are built whole and thrown away whole, which is the property this
/// section is actually about.
///
/// # What is not in here
///
/// **No identity, and no rules about who may pass whom.** Which bodies are in
/// the way is decided where the registry is — the dead do not block, a ghost is
/// stopped by nobody, staff walk through everyone — and by the time a slice
/// reaches here those decisions are made. This is the same line the [`Overlay`]
/// draws: it says a door is in the way and only the shard says which door.
///
/// A body's own feet are simply absent from the slice its own step is decided
/// against, which is what "a mobile may always step off the tile it is standing
/// on" comes to once it is written down.
#[derive(Clone, Copy, Debug)]
pub struct Bodies<'a> {
    /// Where they stand — feet, not heads. **Sorted by `(x, y)`**, which is what
    /// lets [`blocks`](Self::blocks) find a tile's occupants without walking the
    /// crowd: a route planned across a market asks this once per landing, eight
    /// times per node expanded.
    standing: &'a [Point],
}

impl<'a> Bodies<'a> {
    /// Nobody in the way.
    ///
    /// Not an empty world — a reading in which other bodies are not part of the
    /// question. A navigation bake (nobody has spawned yet), a survey over
    /// shipped art, the coarse corridor a long route is proposed along, and a
    /// mover that passes through bodies anyway all want this.
    #[must_use]
    pub const fn nobody() -> Self {
        Self { standing: &[] }
    }

    /// The bodies standing at `feet`, which **must be sorted by `(x, y)`**.
    ///
    /// Debug-checked rather than sorted here: the caller owns the storage and
    /// knows whether it has just built it, and a sort hidden behind a borrow
    /// would be a copy this type exists to avoid.
    #[must_use]
    pub fn standing(feet: &'a [Point]) -> Self {
        debug_assert!(
            feet.windows(2)
                .all(|pair| (pair[0].x, pair[0].y) <= (pair[1].x, pair[1].y)),
            "Bodies::standing was given an unsorted crowd, and its lookup would miss occupants"
        );
        Self { standing: feet }
    }

    /// Whether a body standing at `at` would be inside one of these.
    ///
    /// The vertical test is `MOBILE_OVERLAP` — fifteen, ServUO's, and a unit
    /// short of [`PLAYER_HEIGHT`](crate::PLAYER_HEIGHT): two mobiles on separate
    /// floors of a tower share an `(x, y)` and are not in each other's way.
    #[must_use]
    pub fn blocks(self, at: Point) -> bool {
        let key = (at.x, at.y);
        let z = i32::from(at.z);
        let first = self.standing.partition_point(|body| (body.x, body.y) < key);
        self.standing[first..]
            .iter()
            .take_while(|body| (body.x, body.y) == key)
            .any(|body| {
                let theirs = i32::from(body.z);
                theirs + MOBILE_OVERLAP > z && z + MOBILE_OVERLAP > theirs
            })
    }

    /// Whether this reading has anybody in it at all — what a caller checks
    /// before paying for a lookup it knows the answer to.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.standing.is_empty()
    }
}

/// The map, the live world over it, how the doors are read, and who is standing
/// on it.
///
/// `Copy` and built where it is asked: two pointers, a reference and a byte,
/// with nothing owned and nothing stored. Both ends of the wire already hold
/// the parts — a facet's [`Ground`] and the install's tile table — so this is a
/// view over what the caller has rather than a thing anybody keeps. It is built
/// through [`Footing::of`] wherever a facet's [`Ground`] is what the caller has,
/// and through [`Footing::guide`] where the caller means the bare map the coarse
/// navigation graph was baked over; the bare [`Footing::new`] is left for the one
/// caller that deliberately wants *less* than a facet — a test that is about the
/// overlay and nothing else.
#[derive(Clone, Copy, Debug)]
pub struct Footing<'a> {
    /// The map, or `None` for a world with no map at all: no floor, no walls,
    /// every step allowed and z never changing.
    ///
    /// What a shard with no client files runs, and what a test that is about
    /// the overlay and nothing else asks over. It used to be a type of its own
    /// (`OpenWorld`) and an implementor of the trait, which is how the absence
    /// of a map came to be a kind of map.
    pub map:     Option<MapTerrain<'a>>,
    /// What the live world has put on that map.
    pub overlay: &'a Overlay,
    /// Whether a shut door is in the way, or whether this is a route being
    /// planned by somebody who will open it.
    pub doors:   Doors,
    /// Who else is standing on it, as far as this reading is concerned.
    ///
    /// [`Bodies::nobody`] unless a caller says otherwise, which every
    /// constructor here leaves it at: a footing is assembled out of a facet, and
    /// a crowd is assembled out of a *question* — who is asking, and how far
    /// their step or their route reaches. [`among`](Self::among) is where the
    /// two meet, exactly as [`reading`](Self::reading) is where the doors do.
    pub bodies:  Bodies<'a>,
}

impl<'a> Footing<'a> {
    /// The ground as `doors` reads it, with nobody standing on it.
    #[must_use]
    pub const fn new(map: Option<MapTerrain<'a>>, overlay: &'a Overlay, doors: Doors) -> Self {
        Self {
            map,
            overlay,
            doors,
            bodies: Bodies::nobody(),
        }
    }

    /// The ground one facet's [`Ground`] is, read as `doors` reads it.
    ///
    /// **The one composition**, and the reason [`Ground`] exists: the map, what
    /// is laid over it and the bake that says where a body may stand on it come
    /// out of a single value that says they are the same facet, instead of being
    /// three arguments a caller assembled and could assemble wrongly. Every
    /// production site that used to build a footing field by field goes through
    /// here.
    ///
    /// This used to take the bake as a fourth argument and *check* the pairing —
    /// it panicked on a facet with a map and no bake over it, because a facet
    /// like that decides its steps by re-deriving every column from `tiledata`,
    /// six times more expensively, with nothing at all saying so. There is
    /// nothing left to check: the two are one value, and neither can arrive
    /// without the other.
    ///
    /// The tile table is still its own argument, because its scope is different:
    /// one install has one table and several facets, so what a graphic *is* is
    /// not a fact about this world. That asymmetry is the whole of the
    /// signature.
    #[must_use]
    pub fn of(ground: &'a Ground, tiles: &'a TileData, doors: Doors) -> Self {
        Self {
            map: ground.terrain(tiles),
            overlay: ground.live(),
            doors,
            bodies: Bodies::nobody(),
        }
    }

    /// The same ground with nothing the live world has put on it — what a coarse
    /// navigation graph is guided by and what its ordinary endpoints join over.
    ///
    /// The graph is baked over the bare map (`docs/map/navigation_spans.md`'s
    /// N4), so the corridor it proposes has to be read over the bare map too: a
    /// door that happens to be shut, or a crate somebody dropped, must not be
    /// able to rewrite a route's *topology*. What the live world has to say is
    /// asked of each exact step instead. The exception is an endpoint in a
    /// column where the live layer added a floor: a player house's upper storey
    /// does not exist here at all, so [`find_long_path`](crate::find_long_path)
    /// follows live places until they meet this graph, then still validates
    /// every kept step through [`of`](Self::of).
    ///
    /// **Both ends of the wire want this value**, and each used to build it for
    /// itself out of an empty overlay it kept alive somewhere: the client's
    /// `world::guide` and the shard's `WorldState::guide`.
    ///
    /// The doors are read [`AsTheyStand`](Doors::AsTheyStand), which is a
    /// statement about nothing: an empty overlay has no door in it, so both
    /// readings of it are the same ground.
    ///
    /// Nobody is standing on it either, and for the same reason: a corridor is a
    /// statement about the *topology* of a facet, and a bystander who will have
    /// walked off by the time anyone gets there must not be able to rewrite one.
    #[must_use]
    pub fn guide(ground: &'a Ground, tiles: &'a TileData) -> Self {
        Self {
            map:     ground.terrain(tiles),
            overlay: &NOTHING_PLACED,
            doors:   Doors::AsTheyStand,
            bodies:  Bodies::nobody(),
        }
    }

    /// The same ground, read the other way round.
    ///
    /// A route is planned through shut doors and then walked as they stand, and
    /// both readings are of one facet at one moment — so the pair is made from
    /// one another rather than assembled twice from the parts.
    #[must_use]
    pub const fn reading(self, doors: Doors) -> Self {
        Self { doors, ..self }
    }

    /// The same ground with a crowd on it.
    ///
    /// [`reading`](Self::reading)'s sibling, and separate from every constructor
    /// above for the same reason: a facet is what those assemble, and *who is in
    /// the way* is not a fact about a facet — it is a fact about one mover at one
    /// moment, and only the caller knows which mover is asking and how far the
    /// question reaches. A step wants the eight tiles around it; a route wants
    /// the ground it might cross.
    #[must_use]
    pub const fn among(self, bodies: Bodies<'a>) -> Self {
        Self { bodies, ..self }
    }
}

/// Nothing placed, for a map-only reading to borrow.
///
/// One for the process rather than one per holder, because it is the *absence*
/// of a live layer: a second one would be a second name for the same emptiness,
/// and an empty overlay a caller keeps beside its map is a thing that can be
/// written to by mistake.
static NOTHING_PLACED: std::sync::LazyLock<Overlay> = std::sync::LazyLock::new(Overlay::default);
