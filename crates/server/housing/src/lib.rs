//! Player houses: placing one, and the ground it is allowed to stand on.
//!
//! A **multi** is one item that draws as many. The wire carries a house as an
//! ordinary world item whose graphic is `0x4000 | id`; the client looks the id up
//! in its own files and draws the hundred and forty-eight statics a villa is made
//! of. **This crate sends none of them.**
//!
//! That is what makes a house tractable: the picture is free, because every
//! client already owns every house. What the shard owes is the half the picture
//! does not carry — where a wall is for the purpose of *stopping* somebody, and
//! whether this patch of Britannia was somewhere a house may go at all.
//!
//! See [`docs/housing.md`](../../../../docs/housing.md) for the five phases and
//! the decisions; this is H1.
//!
//! # Where the components come from
//!
//! [`WorldState::multis`](openshard_state::WorldState::multis), the shard's own
//! table. A multi's shape is a fact about the *install* and not about a facet, so
//! it is not reached through the ground a house happens to stand on — an install
//! with no `multi.mul` holds an empty table, which knows about no houses and
//! refuses every placement by name.
//!
//! # The footprint is stored, not recomputed
//!
//! Placement folds the blocking components into
//! [`Obstructions`](openshard_state::Obstructions) once. A step is ten a second
//! and a house does not move, so asking `multi.mul` per step would be paying a
//! hundred lookups for an answer that cannot have changed.

pub mod decay;
pub mod design;
pub mod sign;
pub mod storage;

mod access;
mod classic_doors;
pub use access::{ListRefusal, MAX_BANS, MAX_CO_OWNERS, MAX_FRIENDS, Standing, ban, distrust, trust, unban};

#[cfg(test)]
mod tests;

use openshard_entities::EntityId;
use openshard_map::grid::Tile;
use openshard_map::overlay::Cover;
use openshard_protocol::serial::Serial;
use openshard_protocol::wire::{Graphic, Hue, MultiId};
use openshard_protocol::world::{Facet, Point};
use openshard_state::components::{Drawn, House, Position};
use openshard_state::{FacetState, ItemLocation, WorldState, establish_item_location};
use openshard_uofiles::multi::Component;

/// The first customisable-house foundation id, and the last.
///
/// ServUO's `HousePlacement.Check` adds stairs to any multi in this range,
/// because a foundation's own component list has none — the stairs are part of
/// the *design*, which is a system this engine does not have. A foundation placed
/// without them is a house nobody can get into, so the range is refused by name
/// rather than placed and wondered about. See `docs/housing.md`'s D7.
pub const FOUNDATION_IDS: std::ops::Range<u16> = 0x13EC..0x1D00;

/// Why a house could not go there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    /// No client files, or an id no client knows: there is nothing to place.
    NoSuchMulti,
    /// A customisable foundation, which needs a design system to have stairs.
    /// See [`FOUNDATION_IDS`].
    NeedsCustomisation,
    /// The multi is in the table and draws nothing — a treasure-site marker
    /// rather than a building. See `findings.md`.
    DrawsNothing,
    /// Part of the footprint is off the edge of the world.
    OffTheMap,
    /// Something already stands where the house would.
    Occupied,
    /// A footprint tile is over a road, a furrow or sand stones. ServUO's fifth
    /// rule, and the one a player notices the absence of: without it houses go
    /// up across Britain's streets.
    OnARoad,
    /// The ground will not take the house — a wall in the way, or thin air with
    /// no surface under it. ServUO's rules two and four, which `can_fit` asks as
    /// one question.
    BadGround,
    /// Another house's yard. Every house keeps five tiles to itself.
    TooCloseToAHouse,
    /// A region that does not take houses — ServUO's `no_housing`, which the
    /// shipped dataset sets on twenty-one dungeons. See H6's D9.
    NoHousingHere,
    /// The serial pool is dry, which is a shard in trouble rather than a bad spot.
    NoSerials,
}

impl Refusal {
    /// What to say to whoever tried.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::NoSuchMulti => "No house has that number.",
            Self::NeedsCustomisation => "That is a customisable foundation, which this shard cannot build.",
            Self::DrawsNothing => "That multi is a marker, not a building.",
            Self::OffTheMap => "The house would hang off the edge of the world.",
            Self::Occupied => "Something is in the way.",
            Self::OnARoad => "A house may not be built on a road.",
            Self::BadGround => "The ground will not take a house here.",
            Self::TooCloseToAHouse => "That is too close to another house.",
            Self::NoHousingHere => "Houses may not be built here.",
            Self::NoSerials => "The shard is out of item serials.",
        }
    }
}

/// Every tile a component list draws on, deduped — what [`tiles_of`] is once it
/// has a list, kept apart from it so a caller that already holds a component list
/// need not go back through the multi table to lay it on the ground.
fn drawn_tiles(components: &[Component], at: Point) -> Vec<Tile> {
    let mut out: Vec<Tile> = components
        .iter()
        .filter(|component| component.drawn())
        // `Component::placed_at` and not this function's own addition: a multi
        // is expanded in three places in this workspace and the offset has to
        // mean one thing in all of them.
        .filter_map(|component| component.placed_at(at))
        .map(|at| Tile::new(at.x, at.y))
        .collect();
    out.sort_unstable_by_key(|tile| (tile.x, tile.y));
    out.dedup();
    out
}

/// A house's shape, from wherever this house's comes from.
///
/// **One chooser, not three.** [`sign_spot`], [`tiles_of`] and [`footprint_of`]
/// each read a house's components, and the choice they now have to make is the
/// same choice — so it is written once rather than copied into each and left to
/// drift apart. See `docs/customisation.md`'s C2.
///
/// `None` is the shard's fixed multi table: every classic house, still a borrow,
/// so the common path allocates nothing. `Some` is this house's own design.
///
/// `Option` rather than a modelled state, and `style.md`'s rule that an `Option`
/// means *absent* and not *unknown* is what makes that right: a classic house
/// genuinely has no design. A foundation with no design is a different thing, and
/// C3 makes it unrepresentable rather than letting it hide in here.
fn shape_of<'a>(design: Option<&'a [Component]>, state: &'a WorldState, multi: MultiId) -> &'a [Component] {
    design.unwrap_or_else(|| state.multis.components(multi.0))
}

/// One entry a house lays on one tile, already in world coordinates.
///
/// **A `Cover` and a place to put it**, which is all it ever was: it used to
/// spell out a `z` and a `height` beside the tile and hand those to
/// `FacetState::block`, which is a cover with the kind left out — and leaving
/// the kind out is exactly why a house had no floors. See
/// `docs/map/realtime_map.md`'s R3.
///
/// One component can produce two of these on one tile: a stair tread is a
/// surface and a body. See [`openshard_map::overlay::Cover::of_static`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Footprint {
    /// Where.
    pub tile: Tile,
    /// What the house puts there, based at the house's own z plus the
    /// component's.
    pub cover: Cover,
}

/// Put a house on the ground and make its walls stop people.
///
/// `at` is the multi's **origin**, which is not the corner of its box — see
/// [`Multi::center`](openshard_uofiles::multi::Multi::center). It is the tile the
/// player clicked, and matching the reference's arithmetic for it is what keeps a
/// house from landing one tile off the spot they picked.
///
/// Returns the house's entity. The caller announces it; this does not, for
/// `spawn_item`'s reason — what a placement *is* to the world (a staff command, a
/// deed being consumed) is the caller's business.
///
/// # `actor` and `owner` are two facts, not one
///
/// Who is *asking* and whose house it *is*. Both callers pass the same mobile
/// today, and a game master placing a house for somebody else is the case that
/// separates them — which is why they are two parameters rather than one read
/// two ways. `actor` is an `EntityId` and not an `Option`: a placement always
/// has somebody who caused it, and `style.md` says an `Option` means absent
/// rather than unknown.
///
/// # What staff are exempt from, and what they are not
///
/// D3 has claimed "staff place anywhere" since H1 and it was never true, because
/// there was no actor to ask about. It is true now, and it is **not** the
/// reference's single early return — this engine's [`Refusal`] mixes two kinds
/// of answer, and skipping both kinds would reopen a hole another decision
/// closed:
///
/// | refusal | what it is | exempt |
/// |---|---|---|
/// | `NoHousingHere`, `Occupied`, `OnARoad`, `BadGround`, `TooCloseToAHouse` | a judgement about the plot | **yes** |
/// | `NoSuchMulti`, `DrawsNothing`, `NeedsCustomisation`, `OffTheMap`, `NoSerials` | there is nothing to place, or the shard is broken | **no** |
///
/// A game master laying out a town needs the first row skipped. Nobody needs an
/// invisible house out of a treasure-site marker, or a foundation with no
/// stairs. See H6's D10.
pub fn place(
    state: &mut WorldState,
    actor: EntityId,
    at: Point,
    facet: Facet,
    multi: MultiId,
    owner: Serial,
) -> Result<EntityId, Refusal> {
    // Once, at the top, and threaded — this crate's own idiom, the way `trust`,
    // `distrust`, `ban`, `unban` and `standing_of` all take it rather than each
    // asking again.
    let staff = state.is_staff(actor);
    // A foundation is placed **with a design**, and that is the whole of what
    // `NeedsCustomisation` was waiting for: its own component list has no
    // stairs, so one placed bare is a house nobody can get into. The refusal
    // stands only where the design cannot be built — a shard with no client
    // files has no platform to build one out of either.
    let design = if FOUNDATION_IDS.contains(&multi.0) {
        match design::initial_foundation(state, multi) {
            Some(design) => Some(design),
            None => return Err(Refusal::NeedsCustomisation),
        }
    } else {
        None
    };
    let footprint = footprint_of(state, at, multi, design.as_deref())?;
    if footprint.is_empty() {
        return Err(Refusal::DrawsNothing);
    }
    // Every tile the house *covers*, hoisted: the region check walks it, and so
    // does the lockdown allowance below. One derivation, two readers — it was
    // already being computed here, one line further down.
    let covered = tiles_of(state, at, multi, design.as_deref());
    // The four judgements about the plot, and the one row of D10's table staff
    // skip. Everything above this stays: those refusals are facts about the id
    // or a shard in trouble, and a bypass that reopened `NeedsCustomisation`
    // would undo the decision that stops a foundation going down with no stairs.
    if !staff {
        // **First of the judgements**, and that ordering is the *message*. Every
        // other refusal here means "try a tile over" — `Occupied` as much as
        // `BadGround` — and inside Deceit that is a lie a player spends ten
        // minutes proving. This is the only one that is a statement about the
        // *place*. See H6's D9b.
        check_region(state, facet, at, &covered)?;
        if occupied_tile(state, facet, &footprint).is_some() {
            return Err(Refusal::Occupied);
        }
        check_ground(state, facet, at, &footprint)?;
        check_yard(state, facet, &footprint)?;
    }

    let Ok((entity, _)) = state
        .registry
        .spawn_with_serial(openshard_protocol::serial::SerialKind::Item)
    else {
        return Err(Refusal::NoSerials);
    };
    state.registry.insert(
        entity,
        Drawn {
            id: multi.graphic(),
            hue: Hue(0),
        },
    );
    state.registry.insert(
        entity,
        House {
            multi,
            owner,
            co_owners: Default::default(),
            friends: Default::default(),
            bans: Default::default(),
            // From the footprint, once, and stored — see `storage::allowance_for`.
            // The tiles are counted here because this is the one moment the multi
            // table is in hand.
            age: 0,
            lockdowns: u32::try_from(storage::allowance_for(covered.len()).lockdowns).unwrap_or(u32::MAX),
        },
    );
    establish_item_location(state, entity, ItemLocation::ground(facet, at))
        .expect("a fresh house has one valid plot");
    // Before the walls go in and before the sign hangs: both read the house's
    // *own* shape now, and a foundation's is its design rather than its multi.
    if let Some(components) = design {
        state.registry.insert(
            entity,
            openshard_state::components::HouseDesign {
                components,
                revision: 1,
            },
        );
    }
    // On the sector grid like any item, so a client entering the area is told
    // about it by the ordinary interest sweep rather than by a path of its own.
    state.place_item(facet, entity, at);
    block_footprint(state.facet_state_mut(facet), entity, &footprint);
    install_doors(state, entity, facet, at, multi);
    let sign = hang_sign(state, entity, facet, at, multi);
    // `place_item` only files an entity for later interest sweeps. A house
    // built in front of a stationary client must also be drawn now, just like
    // an ordinary item spawned on the ground.
    state.reveal(entity);
    if let Some(sign) = sign {
        state.reveal(sign);
    }
    Ok(entity)
}

/// The graphic a house sign draws as — ServUO's `HouseSign`.
pub const SIGN_GRAPHIC: u16 = 0x0BD2;

/// How far above a customisable foundation's z its derived sign hangs.
const SIGN_Z: i16 = 7;

/// The sign offset explicitly declared by each classic ServUO house.
///
/// A classic multi's bounds describe everything it draws, not where its sign
/// belongs. In particular, the guild house (`0x74`) spans from `-7` to `+7`,
/// while its sign is attached at `(4, 8, 16)`, beside the entrance. Falling
/// back to a corner of that box puts the plaque on the opposite side of the
/// building and at the wrong height.
fn classic_sign_offset(multi: MultiId) -> Option<(i16, i16, i16)> {
    match multi.0 {
        0x0064 | 0x0066 | 0x0068 | 0x006A | 0x006C | 0x006E => Some((2, 4, 5)),
        0x0074 => Some((4, 8, 16)),
        0x0076 | 0x0078 => Some((2, 8, 16)),
        0x007A => Some((5, 8, 16)),
        0x007C => Some((5, 12, 16)),
        0x007E => Some((5, 17, 16)),
        0x008C => Some((1, 8, 16)),
        0x0096 => Some((1, 8, 11)),
        0x0098 => Some((1, 4, 5)),
        0x009A => Some((5, 8, 20)),
        0x009C => Some((4, 6, 24)),
        0x009E => Some((3, 8, 24)),
        0x00A0 => Some((3, 4, 7)),
        0x00A2 => Some((3, 4, 5)),
        _ => None,
    }
}

/// Where a house's sign hangs, or `None` on a shard with no multi table.
///
/// # Classic houses declare; foundations derive
///
/// ServUO's classic houses each declare theirs — `SetSign(2, 4, 5)`,
/// `SetSign(5, 12, 16)`, fourteen house types. They are not generally a corner
/// of the multi: the explicit per-type table is used for them just as it is for
/// their doors.
///
/// **Customisable** houses do not have that luxury, because the multi is
/// built at run time, so `HouseFoundation` computes one: `x = Components.Min.X`,
/// `y = Components.Height - 1 - Components.Center.Y`, `z = 7`. Reduce it against
/// [`Multi::center`](openshard_uofiles::multi::Multi::center)'s own definition
/// and the y is just `max_y` — so their rule is the box's west-south corner.
/// Unknown non-foundation multis retain that derived fallback.
///
/// The hanger (`0xB98`) that ServUO puts on the same tile is left out. It draws
/// a bracket and does nothing, and one more entity per house is one more to
/// save, restore and take down.
#[must_use]
pub fn sign_spot(
    state: &WorldState,
    at: Point,
    multi: MultiId,
    design: Option<&[Component]>,
) -> Option<Point> {
    let box_ = openshard_uofiles::multi::bounds(shape_of(design, state, multi))?;
    let classic = match design {
        None => classic_sign_offset(multi),
        Some(_) => None,
    };
    let (dx, dy, dz) = classic.unwrap_or((box_.min_x, box_.max_y, SIGN_Z));
    let x = u16::try_from(i32::from(at.x) + i32::from(dx)).ok()?;
    let y = u16::try_from(i32::from(at.y) + i32::from(dy)).ok()?;
    let z = i8::try_from(i32::from(at.z) + i32::from(dz)).ok()?;
    Some(Point::new(x, y, z))
}

/// Hang a house's sign, and return it.
///
/// Separate from [`place`] because the restore needs it too and does not go
/// through `place` — a house that was legal when it was built stays built, and
/// a sign that only existed on the placement path would vanish at the first
/// restart.
pub fn hang_sign(
    state: &mut WorldState,
    house: EntityId,
    facet: Facet,
    at: Point,
    multi: MultiId,
) -> Option<EntityId> {
    let serial = state.registry.serial_of(house)?;
    // The house's own design if it has one: the sign hangs off the box's corner
    // and a designed house's box is not the foundation's.
    let shape = design::shape_of_house(state, house);
    let spot = sign_spot(state, at, multi, shape.as_deref())?;
    let (sign, _) = state
        .registry
        .spawn_with_serial(openshard_protocol::serial::SerialKind::Item)
        .ok()?;
    state.registry.insert(
        sign,
        Drawn {
            id: Graphic(SIGN_GRAPHIC),
            hue: Hue(0),
        },
    );
    state
        .registry
        .insert(sign, openshard_state::components::HouseSign { house: serial });
    establish_item_location(state, sign, ItemLocation::ground(facet, spot))
        .expect("a fresh house sign has one valid ground location");
    state.place_item(facet, sign, spot);
    Some(sign)
}

/// Hand every door standing inside a footprint to the house.
///
/// # Why adoption still exists beside classic fixtures
///
/// The obvious source is the multi itself, and it is not one: of the 326 multis
/// a shipped `multi.mul` holds, **three** carry a door component. The reference
/// agrees — ServUO's houses call `AddDoor` from each house class with an explicit
/// graphic and position. [`install_doors`] carries that table for classic houses.
///
/// Adoption is the remaining rule a player would state: another door standing
/// inside your house is your house's door too. It is right for a door added by a
/// pack, by a staff command or by customisation with no classic catalog row.
///
/// Called at placement, and again whenever a door is put down — a house cannot
/// adopt a door that does not exist yet.
pub fn adopt_doors(state: &mut WorldState, house: EntityId, facet: Facet, at: Point, multi: MultiId) {
    let Some(serial) = state.registry.serial_of(house) else {
        return;
    };
    // **Every drawn tile, not the blocking footprint.** A door stands in a
    // *doorway*, which is by construction a gap in the walls — the one place the
    // footprint does not reach. Using it here adopted nothing, which a test
    // caught rather than a player.
    let area = tiles_of(state, at, multi, design::shape_of_house(state, house).as_deref());
    let inside: Vec<EntityId> = state
        .registry
        .query::<openshard_state::components::Door>()
        .map(|(entity, _)| entity)
        .filter(|&entity| state.facet_of(entity) == facet)
        .filter(|&entity| {
            state
                .registry
                .get::<Position>(entity)
                .is_some_and(|&Position(at)| area.contains(&Tile::new(at.x, at.y)))
        })
        .collect();
    for door in inside {
        state
            .registry
            .insert(door, openshard_state::components::HouseDoor { house: serial });
    }
}

/// Restore or create the separately-defined doors of a classic house.
///
/// Public for the boot path: saved decoration is restored before houses, so
/// this reuses those door entities and fills only fixtures missing from an old
/// save made before classic houses installed their own doors.
pub fn install_doors(state: &mut WorldState, house: EntityId, facet: Facet, at: Point, multi: MultiId) {
    classic_doors::install(state, house, facet, at, multi);
    adopt_doors(state, house, facet, at, multi);
}

/// Where a banned player is put out to.
///
/// One tile west of the house's box, on whatever a body put there stands on.
///
/// **Not** the sign's tile, now that there is one: [`sign_spot`] hangs it on the
/// wall at z+7, which is a place for a plaque and not for a person. ServUO moves
/// the banned to a `BaseBanLocation` each house class declares — a third
/// hand-written table, alongside the doors and the sign offsets — and "just
/// outside, on the side the box ends" is the same intent from data that exists.
///
/// **The height is the world's and not the house's.** It used to be `at.z`
/// carried verbatim: a house on a rise puts its own floor a good way over the
/// slope outside it, and an eviction to that height is a body standing inside
/// the hill next door — the same defect as every other arrival that named a
/// height instead of asking for one (`movement::arrival_z`, and `roadmap.md`'s
/// third pier suspect). The house's z is still what the question is asked
/// *near*: out of the door and down the step, not up onto the roof.
/// The `facet` is the whole reason this takes one: every other question in this
/// module is about a multi's *shape*, which is the same on every facet, and this
/// one is about the ground.
#[must_use]
pub fn doorstep(state: &WorldState, facet: Facet, at: Point, multi: MultiId) -> Point {
    let tiles = tiles_of(state, at, multi, None);
    let west = tiles.iter().map(|tile| tile.x).min().unwrap_or(at.x);
    let outside = Tile::new(west.saturating_sub(1), at.y);
    let z = openshard_movement::arrival_z(
        &state.footing(facet, openshard_map::overlay::Doors::AsTheyStand),
        outside,
        i32::from(at.z),
        openshard_movement::PLAYER_HEIGHT,
    )
    .and_then(|z| i8::try_from(z).ok())
    .unwrap_or(at.z);
    Point::new(outside.x, outside.y, z)
}

/// Put every banned player standing inside a house out of it.
///
/// The one rule in H3 that *acts* on somebody rather than refusing them, and the
/// reason a ban is worth anything at all: a ban that only locked the door would
/// leave whoever was already inside there for good.
///
/// Returns who was moved, so the caller can tell them — this crate does not send
/// packets, for `place`'s reason.
pub fn evict_the_banned(state: &mut WorldState, house: EntityId) -> Vec<EntityId> {
    let Some(entry) = state.registry.get::<House>(house).cloned() else {
        return Vec::new();
    };
    let Some(&Position(at)) = state.registry.get::<Position>(house) else {
        return Vec::new();
    };
    let facet = state.facet_of(house);
    let area = tiles_of(state, at, entry.multi, None);
    let out = doorstep(state, facet, at, entry.multi);

    let caught: Vec<EntityId> = state
        .registry
        .query::<Position>()
        .filter(|(entity, _)| state.registry.has::<openshard_state::components::Body>(*entity))
        .filter(|(entity, _)| state.facet_of(*entity) == facet)
        .filter(|(_, Position(where_they_are))| area.contains(&Tile::new(where_they_are.x, where_they_are.y)))
        .filter(|(entity, _)| {
            state
                .registry
                .serial_of(*entity)
                .is_some_and(|who| entry.standing_of(who, state.is_staff(*entity)) == Standing::Banned)
        })
        .map(|(entity, _)| entity)
        .collect();
    for who in &caught {
        state.registry.insert(*who, Position(out));
    }
    caught
}

/// Every tile a house covers — its drawn components, blocking or not.
///
/// The footprint's counterpart, and the difference matters: a footprint is what
/// *stops* somebody and a doorway is a gap in it, so "does this house cover this
/// tile" and "does this house block this tile" are two questions with two
/// answers.
#[must_use]
pub fn tiles_of(state: &WorldState, at: Point, multi: MultiId, design: Option<&[Component]>) -> Vec<Tile> {
    // A designed house has a shape whether or not the shard has a multi table;
    // one built from a classic multi has none without it. `shape_of` is what says
    // which, so the two cases need no branch here.
    drawn_tiles(shape_of(design, state, multi), at)
}

/// The house standing over `at`, if any.
///
/// A scan over the houses rather than an index: there are a handful on a shard,
/// and this is asked when somebody presses a button, never on a step. That is
/// also the reason the eager-obstruction argument does not apply — the answer is
/// wanted a few times a minute, not ten times a second per player.
#[must_use]
pub fn house_at(state: &WorldState, at: Point, facet: Facet) -> Option<EntityId> {
    state
        .registry
        .query::<House>()
        .filter(|(entity, _)| state.facet_of(*entity) == facet)
        .find(|(entity, house)| {
            state
                .registry
                .get::<Position>(*entity)
                .is_some_and(|&Position(origin)| {
                    tiles_of(state, origin, house.multi, None).contains(&Tile::new(at.x, at.y))
                })
        })
        .map(|(entity, _)| entity)
}

/// Put a house's walls into the obstruction index.
///
/// [`place`]'s last step, public because the boot path takes it on its own: a
/// saved house is not re-placed — that would ask whether it *may* stand there,
/// and a house legal when it was built stays built even if the rules have since
/// tightened — so restoring one is the registry half by hand and this.
pub fn block(state: &mut WorldState, entity: EntityId, facet: Facet, footprint: &[Footprint]) {
    block_footprint(state.facet_state_mut(facet), entity, footprint);
}

/// Take a house's walls back out of the obstruction index.
///
/// The entity itself is the caller's to despawn: this is the half that has to
/// happen *before* it goes, because the footprint is derived from where it stood.
pub fn unblock(state: &mut WorldState, entity: EntityId, facet: Facet, footprint: &[Footprint]) {
    let facet_state = state.facet_state_mut(facet);
    for spot in footprint {
        facet_state.unblock(spot.tile.x, spot.tile.y, entity);
    }
}

/// Where a house standing at `at` would block, and how tall at each tile.
///
/// Public because the boot path needs it to rebuild the index from a saved house
/// without going through [`place`]'s refusals — a house that was legal when it
/// was placed stays placed, even if the rules have since tightened.
pub fn footprint_of(
    state: &WorldState,
    at: Point,
    multi: MultiId,
    design: Option<&[Component]>,
) -> Result<Vec<Footprint>, Refusal> {
    let components = shape_of(design, state, multi);
    if components.is_empty() {
        // No multi table and no design: a shard with no client files knows about
        // no houses, which is the refusal it used to reach by having no terrain.
        return Err(Refusal::NoSuchMulti);
    }
    // A house's walls are tiledata's answer about each component's art, and that
    // table is the shard's rather than the facet's — the ground a house stands on
    // is asked about separately, in `ground_under`.
    let tiledata = state.tiles();
    let mut out = Vec::new();
    for component in components.iter().filter(|c| c.drawn()) {
        let graphic = component.graphic;
        // The one expansion. A footprint *refuses* what falls off the edge of
        // the world rather than skipping it, because a house with a wall
        // missing is a house somebody walks out of.
        let Some(spot) = component.placed_at(at) else {
            return Err(Refusal::OffTheMap);
        };
        // Whatever this component's art lays, which is the same rule a *loose*
        // static laid on the same tile would be read by — `Cover::of_static`,
        // called by both ends of the wire. A wall is a body, a floor is a
        // surface, a stair is one of each, and a roof tile is neither.
        //
        // This used to read `is_blocking` here and take the height itself, so a
        // house was its walls and nothing else: no floor to stand on above the
        // ground, and stairs that stopped a body instead of lifting one.
        let covers = Cover::of_static(tiledata.static_tile(graphic.0)).based_at(spot.z);
        let tile = Tile::new(spot.x, spot.y);
        out.extend(covers.into_iter().map(|cover| Footprint { tile, cover }));
    }
    Ok(out)
}

/// The land tile ranges a house may not stand on — ServUO's `RoadIDs`, inclusive
/// pairs.
///
/// Roads, cobbles, sand stones and ploughed furrows. A furrow is in the list for
/// the same reason a road is: it is somebody's field, and Britannia's farms are
/// as much scenery as its streets.
const ROAD_TILES: [(u16, u16); 8] = [
    (0x0071, 0x0078),
    (0x00E8, 0x00EB),
    (0x07AE, 0x07B1),
    (0x3FF4, 0x3FF4),
    (0x3FF8, 0x3FFB),
    (0x0442, 0x0479), // sand stones
    (0x0501, 0x0510), // sand stones
    (0x0009, 0x0015), // furrows
];

/// A second range of furrows, kept apart only because the array above is a
/// fixed-size literal and this is the ninth pair.
const MORE_FURROWS: (u16, u16) = (0x0150, 0x015C);

/// Whether a land tile is one a house may not stand on.
#[must_use]
pub fn is_road(land: u16) -> bool {
    ROAD_TILES
        .iter()
        .chain(std::iter::once(&MORE_FURROWS))
        .any(|&(low, high)| (low..=high).contains(&land))
}

/// How many tiles of yard a house keeps to itself, in every direction.
///
/// ServUO's `YardSize`, applied as a square rather than as its front-and-back
/// strip. The reference's rule is directional because a *foundation* has a front
/// and a back; a classic multi does not carry which way it faces, so a square is
/// the honest reading of "five tiles clear" for the shape this engine places.
/// Written down because it is a divergence, not an oversight.
pub const YARD: u16 = 5;

/// ServUO's rules two and four: nothing solid in the way, and something to stand
/// on.
///
/// `can_fit` asks both at once — it is "an open gap with a floor", so a solid
/// wall and thin air are the same refusal from its point of view — and it asks
/// them against the *map's* statics, which is the half `occupied_tile` cannot
/// see. And rule five, the road, which is a land-tile id rather than a shape.
/// ServUO's sixth rule, which this engine had the data for and never read: a
/// region may refuse houses outright.
///
/// # Every covered tile, at the house's own height
///
/// **The tiles, not the footprint.** A floor is inside a dungeon as surely as a
/// wall is, and `footprint_of` deliberately drops everything that does not
/// block. And not the origin either: `at` is the multi's origin, which
/// [`Multi::center`](openshard_uofiles::multi::Multi::center) says is *not* the
/// corner of its box — a multi whose components all sit at positive offsets has
/// an origin outside its own drawn area, so an origin test can test a tile no
/// wall stands on.
///
/// **The house's `z`, once, and never the component's.** A `RegionRect` carries
/// a height band and 247 of the shipped rects use one, which is what keeps the
/// sky above a dungeon open. A villa's roof stands twenty units above its
/// foundation, so testing each tile at its component's z would read the top half
/// of a house in Covetous as *not* in Covetous. A house is sited at one height.
/// See H6's D9a.
fn check_region(state: &WorldState, facet: Facet, at: Point, covered: &[Tile]) -> Result<(), Refusal> {
    for tile in covered {
        let point = Point::new(tile.x, tile.y, at.z);
        if state
            .region_at(facet, point)
            .is_some_and(|region| region.flags.no_housing)
        {
            return Err(Refusal::NoHousingHere);
        }
    }
    Ok(())
}

/// ServUO's rules two and four: the ground will take this house, and it is not a
/// street.
///
/// **The ground is asked about the components that stand on it, and no others.**
/// `can_fit` requires a *surface* at the z it is asked about, so asking it at each
/// component's own z refuses every house with an upper storey — a wall twenty
/// units up has nothing under it but the house's own floor, which is not on the
/// map yet and never will be. ServUO gates the same question the same way: its
/// `hasSurface` is only ever set for a component at `addTile.Z == 0`
/// (`HousePlacement.cs:174`), and the higher tiles are checked against the land
/// alone. This is [`check_region`]'s doctrine one field over — a house is sited
/// at one height, and everything above that height stands on the house.
///
/// The road question has no z in it, so it is asked of every tile the house
/// covers, which is what makes a roof overhanging a street a refusal.
///
/// What this deliberately does not do is ServUO's rule two for the *upper*
/// components — a roof driven into a hillside over a tile whose ground level is
/// empty. `can_fit` at the house's z already refuses the hill wherever the house
/// has a wall, and the remaining case needs a terrain question this seam does not
/// have; see `docs/map/terrain_seam.md`.
fn check_ground(state: &WorldState, facet: Facet, at: Point, footprint: &[Footprint]) -> Result<(), Refusal> {
    let Some(terrain) = state.map_terrain(facet) else {
        return Ok(()); // no map, no opinion — every other check here says the same
    };
    for spot in footprint {
        if terrain.land_tile(spot.tile).is_some_and(|land| is_road(land.0)) {
            return Err(Refusal::OnARoad);
        }
        if spot.cover.z == at.z
            && !terrain.can_fit(
                spot.tile,
                i32::from(spot.cover.z),
                i32::from(spot.cover.height).max(1),
            )
        {
            return Err(Refusal::BadGround);
        }
    }
    Ok(())
}

/// ServUO's rule three: a house keeps [`YARD`] tiles to itself.
///
/// Asked against the other houses' own footprints rather than against a stored
/// yard rectangle, because a footprint is what a house *is* and a rectangle would
/// be a second copy of it to keep in step. There are a handful of houses within
/// a few tiles of anywhere, so the scan is over the ones near enough to matter.
fn check_yard(state: &WorldState, facet: Facet, footprint: &[Footprint]) -> Result<(), Refusal> {
    let mine: Vec<Tile> = footprint.iter().map(|spot| spot.tile).collect();
    for (entity, house) in state.registry.query::<House>() {
        if state.registry.get::<Facet>(entity) != Some(&facet) {
            continue;
        }
        let Some(&Position(at)) = state.registry.get::<Position>(entity) else {
            continue;
        };
        let Ok(theirs) = footprint_of(state, at, house.multi, None) else {
            continue;
        };
        for other in &theirs {
            if mine.iter().any(|tile| within_yard(*tile, other.tile)) {
                return Err(Refusal::TooCloseToAHouse);
            }
        }
    }
    Ok(())
}

/// Whether two tiles are inside one yard of each other.
fn within_yard(one: Tile, other: Tile) -> bool {
    one.x.abs_diff(other.x) <= YARD && one.y.abs_diff(other.y) <= YARD
}

/// Register every wall of a footprint against one entity.
///
/// The facet and not its obstruction index, because the index is only half of
/// what a wall has to reach: the overlay every step reads is the other half, and
/// only the facet holds both. See `FacetState::block`.
fn block_footprint(facet_state: &mut FacetState, entity: EntityId, footprint: &[Footprint]) {
    for spot in footprint {
        facet_state.block(spot.tile.x, spot.tile.y, entity, spot.cover);
    }
}

/// The first tile of `footprint` something already stands on, if any.
///
/// The narrow half of ServUO's five rules: this is "no impassable object may come
/// in direct contact with any part of the house", asked of the *dynamic* index
/// only. The map's own statics, the yard clearance, the flat foundation and the
/// road are the rest of D3 and are not here yet — see `docs/housing.md`, which
/// says so rather than letting a reader assume the check is complete.
fn occupied_tile(state: &WorldState, facet: Facet, footprint: &[Footprint]) -> Option<Tile> {
    let obstructions = &state.facet_state(facet).obstructions();
    footprint
        .iter()
        .find(|spot| {
            obstructions
                .blocker_at_z(spot.tile.x, spot.tile.y, i32::from(spot.cover.z))
                .is_some()
        })
        .map(|spot| spot.tile)
}
