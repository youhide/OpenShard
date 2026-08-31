//! The guild window: one dialog, three pages, reached from the paperdoll's
//! Guild button.
//!
//! # One window, not four
//!
//! ServUO draws a gump per question — `GuildmasterGump`, `GuildRosterGump`,
//! `GuildDeclareWarGump`, `GrantGuildTitleGump`, a dozen more — because each is a
//! subclass with its own `OnResponse`. One id and three pages is the same
//! information with one reply handler, which is the shape `openshard-quests`
//! settled on for the same reason: four handlers that must agree about button
//! numbering are four chances to disagree.
//!
//! # The rows are the server's memory
//!
//! A reply names a *row*, and a row means whatever was drawn in it. The client is
//! free to send any number, so which member or which guild row three was comes
//! from [`GuildGumpContext`] — what this side remembers drawing — and never from
//! the packet. A reply to a window this side never opened resolves to nothing.
//!
//! # What it does not draw yet
//!
//! Paging. The lists are capped at [`MAX_ROWS`] and say so on the last line when
//! they are cut, rather than quietly showing the first twelve of a hundred.

use openshard_entities::EntityId;
use openshard_protocol::gump::{
    ButtonId,
    CloseGump,
    GUMP_WHITE,
    GumpButton,
    GumpDisplay,
    GumpId,
    GumpKey,
    GumpLayout,
    GumpPoint,
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_state::{
    Client,
    GuildGumpContext,
    GuildMember,
    GuildPage,
    Name,
    Rank,
    WorldState,
};

use crate::{
    RankFlags,
    may_lead,
    roster,
};

/// The gump id the guild window answers under. Distinct from the quest window's
/// `0x0051_0001` and the admin menu's `0x00AD_0001`, so a reply is never
/// mistaken for either.
pub const GUILD_GUMP: GumpId = openshard_protocol::gump::id::GUILD;

/// Where the window opens.
const WINDOW: (i32, i32) = (100, 100);
/// How wide and tall the frame is. Tall enough for [`MAX_ROWS`] rows.
const FRAME: (i32, i32) = (420, 400);
/// The most rows a list draws before it says it has been cut.
pub const MAX_ROWS: usize = 12;

/// The first row's top edge, and how far apart rows sit.
const ROW_TOP: i32 = 90;
const ROW_HEIGHT: i32 = 24;

/// Hues: one for what a guild is, one for what it is at war with.
const HUE_HEADING: u32 = 1153;
const HUE_WAR: u32 = 33;
const HUE_ALLY: u32 = 68;

/// The buttons that mean one thing wherever they appear.
pub(crate) mod button {
    use openshard_protocol::gump::ButtonId;

    /// Dismiss. The layout draws no `X` of its own, so this is only ever the
    /// client's close box.
    pub const CLOSE: ButtonId = ButtonId::CLOSE_BOX;
    /// Found the guild named in the two fields.
    pub const FOUND: ButtonId = ButtonId(1);
    /// Say yes to an invitation.
    pub const ACCEPT: ButtonId = ButtonId(2);
    /// Say no to one.
    pub const DECLINE: ButtonId = ButtonId(3);
    /// To the roster page.
    pub const ROSTER: ButtonId = ButtonId(4);
    /// To the diplomacy page.
    pub const DIPLOMACY: ButtonId = ButtonId(5);
    /// Back to the front page.
    pub const MAIN: ButtonId = ButtonId(6);
    /// Leave the guild.
    pub const LEAVE: ButtonId = ButtonId(7);
    /// Raise a cursor to ask someone to join.
    pub const INVITE: ButtonId = ButtonId(8);
    /// Disband it.
    pub const DISBAND: ButtonId = ButtonId(9);
    /// Leave the alliance, or decline the invitation to one. Not a row: it
    /// names the alliance rather than any guild on the page.
    pub const LEAVE_ALLIANCE: ButtonId = ButtonId(10);
    /// Accept an invitation into an alliance.
    pub const JOIN_ALLIANCE: ButtonId = ButtonId(11);
}

/// The two text fields on the founding form.
pub(crate) const FIELD_NAME: u32 = 1;
pub(crate) const FIELD_ABBREVIATION: u32 = 2;

/// One action column inside an encoded row button.
///
/// A raw action is also a `u32`, like a button base and stride. Keeping it
/// distinct makes exchanging any two terms of the encoding formula a type
/// error rather than a different, valid-looking [`ButtonId`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct RowAction(u32);

/// What a member's row can ask for. Named rather than numbered at the call
/// sites, because five columns whose meaning is their position is exactly the
/// arithmetic the admin menu got wrong.
pub(crate) const ROSTER_TITLE: RowAction = RowAction(0);
pub(crate) const ROSTER_PROMOTE: RowAction = RowAction(1);
pub(crate) const ROSTER_DEMOTE: RowAction = RowAction(2);
pub(crate) const ROSTER_DISMISS: RowAction = RowAction(3);
pub(crate) const ROSTER_LEAD: RowAction = RowAction(4);
const ROSTER_ACTIONS: [RowAction; 5] = [
    ROSTER_TITLE,
    ROSTER_PROMOTE,
    ROSTER_DEMOTE,
    ROSTER_DISMISS,
    ROSTER_LEAD,
];

/// And what a guild's row on the diplomacy page can ask for.
pub(crate) const DIPLOMACY_WAR: RowAction = RowAction(0);
pub(crate) const DIPLOMACY_PEACE: RowAction = RowAction(1);
pub(crate) const DIPLOMACY_ALLY: RowAction = RowAction(2);
const DIPLOMACY_ACTIONS: [RowAction; 3] = [DIPLOMACY_WAR, DIPLOMACY_PEACE, DIPLOMACY_ALLY];

/// The field a new alliance's name is typed into.
pub(crate) const FIELD_ALLIANCE: u32 = 3;

/// The button-number space occupied by one kind of row.
///
/// The base and stride travel as one value so drawing and reply decoding cannot
/// accidentally pair a roster base with diplomacy's stride.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct RowButtons {
    base:   ButtonId,
    stride: u32,
}

impl RowButtons {
    const fn new(base: ButtonId, actions: &[RowAction]) -> Self {
        assert!(!actions.is_empty(), "a row-button stride cannot be zero");
        Self {
            base,
            stride: actions.len() as u32,
        }
    }

    /// The button id for one action on one row.
    pub(crate) fn button(self, row: usize, action: RowAction) -> ButtonId {
        assert!(action.0 < self.stride, "action is outside this row's stride");
        let row = u32::try_from(row).expect("a row-button index must fit on the wire");
        let offset = row
            .checked_mul(self.stride)
            .and_then(|offset| offset.checked_add(action.0))
            .and_then(|offset| self.base.0.checked_add(offset))
            .expect("a row button must fit on the wire");
        ButtonId(offset)
    }

    /// Which row and action a button id names, or `None` if it is outside the
    /// rows the server remembers drawing.
    pub(crate) fn decode(self, button: ButtonId, rows: usize) -> Option<(usize, RowAction)> {
        let offset = button.0.checked_sub(self.base.0)?;
        let row = usize::try_from(offset / self.stride).ok()?;
        if row >= rows {
            return None;
        }
        Some((row, RowAction(offset % self.stride)))
    }
}

/// A member row: title, promote, demote, dismiss, or pass leadership.
pub(crate) const ROSTER_BUTTONS: RowButtons = RowButtons::new(ButtonId(100), &ROSTER_ACTIONS);
/// Another guild's row: war, peace, or alliance.
pub(crate) const DIPLOMACY_BUTTONS: RowButtons = RowButtons::new(ButtonId(1000), &DIPLOMACY_ACTIONS);

/// Draw the guild window for a player, and remember what it drew.
///
/// Closes the window already open first. The pages replace each other under one
/// id, and a client told to draw the same id twice draws two windows — the same
/// close-then-draw every other dialog here opens with.
pub fn show(state: &mut WorldState, player: EntityId, page: GuildPage) {
    let Some(&Client { connection, .. }) = state.registry.get::<Client>(player) else {
        return;
    };
    let Some(serial) = state.registry.serial_of(player) else {
        return;
    };
    // A player who is not in a guild has one page. Asking for another is not an
    // error — it is a stale button on a window drawn before they left — so it
    // lands on the page they do have.
    let page = if state.guild_of(player).is_some() {
        page
    } else {
        GuildPage::Main
    };
    let (layout, context) = build(state, player, page);
    let (string, lines) = layout.finish();

    let close = ServerPacket::CloseGump(CloseGump {
        gump_id: GUILD_GUMP,
        button:  ButtonId::CLOSE_BOX,
    });
    let draw = ServerPacket::GumpDisplay(GumpDisplay {
        serial:  GumpKey::on(serial),
        gump_id: GUILD_GUMP,
        at:      GumpPoint::new(WINDOW.0, WINDOW.1),
        layout:  string.to_owned(),
        lines:   lines.to_vec(),
    });
    state.send_packet(connection, &close);
    state.send_packet(connection, &draw);
    if let Some(row) = state.row_of_mut(player) {
        row.guild_gump = Some(context);
    }
}

/// Build one page, and the record of what its rows meant.
fn build(state: &WorldState, player: EntityId, page: GuildPage) -> (GumpLayout, GuildGumpContext) {
    let mut layout = GumpLayout::new();
    layout.no_resize();
    layout.page(0);
    layout.background(0, 0, FRAME.0, FRAME.1, 5054);

    let mut context = GuildGumpContext {
        page,
        guilds: Vec::new(),
        members: Vec::new(),
    };
    match page {
        GuildPage::Main => main_page(&mut layout, state, player),
        GuildPage::Roster => roster_page(&mut layout, state, player, &mut context),
        GuildPage::Diplomacy => diplomacy_page(&mut layout, state, player, &mut context),
    }
    (layout, context)
}

/// A plain reply button with the menu's art, and its label.
fn action(layout: &mut GumpLayout, x: i32, y: i32, id: ButtonId, hue: u32, label: &str) {
    layout.button(x, y, 4005, 4007, GumpButton::Reply, 0, id);
    layout.label(x + 36, y + 2, hue, label);
}

/// The front page: found a guild, or what yours is.
fn main_page(layout: &mut GumpLayout, state: &WorldState, player: EntityId) {
    let Some(guild) = state.guild_of(player) else {
        return no_guild_page(layout, state, player);
    };
    layout.label(
        20,
        20,
        HUE_HEADING,
        format!("{} [{}]", guild.name, guild.abbreviation),
    );
    let title = state
        .registry
        .get::<GuildMember>(player)
        .map_or("", |member| member.title.as_str());
    let title = if title.is_empty() { "no title" } else { title };
    layout.label(20, 44, GUMP_WHITE, format!("You hold {title}."));

    let mut y = ROW_TOP;
    action(layout, 20, y, button::ROSTER, GUMP_WHITE, "Members");
    y += ROW_HEIGHT + 8;
    // Row by row on the flag it needs, not on "is this the leader". A Warlord
    // gets the diplomacy page and not the invite cursor; an Emissary gets the
    // reverse. Hiding a button is a courtesy either way — `reply` checks the
    // same flag when one comes back, because a window outlives the rank that
    // drew it and the gump id is not a secret.
    if crate::may(state, player, RankFlags::CAN_INVITE).is_ok() {
        action(layout, 20, y, button::INVITE, GUMP_WHITE, "Ask someone to join");
        y += ROW_HEIGHT + 8;
    }
    if crate::may(state, player, RankFlags::CONTROL_WAR_STATUS).is_ok()
        || crate::may(state, player, RankFlags::ALLIANCE_CONTROL).is_ok()
    {
        action(layout, 20, y, button::DIPLOMACY, GUMP_WHITE, "Wars and alliances");
        y += ROW_HEIGHT + 8;
    }
    if may_lead(state, player).is_ok() {
        action(layout, 20, y, button::DISBAND, HUE_WAR, "Disband this guild");
    } else {
        action(layout, 20, y, button::LEAVE, HUE_WAR, "Leave this guild");
    }
}

/// The front page for someone in no guild: the invitation, if there is one, and
/// the form for founding.
fn no_guild_page(layout: &mut GumpLayout, state: &WorldState, player: EntityId) {
    layout.label(20, 20, HUE_HEADING, "Guild");

    let invitation = state
        .registry
        .get::<openshard_state::GuildCandidate>(player)
        .and_then(|asked| state.guilds.get(asked.guild));
    let mut y = 50;
    if let Some(guild) = invitation {
        layout.label(
            20,
            y,
            GUMP_WHITE,
            format!("{} has asked you to join.", guild.name),
        );
        y += ROW_HEIGHT;
        action(layout, 20, y, button::ACCEPT, HUE_ALLY, "Accept");
        action(layout, 200, y, button::DECLINE, HUE_WAR, "Decline");
        y += ROW_HEIGHT + 16;
    }

    layout.label(20, y, GUMP_WHITE, "Found a guild of your own:");
    y += ROW_HEIGHT;
    layout.label(20, y, GUMP_WHITE, "Name");
    layout.text_entry(90, y, 260, 20, GUMP_WHITE, FIELD_NAME, "");
    y += ROW_HEIGHT + 4;
    layout.label(20, y, GUMP_WHITE, "Abbrev.");
    layout.text_entry(90, y, 60, 20, GUMP_WHITE, FIELD_ABBREVIATION, "");
    y += ROW_HEIGHT + 12;
    action(layout, 20, y, button::FOUND, HUE_ALLY, "Found it");
}

/// The roster. A leader gets a field and three buttons per row; everyone else
/// gets the list.
fn roster_page(
    layout: &mut GumpLayout,
    state: &WorldState,
    player: EntityId,
    context: &mut GuildGumpContext,
) {
    let Some(guild) = state.guild_of(player).map(|g| g.id) else {
        return;
    };
    layout.label(20, 20, HUE_HEADING, "Members");
    action(layout, 20, 46, button::MAIN, GUMP_WHITE, "Back");

    let leads = may_lead(state, player).is_ok();
    let may_title = crate::may(state, player, RankFlags::CAN_SET_GUILD_TITLE).is_ok();
    let may_rank = crate::may(state, player, RankFlags::CAN_PROMOTE_DEMOTE).is_ok();
    let own_rank = crate::rank_of(state, player).unwrap_or_default();
    let members = roster(state, guild);
    let shown = members.len().min(MAX_ROWS);
    let mut any_buttons = false;
    for &member in members.iter().take(shown) {
        let Some(serial) = state.registry.serial_of(member) else {
            continue;
        };
        context.members.push(serial);
        let row = context.members.len() - 1;
        let y = ROW_TOP + (row as i32) * ROW_HEIGHT;
        let name = state
            .registry
            .get::<Name>(member)
            .map_or("someone", |name| name.0.as_str());
        layout.label(20, y, GUMP_WHITE, name);
        let entry = state.registry.get::<GuildMember>(member);
        let title = entry.map_or("", |entry| entry.title.as_str());
        let rank = entry.map_or_else(Rank::default, |entry| entry.rank);
        layout.label(140, y, GUMP_WHITE, rank.name());

        // Each button on the rule that would let it through — the same pair of
        // questions the operation asks: the flag, and whether this member is
        // reachable from where the viewer stands. A row about yourself keeps
        // only the title field, because none of the rest applies to you.
        let outranked = own_rank > rank;
        let mut column = 258;
        let mut cell = |layout: &mut GumpLayout, normal: u32, pressed: u32, action: RowAction| {
            layout.button(
                column,
                y,
                normal,
                pressed,
                GumpButton::Reply,
                0,
                ROSTER_BUTTONS.button(row, action),
            );
            column += 30;
            any_buttons = true;
        };
        if may_title && (member == player || outranked) {
            layout.text_entry(190, y, 60, 20, GUMP_WHITE, row as u32, title);
            cell(layout, 4005, 4007, ROSTER_TITLE);
        } else {
            layout.label(190, y, GUMP_WHITE, title);
        }
        if member != player && may_rank {
            // Two rungs to promote, one to demote — `membership::promote` says
            // why they differ. Drawn on the same condition so the button is
            // absent rather than refused.
            let can_promote = match own_rank {
                Rank::Leader => outranked,
                _ => own_rank.number().saturating_sub(1) > rank.number(),
            };
            if can_promote && rank.above().is_some_and(|next| next != Rank::Leader) {
                cell(layout, 2435, 2436, ROSTER_PROMOTE);
            }
            if outranked && rank.below().is_some() {
                cell(layout, 2437, 2438, ROSTER_DEMOTE);
            }
        }
        if member != player && crate::may_dismiss(state, player, member) {
            cell(layout, 4017, 4019, ROSTER_DISMISS);
        }
        if leads && member != player {
            cell(layout, 4011, 4013, ROSTER_LEAD);
        }
    }
    layout.label(140, ROW_TOP - 22, GUMP_WHITE, "rank");
    if any_buttons {
        layout.label(258, ROW_TOP - 22, GUMP_WHITE, "actions");
    }
    cut_notice(layout, members.len(), shown);
}

/// Every other guild, and where this one stands with it.
///
/// # Four columns, and two of them are the same button
///
/// A guild is at war, allied, or neither, and the actions are: declare war, make
/// peace, ask into the alliance, and leave the one you are in. The last is not
/// about a *row* — it names the alliance rather than the guild beside it — so it
/// sits at the top with the alliance's name, and the rows carry the other three.
fn diplomacy_page(
    layout: &mut GumpLayout,
    state: &WorldState,
    player: EntityId,
    context: &mut GuildGumpContext,
) {
    layout.label(20, 20, HUE_HEADING, "Wars and alliances");
    action(layout, 20, 46, button::MAIN, GUMP_WHITE, "Back");
    let Some(own) = state.guild_of(player).map(|guild| guild.id) else {
        return;
    };
    // Each button on the flag that would let it through, as on the main page:
    // a Warlord gets the war columns and an Emissary gets neither, and the
    // reply path checks the same flags again.
    let may_war = crate::may(state, player, RankFlags::CONTROL_WAR_STATUS).is_ok();
    let may_ally = crate::may(state, player, RankFlags::ALLIANCE_CONTROL).is_ok();

    // The alliance this guild is in, named, with the one action that is about
    // the alliance rather than about any row.
    let alliance = state.guilds.get(own).and_then(|guild| guild.alliance);
    match alliance.and_then(|id| state.alliances.get(id)) {
        Some(entry) => {
            layout.label(
                20,
                66,
                HUE_ALLY,
                format!("{} — {} guilds", entry.name, entry.members.len()),
            );
            if may_ally {
                action(layout, 258, 66, button::LEAVE_ALLIANCE, HUE_WAR, "Leave");
            }
        }
        None => {
            layout.label(20, 66, GUMP_WHITE, "In no alliance.");
            if may_ally {
                // The name a new alliance would take. Only read when this guild
                // is in none — see `invite_to_alliance`, which does not rename.
                layout.text_entry(150, 66, 200, 20, GUMP_WHITE, FIELD_ALLIANCE, "");
            }
        }
    }

    if may_war || may_ally {
        layout.label(258, ROW_TOP - 22, GUMP_WHITE, "war  peace  ally");
    }
    let others: Vec<_> = state.guilds.iter().filter(|guild| guild.id != own).collect();
    let shown = others.len().min(MAX_ROWS);
    for (row, guild) in others.iter().take(shown).enumerate() {
        context.guilds.push(guild.id);
        let y = ROW_TOP + (row as i32) * ROW_HEIGHT;
        let ours = state.guilds.get(own);
        let (standing, hue) = if ours.is_some_and(|ours| ours.at_war_with(guild.id)) {
            ("at war", HUE_WAR)
        } else if state.allied(own, guild.id) {
            ("allied", HUE_ALLY)
        } else if ours.is_some_and(|ours| ours.has_declared_on(guild.id)) {
            ("war declared", HUE_WAR)
        } else if alliance.is_some_and(|id| {
            state
                .alliances
                .get(id)
                .is_some_and(|entry| entry.pending.contains(&guild.id))
        }) {
            ("asked in", HUE_ALLY)
        } else {
            ("", GUMP_WHITE)
        };
        layout.label(
            20,
            y,
            GUMP_WHITE,
            format!("{} [{}]", guild.name, guild.abbreviation),
        );
        layout.label(150, y, hue, standing);
        if may_war {
            layout.button(
                258,
                y,
                4017,
                4019,
                GumpButton::Reply,
                0,
                DIPLOMACY_BUTTONS.button(row, DIPLOMACY_WAR),
            );
            layout.button(
                288,
                y,
                4005,
                4007,
                GumpButton::Reply,
                0,
                DIPLOMACY_BUTTONS.button(row, DIPLOMACY_PEACE),
            );
        }
        if may_ally {
            layout.button(
                318,
                y,
                4011,
                4013,
                GumpButton::Reply,
                0,
                DIPLOMACY_BUTTONS.button(row, DIPLOMACY_ALLY),
            );
        }
    }
    cut_notice(layout, others.len(), shown);
}

/// Say so when a list was cut, rather than showing the first twelve of a hundred
/// and looking complete.
fn cut_notice(layout: &mut GumpLayout, total: usize, shown: usize) {
    if total > shown {
        let y = ROW_TOP + (shown as i32) * ROW_HEIGHT;
        layout.label(20, y, HUE_HEADING, format!("...and {} more.", total - shown));
    }
}

#[cfg(test)]
mod tests {
    use openshard_protocol::gump::ButtonId;

    use super::{
        DIPLOMACY_ACTIONS,
        DIPLOMACY_BUTTONS,
        MAX_ROWS,
        ROSTER_ACTIONS,
        ROSTER_BUTTONS,
    };

    #[test]
    fn a_row_button_reads_back_as_the_row_it_was_drawn_for() {
        for row in 0..8 {
            for action in ROSTER_ACTIONS {
                let id = ROSTER_BUTTONS.button(row, action);
                assert_eq!(ROSTER_BUTTONS.decode(id, 8), Some((row, action)));
            }
            for action in DIPLOMACY_ACTIONS {
                let id = DIPLOMACY_BUTTONS.button(row, action);
                assert_eq!(DIPLOMACY_BUTTONS.decode(id, 8), Some((row, action)));
            }
        }
    }

    #[test]
    fn a_button_past_the_end_of_the_list_names_no_row() {
        // The client sends whatever it likes. A row the window never drew has to
        // resolve to nothing rather than to the row arithmetic's opinion.
        assert_eq!(
            ROSTER_BUTTONS.decode(ROSTER_BUTTONS.button(9, ROSTER_ACTIONS[0]), 4),
            None
        );
        assert_eq!(ROSTER_BUTTONS.decode(ButtonId(1), 4), None);
    }

    #[test]
    fn an_action_at_the_stride_boundary_is_not_a_fallback() {
        let last = ROSTER_BUTTONS.button(0, ROSTER_ACTIONS[ROSTER_ACTIONS.len() - 1]);
        let forged = ButtonId(last.0 + 1);

        assert_eq!(
            ROSTER_BUTTONS.decode(forged, 1),
            None,
            "the next number is row one, not another action on row zero"
        );
    }

    /// The two lists share one button space and no longer share a stride, so
    /// "they do not overlap" stopped being obvious the moment the roster grew
    /// from three actions to five. Asserted against the widest either can be —
    /// a full page of rows at its own stride — rather than against a sample.
    #[test]
    fn a_full_roster_never_reaches_the_diplomacy_numbers() {
        let last = ROSTER_BUTTONS.button(MAX_ROWS - 1, ROSTER_ACTIONS[ROSTER_ACTIONS.len() - 1]);
        assert!(
            last.0 < DIPLOMACY_BUTTONS.base.0,
            "roster buttons run to {}, and diplomacy starts at {}",
            last.0,
            DIPLOMACY_BUTTONS.base.0,
        );
        assert!(
            ROSTER_BUTTONS
                .decode(DIPLOMACY_BUTTONS.button(0, DIPLOMACY_ACTIONS[0]), MAX_ROWS)
                .is_none(),
            "and a diplomacy button must not resolve as a member's"
        );
    }
}
