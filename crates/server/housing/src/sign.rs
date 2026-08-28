//! The house sign: the window a double-click on it opens, and the answer.
//!
//! # One window, and it is a window over the five verbs that already exist
//!
//! Everything under this was built before it: [`trust`](crate::trust),
//! [`distrust`](crate::distrust), [`ban`](crate::ban), [`unban`](crate::unban)
//! and the eviction. Staff reached them through five commands, and the commands
//! raised a cursor because naming a mobile needs a lookup this engine has no
//! verb for. The sign changes nothing about any of that: the five buttons raise
//! the same cursor, and the rows are the one thing a cursor cannot do — take
//! somebody *off* a list without asking them to stand still for it.
//!
//! # The rows are the server's memory
//!
//! [`HouseGumpContext`] holds who each row was. A reply names a number, the
//! client may send any number it likes, and a window this side never drew
//! resolves to nothing. `openshard_guilds::gump` says the same thing at greater
//! length and this is the same rule.
//!
//! # Why the authority is asked twice
//!
//! A window is drawn for whoever clicked the sign and lives until they answer
//! it. The house's lists can change in between — a co-owner dropped while their
//! window is open still has the window — so every branch re-asks
//! [`Standing`](openshard_state::Standing) through the verb it calls, and the
//! hiding of a button is a courtesy rather than the check.

use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_protocol::gump::{
    ButtonId, CloseGump, GUMP_WHITE, GumpAnswer, GumpButton, GumpDisplay, GumpId, GumpKey, GumpLayout,
    GumpPoint, GumpResponse,
};
use openshard_protocol::serial::Serial;
use openshard_protocol::server_packet::ServerPacket;
use openshard_state::components::{Client, House, Name};
use openshard_state::{
    HouseChange, HouseGumpContext, HouseGumpRow, HouseList, HouseStorage, Standing, TargetPurpose, WorldState,
};

/// The id the house window answers under.
pub const HOUSE_GUMP: GumpId = openshard_protocol::gump::id::HOUSE;

/// Where the window opens, and how big it is. Wider than the guild window
/// because it draws three lists abreast rather than one.
const WINDOW: (i32, i32) = (100, 100);
const FRAME: (i32, i32) = (520, 400);

/// The most names a column draws before it says it has been cut. The lists hold
/// [`MAX_FRIENDS`](crate::MAX_FRIENDS) — a hundred and forty — and a gump has no
/// scrollbar, so the column says how many it did not draw.
pub const MAX_ROWS: usize = 10;

/// The first row's top edge, and how far apart rows sit.
const ROW_TOP: i32 = 130;
const ROW_HEIGHT: i32 = 22;

/// Where each column starts.
const COLUMNS: [i32; 3] = [20, 190, 360];

/// Hues: the heading, and the one the ban column is drawn in.
const HUE_HEADING: u32 = 1153;
const HUE_BAN: u32 = 33;

/// The buttons that are not rows.
pub(crate) mod button {
    use openshard_protocol::gump::ButtonId;

    /// Dismiss. The layout draws no `X` of its own.
    pub const CLOSE: ButtonId = ButtonId::CLOSE_BOX;
    /// Raise a cursor to make somebody a friend.
    pub const FRIEND: ButtonId = ButtonId(1);
    /// A co-owner.
    pub const CO_OWNER: ButtonId = ButtonId(2);
    /// Take somebody off both lists.
    pub const DROP: ButtonId = ButtonId(3);
    /// Ban them.
    pub const BAN: ButtonId = ButtonId(4);
    /// Lift a ban.
    pub const UNBAN: ButtonId = ButtonId(5);
    /// Raise a cursor to pin an item in place.
    pub const LOCK_DOWN: ButtonId = ButtonId(6);
    /// To make a container a secure only co-owners open.
    pub const SECURE_CO_OWNERS: ButtonId = ButtonId(7);
    /// The same, open to friends.
    pub const SECURE_FRIENDS: ButtonId = ButtonId(8);
    /// The same, open to anybody who walks in — `Standing::Stranger`, which is
    /// what ServUO's fourth `SecureLevel` means.
    pub const SECURE_ANYONE: ButtonId = ButtonId(9);
    /// And to let one go loose again.
    pub const RELEASE: ButtonId = ButtonId(10);
    /// Pull the house down. The owner's, and nobody else's.
    pub const DEMOLISH: ButtonId = ButtonId(11);
}

/// The first row button, and how many a row draws.
///
/// One: a name on a list has exactly one thing to ask for, and which one it is
/// comes from the *list* — a co-owner or a friend is dropped, a banned player is
/// let back to the door. That is why one base serves all three columns: the rows
/// are drawn into one list in draw order and the context remembers which column
/// each came from.
pub(crate) const ROW_BASE: u32 = 100;
pub(crate) const ROW_ACTIONS: u32 = 1;

/// The button id for the row at `index`.
pub(crate) const fn row_button(index: usize) -> ButtonId {
    ButtonId(ROW_BASE + (index as u32) * ROW_ACTIONS)
}

/// Which row a button names, or `None` if it is not one.
pub(crate) fn row_of(button: ButtonId, rows: usize) -> Option<usize> {
    let index = (button.0.checked_sub(ROW_BASE)? / ROW_ACTIONS) as usize;
    (index < rows).then_some(index)
}

/// Open a house's window for a player.
///
/// Closes whatever is under the id first: a client told to draw the same id
/// twice draws two windows, which every other dialog here opens by avoiding.
pub fn show(state: &mut WorldState, player: EntityId, house: EntityId) {
    let Some(&Client { connection, .. }) = state.registry.get::<Client>(player) else {
        return;
    };
    let (Some(serial), true) = (
        state.registry.serial_of(player),
        state.registry.has::<House>(house),
    ) else {
        return;
    };
    // Opening the sign is what refreshes the house — see `decay`'s own header for
    // why it is this and not the owner walking in. Anybody the house trusts, and
    // it happens before the window is built so the condition line it draws is the
    // one the viewer just caused.
    if standing_of(state, player, house) >= Standing::Friend {
        crate::decay::refresh(state, house);
    }
    let (layout, context) = build(state, player, house);
    let (string, lines) = layout.finish();

    let close = ServerPacket::CloseGump(CloseGump {
        gump_id: HOUSE_GUMP,
        button: ButtonId::CLOSE_BOX,
    });
    let draw = ServerPacket::GumpDisplay(GumpDisplay {
        serial: GumpKey::on(serial),
        gump_id: HOUSE_GUMP,
        at: GumpPoint::new(WINDOW.0, WINDOW.1),
        layout: string.to_owned(),
        lines: lines.to_vec(),
    });
    state.send_packet(connection, &close);
    state.send_packet(connection, &draw);
    if let Some(row) = state.row_of_mut(player) {
        row.house_gump = Some(context);
    }
}

/// Where `player` stands with `house`, or a stranger if either is not what it
/// says it is.
fn standing_of(state: &WorldState, player: EntityId, house: EntityId) -> Standing {
    let (Some(entry), Some(who)) = (
        state.registry.get::<House>(house),
        state.registry.serial_of(player),
    ) else {
        return Standing::Stranger;
    };
    entry.standing_of(who, state.is_staff(player))
}

/// What to call somebody who is on a list.
///
/// A serial rather than a mobile, because a list outlives a logout — so the name
/// is only there while its owner is. The fallback is the serial itself rather
/// than "someone", so two absent friends are two rows a player can tell apart
/// and drop the right one of. A roster that reads names off the character store
/// would fix it for the guild window at the same time; neither has one.
fn name_of(state: &WorldState, who: Serial) -> String {
    state
        .registry
        .entity_of(who)
        .and_then(|entity| state.registry.get::<Name>(entity))
        .map_or_else(|| format!("{who}"), |name| name.0.clone())
}

/// Build the window, and the record of what its rows meant.
fn build(state: &WorldState, player: EntityId, house: EntityId) -> (GumpLayout, HouseGumpContext) {
    let mut layout = GumpLayout::new();
    layout.no_resize();
    layout.page(0);
    layout.background(0, 0, FRAME.0, FRAME.1, 5054);

    let mut context = HouseGumpContext {
        house,
        rows: Vec::new(),
    };
    let Some(entry) = state.registry.get::<House>(house) else {
        return (layout, context);
    };
    let staff = state.is_staff(player);
    let standing = state
        .registry
        .serial_of(player)
        .map_or(Standing::Stranger, |who| entry.standing_of(who, staff));
    let owns = standing >= Standing::Owner;

    layout.label(
        20,
        20,
        HUE_HEADING,
        format!("The house of {}", name_of(state, entry.owner)),
    );
    // What the viewer is to this house, said out loud. A friend who wonders why
    // there are no buttons has the answer on the same screen.
    layout.label(20, 44, GUMP_WHITE, format!("You are: {}", standing.name()));
    // And how the house itself is doing. The one line a player comes to the sign
    // for that is not about a list.
    let condition = crate::decay::condition(state, house);
    layout.label(
        260,
        20,
        if condition >= crate::decay::Condition::Greatly {
            HUE_BAN
        } else {
            GUMP_WHITE
        },
        condition.message(),
    );

    // The five verbs, each raising the same cursor the staff commands do. Drawn
    // for a co-owner and above, which is what all five refuse below — see
    // `crate::trust`. Hiding them is the courtesy; the verb is the check.
    if standing >= Standing::CoOwner {
        let mut x = 20;
        for (id, label) in [
            (button::FRIEND, "Add a friend"),
            (button::CO_OWNER, "Add a co-owner"),
            (button::DROP, "Remove someone"),
            (button::BAN, "Ban someone"),
            (button::UNBAN, "Lift a ban"),
        ] {
            layout.button(x, 74, 4005, 4007, GumpButton::Reply, 0, id);
            layout.label(x, 98, GUMP_WHITE, label);
            x += 100;
        }
        // And the storage verbs, on the same terms and the same authority. The
        // count is drawn beside them because it is the one number a player needs
        // before pressing any of them, and there is nowhere else to read it.
        let allowance = crate::storage::allowance(state, house);
        let used = crate::storage::locked_down(state, house).len();
        let stored = crate::storage::stored(state, house);
        layout.label(
            20,
            FRAME.1 - 60,
            HUE_HEADING,
            format!(
                "Locked down: {used} of {}.  In the secures: {stored} of {}.",
                allowance.lockdowns, allowance.storage
            ),
        );
        let mut x = 20;
        for (id, label) in [
            (button::LOCK_DOWN, "Lock down"),
            (button::SECURE_CO_OWNERS, "Secure: co-owners"),
            (button::SECURE_FRIENDS, "Secure: friends"),
            (button::SECURE_ANYONE, "Secure: anyone"),
            (button::RELEASE, "Release"),
        ] {
            layout.button(x, FRAME.1 - 36, 4005, 4007, GumpButton::Reply, 0, id);
            layout.label(x, FRAME.1 - 14, GUMP_WHITE, label);
            x += 100;
        }
        // And the one thing only the owner may do.
        if owns {
            layout.button(
                x,
                FRAME.1 - 36,
                4005,
                4007,
                GumpButton::Reply,
                0,
                button::DEMOLISH,
            );
            layout.label(x, FRAME.1 - 14, HUE_BAN, "Demolish");
        }
    }

    // Three columns. A row's button appears only for somebody who may press it,
    // and the rows go into one list in draw order — the context carries which
    // column each came from, so one button id serves all three.
    let may_change = standing >= Standing::CoOwner;
    for (column, (list, heading, hue, people)) in [
        (HouseList::CoOwners, "Co-owners", GUMP_WHITE, &entry.co_owners),
        (HouseList::Friends, "Friends", GUMP_WHITE, &entry.friends),
        (HouseList::Bans, "Banned", HUE_BAN, &entry.bans),
    ]
    .into_iter()
    .enumerate()
    {
        let x = COLUMNS[column];
        layout.label(x, ROW_TOP - 24, hue, heading);
        for (nth, &who) in people.iter().take(MAX_ROWS).enumerate() {
            let y = ROW_TOP + (nth as i32) * ROW_HEIGHT;
            layout.label(x, y, GUMP_WHITE, name_of(state, who));
            if may_change {
                context.rows.push(HouseGumpRow::new(list, who));
                let index = context.rows.len() - 1;
                layout.button(x + 130, y, 4017, 4019, GumpButton::Reply, 0, row_button(index));
            }
        }
        if people.len() > MAX_ROWS {
            let y = ROW_TOP + (MAX_ROWS as i32) * ROW_HEIGHT;
            layout.label(
                x,
                y,
                HUE_HEADING,
                format!("...and {} more.", people.len() - MAX_ROWS),
            );
        }
    }
    (layout, context)
}

/// Act on a house window's reply.
///
/// Returns whether the reply was one of ours, so the router can fall through.
pub fn handle(state: &mut WorldState, connection: ConnectionId, response: &GumpResponse) -> bool {
    if response.gump_id.validate(&[HOUSE_GUMP]).is_none() {
        return false;
    }
    let Some(&player) = state.players.get(&connection) else {
        return true;
    };
    // Taken, not read: the window is gone the moment it is answered, and a
    // branch that wants another draws it fresh. That is what makes a
    // double-click on one button a single change rather than two.
    let Some(context) = state.row_of_mut(player).and_then(|row| row.house_gump.take()) else {
        return true;
    };
    let GumpAnswer::Pressed(pressed) = response.button.interpret() else {
        return true; // the close box
    };

    let raise = |state: &mut WorldState, change: HouseChange, prompt: &str| {
        state.raise_target(player, TargetPurpose::HouseList { change });
        state.system_message(player, prompt);
    };
    // The storage cursors carry the *house* as well, because unlike a list
    // change they are not answered by "the house the actor is standing in": a
    // player pressing Release steps to the item, which may be through a wall
    // from where they pressed it.
    let house = context.house;
    let pin = |state: &mut WorldState, change: HouseStorage, prompt: &str| {
        state.raise_target(player, TargetPurpose::HouseStorage { change, house });
        state.system_message(player, prompt);
    };
    match pressed {
        button::CLOSE => {}
        button::FRIEND => raise(
            state,
            HouseChange::Friend,
            "Whom shall be a friend of this house?",
        ),
        button::CO_OWNER => raise(state, HouseChange::CoOwner, "Whom shall co-own this house?"),
        button::DROP => raise(state, HouseChange::Drop, "Whom shall be removed?"),
        button::BAN => raise(state, HouseChange::Ban, "Whom shall be banned?"),
        button::UNBAN => raise(state, HouseChange::Unban, "Whose ban shall be lifted?"),
        button::LOCK_DOWN => pin(state, HouseStorage::LockDown, "What shall be locked down?"),
        button::SECURE_CO_OWNERS => pin(
            state,
            HouseStorage::Secure(Standing::CoOwner),
            "Which container shall the co-owners keep?",
        ),
        button::SECURE_FRIENDS => pin(
            state,
            HouseStorage::Secure(Standing::Friend),
            "Which container shall your friends open?",
        ),
        button::SECURE_ANYONE => pin(
            state,
            HouseStorage::Secure(Standing::Stranger),
            "Which container shall stand open to anyone?",
        ),
        button::RELEASE => pin(state, HouseStorage::Release, "What shall be released?"),
        button::DEMOLISH => {
            // The owner's alone, re-asked here: the window outlives the standing
            // that drew it, and hiding the button hid it on one screen.
            if standing_of(state, player, context.house) < Standing::Owner {
                state.system_message(player, "That is not your house to pull down.");
                return true;
            }
            crate::decay::demolish(state, context.house);
            state.system_message(player, "The house comes down. What it held is in the crate.");
        }
        other => {
            let Some(index) = row_of(other, context.rows.len()) else {
                return true;
            };
            let HouseGumpRow { list, member: who } = context.rows[index];
            // The list the row was drawn under decides the verb: a co-owner or a
            // friend is taken off, a banned player is let back to the door.
            let change = match list {
                HouseList::CoOwners | HouseList::Friends => HouseChange::Drop,
                HouseList::Bans => HouseChange::Unban,
            };
            apply(state, player, context.house, change, who);
            show(state, player, context.house);
        }
    }
    true
}

/// Make one change to one house's lists on behalf of `player`.
///
/// Public because the cursor's answer lands in the world crate — which is where
/// an eviction is announced from — and both paths must go through the same
/// authority check. See `World::change_house_list`.
pub fn apply(state: &mut WorldState, player: EntityId, house: EntityId, change: HouseChange, who: Serial) {
    let Some(actor) = state.registry.serial_of(player) else {
        return;
    };
    let staff = state.is_staff(player);
    let Some(entry) = state.registry.get_mut::<House>(house) else {
        return;
    };
    let outcome = match change {
        HouseChange::Friend => crate::trust(entry, actor, who, Standing::Friend, staff),
        HouseChange::CoOwner => crate::trust(entry, actor, who, Standing::CoOwner, staff),
        HouseChange::Drop => crate::distrust(entry, actor, who, staff),
        HouseChange::Ban => crate::ban(entry, actor, who, staff),
        HouseChange::Unban => crate::unban(entry, actor, who, staff),
    };
    match outcome {
        Ok(()) => {
            for evicted in crate::evict_the_banned(state, house) {
                state.system_message(evicted, "You have been banned from this house.");
            }
            state.system_message(player, "Done.");
        }
        Err(refusal) => state.system_message(player, refusal.message()),
    }
}
