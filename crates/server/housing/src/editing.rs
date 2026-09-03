//! The three verbs that change a design while it is open: lay a piece, take one
//! away, and say which storey the other two apply to.
//!
//! [`session`](crate::session) made "this house is open in somebody's editor" a
//! state the shard can be asked about. This is what that state is *for*, and it
//! is deliberately the smallest thing it can be:
//! [`plans/housing/customisation/PLAN.md`](../../../../plans/housing/customisation/PLAN.md)'s
//! step 2, which is the working copy and nothing else.
//!
//! # Nothing here is visible from outside the editor
//!
//! `docs/housing/design_customisation.md`'s C7: while a session is open the
//! world still shows and blocks the **committed** design. So every function here
//! writes to [`DesignSession::working`] and to nothing else — no revision bump,
//! no obstruction churn, no packet to anybody. What a stranger walking past sees
//! is the house as it was when the editor opened, until the commit that is step
//! 3 swaps the two in one move.
//!
//! That is also why a refused edit says nothing. The reference answers one by
//! *resending the design*, which is the synch verb and is step 5; until that
//! exists, the honest answer to a refused edit is to change nothing, and the
//! [`EditRefusal`] goes to the log.
//!
//! # The grid is the foundation's, not the working copy's
//!
//! Every offset is checked against [`buildable_box`], which is derived from the
//! *foundation's own multi* and not from whatever the working copy currently
//! holds. The reference gets this for free — its `MultiComponentList` allocates
//! a fixed grid when the design is created and `Add` silently drops anything
//! outside it — and recomputing the box from the components each time would not:
//! erasing a corner piece would shrink the house's buildable area, and there
//! would be no way to put the corner back.

use openshard_entities::EntityId;
use openshard_protocol::encoded::{
    DesignEdit,
    RawStorey,
};
use openshard_protocol::wire::Graphic;
use openshard_state::WorldState;
use openshard_state::components::{
    DesignSession,
    House,
};
use openshard_uofiles::multi::{
    Bounds,
    Component,
    bounds,
};

/// The height the first storey's pieces are laid at — ServUO's `GetLevelZ` for
/// level one, which is `(1 - 1) * 20 + 7`.
///
/// Seven and not zero: zero is the foundation's own platform, which is a storey
/// nobody builds on. It is also the height the reference looks for when it asks
/// whether a tile still has a floor under it, and `erase` asks the same
/// question.
pub const FIRST_STOREY_Z: i16 = 7;

/// How far apart two storeys are — ServUO's `GetLevelZ` again, the `* 20`.
pub const STOREY_HEIGHT: i16 = 20;

/// The dirt a hole in the first storey's floor is filled with.
///
/// ServUO's `Designer_Delete` lays this when the tile a piece came off has
/// nothing left at [`FIRST_STOREY_Z`]. A design with a hole in it is one the
/// client draws the ground through, which reads as a bug rather than as a
/// choice; the reference fills it and so does this.
pub const DIRT: u16 = 0x31F4;

/// The `.mul`'s "the client draws this" flag value. See
/// [`Component::drawn`](openshard_uofiles::multi::Component::drawn) for why zero
/// is the *skip* value and this is not.
const DRAWN: u64 = 1;

/// The entry-zero signature every multi starts with — item id `1` at the origin,
/// which the reference skips when it asks whether a tile has a floor because it
/// is not really a tile.
const SIGNATURE: Graphic = Graphic(1);

/// Why an edit to a working design was refused.
///
/// Nothing sends these to a player. They are the log's word for what a client
/// asked that this shard would not do, and the tests' word for the same — see
/// the module header on why a refusal is silent until the synch verb exists.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditRefusal {
    /// This player has no house open. The ordinary answer to an edit that
    /// arrives after a logout race or a window the shard already closed, and
    /// the reason it is not an error worth telling anybody about.
    NoSession,
    /// The house's own foundation could not be read, so there is no grid to
    /// check an offset against. A shard with no client files, or an id whose
    /// platform this install does not hold — damaged state rather than a bad
    /// edit.
    FoundationUnreadable,
    /// The offset is off the foundation's grid. The reference drops these
    /// silently inside `MultiComponentList.Add`; naming it is what lets a test
    /// tell "refused" from "laid somewhere else".
    OutsideTheHouse,
    /// The foundation's own floor is not the player's to remove: a hole in it is
    /// a house standing on nothing. ServUO's "component is not deletable", which
    /// covers every tile at height zero inside the box except the stair row
    /// along the south edge.
    FixedFloor,
    /// Nothing of that graphic stands at that offset and height.
    NothingThere,
    /// A roof tile, which the build verb does not lay: roofs are their own two
    /// subcommands and their own plane, and they are step 5. ServUO's
    /// `ValidPiece(itemID, roof: false)` refuses the same piece for the same
    /// reason.
    RoofPiece,
}

/// The storey `storey` is built at — ServUO's `GetLevelZ`.
///
/// Storeys are counted from one ([`GROUND_FLOOR`](crate::session::GROUND_FLOOR)),
/// so the first is at [`FIRST_STOREY_Z`] and each one above is
/// [`STOREY_HEIGHT`] higher.
#[must_use]
pub fn storey_z(storey: u8) -> i16 {
    (i16::from(storey) - 1) * STOREY_HEIGHT + FIRST_STOREY_Z
}

/// How many storeys a house on this foundation may have — ServUO's `MaxLevels`.
///
/// Four for a foundation fourteen tiles or more on either side, three for
/// anything smaller. A rule about the *building*, which is why it is derived
/// from the grid rather than stored.
#[must_use]
pub fn max_storeys(grid: Bounds) -> u8 {
    let width = i32::from(grid.max_x) - i32::from(grid.min_x) + 1;
    let height = i32::from(grid.max_y) - i32::from(grid.min_y) + 1;
    if width >= 14 || height >= 14 { 4 } else { 3 }
}

/// The grid a house's design is edited on: its foundation's own box, one row
/// deeper.
///
/// The extra row is where the stairs go, and it is the same row
/// [`initial_foundation`](crate::design::initial_foundation) lays them on —
/// ServUO's `Resize(Width, Height + 1)`, asked here so that the two cannot drift
/// apart.
///
/// `None` when the foundation's platform cannot be read; see
/// [`EditRefusal::FoundationUnreadable`].
#[must_use]
pub fn buildable_box(state: &WorldState, house: EntityId) -> Option<Bounds> {
    let entry = state.registry.get::<House>(house)?;
    let platform = state.multis.components(entry.multi.0);
    let box_ = bounds(platform)?;
    Some(Bounds {
        min_x: box_.min_x,
        min_y: box_.min_y,
        max_x: box_.max_x,
        max_y: box_.max_y.checked_add(1)?,
    })
}

/// Apply one editing verb to whatever design `actor` has open.
///
/// # Errors
///
/// Every [`EditRefusal`], and each of them before the working copy is touched:
/// a refused edit leaves the design exactly as it was.
pub fn apply(state: &mut WorldState, actor: EntityId, edit: DesignEdit) -> Result<(), EditRefusal> {
    let house = open_house(state, actor)?;
    match edit {
        DesignEdit::Build { graphic, dx, dy } => build(state, house, graphic, dx, dy),
        DesignEdit::Erase { graphic, dx, dy, dz } => erase(state, house, graphic, dx, dy, dz),
        DesignEdit::Floor { storey } => select_floor(state, house, storey),
    }
}

/// The house `actor` has open, or [`EditRefusal::NoSession`].
fn open_house(state: &WorldState, actor: EntityId) -> Result<EntityId, EditRefusal> {
    let who = state.registry.serial_of(actor).ok_or(EditRefusal::NoSession)?;
    crate::session::house_of_editor(state, who).ok_or(EditRefusal::NoSession)
}

/// Lay `graphic` at an offset on the storey the editor is on.
///
/// The height is not on the wire — ServUO's `Designer_Build` derives it from the
/// session's own level — with one exception the reference states outright: a
/// piece on the far-south row is laid at zero, because that row is the stair
/// strip outside the building rather than a storey of it.
fn build(
    state: &mut WorldState,
    house: EntityId,
    graphic: Graphic,
    dx: i32,
    dy: i32,
) -> Result<(), EditRefusal> {
    let (Ok(dx), Ok(dy)) = (i16::try_from(dx), i16::try_from(dy)) else {
        // Wider than a house could ever be, so it is off the grid by the same
        // rule the check below applies — not a separate kind of refusal.
        return Err(EditRefusal::OutsideTheHouse);
    };
    let grid = buildable_box(state, house).ok_or(EditRefusal::FoundationUnreadable)?;
    if dx < grid.min_x || dx > grid.max_x || dy < grid.min_y || dy > grid.max_y {
        return Err(EditRefusal::OutsideTheHouse);
    }

    let laid = state.tiles().static_tile(graphic.0);
    if laid.flags.is_roof() {
        return Err(EditRefusal::RoofPiece);
    }
    let (solid, roofing) = (laid.height > 0, laid.flags.is_roof());

    let session = state
        .registry
        .get::<DesignSession>(house)
        .expect("the session was found by the entity it sits on");
    let dz = match dy == grid.max_y {
        true => 0,
        false => storey_z(session.floor),
    };
    // What this piece stands in the place of. ServUO's `MultiComponentList.Add`
    // takes out whatever is already at that height *in the same sense* — one
    // floor replaces another, one wall replaces another, and a wall standing on
    // a floor replaces neither. Without it a client's repeated clicks stack a
    // hundred copies of the same wall on one tile, all of them on the wire.
    let replaced: Vec<usize> = session
        .working
        .iter()
        .enumerate()
        .filter(|(_, standing)| standing.dx == dx && standing.dy == dy && standing.dz == dz)
        .filter(|(_, standing)| {
            let there = state.tiles().static_tile(standing.graphic.0);
            (there.height > 0) == solid && there.flags.is_roof() == roofing
        })
        .map(|(nth, _)| nth)
        .collect();

    let session = state
        .registry
        .get_mut::<DesignSession>(house)
        .expect("the session was read a line ago");
    for nth in replaced.into_iter().rev() {
        session.working.remove(nth);
    }
    session.working.push(Component {
        graphic,
        dx,
        dy,
        dz,
        flags: DRAWN,
    });
    Ok(())
}

/// Take the piece standing at an offset and height off the working design.
///
/// The graphic is on the wire as well as the place, because several pieces stand
/// on one tile and the editor is naming one of them.
fn erase(
    state: &mut WorldState,
    house: EntityId,
    graphic: Graphic,
    dx: i32,
    dy: i32,
    dz: i32,
) -> Result<(), EditRefusal> {
    let (Ok(dx), Ok(dy), Ok(dz)) = (i16::try_from(dx), i16::try_from(dy), i16::try_from(dz)) else {
        return Err(EditRefusal::OutsideTheHouse);
    };
    let grid = buildable_box(state, house).ok_or(EditRefusal::FoundationUnreadable)?;

    // The foundation's floor, and the one refusal that is about the building
    // rather than about the request: everything at height zero inside the box is
    // the platform the house stands on. The south row is excluded because that
    // is the stair strip, which a player is meant to be able to move.
    let inside_x = dx >= grid.min_x && dx <= grid.max_x;
    let above_the_stairs = dy >= grid.min_y && dy < grid.max_y;
    if dz == 0 && inside_x && above_the_stairs {
        return Err(EditRefusal::FixedFloor);
    }

    let session = state
        .registry
        .get::<DesignSession>(house)
        .expect("the session was found by the entity it sits on");
    let Some(nth) = session.working.iter().position(|standing| {
        standing.graphic == graphic && standing.dx == dx && standing.dy == dy && standing.dz == dz
    }) else {
        return Err(EditRefusal::NothingThere);
    };
    // Whether the first storey still has a floor on this tile once that piece is
    // gone — asked of every *other* component, which is the reference asking it
    // after the removal. The signature tile does not count, for the reason it
    // does not count towards a multi's drawn set either.
    let floored = session.working.iter().enumerate().any(|(other, standing)| {
        other != nth
            && standing.dx == dx
            && standing.dy == dy
            && standing.dz == FIRST_STOREY_Z
            && standing.graphic != SIGNATURE
    });
    // And whether this tile is one the reference fills at all: the interior,
    // which is the grid without its west and north edges and without the stair
    // row. An edge is where a wall goes, and a wall is not a hole.
    let interior = dx > grid.min_x && dx <= grid.max_x && dy > grid.min_y && dy < grid.max_y;

    let session = state
        .registry
        .get_mut::<DesignSession>(house)
        .expect("the session was read a line ago");
    session.working.remove(nth);
    if interior && !floored {
        session.working.push(Component {
            graphic: Graphic(DIRT),
            dx,
            dy,
            dz: FIRST_STOREY_Z,
            flags: DRAWN,
        });
    }
    Ok(())
}

/// Move the editor to another storey, which is what the next
/// [`build`] will be laid at.
///
/// Out of range is **clamped to the ground floor and not refused**, which is the
/// reference's own answer (`Designer_Level`'s `newLevel = 1`): the storey picker
/// is a row of buttons, and a client that has one more of them than this house
/// has storeys is showing the player a floor that does not exist rather than
/// asking for something forbidden.
///
/// What the reference does here and this does not: teleport the editor's body up
/// to the new storey. That is the same class of thing step 1 left out of
/// `BeginCustomize` — about bodies rather than about the session's state — and it
/// belongs with the client-side editor.
fn select_floor(state: &mut WorldState, house: EntityId, storey: RawStorey) -> Result<(), EditRefusal> {
    let ceiling = buildable_box(state, house)
        .map(max_storeys)
        .ok_or(EditRefusal::FoundationUnreadable)?;
    let chosen = u8::try_from(storey.0)
        .ok()
        .filter(|asked| (crate::session::GROUND_FLOOR..=ceiling).contains(asked))
        .unwrap_or(crate::session::GROUND_FLOOR);
    state
        .registry
        .get_mut::<DesignSession>(house)
        .expect("the session was found by the entity it sits on")
        .floor = chosen;
    Ok(())
}
