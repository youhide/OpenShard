//! The two verbs that decide what a working design was for: make it the house,
//! or throw it away.
//!
//! [`session`](crate::session) made "this house is open in somebody's editor" a
//! state, and [`editing`](crate::editing) filled that state with a working copy
//! nobody outside the editor can see. This is
//! [`plans/housing/customisation/PLAN.md`](../../../../plans/housing/customisation/PLAN.md)'s
//! step 3, and it is the first one a player can see the result of: until a
//! commit, every shape standing on this shard is a shipped multi, a staff
//! `.hdesign` copy or an imported template.
//!
//! # One commit, one swap
//!
//! `docs/housing/design_customisation.md`'s C7 buys its whole tractability by
//! *not* touching the world while a session is open — so everything that has to
//! change when the shape does happens here, in one move. That tail is
//! [`design::redesign`](crate::design::redesign) and not this module: the walls
//! come out as the old shape and go back in as the new one, a doorway a design
//! cut is adopted, the sign is re-hung because the box moved, the lockdown
//! allowance is recounted, and the revision goes up. `.hdesign` has been paying
//! for that tail since C2, which is why committing an editor's design is the
//! shorter half of this file.
//!
//! # What is refused, and what it leaves behind
//!
//! **Nothing comes down until the new shape is legal**, and the session is part
//! of "nothing": a refused commit leaves the editor open over the working copy
//! that was refused, because the player's next move is to fix it. That is the
//! one thing this module adds to the tail's own rule.

use openshard_entities::EntityId;
use openshard_state::WorldState;
use openshard_state::components::DesignSession;

use crate::design::DesignRefusal;

/// Why a commit did not happen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CommitRefusal {
    /// This player has no house open — the ordinary answer to a commit that
    /// crossed a window the shard had already closed, and the same answer
    /// [`editing::apply`](crate::editing::apply) gives the same race.
    NoSession,
    /// The tail refused the working design. Carried rather than flattened, so
    /// that which of the four it was survives to whoever is told: a commit is
    /// the one design verb a player watches happen, and "no" without a reason
    /// is a button that looks broken.
    Rejected(DesignRefusal),
}

impl CommitRefusal {
    /// What to say to whoever tried.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            // Their editor is already shut, or was never open. The message is
            // for the case where a shard-side ender got there first.
            Self::NoSession => "You are not working on a house.",
            Self::Rejected(refusal) => refusal.message(),
        }
    }
}

/// Make the working design the house's shape, and close the editor over it.
///
/// Answers the new revision — the cache key every client that can see the house
/// is about to be handed.
///
/// # The session is ended last
///
/// ServUO removes its `DesignContext` before it sends the new shape; this ends
/// the session after the swap, because the swap is the thing that can be
/// refused and a player whose design was refused still needs their editor. The
/// difference is invisible to the client either way: the design detail
/// [`redesign`](crate::design::redesign) broadcasts and the `0xBF 0x20` end
/// bracket leave in the same flush, and a client that reads them in that order
/// redraws its editor's plan one packet before disposing the window.
///
/// # Errors
///
/// [`CommitRefusal::NoSession`] when nothing is open, and
/// [`CommitRefusal::Rejected`] for anything the tail refuses. Both leave the
/// house *and* the session exactly as they were.
pub fn commit(state: &mut WorldState, actor: EntityId) -> Result<u32, CommitRefusal> {
    let house = open_house(state, actor)?;
    // Taken out by clone rather than by move: the session has to survive a
    // refusal intact, and a working copy moved out of it and then handed back
    // would be two places that know what the editor was holding.
    let working = state
        .registry
        .get::<DesignSession>(house)
        .expect("the session was found by the entity it sits on")
        .working
        .clone();

    let revision = crate::design::redesign(state, actor, house, working).map_err(CommitRefusal::Rejected)?;

    // And the working copy goes with the session, which is the whole of
    // "throwing it away": the committed design is now what it was a copy of.
    crate::session::end(state, actor).expect("the session was read a moment ago");
    Ok(revision)
}

/// Throw the working design away and start again from the house as it stands.
///
/// Answers the house that was reverted, or `None` when this player has nothing
/// open — [`session::end`](crate::session::end)'s shape and its reason: the
/// state a revert asks for is the state a player with no session is already in.
///
/// The editor is sent the design it reverted to, which is ServUO's own
/// `SendDetailedInfoTo` and the only reason a revert is visible at all: the
/// client has been drawing its own copy of every edit since the session opened,
/// and nothing else on this shard would tell it those edits are gone.
pub fn revert(state: &mut WorldState, actor: EntityId) -> Option<EntityId> {
    let who = state.registry.serial_of(actor)?;
    let house = crate::session::house_of_editor(state, who)?;
    let committed = crate::design::shape_of_house(state, house)
        .expect("a session is only ever opened over a house that has a design");
    state
        .registry
        .get_mut::<DesignSession>(house)
        .expect("the session was found by the entity it sits on")
        .working = committed;
    // The storey the editor is on is deliberately left where it is: which floor
    // is on screen is a fact about the window rather than about the design, and
    // the reference's `Designer_Revert` does not touch its `Level` either.
    state.send_design_detail(actor, house);
    Some(house)
}

/// The house `actor` has open, or [`CommitRefusal::NoSession`].
fn open_house(state: &WorldState, actor: EntityId) -> Result<EntityId, CommitRefusal> {
    let who = state.registry.serial_of(actor).ok_or(CommitRefusal::NoSession)?;
    crate::session::house_of_editor(state, who).ok_or(CommitRefusal::NoSession)
}
