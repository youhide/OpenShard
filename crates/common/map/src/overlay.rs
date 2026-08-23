//! What the live world lays over the map.
//!
//! # Why the map is not enough, said once
//!
//! [`WorldMap`](crate::map::WorldMap) is the ground and the statics an install
//! shipped, and nothing else. A door is an *entity*: the doorway it stands in is
//! an open gap in the statics by construction, and the leaf that closes it lives
//! in the shard's registry. A barrel is an entity. A ship is an entity, and it is
//! the one that can put ground where the map says there is none.
//!
//! Both ends of the wire had to say that, and until this module existed both
//! said it themselves: `openshard-state`'s `Obstructions` and `client/app`'s
//! `Clutter`, two indexes of the same fact that agreed only by resemblance —
//! each with its own blocker struct, its own span arithmetic and its own body
//! height. This is the one type they now both build, so a step refused here is
//! refused there for the same reason rather than for a similar one.
//!
//! # An overlay is not a terrain
//!
//! It answers three questions and none of them is "may I walk here":
//!
//! - **Is anything in the way of a body standing at this height?** — the span
//!   overlap, which is what lets a wall on an upper floor leave the ground floor
//!   open.
//! - **Is there somewhere to stand the map does not know about?** — a deck.
//!   The only *positive* thing here, and the reason this is not a bitmask.
//! - **Was it a door?** — because a route that stops at a door means "go and
//!   open it" and a route that stops at a crate means "there is no way through".
//!
//! Who *put* a cover here is not one of them. The server knows — a door is an
//! entity it can open, a plank belongs to a ship somebody is standing on — and
//! keeps that in its own indexes, because the client has no identity to offer:
//! a `GroundItem` is a position, a graphic, a hue and an amount. An owner field
//! here would be a hole one end fills with a lie. See
//! `docs/map/terrain_seam.md`'s node E.
//!
//! # Why this is the map's crate and not movement's
//!
//! It is storage — a span and a kind per tile — and the third layer of the map
//! `docs/map/map_rebuild.md` describes: the ground, the statics, and what the
//! live world has laid over them. Every *rule* that reads one stayed in
//! `openshard-movement`, which is why nothing here knows how tall a body is; see
//! [`Body`].

use rustc_hash::FxHashMap;

use openshard_tiles::StaticTile;

use crate::grid::Tile;

/// Which of the two readings of the same ground a question is asked under.
///
/// A `bool` would do and is exactly what this must not be: the two are read at
/// call sites several crates apart, and "true" there says nothing about which
/// way round it is. Both ends had this distinction already — the server as
/// `LiveTerrain::through_doors`, the client as a private enum of this name —
/// which is two spellings of one idea and the argument for it living here.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Doors {
    /// Shut is shut: what a step is actually allowed by, and the only reading a
    /// step that reaches the wire may use.
    #[default]
    AsTheyStand,
    /// Every shut door stands open: what a *route* may be planned through, by a
    /// body that intends to open its way along it. Never a walkability answer —
    /// a mobile walked into a door on this reading is refused by the shard.
    AllOpen,
}

impl Doors {
    /// The reading a body plans under, given whether it can work a latch.
    ///
    /// The one place a `bool` legitimately becomes one of these: whether a
    /// creature opens doors is a fact about the creature, held as a flag on its
    /// brain, and this is the seam where that fact turns into a way of reading
    /// the ground. Everywhere else the enum is what travels.
    #[must_use]
    pub const fn for_opener(opens_doors: bool) -> Self {
        match opens_doors {
            true => Self::AllOpen,
            false => Self::AsTheyStand,
        }
    }
}

/// The z-span a body takes up while it stands somewhere: `[z, z + height)`.
///
/// **The whole of what the overlay knows about a body**, and it is an argument
/// rather than a constant on purpose. How tall a creature is is a movement rule
/// — `openshard_movement::PLAYER_HEIGHT`, whose own comment admits it should
/// vary by creature — and this module is storage. A `blocker_at` that reached
/// for that constant would be the map's crate deciding how big a person is.
///
/// It is a type and not two `i32` arguments because the two are a position and
/// a length in the same units, side by side, on the hot path of every step:
/// nothing but the order of the pair would say which was which.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Body {
    z: i32,
    height: i32,
}

impl Body {
    /// A body whose feet are at `z` and which stands `height` units tall.
    #[must_use]
    pub const fn new(z: i32, height: i32) -> Self {
        Self { z, height }
    }

    /// Where its feet are.
    #[must_use]
    pub const fn feet(self) -> i32 {
        self.z
    }

    /// One past the top of its head — the exclusive end of its span.
    #[must_use]
    pub const fn head(self) -> i32 {
        self.z + self.height
    }
}

/// What one entry over one tile does to a body.
///
/// An enum and not a pair of flags, and **one thing can still be both** — as
/// two entries rather than as one entry with two answers. A ship's plank is the
/// case that shaped this: it is read through two filters that partition it, the
/// hull half stopping a body and the deck half carrying one, and a tile with a
/// gunwale on it holds one of each. A house's stair tread is the same shape —
/// something a body beside it walks into, and somewhere a body on top of it
/// stands.
///
/// Keeping them separate is what keeps [`Stands`](Self::Stands) the *positive*
/// arm: a reader asking what is in the way never has to know that some of the
/// things in the way are also floors.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CoverKind {
    /// In the way.
    Blocks {
        /// A shut door: in the way now, and not in the way at all to a mobile
        /// that will open it. What [`Doors::AllOpen`] leaves out, and the whole
        /// of what "potentially passable" means here.
        door: bool,
    },
    /// Somewhere to stand that the map does not have — a deck over open water,
    /// the first floor of a house somebody built this morning.
    ///
    /// **The one thing that can overrule the map's refusal.** Open water is not
    /// ground, so the map answers "nothing to stand on" — correctly, right up
    /// until a ship is moored there. An index that only ever subtracted could
    /// not say this, which is why boats were a structure of their own before
    /// this type existed.
    Stands {
        /// A stair, a ramp, a ladder — UO's `CLIMBABLE` bit, and two different
        /// numbers because of it. You stand **half way up** one rather than on
        /// top of it, and you step onto it at its **base** rather than at its
        /// top, which is what lets a staircase be climbed one tread at a time
        /// instead of reading as a wall the height of the whole flight.
        ///
        /// A field on this arm and not a second variant, because the two are
        /// the same *kind* of thing — a surface — differing in an arithmetic.
        /// See [`Cover::surface`] and [`Cover::reach`], which are where the
        /// difference is written, once.
        climbable: bool,
    },
}

/// One thing the live world has put on one tile.
///
/// No graphic, no serial, no entity — see the module header. What a reader
/// needs is where its body starts, how far up it reaches, and which of the two
/// things above it does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cover {
    /// The base z its body sits at.
    pub z: i8,
    /// How tall it is: its body spans `[z, z + height)`.
    ///
    /// Zero-height art still occupies its own tile — `tiledata.mul` gives
    /// plenty of impassable art a height of zero, and a flat span would overlap
    /// nothing and block nowhere, which reads exactly like the bug this type
    /// exists to fix. [`Cover::top`] is where that is fixed, once.
    pub height: u8,
    /// What it does to a body.
    pub kind: CoverKind,
}

impl Cover {
    /// Something in the way, at `z`, `height` tall.
    #[must_use]
    pub const fn blocking(z: i8, height: u8) -> Self {
        Self {
            z,
            height,
            kind: CoverKind::Blocks { door: false },
        }
    }

    /// A shut door — in the way, and openable.
    #[must_use]
    pub const fn door(z: i8, height: u8) -> Self {
        Self {
            z,
            height,
            kind: CoverKind::Blocks { door: true },
        }
    }

    /// A floor to stand on at `z + height`, carrying rather than stopping.
    #[must_use]
    pub const fn standing(z: i8, height: u8) -> Self {
        Self {
            z,
            height,
            kind: CoverKind::Stands { climbable: false },
        }
    }

    /// A stair `height` tall based at `z`: stood on half way up, stepped onto
    /// at its base. See [`CoverKind::Stands`]'s `climbable`.
    #[must_use]
    pub const fn climbable(z: i8, height: u8) -> Self {
        Self {
            z,
            height,
            kind: CoverKind::Stands { climbable: true },
        }
    }

    /// What a placed item lays over its own tile, from the client-file entry
    /// that says what its art is.
    ///
    /// **The one rule both ends of the wire call**, and the reason the two
    /// agree by construction rather than by resemblance. It used to be written
    /// twice: `world::tick::decor::place_decoration` filtered on
    /// `flags.is_blocking()` and took `tile.height`, and `client/app`'s
    /// `clutter::fill` did the same three lines a crate away. Same predicate,
    /// same span, two places to change one of them.
    ///
    /// # The platform arm, and the order the flags are read in
    ///
    /// `PLATFORM` is asked **first**, and `BLOCK` only where it is absent. That
    /// is not a preference: it is the same order the map's own reading takes
    /// (`MapTerrain::static_top` branches on `is_platform` and never looks at
    /// `is_blocking` after), and reading them the other way round would give
    /// one piece of art two heights depending on which layer asked.
    ///
    /// A platform lays **two** covers. It is a surface — where a body on top of
    /// it puts its feet — and it is also a body in the way of anything standing
    /// beside it, which is what stops a staircase being walked into from the
    /// side. The blocking half reaches exactly as far as the surface does, so
    /// **a body standing on a platform is never blocked by it**.
    ///
    /// A platform of no thickness lays no blocking half at all, and that is
    /// load-bearing: a house's ground floor is a `PLATFORM` of height zero laid
    /// on the ground it duplicates, and a blocking half there — which the
    /// `max(1)` in [`Cover::top`] would give it — would seal every house in
    /// Britannia shut.
    ///
    /// **Not a door**, whichever way round that is. Which leaves are doors is
    /// not a property of the tiledata — the shard knows because it made the
    /// entity, and the client knows from `client/render`'s ported door table —
    /// so the caller refines this with [`Covers::as_door`] where it knows
    /// better.
    #[must_use]
    pub fn of_static(tile: &StaticTile) -> Covers {
        if tile.flags.is_platform() {
            let stands = match tile.flags.is_climbable() {
                true => Self::climbable(0, tile.height),
                false => Self::standing(0, tile.height),
            };
            // Based at zero, the surface *is* the rise, and `u8` is where it
            // came from — a halved `u8` is still one.
            let rise = stands.surface() as u8;
            return Covers {
                blocks: (rise > 0).then(|| Self::blocking(0, rise)),
                stands: Some(stands),
            };
        }
        Covers {
            blocks: tile.flags.is_blocking().then(|| Self::blocking(0, tile.height)),
            stands: None,
        }
    }

    /// The same cover, based at `z`.
    ///
    /// [`of_static`](Self::of_static) reads a table, which knows a height and
    /// not a position; this is where the placement supplies the other half.
    #[must_use]
    pub const fn based_at(self, z: i8) -> Self {
        Self { z, ..self }
    }

    /// Whether this is a door somebody could open.
    #[must_use]
    pub const fn is_door(self) -> bool {
        matches!(self.kind, CoverKind::Blocks { door: true })
    }

    /// Whether this is something in the way rather than somewhere to stand.
    ///
    /// The two questions the enum partitions, asked as predicates so a reader
    /// that only cares which arm it is does not have to name the arm's fields.
    #[must_use]
    pub const fn is_blocker(self) -> bool {
        matches!(self.kind, CoverKind::Blocks { .. })
    }

    /// Whether this is somewhere to stand rather than something in the way.
    #[must_use]
    pub const fn is_surface(self) -> bool {
        matches!(self.kind, CoverKind::Stands { .. })
    }

    /// The bottom of its body.
    #[must_use]
    pub const fn bottom(self) -> i32 {
        self.z as i32
    }

    /// One past the top of its body. Never equal to [`bottom`](Self::bottom):
    /// see [`Cover::height`].
    #[must_use]
    pub const fn top(self) -> i32 {
        self.bottom() + if self.height == 0 { 1 } else { self.height as i32 }
    }

    /// Where a body standing on this has its feet.
    ///
    /// Not [`top`](Self::top): the `max(1)` there is about a body of zero
    /// height still occupying its tile, and a *surface* of zero height is at
    /// its own base — the deck is where the plank is, not one z above it.
    ///
    /// **Half way up a climbable**, which is UO's own rule for a stair: Sphere
    /// halves the height of a `CLIMBABLE`, ServUO's `CalcHeight` does the same,
    /// and this is the one place in this workspace it is written — the map's
    /// reading of a *shipped* stair comes through here too. A flight of five
    /// treads is five surfaces two units apart rather than one wall ten tall.
    #[must_use]
    pub const fn surface(self) -> i32 {
        self.bottom()
            + match self.kind {
                CoverKind::Stands { climbable: true } => self.height as i32 / 2,
                _ => self.height as i32,
            }
    }

    /// How far the art itself reaches above its base.
    ///
    /// The third of three tops, and they are three because ServUO's step check
    /// needs all three of them under different names:
    ///
    /// | | what it is | who asks |
    /// |---|---|---|
    /// | [`top`](Self::top) | the *body*, never empty — a zero-tall wall is still a wall | what is in the way |
    /// | [`surface`](Self::surface) | where feet go, half way up a climbable | where a body lands |
    /// | `crest` | the art's own extent | how far the **next** step reaches |
    ///
    /// The last is the one a staircase needs: standing half way up a tread, the
    /// step off it is measured from the top of the whole tread, which is what
    /// carries a body from the flight onto the floor it arrives at. ServUO's
    /// `GetStartZ` calls it `zTop`.
    #[must_use]
    pub const fn crest(self) -> i32 {
        self.bottom() + self.height as i32
    }

    /// The edge a step has to reach to get onto this.
    ///
    /// The same as [`surface`](Self::surface) for anything flat — you arrive
    /// where you end up — and the **base** of a climbable, which is the whole
    /// trick of a staircase: you meet a stair at the low end of its ramp and
    /// are lifted half way up by standing on it. Checking the top instead makes
    /// every riser a wall taller than a step, and the flight unclimbable.
    ///
    /// ServUO's `Movement.Check` calls this pair `(itemTop, ourZ)`; the two are
    /// equal for a solid floor and a table.
    #[must_use]
    pub const fn reach(self) -> i32 {
        match self.kind {
            CoverKind::Stands { climbable: true } => self.bottom(),
            _ => self.surface(),
        }
    }

    /// Whether `body` has this in its way.
    ///
    /// The body spans `[feet, head)` and this spans `[bottom, top)`; they are in
    /// the way of each other when the two overlap. The z-span and not the tile,
    /// so a crate on a building's upper floor leaves the ground floor beneath it
    /// open.
    #[must_use]
    pub const fn meets(self, body: Body) -> bool {
        self.bottom() < body.head() && body.feet() < self.top()
    }
}

/// Everything one piece of static art lays over its own tile: nothing, one
/// thing, or both halves of a platform.
///
/// **Two named fields and not a `Vec`**, because there are exactly two and they
/// are not interchangeable: a caller refining a door refines the blocking half,
/// and a caller asking where a body's feet go wants the other. A list would
/// make both of those a search.
///
/// Built by [`Cover::of_static`], based by [`based_at`](Self::based_at), and
/// then poured into whatever index the caller keeps — it is an *answer in
/// transit*, not something anybody stores.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Covers {
    /// What of it is in the way, if any of it is.
    blocks: Option<Cover>,
    /// What of it can be stood on, if any of it can.
    stands: Option<Cover>,
}

impl Covers {
    /// The half that is in the way, if there is one.
    #[must_use]
    pub const fn blocks(self) -> Option<Cover> {
        self.blocks
    }

    /// The half that can be stood on, if there is one.
    #[must_use]
    pub const fn stands(self) -> Option<Cover> {
        self.stands
    }

    /// Whether this art lays nothing at all — a rug, a bush, a pile of gold.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.blocks.is_none() && self.stands.is_none()
    }

    /// The same covers, based at `z`.
    ///
    /// [`Cover::of_static`] reads a table, which knows a height and not a
    /// position; this is where the placement supplies the other half, for both
    /// halves at once so neither can be left behind at zero.
    #[must_use]
    pub fn based_at(self, z: i8) -> Self {
        Self {
            blocks: self.blocks.map(|cover| cover.based_at(z)),
            stands: self.stands.map(|cover| cover.based_at(z)),
        }
    }

    /// The same covers, with the blocking half marked a shut door.
    ///
    /// Only that half: a door is a leaf hanging in a gap and has never been a
    /// floor, so if the art somehow claims to be both, what a body *walks
    /// through* is the part the latch governs.
    #[must_use]
    pub fn as_door(self) -> Self {
        Self {
            blocks: self.blocks.map(|cover| Cover::door(cover.z, cover.height)),
            ..self
        }
    }
}

impl IntoIterator for Covers {
    type Item = Cover;
    type IntoIter = std::iter::Chain<std::option::IntoIter<Cover>, std::option::IntoIter<Cover>>;

    /// Both halves, in the way an index wants them: blocking first, because
    /// that is the one a step asks about and [`Overlay::blocker_at`] stops at
    /// the first match.
    fn into_iter(self) -> Self::IntoIter {
        self.blocks.into_iter().chain(self.stands)
    }
}

/// Everything the live world has laid over one facet's map, by tile.
///
/// **One per facet, owned by whoever owns the facet, and read by every step
/// decision.** Not built per query: a search asks about hundreds of tiles for
/// one click, and a structure assembled for each of them would be the answer
/// bought a hundred times over. The server keeps this in step from its own
/// indexes as doors flip and ships move; the client rebuilds it whole whenever
/// the shard sends it a new picture of the ground.
#[derive(Clone, Default, Debug)]
pub struct Overlay {
    tiles: FxHashMap<Tile, Vec<Cover>>,
}

impl Overlay {
    /// Whether the live world has put anything anywhere on this facet.
    ///
    /// **The hot path's first question.** A length against zero, before any
    /// hash is computed — a shard with no doors, no decor and no ships asks
    /// this once per step instead of hashing a tile that cannot be there.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// How many tiles hold anything at all.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    /// Everything on `tile`.
    #[must_use]
    pub fn at(&self, tile: Tile) -> &[Cover] {
        self.tiles.get(&tile).map_or(&[], Vec::as_slice)
    }

    /// Replace everything on `tile`.
    ///
    /// **The only mutator, and deliberately whole-tile.** A cover here has no
    /// identity, so there is nothing to address a finer edit to; the owner that
    /// does have one recomputes its tile and hands the result over. That is
    /// what keeps this type honest for both ends — the client, which has no
    /// owners at all and sets a whole picture, and the server, whose indexes
    /// key by entity and project one tile at a time.
    ///
    /// An empty list removes the tile, so [`is_empty`](Self::is_empty) stays
    /// the question it claims to be.
    pub fn set(&mut self, tile: Tile, covers: Vec<Cover>) {
        match covers.is_empty() {
            true => {
                self.tiles.remove(&tile);
            }
            false => {
                self.tiles.insert(tile, covers);
            }
        }
    }

    /// Forget everything, keeping the allocation.
    ///
    /// What a whole-picture rebuild starts with — the client's, which throws the
    /// previous view away rather than diffing it.
    pub fn clear(&mut self) {
        self.tiles.clear();
    }

    /// The first thing in the way of `body` standing on `tile`, under this
    /// reading of the doors.
    ///
    /// What a step asks. `None` is "nothing here stops you", which is not the
    /// same as "you may stand here" — the map answers that.
    #[must_use]
    pub fn blocker_at(&self, tile: Tile, body: Body, doors: Doors) -> Option<Cover> {
        self.at(tile)
            .iter()
            .copied()
            .find(|cover| cover.blocks_body(body, doors))
    }

    /// The first thing in the way on `tile` at *any* height.
    ///
    /// Where the height genuinely does not enter it: a sight line, which a shut
    /// door stops wherever it hangs, and the question "is there a door on this
    /// tile" a body asks before deciding to open one.
    #[must_use]
    pub fn blocker_anywhere(&self, tile: Tile) -> Option<Cover> {
        self.at(tile)
            .iter()
            .copied()
            .find(|cover| matches!(cover.kind, CoverKind::Blocks { .. }))
    }

    /// Every surface the live world put on `tile` — a deck, a house's floor, a
    /// tread of its stairs.
    ///
    /// The candidate list, unfiltered and in the tile's own order. What a
    /// *step* does with it is a movement rule and lives there: which of these
    /// is in reach, and which of them a body fits on.
    pub fn surfaces_at(&self, tile: Tile) -> impl Iterator<Item = Cover> + '_ {
        self.at(tile).iter().copied().filter(|cover| cover.is_surface())
    }

    /// The surface a body coming from `near_z` would stand on at `tile`, if the
    /// live world put one there.
    ///
    /// The nearest one to where the body already is, so stepping up onto a deck
    /// from a pier and stepping down onto it from a mast are the same rule.
    #[must_use]
    pub fn surface_at(&self, tile: Tile, near_z: i32) -> Option<i32> {
        self.surfaces_at(tile)
            .map(|cover| cover.surface())
            .min_by_key(|surface| (surface - near_z).abs())
    }
}

impl Cover {
    /// Whether this stops `body`, under `doors`.
    ///
    /// Private, because the two halves are not separable: a door left open by
    /// the reading is not "a blocker that does not block", it is nothing at
    /// all, and a caller that saw it as the former would report a doorway as
    /// obstructed.
    const fn blocks_body(self, body: Body, doors: Doors) -> bool {
        match self.kind {
            CoverKind::Blocks { door: true } if matches!(doors, Doors::AllOpen) => false,
            CoverKind::Blocks { .. } => self.meets(body),
            CoverKind::Stands { .. } => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_tiles::TileFlags;

    const HERE: Tile = Tile::new(100, 100);

    /// A person, as `openshard-movement` asks about one. Spelled here so the
    /// tests read like the call sites do; the constant itself is movement's.
    fn person(z: i32) -> Body {
        Body::new(z, 16)
    }

    /// The whole of what the z-span is for: a wall on an upper floor is not a
    /// sealed ground floor. Registering both used to be the only way to say it
    /// on the server and a separate arithmetic on the client.
    #[test]
    fn a_wall_upstairs_leaves_the_floor_below_open() {
        let mut overlay = Overlay::default();
        overlay.set(HERE, vec![Cover::blocking(20, 20)]);

        assert!(overlay.blocker_at(HERE, person(0), Doors::AsTheyStand).is_none());
        assert!(overlay.blocker_at(HERE, person(25), Doors::AsTheyStand).is_some());
        assert!(overlay.blocker_at(HERE, person(60), Doors::AsTheyStand).is_none());
    }

    /// Impassable art with a tiledata height of zero still occupies its tile.
    /// A flat span would overlap nothing and block nowhere, which reads exactly
    /// like the bug this type exists to fix.
    #[test]
    fn zero_height_art_still_occupies_its_own_tile() {
        let mut overlay = Overlay::default();
        overlay.set(HERE, vec![Cover::blocking(0, 0)]);

        assert!(overlay.blocker_at(HERE, person(0), Doors::AsTheyStand).is_some());
        // And nothing above it: a flat blocker is one z tall, not infinite.
        assert!(overlay.blocker_at(HERE, person(1), Doors::AsTheyStand).is_none());
    }

    /// The two readings, on the same tile, differing only in the door.
    #[test]
    fn a_plan_walks_through_a_door_and_not_through_a_crate() {
        let mut overlay = Overlay::default();
        overlay.set(HERE, vec![Cover::door(0, 20)]);
        assert!(overlay.blocker_at(HERE, person(0), Doors::AsTheyStand).is_some());
        assert!(overlay.blocker_at(HERE, person(0), Doors::AllOpen).is_none());

        // A crate dragged into the doorway is still there once the door swings:
        // opening it does not move the crate.
        overlay.set(HERE, vec![Cover::door(0, 20), Cover::blocking(0, 12)]);
        assert!(overlay.blocker_at(HERE, person(0), Doors::AllOpen).is_some());
    }

    /// A deck is somewhere to stand and not something in the way, and the same
    /// tile can hold both: a crate lashed to the deck.
    #[test]
    fn a_deck_carries_and_a_hull_stops() {
        let mut overlay = Overlay::default();
        overlay.set(HERE, vec![Cover::standing(-2, 5)]);

        assert_eq!(overlay.surface_at(HERE, 0), Some(3));
        assert!(
            overlay.blocker_at(HERE, person(3), Doors::AsTheyStand).is_none(),
            "a body standing on the deck is not blocked by the deck"
        );

        overlay.set(HERE, vec![Cover::standing(-2, 5), Cover::blocking(3, 12)]);
        assert_eq!(overlay.surface_at(HERE, 0), Some(3), "the crate is not a surface");
        assert!(overlay.blocker_at(HERE, person(3), Doors::AsTheyStand).is_some());
    }

    /// A gunwale at deck height seals neither the deck nor the water under the
    /// ship — the case the hull rule was written for, restated as a span.
    ///
    /// This is the one behaviour folding boats into this type changed: `Boats`
    /// asked whether the body's *feet* were inside the plank, and everything
    /// else asks whether the body's *span* meets it. The span test is the
    /// stricter of the two, so it is the one that had to be checked against the
    /// case the looser one was written for.
    #[test]
    fn a_hull_ends_where_its_own_deck_begins() {
        let mut overlay = Overlay::default();
        // A plank at z = -2, five tall: its deck is at 3, and a hull piece of
        // the same shape spans [-2, 3).
        overlay.set(HERE, vec![Cover::standing(-2, 5), Cover::blocking(-2, 5)]);

        assert!(
            overlay.blocker_at(HERE, person(3), Doors::AsTheyStand).is_none(),
            "the hull sealed the deck standing on top of it"
        );
        assert!(
            overlay.blocker_at(HERE, person(-2), Doors::AsTheyStand).is_some(),
            "the water inside the hull is not somewhere to swim"
        );
    }

    /// The nearest deck to where the body already is: a mast's crow's nest and
    /// the deck under it are two surfaces on one tile, and which one a body
    /// lands on is which one it is coming from.
    #[test]
    fn the_nearest_deck_is_the_one_stepped_onto() {
        let mut overlay = Overlay::default();
        overlay.set(HERE, vec![Cover::standing(-2, 5), Cover::standing(40, 2)]);

        assert_eq!(overlay.surface_at(HERE, 0), Some(3));
        assert_eq!(overlay.surface_at(HERE, 50), Some(42));
    }

    /// Emptying a tile empties the overlay, so the hot path's first question
    /// stays the question it claims to be.
    #[test]
    fn a_tile_set_empty_leaves_nothing_behind() {
        let mut overlay = Overlay::default();
        assert!(overlay.is_empty());
        overlay.set(HERE, vec![Cover::blocking(0, 20)]);
        assert!(!overlay.is_empty());
        overlay.set(HERE, Vec::new());
        assert!(overlay.is_empty(), "an emptied tile is still hashed");
    }

    /// A body's height is the caller's, and the overlay answers differently for
    /// two of them over the same cover: a gap a person does not fit under is one
    /// a rat walks straight through.
    #[test]
    fn how_tall_the_body_is_is_the_askers_business() {
        let mut overlay = Overlay::default();
        // A shelf whose underside is at z = 8.
        overlay.set(HERE, vec![Cover::blocking(8, 4)]);

        assert!(
            overlay
                .blocker_at(HERE, Body::new(0, 16), Doors::AsTheyStand)
                .is_some(),
            "a person standing under it reaches into it"
        );
        assert!(
            overlay
                .blocker_at(HERE, Body::new(0, 8), Doors::AsTheyStand)
                .is_none(),
            "something half as tall does not"
        );
    }

    /// The rule both ends of the wire lay a placed item's cover with.
    ///
    /// This is the whole of the agreement `clutter.rs`'s header claims and that
    /// nothing checked before node E: the shard's `place_decoration` and the
    /// client's `clutter::fill` both call [`Cover::of_static`], so a step refused
    /// at one end is refused at the other for the same reason rather than for a
    /// similar one. What is asserted here is the contract they share.
    #[test]
    fn a_placed_item_covers_its_tile_exactly_when_its_art_blocks() {
        let barrel = StaticTile {
            flags: TileFlags::new(TileFlags::BLOCK),
            height: 12,
            ..StaticTile::default()
        };
        let barrel = Cover::of_static(&barrel).based_at(-3);
        assert_eq!(
            barrel.blocks(),
            Some(Cover::blocking(-3, 12)),
            "the span is the art's own height, based where the item was placed"
        );
        assert_eq!(barrel.stands(), None, "a barrel is not a table");

        let rug = StaticTile {
            flags: TileFlags::new(0),
            height: 0,
            ..StaticTile::default()
        };
        assert!(
            Cover::of_static(&rug).is_empty(),
            "art that neither blocks nor carries covers nothing"
        );

        // Impassable art with a tiledata height of zero is common, and it still
        // occupies its tile — see `Cover::height`.
        let flat = StaticTile {
            flags: TileFlags::new(TileFlags::BLOCK),
            height: 0,
            ..StaticTile::default()
        };
        let flat = Cover::of_static(&flat).based_at(0).blocks().expect("it blocks");
        assert!(flat.meets(person(0)));
        assert!(!flat.meets(person(1)));
    }

    /// A house's floor: `PLATFORM`, no `BLOCK`, and a tiledata height of zero.
    ///
    /// **The case the whole platform arm is about.** Every wooden-boards
    /// component of every classic multi looks exactly like this, and until this
    /// arm existed a house had no floors at all — `of_static` filtered on
    /// `is_blocking` and a floor blocks nothing, so it laid nothing.
    ///
    /// The second assertion is the one that could have gone wrong: a blocking
    /// half of height zero would reach one z up (see [`Cover::top`]) and seal
    /// the very floor it is, which is a house nobody can stand in.
    #[test]
    fn a_floor_is_a_surface_and_not_a_body() {
        let boards = StaticTile {
            flags: TileFlags::new(TileFlags::PLATFORM),
            height: 0,
            ..StaticTile::default()
        };
        let floor = Cover::of_static(&boards).based_at(7);

        assert_eq!(floor.stands(), Some(Cover::standing(7, 0)));
        assert_eq!(floor.stands().expect("a floor").surface(), 7);
        assert_eq!(
            floor.blocks(),
            None,
            "a floor of no thickness is nothing to walk into"
        );

        let mut overlay = Overlay::default();
        overlay.set(HERE, floor.into_iter().collect());
        assert_eq!(overlay.surface_at(HERE, 0), Some(7));
        assert!(
            overlay.blocker_at(HERE, person(7), Doors::AsTheyStand).is_none(),
            "a body standing on the floor is not blocked by it"
        );
        assert!(
            overlay.blocker_at(HERE, person(0), Doors::AsTheyStand).is_none(),
            "and neither is one on the ground beneath it"
        );
    }

    /// A stair: `PLATFORM | CLIMBABLE`, five tall. Both halves, and both of the
    /// climbable's two numbers.
    ///
    /// This is a real component of multi `0x0064` — "stone stairs", at `dz = 2`
    /// against a first floor at `dz = 7`. The arithmetic here is what decides
    /// whether that flight is climbable at all: you meet the tread at z 2, you
    /// stand on it at z 4, and the *art* still reaches z 7, which is the height
    /// the next step is measured from.
    #[test]
    fn a_stair_is_met_at_its_base_and_stood_on_half_way_up() {
        let stone_stairs = StaticTile {
            flags: TileFlags::new(TileFlags::PLATFORM | TileFlags::CLIMBABLE),
            height: 5,
            ..StaticTile::default()
        };
        let tread = Cover::of_static(&stone_stairs).based_at(2);
        let stands = tread.stands().expect("a stair is somewhere to stand");

        assert_eq!(stands.surface(), 4, "half way up, Sphere's halved CLIMBABLE");
        assert_eq!(stands.reach(), 2, "and met at the low end of the ramp");
        assert_eq!(stands.top(), 7, "the art still reaches its full height");
        assert_eq!(
            tread.blocks(),
            Some(Cover::blocking(2, 2)),
            "its body reaches exactly as far as its surface and no further"
        );

        // Which is the whole point of the second half: a body standing beside
        // the tread walks into it, and a body standing *on* it does not.
        let mut overlay = Overlay::default();
        overlay.set(HERE, tread.into_iter().collect());
        assert!(overlay.blocker_at(HERE, person(2), Doors::AsTheyStand).is_some());
        assert!(overlay.blocker_at(HERE, person(4), Doors::AsTheyStand).is_none());
    }

    /// A table — `PLATFORM | BLOCK` — is read as a platform, and the order the
    /// two flags are asked in is what says so.
    ///
    /// Read the other way round it would be a solid twelve-tall body with
    /// nothing on top, which is both ends of the wire disagreeing with the
    /// map's own reading of the same art (`MapTerrain::static_top`).
    #[test]
    fn art_that_is_both_flags_is_a_platform() {
        let table = StaticTile {
            flags: TileFlags::new(TileFlags::PLATFORM | TileFlags::BLOCK),
            height: 12,
            ..StaticTile::default()
        };
        let table = Cover::of_static(&table).based_at(0);
        assert_eq!(table.stands(), Some(Cover::standing(0, 12)));
        assert_eq!(table.blocks(), Some(Cover::blocking(0, 12)));
    }

    /// The blocking half of a door is the half the latch governs.
    #[test]
    fn a_door_refines_only_what_is_in_the_way() {
        let leaf = StaticTile {
            flags: TileFlags::new(TileFlags::BLOCK),
            height: 20,
            ..StaticTile::default()
        };
        let leaf = Cover::of_static(&leaf).based_at(0).as_door();
        assert_eq!(leaf.blocks(), Some(Cover::door(0, 20)));

        let mut overlay = Overlay::default();
        overlay.set(HERE, leaf.into_iter().collect());
        assert!(overlay.blocker_at(HERE, person(0), Doors::AsTheyStand).is_some());
        assert!(overlay.blocker_at(HERE, person(0), Doors::AllOpen).is_none());
    }

    /// Sight and door-detection ask about the tile and not about a height.
    #[test]
    fn a_door_is_found_whatever_height_it_hangs_at() {
        let mut overlay = Overlay::default();
        overlay.set(HERE, vec![Cover::standing(-2, 5), Cover::door(80, 20)]);

        assert!(overlay.blocker_at(HERE, person(0), Doors::AsTheyStand).is_none());
        assert!(
            overlay.blocker_anywhere(HERE).is_some_and(Cover::is_door),
            "a door three storeys up is still a door on this tile"
        );
    }
}
