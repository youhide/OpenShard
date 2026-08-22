//! Changing the shape of a house that is already standing.
//!
//! A classic house *is* its multi id: the walls, the sign's tile, the doorstep
//! and the lockdown allowance are all read out of `multi.mul` every time they
//! are wanted, and none of them is stored. A **designed** house breaks that —
//! its shape is on the entity, and every one of those derivations has to ask the
//! entity first and the table second.
//!
//! [`shape_of_house`] is that question, asked once and answered for every
//! reader. `docs/customisation.md`'s C2 calls it the chooser; this is where the
//! callers that hold a house rather than a multi id reach it.
//!
//! # The commit tail
//!
//! [`redesign`] is C7's: swapping the components is one line and the six lines
//! around it are the work. The walls come out of the obstruction index *as the
//! old shape* and go back in as the new one; a door that is now inside is
//! adopted; the sign moves, because it hangs off the box's corner and the box
//! just changed; the lockdown allowance is recounted, because it is derived from
//! the area; and the revision goes up, because that is what tells a client its
//! cached picture is stale.
//!
//! Every one of those is a thing that silently keeps working while being wrong
//! if it is forgotten — the walls of a house you can walk through, a sign
//! floating where a wall used to be — which is why they are one function and not
//! a checklist.

use openshard_entities::EntityId;
use openshard_protocol::world::{Facet, Point};
use openshard_state::WorldState;
use openshard_state::components::{House, HouseDesign, HouseSign, Position, Standing};
use openshard_uofiles::multi::Component;

/// The floor a foundation is finished in, as four graphics.
///
/// ServUO's `GetFoundationGraphics`, and it is a **material** table rather than
/// a per-house-type one — eight rows keyed by what the owner chose it to look
/// like, not thirty keyed by which house it is. That is the distinction that
/// kept the door positions and the sign offsets out of this engine, and this
/// falls on the other side of it.
///
/// Only the reference's own `default` arm is here. Which material a player picks
/// is the editor's question and the editor is C3; a foundation placed today is
/// dark wood, and the constant says so rather than a table with seven rows
/// nothing can reach.
mod floor {
    /// The north-west post.
    pub const POST: u16 = 0x0017;
    /// The north and south edges.
    pub const SOUTH: u16 = 0x0016;
    /// The east and west edges.
    pub const EAST: u16 = 0x0015;
    /// The south-east corner.
    pub const CORNER: u16 = 0x0014;
    /// One step of the stair strip along the south edge — ServUO's
    /// `GetEmptyFoundation`, which lays this and not the `0x63` its *placement
    /// preview* uses.
    pub const STAIR: u16 = 0x0751;
}

/// The design a foundation is placed with: its own platform, a floor, and a
/// strip of stairs along the south edge.
///
/// # Why this is here rather than in C3
///
/// `Refusal::NeedsCustomisation` exists for one reason — a foundation's
/// component list has no stairs, so one placed as-is is a house nobody can get
/// into. `customisation.md`'s C2 calls the fix "C3's initial design at
/// placement", and C3 is the editor. The editor is not what makes a foundation
/// enterable; this is.
///
/// # It is a derivation, not a table
///
/// ServUO's `GetEmptyFoundation` copies the foundation's own components, grows
/// the box **one row south**, lays the four floor graphics around the perimeter
/// and a stair along the new row. Every position falls out of the box, so there
/// is no per-house-type table to port and nothing to invent — which is the
/// answer this phase went looking for.
///
/// `None` when the shard has no client files or the id is not a multi it knows:
/// a foundation whose own platform cannot be read is one there is nothing to
/// build a design out of.
#[must_use]
pub fn initial_foundation(state: &WorldState, multi: u16) -> Option<Vec<Component>> {
    let multi = multi & !crate::MULTI_FLAG;
    let platform = state.multi_components(multi);
    if platform.is_empty() {
        return None;
    }
    let box_ = openshard_uofiles::multi::bounds(platform)?;
    let (min_x, min_y) = (box_.min_x, box_.min_y);
    let (max_x, max_y) = (box_.max_x, box_.max_y);
    let width = i32::from(max_x) - i32::from(min_x) + 1;
    // The row the stairs go on: one south of the platform's own last row, which
    // is what `Resize(Width, Height + 1)` buys.
    let stair_y = i16::try_from(i32::from(max_y) + 1).ok()?;

    let mut out: Vec<Component> = platform.to_vec();
    let mut put = |graphic: u16, dx: i16, dy: i16| {
        out.push(Component {
            graphic,
            dx,
            dy,
            dz: 0,
            flags: 1,
        });
    };

    put(floor::POST, min_x, min_y);
    put(floor::CORNER, max_x, max_y);
    for x in 1..width {
        let Ok(dx) = i16::try_from(i32::from(min_x) + x) else {
            continue;
        };
        put(floor::SOUTH, dx, min_y);
        if x < width - 1 {
            put(floor::SOUTH, dx, max_y);
        }
        put(floor::STAIR, dx, stair_y);
    }
    // The east and west edges, between the two rows the loop above laid.
    let mut y = i32::from(min_y) + 1;
    while y <= i32::from(max_y) {
        let Ok(dy) = i16::try_from(y) else { break };
        put(floor::EAST, min_x, dy);
        if y < i32::from(max_y) {
            put(floor::EAST, max_x, dy);
        }
        y += 1;
    }
    Some(out)
}

/// Why a change of design was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DesignRefusal {
    /// That entity is not a house.
    NotAHouse,
    /// The actor does not own it. Not co-owner: a co-owner may lock things down
    /// and let people in, and neither of those changes what the building *is*.
    NotYours,
    /// The design has no drawn components, so the house would have no walls.
    /// Refused before anything is taken down, so a house is never left as a
    /// hole in the ground by a bad design.
    DrawsNothing,
}

impl DesignRefusal {
    /// What to say to whoever tried.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::NotAHouse => "That is not a house.",
            Self::NotYours => "That is not your house to change.",
            Self::DrawsNothing => "A house has to have walls.",
        }
    }
}

/// The components a house standing on the ground is actually made of, or `None`
/// when it is an ordinary multi and the table has the answer.
///
/// The shape every derivation should be asking about. Cloned rather than
/// borrowed because every caller goes on to want `&mut WorldState` — a design is
/// a few hundred `Component`s, wanted when somebody presses a button rather than
/// on a step, and threading a lifetime through `place`'s whole tail to save the
/// copy would be paying in the wrong currency.
#[must_use]
pub fn shape_of_house(state: &WorldState, house: EntityId) -> Option<Vec<Component>> {
    state
        .registry
        .get::<HouseDesign>(house)
        .map(|design| design.components.clone())
}

/// What revision a house's design is at. Zero for a house that has never been
/// designed, which is also what the first [`redesign`] increments from.
#[must_use]
pub fn revision(state: &WorldState, house: EntityId) -> u32 {
    state
        .registry
        .get::<HouseDesign>(house)
        .map_or(0, |design| design.revision)
}

/// Give a standing house a new shape, and put everything derived from the old
/// one back in step.
///
/// Returns the new revision. See the module header for what the tail is and why
/// it is not a checklist.
///
/// **Nothing is taken down until the new design is known to be legal.** The new
/// footprint is computed first, so a design that draws nothing is refused with
/// the house exactly as it was rather than with its walls already gone.
pub fn redesign(
    state: &mut WorldState,
    actor: EntityId,
    house: EntityId,
    components: Vec<Component>,
) -> Result<u32, DesignRefusal> {
    let Some(entry) = state.registry.get::<House>(house) else {
        return Err(DesignRefusal::NotAHouse);
    };
    let multi = entry.multi;
    let Some(who) = state.registry.serial_of(actor) else {
        return Err(DesignRefusal::NotYours);
    };
    if entry.standing_of(who, state.is_staff(actor)) < Standing::Owner {
        return Err(DesignRefusal::NotYours);
    }
    let Some(&Position(at)) = state.registry.get::<Position>(house) else {
        return Err(DesignRefusal::NotAHouse);
    };
    let facet = state.facet_of(house);

    // First, and before anything comes down.
    let footprint =
        crate::footprint_of(state, at, multi, Some(&components)).map_err(|_| DesignRefusal::DrawsNothing)?;
    if footprint.is_empty() {
        return Err(DesignRefusal::DrawsNothing);
    }

    // The old walls come out *as the old shape*. Deriving them from the new
    // design would leave every tile the two do not share blocked forever, by an
    // entity that no longer stands there — the sort of leak nothing reports and
    // a player finds by walking into thin air.
    let old = shape_of_house(state, house);
    let old_footprint = crate::footprint_of(state, at, multi, old.as_deref()).unwrap_or_default();
    crate::unblock(state, house, facet, &old_footprint);

    let revision = revision(state, house).wrapping_add(1);
    state.registry.insert(house, HouseDesign { components, revision });
    crate::block(state, house, facet, &footprint);

    // A doorway is a gap in the walls, so a redesign can open one where there
    // was none — and the door already standing there is the house's now.
    crate::adopt_doors(state, house, facet, at, multi);
    rehang_sign(state, house, facet, at, multi);

    // The allowance is area, and the area changed. Recounted here for the same
    // reason `place` computes it once: this is the moment the shape is in hand.
    let covered = crate::tiles_of(state, at, multi, shape_of_house(state, house).as_deref());
    let lockdowns = u32::try_from(crate::storage::allowance_for(covered.len()).lockdowns).unwrap_or(u32::MAX);
    if let Some(entry) = state.registry.get_mut::<House>(house) {
        entry.lockdowns = lockdowns;
    }
    // And tell everyone looking at it that the picture they have is stale. The
    // draw sends this too, but a client already standing there will never be
    // shown the house a second time, so the draw's copy cannot reach them.
    state.broadcast_design_revision(house);
    Ok(revision)
}

/// Take the old sign down and hang a new one.
///
/// The sign sits on the west-south corner of the house's *box*, so a design that
/// changes the box moves it. Moving the existing entity would do as well and
/// costs a serial less; it is spawned fresh because
/// [`hang_sign`](crate::hang_sign) is the one place that knows where a sign goes
/// and what it is made of, and two places that know would drift.
fn rehang_sign(state: &mut WorldState, house: EntityId, facet: Facet, at: Point, multi: u16) {
    let Some(serial) = state.registry.serial_of(house) else {
        return;
    };
    let old: Vec<EntityId> = state
        .registry
        .query::<HouseSign>()
        .filter(|(_, sign)| sign.house == serial)
        .map(|(entity, _)| entity)
        .collect();
    for sign in old {
        crate::decay::take_off_the_ground(state, sign);
    }
    crate::hang_sign(state, house, facet, at, multi);
}
