//! The brackets a house-design session opens and closes between.
//!
//! A [`DesignSession`] is the answer to one question the shard could not be
//! asked before: **is this house open in somebody's editor right now.** Nothing
//! here edits a design — the verbs that do are
//! [`plans/housing/customisation/PLAN.md`](../../../../plans/housing/customisation/PLAN.md)'s
//! steps 2 and 3 — and that is deliberate: a session that can be entered, left,
//! and *cannot be left dangling* is the thing every one of those verbs is going
//! to assume.
//!
//! # What the brackets are worth on their own
//!
//! `docs/housing/design_customisation.md`'s C7 states the rule that makes the
//! editor tractable: **while a session is open the world still shows and blocks
//! the committed design.** So opening one changes nothing anybody outside can
//! see — no obstruction churn, no partial design on the wire — and the only
//! thing that *does* change is the editing client's own screen, which is what
//! [`HouseCustomisation`] carries.
//!
//! # A session outlives nothing
//!
//! Logout, death and demolition all end one, and that is the half of this step
//! with teeth. A dangling session is not a missing feature: it names a serial
//! nobody holds, or sits on an entity that has been despawned, and the first
//! verb to reach it fails somewhere far from here. The three enders are
//! [`end_over`] and [`end_for`], called from the world's disconnect and death
//! paths and from [`decay::demolish`](crate::decay::demolish) — which is the one
//! call that destroys a house, so the clock's collapse and the owner's own
//! Demolish button are both covered by the one hook.

use openshard_entities::EntityId;
use openshard_protocol::design::{
    CustomisationBracket,
    HouseCustomisation,
};
use openshard_protocol::feature::Feature;
use openshard_protocol::serial::{
    RawSerial,
    Serial,
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_state::WorldState;
use openshard_state::components::{
    Client,
    DesignSession,
    Ghost,
    House,
    HouseDesign,
    Standing,
};

/// The storey an editor opens on.
///
/// One and not zero: ServUO's `DesignContext` constructor sets `Level = 1`, and
/// the ground floor is a storey like any other. A zero would make "no floor
/// chosen" representable in a state where it cannot happen.
pub const GROUND_FLOOR: u8 = 1;

/// Why a design session could not be opened.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SessionRefusal {
    /// That entity is not a house.
    NotAHouse,
    /// The actor does not own it. Not a co-owner's, for
    /// [`redesign`](crate::design::redesign)'s reason: a co-owner may lock
    /// things down and let people in, and neither changes what the building
    /// *is*.
    NotYours,
    /// A classic house, whose shape is a multi id in every client's own files.
    /// There is nothing on this shard to edit, and inventing a design for it
    /// would give it walls no client could draw.
    NotDesignable,
    /// Somebody already has it open. One editor at a time, because two working
    /// copies of one house are two commits racing to be the shape.
    AlreadyOpen,
    /// A ghost cannot rebuild their house. ServUO's `CheckAlive`, and the same
    /// rule death's own ender states from the other side.
    Dead,
    /// The client is older than the design packets, so it has no editor to open
    /// and no way to say it closed one. Refused rather than left to a session
    /// only a logout could end.
    ClientTooOld,
}

impl SessionRefusal {
    /// What to say to whoever tried.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::NotAHouse => "That is not a house.",
            Self::NotYours => "That is not your house to change.",
            Self::NotDesignable => "This house was built to a plan that cannot be changed.",
            Self::AlreadyOpen => "Someone is already working on this house.",
            Self::Dead => "You cannot build while you are dead.",
            Self::ClientTooOld => "Your client is too old to design a house.",
        }
    }
}

/// Whether `house` is open in somebody's editor.
#[must_use]
pub fn is_open(state: &WorldState, house: EntityId) -> bool {
    state.registry.has::<DesignSession>(house)
}

/// Who has `house` open, if anybody.
#[must_use]
pub fn editor_of(state: &WorldState, house: EntityId) -> Option<Serial> {
    state
        .registry
        .get::<DesignSession>(house)
        .map(|session| session.editor)
}

/// The house `editor` has open, if any.
///
/// A scan over the open sessions rather than an index on the player: there is
/// at most a handful of them on a shard at once — one per player who is
/// *currently* rebuilding a house — and a second copy of the pairing would be
/// one more thing to keep in step through every ender below.
#[must_use]
pub fn house_of_editor(state: &WorldState, editor: Serial) -> Option<EntityId> {
    state
        .registry
        .query::<DesignSession>()
        .find(|(_, session)| session.editor == editor)
        .map(|(house, _)| house)
}

/// Open `house` in `actor`'s editor.
///
/// The working copy starts as the committed design, because an editor opens on
/// the house as it stands. Nothing else changes: the walls, the sign and the
/// picture every other client holds are the committed design's and stay that
/// way until a commit.
///
/// # Errors
///
/// Every [`SessionRefusal`], and each of them before the component is inserted:
/// a refused entry leaves the house exactly as it was.
pub fn begin(state: &mut WorldState, actor: EntityId, house: EntityId) -> Result<(), SessionRefusal> {
    let Some(entry) = state.registry.get::<House>(house) else {
        return Err(SessionRefusal::NotAHouse);
    };
    let Some(who) = state.registry.serial_of(actor) else {
        return Err(SessionRefusal::NotYours);
    };
    // Asked through `standing_of` rather than against `owner` directly, which is
    // the third time that has been the right answer after `Standing` itself and
    // the door. Staff read as a co-owner there, so this refuses them too — a
    // game master's `.hdesign` is the staff path to a shape.
    if entry.standing_of(who, state.is_staff(actor)) < Standing::Owner {
        return Err(SessionRefusal::NotYours);
    }
    if !state.registry.has::<HouseDesign>(house) {
        return Err(SessionRefusal::NotDesignable);
    }
    if state.registry.has::<DesignSession>(house) {
        return Err(SessionRefusal::AlreadyOpen);
    }
    if state.registry.has::<Ghost>(actor) {
        return Err(SessionRefusal::Dead);
    }
    // The client has to be able to *close* what this opens: `0xD7 0x0C` is the
    // only thing that ends a session from the player's side, and a client with
    // no design packets has never heard of it.
    let Some(&Client { connection, .. }) = state.registry.get::<Client>(actor) else {
        return Err(SessionRefusal::ClientTooOld);
    };
    if !state
        .version_of(connection)
        .is_some_and(|version| version.supports(Feature::CustomMulti))
    {
        return Err(SessionRefusal::ClientTooOld);
    }

    // The working copy, taken last: every refusal above leaves the house exactly
    // as it was, and none of them pays for a clone of a few hundred components.
    let working = state
        .registry
        .get::<HouseDesign>(house)
        .expect("the design was checked above")
        .components
        .clone();
    state.registry.insert(
        house,
        DesignSession {
            editor: who,
            working,
            floor: GROUND_FLOOR,
        },
    );
    tell(state, house, actor, CustomisationBracket::Begin);
    Ok(())
}

/// Close whatever session `actor` has open, and answer with the house it was
/// over.
///
/// `None` when they had none, which is the ordinary answer for a `0xD7 0x0C`
/// from a client that closed a window the shard had already closed for it — a
/// logout race, or a second click. Not an error: the state it asks for is the
/// state it ends in.
pub fn end(state: &mut WorldState, actor: EntityId) -> Option<EntityId> {
    let who = state.registry.serial_of(actor)?;
    let house = house_of_editor(state, who)?;
    state.registry.remove::<DesignSession>(house);
    tell(state, house, actor, CustomisationBracket::End);
    Some(house)
}

/// End the session `who` has open, wherever it is — the ender for a player
/// leaving the world, by logout or by death.
///
/// Takes the serial as well as the entity because the two callers hold both and
/// one of them is about to despawn the entity: reading the serial back out of a
/// registry mid-teardown is the sort of ordering the disconnect path has been
/// bitten by before.
pub fn end_for(state: &mut WorldState, actor: EntityId, who: Serial) -> Option<EntityId> {
    let house = house_of_editor(state, who)?;
    state.registry.remove::<DesignSession>(house);
    tell(state, house, actor, CustomisationBracket::End);
    Some(house)
}

/// End whatever session is open over `house`, and answer with who was in it.
///
/// The house's own ender, called before a demolition takes the entity away.
/// The editor is told, because their window is about to be over nothing.
pub fn end_over(state: &mut WorldState, house: EntityId) -> Option<Serial> {
    let editor = editor_of(state, house)?;
    state.registry.remove::<DesignSession>(house);
    if let Some(actor) = state.registry.entity_of(editor) {
        tell(state, house, actor, CustomisationBracket::End);
    }
    Some(editor)
}

/// Tell one client that its editor opened or closed over `house`.
///
/// To the editor and to nobody else: a session is a state of that client's
/// screen, and every other client goes on being shown the committed design —
/// which is the whole of what C7's "the working design touches nothing" buys.
///
/// Silent for an editor with no connection, which is a staff-placed session or
/// one whose player has already gone. The component is removed either way: the
/// packet is a courtesy to a screen, and the state is the thing that must not
/// dangle.
fn tell(state: &mut WorldState, house: EntityId, actor: EntityId, bracket: CustomisationBracket) {
    let (Some(&Client { connection, .. }), Some(serial)) = (
        state.registry.get::<Client>(actor),
        state.registry.serial_of(house),
    ) else {
        return;
    };
    state.send_packet(
        connection,
        &ServerPacket::HouseCustomisation(HouseCustomisation {
            serial: RawSerial(serial.raw()),
            bracket,
        }),
    );
}
