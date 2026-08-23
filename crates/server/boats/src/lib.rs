//! Ships: putting one on the water, and what it does to the tiles it covers.
//!
//! A boat is a multi, exactly as a house is — one entity whose graphic is
//! `0x4000 | id`, drawn by every client out of its own `multi.mul`, with no
//! component ever on the wire. Everything in `openshard-housing`'s placement
//! path applies, and this crate is deliberately that path's shape rather than a
//! generalisation of it — and it does **not** depend on that crate. The two are
//! siblings: a shared "multi placement" abstraction would have to be designed
//! before either caller needed it, and the shape is cheaper to repeat than the
//! abstraction is to get wrong.
//!
//! # What is different from a house, and it is only two things
//!
//! **It goes on water, and a house may not.** [`place`] refuses a berth whose
//! tiles are not sea, through `Terrain::land_is_water` — the client's own tile
//! flags, rather than a third notion of "water" beside them and fishing's id
//! ranges. What a ship is *made of* is not asked of the ground at all: it is the
//! shard's multi table, beside its tiledata.
//!
//! **Its tiles go in [`Boats`] and not in `Obstructions`.** That index only
//! subtracts, and a deck is somewhere to stand over water that is otherwise not
//! ground at all — `docs/boats.md`'s B3, argued in `openshard_state::boat`.
//!
//! # And it sails
//!
//! [`step`] moves the hull a tile, takes whoever is standing on the deck with
//! it, and redraws the ship on every screen that can see it. [`set_course`] puts
//! one under way and [`sail`] is the pass the tick runs, which steps every ship
//! whose cadence is up and furls the ones whose way is blocked.
//!
//! What is still B2's is the **tiller** — the item a player speaks to. `sail` is
//! driven from `.sail` today, which is a game master's toy rather than a ship's
//! crew.

use openshard_entities::EntityId;
use openshard_map::grid::Tile;
use openshard_protocol::serial::Serial;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::{Facet, Point};
use openshard_state::boat::Plank;
use openshard_state::components::{Boat, Drawn, Movement, Position, Sailing};
use openshard_state::{Occupant, WorldState};

/// The bit a multi's graphic carries on the wire.
///
/// The protocol's own, not a copy of housing's: a boat is a multi for the same
/// reason a house is, and both are reading the same wire fact. This crate does
/// **not** depend on `openshard-housing` — they are siblings, and the one thing
/// they share is a constant that belongs to neither.
pub const MULTI_FLAG: u16 = openshard_protocol::wire::MultiId::FLAG;

/// Why a boat could not be launched.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    /// No multi by that id, or one that draws nothing. A fact about the id.
    NoSuchMulti,
    /// A component would land off the edge of the world. Arithmetic, not
    /// judgement — there is no tile there to float on.
    OffTheMap,
    /// Some of the berth is not sea.
    NotOnWater,
    /// Another ship is already there.
    Occupied,
    /// The registry could not mint a serial.
    NoSerials,
}

impl Refusal {
    /// What to say to whoever tried.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::NoSuchMulti => "No ship by that name.",
            Self::OffTheMap => "That is off the edge of the world.",
            Self::NotOnWater => "A ship has to float.",
            Self::Occupied => "There is already a ship there.",
            Self::NoSerials => "The world is full.",
        }
    }
}

/// One tile of a berth: where it is, and what the ship puts there.
///
/// The pair [`Boats::moor`](openshard_state::Boats::moor) takes, named so that
/// deriving a berth and mooring one are visibly the same thing.
type Berth = ((u16, u16), Plank);

/// The tiles a boat standing at `at` would cover, and what each one is.
///
/// Public because the boot path needs it without going through [`place`]'s
/// refusals — a ship that was afloat when it was launched stays afloat, the way
/// a house that was legal when it was built stays built.
///
/// A component that blocks by its tiledata is hull; everything else is deck. The
/// split is the whole of what [`Boats`](openshard_state::Boats) needs, and it is
/// made once here rather than per step.
///
/// Undrawn components are skipped, the way a house's footprint skips them: the
/// signature tile every multi opens with is not part of the ship.
pub fn planks_of(state: &WorldState, boat: EntityId, at: Point, multi: u16) -> Result<Vec<Berth>, Refusal> {
    let multi = multi & !MULTI_FLAG;
    // The shard's tables, not the facet's ground: what a ship is made of and how
    // tall each piece of it is are facts about the install. Where it may float is
    // the facet's business, and that is `check_berth`'s question.
    let components = state.multis.components(multi);
    if components.is_empty() {
        return Err(Refusal::NoSuchMulti);
    }
    let tiledata = state.tiles();
    let mut out = Vec::new();
    for component in components.iter().filter(|c| c.drawn()) {
        let graphic = Graphic(component.graphic);
        let (Ok(x), Ok(y)) = (
            u16::try_from(i32::from(at.x) + i32::from(component.dx)),
            u16::try_from(i32::from(at.y) + i32::from(component.dy)),
        ) else {
            return Err(Refusal::OffTheMap);
        };
        let Ok(z) = i8::try_from(i32::from(at.z) + i32::from(component.dz)) else {
            return Err(Refusal::OffTheMap);
        };
        out.push((
            (x, y),
            Plank {
                boat,
                z,
                height: tiledata.static_tile(graphic.0).height,
                blocks: tiledata.static_tile(graphic.0).flags.is_blocking(),
            },
        ));
    }
    if out.is_empty() {
        return Err(Refusal::NoSuchMulti);
    }
    Ok(out)
}

/// Put a ship on the water.
///
/// `housing::place`'s shape, including the staff exemption: `actor`
/// is taken so the *judgements* about the berth can be skipped for a game
/// master, while the facts about the id — no such multi, off the edge of the
/// world — hold for everybody. Housing's D10 table is the specification and the
/// split is the same one, with the same reasoning.
pub fn place(
    state: &mut WorldState,
    actor: EntityId,
    at: Point,
    facet: Facet,
    multi: u16,
    owner: Serial,
) -> Result<EntityId, Refusal> {
    let staff = state.is_staff(actor);
    let multi = multi & !MULTI_FLAG;

    let Ok((entity, _)) = state
        .registry
        .spawn_with_serial(openshard_protocol::serial::SerialKind::Item)
    else {
        return Err(Refusal::NoSerials);
    };
    // The shape is derived before anything is written, so a refusal below leaves
    // nothing behind but the serial — which is the one thing that cannot be
    // taken back, and is why this is the first fallible step after it.
    let berth = match planks_of(state, entity, at, multi) {
        Ok(berth) => berth,
        Err(refusal) => {
            state.registry.despawn(entity);
            return Err(refusal);
        }
    };

    if !staff {
        if let Err(refusal) = check_berth(state, facet, &berth) {
            state.registry.despawn(entity);
            return Err(refusal);
        }
    }

    state.registry.insert(
        entity,
        Drawn {
            id: Graphic(MULTI_FLAG | multi),
            hue: Hue(0),
        },
    );
    state.registry.insert(entity, Position(at));
    state.registry.insert(entity, Boat { multi, owner });
    state.registry.insert(entity, facet);
    // On the sector grid like any item, so a client sailing into view is told
    // about it by the ordinary interest sweep rather than by a path of its own.
    state
        .facet_state_mut(facet)
        .sectors
        .insert(entity, at, Occupant::Item);
    state.facet_state_mut(facet).moor(entity, berth);
    Ok(entity)
}

/// The two judgements about a berth: it is all sea, and nothing else is in it.
///
/// Both are staff-exempt, for housing's D10 reason — they are statements about
/// the *place* and a game master is allowed to put a ship in a fountain.
fn check_berth(state: &WorldState, facet: Facet, berth: &[((u16, u16), Plank)]) -> Result<(), Refusal> {
    let facet_state = state.facet_state(facet);
    let Some(terrain) = state.map_terrain(facet) else {
        return Err(Refusal::NoSuchMulti);
    };
    for &((x, y), _) in berth {
        if !terrain.land_is_water(Tile::new(x, y)) {
            return Err(Refusal::NotOnWater);
        }
        if facet_state.boats().boat_at(x, y).is_some() {
            return Err(Refusal::Occupied);
        }
    }
    Ok(())
}

/// Sail one tile, carrying whoever is aboard.
///
/// `World::step`'s structure — decide, then apply — with the terrain check
/// replaced by *does the whole translated berth fit*. Nothing is written until
/// every question has been asked, so a refused course leaves the ship exactly
/// where it was with everybody still standing on it.
///
/// # The manifest is derived, never stored
///
/// Who moves with the ship is worked out here, per move, from the tiles it
/// covers and the sector grid — `docs/boats.md`'s B1a. An `OnDeck` component
/// would be a second copy of a fact [`Position`] already holds, and the two
/// would disagree the first time anything moved a body without going through
/// this function.
///
/// # And each of them is moved absolutely
///
/// B1's refusal of a parent transform, on the engine's own evidence: this world
/// has no way to carry one entity by moving another — mounting *deletes* the
/// mount rather than carrying it. So the deck's occupants are relocated one by
/// one through [`WorldState::move_to`], which is the **fourth** caller of the
/// `disrupt` → position → `refresh_around` → `broadcast_move` sequence after
/// `npc::live`, `quests::advance_escorts` and the tick's own `step`, and the
/// point `docs/boats.md:384` names as when that tail wants a name of its own.
///
/// # And the hull is redrawn by forget-then-reveal
///
/// See [`redraw`]. **The number of packets a move costs**, which `docs/boats.md`
/// asks for by name: two per client that can see the ship — a `0x1D` and the
/// `0x1A`/`0xF3` that draws it again — plus, per occupant, one `0x20` to its own
/// client and one `0x77` to each client watching it. A sloop under way with one
/// player aboard and one watching from the shore is six packets a tile, and the
/// hull's two of those are what `Feature::SmoothShip`'s single `0xF6` replaces
/// in B3.
pub fn step(
    state: &mut WorldState,
    boat: EntityId,
    direction: openshard_protocol::direction::Direction,
) -> Result<Point, Refusal> {
    let facet = state.facet_of(boat);
    let (Some(&Position(at)), Some(&Boat { multi, .. })) = (
        state.registry.get::<Position>(boat),
        state.registry.get::<Boat>(boat),
    ) else {
        return Err(Refusal::NoSuchMulti);
    };
    let Some(to) = openshard_movement::step_from(at, direction) else {
        return Err(Refusal::OffTheMap);
    };

    // Decide. The new berth is derived before anything is written and checked
    // against the water and against every *other* ship — a hull is not in
    // `Obstructions`, so nothing else in this engine would notice two of them in
    // one tile.
    let berth = planks_of(state, boat, to, multi)?;
    check_course(state, facet, boat, &berth)?;
    let manifest = aboard(state, boat, facet);

    // Apply. The index first, so a body relocated below lands on a deck that is
    // already where the ship is going rather than on the one it is leaving.
    state.facet_state_mut(facet).moor(boat, berth);
    state.registry.insert(boat, Position(to));
    state
        .facet_state_mut(facet)
        .sectors
        .insert(boat, to, Occupant::Item);

    let (dx, dy) = (
        i32::from(to.x) - i32::from(at.x),
        i32::from(to.y) - i32::from(at.y),
    );
    for (occupant, was) in manifest {
        let (Ok(x), Ok(y)) = (
            u16::try_from(i32::from(was.x) + dx),
            u16::try_from(i32::from(was.y) + dy),
        ) else {
            // The berth fits, so this cannot happen for anyone standing inside
            // it — but a body at the edge of the coordinate space is left where
            // it is rather than wrapped to the far side of the world.
            continue;
        };
        state.disrupt(occupant);
        state.move_to(occupant, facet, Point { x, y, z: was.z });
    }
    // Last, so the reveal below finds every screen already refreshed by the
    // occupants' own moves and only has the hull left to say.
    redraw(state, boat);

    Ok(to)
}

/// Take the hull off every screen it is on and put it back where it now is.
///
/// **A multi cannot be told it moved.** `0x77` moves a *mobile*; an item's
/// position arrives with the item, in the `0x1A`/`0xF3` that draws it, and there
/// is no packet in the classic protocol that relocates one that is already
/// drawn. So the ship is removed and drawn again — `docs/boats.md`'s B2, and the
/// reason `Feature::SmoothShip`'s `0xF6` exists at all for the clients that have
/// it.
///
/// The order matters and is not symmetric with the forget: `forget` walks the
/// clients that *have* the ship, and `refresh_around` then draws it for
/// everybody in range of where it now is — which is a different set the moment
/// the ship sails into somebody's view. Doing only the second would leave the
/// old hull drawn on the screens that already had it, because `show` returns
/// early for anything already seen.
fn redraw(state: &mut WorldState, boat: EntityId) {
    let Some(serial) = state.registry.serial_of(boat) else {
        return;
    };
    for watcher in state.watchers_of(boat) {
        state.forget(watcher, boat, serial);
    }
    state.refresh_around(boat);
}

/// The two questions a course has to answer: it is all sea, and no *other* ship
/// is in it.
///
/// Not [`check_berth`]: that one refuses any boat at all in the tile, which for
/// a move is the ship itself in the berth it is leaving. The difference is one
/// comparison and it is the whole of why a ship can sail forward.
fn check_course(state: &WorldState, facet: Facet, boat: EntityId, berth: &[Berth]) -> Result<(), Refusal> {
    let facet_state = state.facet_state(facet);
    let Some(terrain) = state.map_terrain(facet) else {
        return Err(Refusal::NoSuchMulti);
    };
    for &((x, y), _) in berth {
        if !terrain.land_is_water(Tile::new(x, y)) {
            return Err(Refusal::NotOnWater);
        }
        if facet_state
            .boats()
            .at(x, y)
            .iter()
            .any(|plank| plank.boat != boat)
        {
            return Err(Refusal::Occupied);
        }
    }
    Ok(())
}

/// Everyone standing on the deck right now, and the tile each is standing on.
///
/// A mobile is aboard when it is on a tile the ship covers **and its feet are on
/// a plank** — the second half matters, because a swimmer beside the hull and a
/// body on a pier the ship is moored against are both on a covered tile and
/// neither is a passenger.
///
/// Mobiles only. A crate lying on the deck is cargo and stays where it is until
/// B4 gives the ship a hold; carrying it would need the item move path rather
/// than [`WorldState::move_to`], which is a mobile's.
fn aboard(state: &WorldState, boat: EntityId, facet: Facet) -> Vec<(EntityId, Point)> {
    let facet_state = state.facet_state(facet);
    let covered = facet_state.boats().covered_by(boat);
    let Some(&first) = covered.first() else {
        return Vec::new();
    };
    // One sector query for the whole ship rather than one per tile: the berth is
    // a handful of tiles and a lookup is a block sweep, so asking it four times
    // for a sloop would walk the same neighbourhood four times.
    let centre = Point::new(first.0, first.1, 0);
    let reach = covered
        .iter()
        .map(|&(x, y)| u32::from(x.abs_diff(first.0)).max(u32::from(y.abs_diff(first.1))))
        .max()
        .unwrap_or(0);

    facet_state
        .sectors
        .mobiles_near(centre, reach)
        .filter(|&(entity, _)| entity != boat)
        .filter(|(entity, _)| state.registry.has::<Movement>(*entity))
        .filter(|&(_, at)| facet_state.boats().deck_at(at.x, at.y, i32::from(at.z)) == Some(i32::from(at.z)))
        .collect()
}

/// How many ticks between one step and the next, at each of the two speeds a
/// ship has.
///
/// ServUO's `BaseBoat`: `SlowInterval` is 1000ms and `FastInterval` 250ms, and
/// under `NewBoatMovement` both move one tile — the older two-speed arrangement
/// moved three tiles per fast interval, which is a different thing to port and
/// not the one modern clients see. At this engine's twenty ticks a second that
/// is twenty ticks and five.
pub const SLOW_TICKS: u64 = openshard_state::TICKS_PER_SECOND;
/// See [`SLOW_TICKS`].
pub const FAST_TICKS: u64 = openshard_state::TICKS_PER_SECOND / 4;

/// Put a ship under way, or turn one that already is.
///
/// The first step is taken on the **next** tick rather than now, so ordering a
/// course and holding one cost the same and a player cannot outrun the cadence
/// by repeating the order.
pub fn set_course(
    state: &mut WorldState,
    boat: EntityId,
    direction: openshard_protocol::direction::Direction,
    fast: bool,
) {
    if state.registry.get::<Boat>(boat).is_none() {
        return;
    }
    let every = if fast { FAST_TICKS } else { SLOW_TICKS };
    state.registry.insert(
        boat,
        Sailing {
            direction,
            next: state.ticks + every,
            every,
        },
    );
}

/// Bring a ship to a stop. A no-op on one already moored, which is what makes
/// "stop" safe to say twice.
pub fn furl(state: &mut WorldState, boat: EntityId) {
    state.registry.remove::<Sailing>(boat);
}

/// Step every ship whose cadence is up. The tick's boat pass.
///
/// Returns the ships that **stopped** because their way was blocked, so the
/// caller can tell whoever is aboard — this crate has no opinion about messages
/// and the tick does, which is where the same split already puts a house's
/// collapse.
///
/// A ship refused its course furls rather than retrying: a hull grinding against
/// a rock twenty times a second is the shape of a stuck NPC, and a ship that has
/// stopped is a thing a player can see and correct.
pub fn sail(state: &mut WorldState) -> Vec<EntityId> {
    // Collected first: the step below writes positions and the index, and a
    // query held across that borrows the registry it is about to change.
    let due: Vec<(EntityId, Sailing)> = state
        .registry
        .query::<Sailing>()
        .filter(|(_, course)| course.next <= state.ticks)
        .map(|(entity, course)| (entity, *course))
        .collect();

    let mut stopped = Vec::new();
    for (boat, course) in due {
        if step(state, boat, course.direction).is_err() {
            furl(state, boat);
            stopped.push(boat);
            continue;
        }
        state.registry.insert(
            boat,
            Sailing {
                next: state.ticks + course.every,
                ..course
            },
        );
    }
    stopped
}

/// Take a ship off the water: out of the boat index, off the sector grid, and
/// out of the registry.
///
/// Separate from any decay path, which is B4's: this is what `.boat` needs to be
/// undoable and what a scuttled ship will need when there is one.
pub fn sink(state: &mut WorldState, boat: EntityId) {
    let facet = state.facet_of(boat);
    state.facet_state_mut(facet).cast_off(boat);
    state.facet_state_mut(facet).sectors.remove(boat);
    state.registry.despawn(boat);
}

/// The ship covering `at`, if one does.
///
/// A lookup in the tile index rather than a scan over the boats, because unlike
/// `housing::house_at` this one is asked by the step path.
#[must_use]
pub fn boat_at(state: &WorldState, at: Point, facet: Facet) -> Option<EntityId> {
    state.facet_state(facet).boats().boat_at(at.x, at.y)
}

#[cfg(test)]
mod tests;
