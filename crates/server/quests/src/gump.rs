//! The quest window — one dialog for the log, the offer, the objectives, the
//! rewards and everything the giver says.
//!
//! A port of ServUO's `MondainQuestGump`: the same frame art, the same eight
//! sections, the same button ids, the same page order. It is one window and not
//! six because the client's paperdoll button, an offer and a turn-in all end up
//! looking at the same frame with a different middle, and sharing the id means
//! one reply handler rather than six that must agree.
//!
//! # Why the button ids are copied rather than chosen
//!
//! Nothing on the wire says what a button *means* — the client sends back the
//! number the layout gave it. Keeping ServUO's numbering costs nothing and means
//! the reply handler can be read against `MondainQuestGump.OnResponse` line by
//! line, which is the only way to be sure a page chain is right.

use openshard_entities::EntityId;
use openshard_protocol::gump::id;
use openshard_protocol::gump::{
    ButtonId, CloseGump, GUMP_DARK_GREEN, GUMP_LIGHT_GREEN, GUMP_RED, GUMP_WHITE, GumpButton, GumpDisplay,
    GumpId, GumpKey, GumpLayout, GumpPoint, SwitchId,
};
use openshard_protocol::serial::Serial;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::wire::ClilocId;
use openshard_state::components::{Client, QuestLog};
use openshard_state::quest::{ObjectiveKind, QuestDef, QuestKey};
use openshard_state::{QuestGumpContext, QuestSection, WorldState};

/// The gump id the quest window answers under. Distinctive, so a reply is never
/// mistaken for the admin menu's or a pack dialog's.
pub const QUEST_GUMP: GumpId = id::QUEST;

/// The gump id the resign confirmation answers under.
pub const QUEST_RESIGN_GUMP: GumpId = id::QUEST_RESIGN;

/// Where the window opens. ServUO's `base(75, 25)`.
const WINDOW_X: u16 = 75;
const WINDOW_Y: u16 = 25;

/// The button ids, from ServUO's `Buttons` enum. A quest row in the log is
/// [`ROW_OFFSET`] plus its index.
pub(crate) mod button {
    use openshard_protocol::gump::ButtonId;

    /// Dismiss the window. The layout gives its own `X` the close box's id, so
    /// pressing it and dismissing the window are one answer — see
    /// [`ButtonId::CLOSE_BOX`].
    pub const CLOSE: ButtonId = ButtonId::CLOSE_BOX;
    /// Back to the quest log from a quest's detail page.
    pub const CLOSE_QUEST: ButtonId = ButtonId(1);
    /// Turn an offer down.
    pub const REFUSE_QUEST: ButtonId = ButtonId(2);
    /// Give up a quest already taken.
    pub const RESIGN_QUEST: ButtonId = ButtonId(3);
    /// Take an offered quest.
    pub const ACCEPT_QUEST: ButtonId = ButtonId(4);
    /// Take the reward for a finished quest.
    pub const ACCEPT_REWARD: ButtonId = ButtonId(5);
    /// Back a page.
    pub const PREVIOUS_PAGE: ButtonId = ButtonId(6);
    /// On a page.
    pub const NEXT_PAGE: ButtonId = ButtonId(7);
    /// Hand in a finished quest.
    pub const COMPLETE: ButtonId = ButtonId(8);
}

/// The first button id a quest row in the log uses. ServUO's `ButtonOffset`.
const ROW_OFFSET: u32 = 11;

/// A position in [`QuestLog::active`], never an objective index or an
/// arbitrary gump button number.
///
/// It crosses the layout/reply boundary encoded as a [`ButtonId`], so keeping
/// the position named on both sides is what makes [`row_button`] and
/// [`row_of`] one conversion rather than two casts that merely happen to
/// agree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct QuestLogIndex(usize);

impl QuestLogIndex {
    pub(crate) const fn new(position: usize) -> Self {
        Self(position)
    }

    pub(crate) const fn position(self) -> usize {
        self.0
    }
}

/// The button id the log gives the row at `index`.
///
/// The rows are the one part of this window whose ids are computed rather than
/// named, so the encoding gets both directions with names — [`row_of`] is the
/// inverse the reply reads, and neither side re-derives the arithmetic.
pub(crate) fn row_button(index: QuestLogIndex) -> ButtonId {
    let Some(id) = u32::try_from(index.position())
        .ok()
        .and_then(|index| ROW_OFFSET.checked_add(index))
    else {
        panic!("a quest-log index must fit in the gump button-id space");
    };
    ButtonId(id)
}

/// Which log row a button id names, or `None` for a button that is not a row.
pub(crate) fn row_of(button: ButtonId) -> Option<QuestLogIndex> {
    let position = button.0.checked_sub(ROW_OFFSET)?;
    usize::try_from(position).ok().map(QuestLogIndex)
}

/// The sounds a quest plays, from `BaseQuest`. All four are per-player: they are
/// feedback on a dialog one person is looking at.
pub(crate) mod sound {
    use openshard_protocol::wire::SoundId;

    /// A quest accepted.
    pub const ACCEPT: SoundId = SoundId(0x5B4);
    /// A quest resigned.
    pub const RESIGN: SoundId = SoundId(0x5B3);
    /// A quest finished.
    pub const COMPLETE: SoundId = SoundId(0x5B5);
    /// An objective moved.
    pub const UPDATE: SoundId = SoundId(0x5B6);
}

/// Clilocs the frame and the objective lines use, from `MondainQuestGump`.
mod cliloc {
    use openshard_protocol::wire::ClilocId;

    pub const QUEST_LOG: ClilocId = ClilocId(1_046_026);
    pub const QUEST_OFFER: ClilocId = ClilocId(1_049_010);
    pub const QUEST_CONVERSATION: ClilocId = ClilocId(3_006_156);
    pub const FAILED: ClilocId = ClilocId(500_039);
    pub const DESCRIPTION: ClilocId = ClilocId(1_072_202);
    pub const OBJECTIVE: ClilocId = ClilocId(1_049_073);
    pub const ALL_OF_THE_FOLLOWING: ClilocId = ClilocId(1_072_208);
    pub const ONLY_ONE_OF_THE_FOLLOWING: ClilocId = ClilocId(1_072_209);
    pub const REWARD: ClilocId = ClilocId(1_072_201);
    pub const SLAY: ClilocId = ClilocId(1_072_204);
    pub const OBTAIN: ClilocId = ClilocId(1_072_205);
    pub const ESCORT_TO: ClilocId = ClilocId(1_072_206);
    pub const DELIVER: ClilocId = ClilocId(1_072_207);
    pub const DELIVER_TO: ClilocId = ClilocId(1_072_379);
    pub const TOTAL: ClilocId = ClilocId(3_000_087);
    pub const TIME_REMAINING: ClilocId = ClilocId(1_062_379);
}

/// The label hue an objective's numbers are drawn in (ServUO's `0x481`).
const LABEL_HUE: u32 = 0x481;
/// The header colour, and the two body colours, as `MondainQuestGump` passes them.
const HEADER_WHITE: u32 = 0x00FF_FFFF;
const SECTION_GREEN: u32 = 0x2710;
const OBJECTIVE_GREEN: u32 = 0x0001_5F90;

/// Draw the quest window for a player, on whichever section the context names,
/// and remember what they are looking at so the reply can be read.
pub(crate) fn show(state: &mut WorldState, player: EntityId, context: QuestGumpContext) {
    let Some(&Client { connection, .. }) = state.registry.get::<Client>(player) else {
        return;
    };
    let Some(serial) = state.registry.serial_of(player) else {
        return;
    };
    let layout = build(state, player, &context);
    let (string, lines) = layout.finish();
    // Close the window already open before drawing the new one. The section chain
    // replaces itself, and a client told to draw twice draws two — ServUO closes
    // first in every branch of `OnResponse`, and this is that.
    let close = ServerPacket::CloseGump(CloseGump {
        gump_id: QUEST_GUMP,
        button: ButtonId::CLOSE_BOX,
    });
    let packet = ServerPacket::GumpDisplay(GumpDisplay {
        serial: GumpKey::on(serial),
        gump_id: QUEST_GUMP,
        at: GumpPoint::new(i32::from(WINDOW_X), i32::from(WINDOW_Y)),
        layout: string.to_owned(),
        lines: lines.to_vec(),
    });
    state.send_packet(connection, &close);
    state.send_packet(connection, &packet);
    if let Some(row) = state.row_of_mut(player) {
        row.quest_gump = Some(context);
    }
}

/// Build the layout for one section.
fn build(state: &WorldState, player: EntityId, context: &QuestGumpContext) -> GumpLayout {
    let mut layout = GumpLayout::new();
    // ServUO: `Closable = false` (the player must answer with a button) and
    // `Resizable = false`. Dragable and Disposable stay on, so no token for them.
    layout.no_close();
    layout.no_resize();
    layout.page(0);
    frame(&mut layout, context.quest.is_none());

    let quest = context.quest.as_ref().and_then(|key| state.quests.get(key));
    match context.section {
        QuestSection::Main => section_main(&mut layout, state, player),
        QuestSection::Description => {
            if let Some(quest) = quest {
                section_description(&mut layout, state, player, quest, context);
            }
        }
        QuestSection::Objectives => {
            if let Some(quest) = quest {
                section_objectives(&mut layout, state, player, quest, context);
            }
        }
        QuestSection::Rewards => {
            if let Some(quest) = quest {
                section_rewards(&mut layout, quest, context);
            }
        }
        QuestSection::Refuse => {
            if let Some(quest) = quest {
                conversation(&mut layout, &quest.title, &quest.refuse, false);
            }
        }
        QuestSection::InProgress => {
            if let Some(quest) = quest {
                conversation(&mut layout, &quest.title, &quest.uncomplete, false);
            }
        }
        QuestSection::Complete => {
            if let Some(quest) = quest {
                conversation(&mut layout, &quest.title, &quest.complete, true);
            }
        }
        QuestSection::Failed => {
            if let Some(quest) = quest {
                let text = if quest.failed.is_empty() {
                    "You have failed to meet the conditions of the quest."
                } else {
                    &quest.failed
                };
                conversation(&mut layout, &quest.title, text, false);
            }
        }
    }
    layout
}

/// The scroll-and-parchment frame every section is drawn inside.
///
/// Every id here is ServUO's, in ServUO's order. It is decoration and nothing
/// reads it, which is exactly why it has to be copied rather than approximated:
/// there is no way to tell a subtly wrong frame from a right one except by
/// looking at it in a client.
fn frame(layout: &mut GumpLayout, log_page: bool) {
    layout.image_tiled(50, 20, 400, 460, 0x1404);
    layout.image_tiled(50, 29, 30, 450, 0x28DC);
    layout.image_tiled(34, 140, 17, 339, 0x242F);
    layout.image(48, 135, 0x28AB);
    layout.image(-16, 285, 0x28A2);
    layout.image(0, 10, 0x28B5);
    layout.image(25, 0, 0x28B4);
    layout.image_tiled(83, 15, 350, 15, 0x280A);
    layout.image(34, 479, 0x2842);
    layout.image(442, 479, 0x2840);
    layout.image_tiled(51, 479, 392, 17, 0x2775);
    layout.image_tiled(415, 29, 44, 450, 0x0A2D);
    layout.image_tiled(415, 29, 30, 450, 0x28DC);
    layout.image(370, 50, 0x0589);
    // The seal: one picture on the log page, another on a single quest. ServUO
    // makes this a live button for staff (a force-complete); that is deferred
    // with the rest of the GM quest tooling.
    layout.image(379, 60, if log_page { 0x15A9 } else { 0x1580 });
    layout.image(425, 0, 0x28C9);
    layout.image(90, 33, 0x232D);
    layout.image_tiled(130, 65, 175, 1, 0x238D);
}

/// The quest log: one row per quest in progress, newest first.
fn section_main(layout: &mut GumpLayout, state: &WorldState, player: EntityId) {
    layout.html_localized_colored(130, 45, 270, 16, cliloc::QUEST_LOG, HEADER_WHITE, false, false);

    let Some(log) = state.registry.get::<QuestLog>(player) else {
        layout.button(313, 455, 0x2EEC, 0x2EEE, GumpButton::Reply, 0, button::CLOSE);
        return;
    };
    let mut offset = 140;
    // Newest first, the way ServUO walks the list backwards — but the *button id*
    // stays the quest's own index, so a row always names the same quest however it
    // is displayed.
    for index in (0..log.active.len()).rev() {
        let entry = &log.active[index];
        let title = state
            .quests
            .get(&entry.key)
            .map_or(entry.key.as_str(), |quest| quest.title.as_str());
        let colour = if entry.failed { GUMP_RED } else { GUMP_WHITE };
        layout.html_colored(98, offset, 270, 21, title, colour, false, false);
        layout.button(
            368,
            offset,
            0x26B0,
            0x26B1,
            GumpButton::Reply,
            0,
            row_button(QuestLogIndex::new(index)),
        );
        offset += 21;
    }
    layout.button(313, 455, 0x2EEC, 0x2EEE, GumpButton::Reply, 0, button::CLOSE);
}

/// The quest's prose — the offer's first page, and the log's detail page.
fn section_description(
    layout: &mut GumpLayout,
    state: &WorldState,
    player: EntityId,
    quest: &QuestDef,
    context: &QuestGumpContext,
) {
    header(layout, context.offer);
    if failed(state, player, &quest.key) {
        layout.html_localized_colored(160, 80, 200, 32, cliloc::FAILED, GUMP_RED, false, false);
    }
    layout.html_colored(160, 70, 200, 40, &quest.title, GUMP_DARK_GREEN, false, false);
    layout.html_localized_colored(98, 140, 312, 16, cliloc::DESCRIPTION, SECTION_GREEN, false, false);
    layout.html_colored(
        98,
        156,
        312,
        180,
        &quest.description,
        GUMP_LIGHT_GREEN,
        false,
        true,
    );
    accept_or_resign(layout, context.offer);
    layout.button(275, 430, 0x2EE9, 0x2EEB, GumpButton::Reply, 0, button::NEXT_PAGE);
}

/// What the quest asks for, with progress on each line.
fn section_objectives(
    layout: &mut GumpLayout,
    state: &WorldState,
    player: EntityId,
    quest: &QuestDef,
    context: &QuestGumpContext,
) {
    accept_or_resign(layout, context.offer);
    header(layout, context.offer);
    layout.html_colored(160, 70, 200, 40, &quest.title, GUMP_DARK_GREEN, false, false);
    layout.html_localized_colored(98, 140, 312, 16, cliloc::OBJECTIVE, SECTION_GREEN, false, false);
    let rule = if quest.all_objectives {
        cliloc::ALL_OF_THE_FOLLOWING
    } else {
        cliloc::ONLY_ONE_OF_THE_FOLLOWING
    };
    layout.html_localized_colored(98, 156, 312, 16, rule, SECTION_GREEN, false, false);

    let progress = state
        .registry
        .get::<QuestLog>(player)
        .and_then(|log| log.active_quest(&quest.key))
        .map(|entry| (entry.progress.clone(), entry.seconds_left.clone()));
    let mut offset = 172;
    for (index, objective) in quest.objectives.iter().enumerate() {
        let done = progress
            .as_ref()
            .and_then(|(counts, _)| counts.get(index).copied())
            .unwrap_or(0);
        let left = progress
            .as_ref()
            .and_then(|(_, seconds)| seconds.get(index).copied())
            .unwrap_or(objective.seconds);
        match &objective.kind {
            ObjectiveKind::Slay { .. } => {
                layout.html_localized_colored(
                    98,
                    offset,
                    30,
                    16,
                    cliloc::SLAY,
                    OBJECTIVE_GREEN,
                    false,
                    false,
                );
                layout.label(
                    133,
                    offset,
                    LABEL_HUE,
                    format!("{} {}", objective.count, objective.name),
                );
                offset += 16;
            }
            ObjectiveKind::Obtain { .. } => {
                layout.html_localized_colored(
                    98,
                    offset,
                    40,
                    16,
                    cliloc::OBTAIN,
                    OBJECTIVE_GREEN,
                    false,
                    false,
                );
                layout.label(
                    143,
                    offset,
                    LABEL_HUE,
                    format!("{} {}", objective.count, objective.name),
                );
                offset += 16;
            }
            ObjectiveKind::Deliver { to, .. } => {
                layout.html_localized_colored(
                    98,
                    offset,
                    40,
                    16,
                    cliloc::DELIVER,
                    OBJECTIVE_GREEN,
                    false,
                    false,
                );
                layout.label(
                    143,
                    offset,
                    LABEL_HUE,
                    format!("{} {}", objective.count, objective.name),
                );
                offset += 16;
                layout.html_localized_colored(
                    103,
                    offset,
                    120,
                    16,
                    cliloc::DELIVER_TO,
                    OBJECTIVE_GREEN,
                    false,
                    false,
                );
                layout.label(223, offset, LABEL_HUE, to.clone());
                offset += 16;
            }
            ObjectiveKind::Escort { region } => {
                layout.html_localized_colored(
                    98,
                    offset,
                    312,
                    16,
                    cliloc::ESCORT_TO,
                    OBJECTIVE_GREEN,
                    false,
                    false,
                );
                // An objective with no region of its own takes the giver's, and
                // that is only chosen when the quest is accepted — so the offer
                // says "a destination" and the log says where.
                let named = if region.is_empty() {
                    context
                        .giver
                        .and_then(|giver| crate::log::escort_destination(state, giver))
                        .unwrap_or_else(|| "a destination".to_owned())
                } else {
                    region.clone()
                };
                layout.html_colored(173, offset, 200, 16, &named, GUMP_WHITE, false, false);
                offset += 16;
            }
        }
        // An offer shows what is being asked; the log shows how far it has got.
        // Escort has no count worth showing either way — you are there or you are
        // not — which is ServUO's rendering too.
        if !context.offer && !matches!(objective.kind, ObjectiveKind::Escort { .. }) {
            layout.html_localized_colored(103, offset, 120, 16, cliloc::TOTAL, OBJECTIVE_GREEN, false, false);
            layout.label(223, offset, LABEL_HUE, done.to_string());
            offset += 16;
        }
        if objective.timed() {
            layout.html_localized_colored(
                103,
                offset,
                120,
                16,
                cliloc::TIME_REMAINING,
                OBJECTIVE_GREEN,
                false,
                false,
            );
            layout.label(223, offset, LABEL_HUE, format_seconds(left));
            offset += 16;
        }
    }
    layout.button(
        130,
        430,
        0x2EEF,
        0x2EF1,
        GumpButton::Reply,
        0,
        button::PREVIOUS_PAGE,
    );
    layout.button(275, 430, 0x2EE9, 0x2EEB, GumpButton::Reply, 0, button::NEXT_PAGE);
}

/// What the quest pays.
fn section_rewards(layout: &mut GumpLayout, quest: &QuestDef, context: &QuestGumpContext) {
    header(layout, context.offer);
    layout.html_colored(160, 70, 200, 40, &quest.title, GUMP_DARK_GREEN, false, false);
    layout.html_localized_colored(98, 140, 312, 16, cliloc::REWARD, SECTION_GREEN, false, false);

    let mut offset = 163;
    let height = if quest.rewards.len() == 1 { 100 } else { 16 };
    for reward in &quest.rewards {
        layout.image(105, offset, 0x04B9);
        layout.html_colored(
            133,
            offset,
            280,
            height,
            &reward.name,
            GUMP_LIGHT_GREEN,
            false,
            false,
        );
        offset += 16;
    }

    if context.completed {
        layout.button(
            95,
            455,
            0x2EE0,
            0x2EE2,
            GumpButton::Reply,
            0,
            button::ACCEPT_REWARD,
        );
        layout.button(313, 455, 0x2EE6, 0x2EE8, GumpButton::Reply, 0, button::CLOSE);
    } else {
        accept_or_resign(layout, context.offer);
        layout.button(
            130,
            430,
            0x2EEF,
            0x2EF1,
            GumpButton::Reply,
            0,
            button::PREVIOUS_PAGE,
        );
    }
}

/// The giver talking: a refusal, a nudge, a thank-you. `hand_in` adds the button
/// that finishes the quest.
fn conversation(layout: &mut GumpLayout, title: &str, text: &str, hand_in: bool) {
    layout.html_localized_colored(
        130,
        45,
        270,
        16,
        cliloc::QUEST_CONVERSATION,
        HEADER_WHITE,
        false,
        false,
    );
    layout.image(140, 110, 0x04B9);
    layout.html_colored(160, 70, 200, 40, title, GUMP_DARK_GREEN, false, false);
    layout.html_colored(98, 140, 312, 180, text, GUMP_LIGHT_GREEN, false, true);
    layout.button(313, 455, 0x2EE6, 0x2EE8, GumpButton::Reply, 0, button::CLOSE);
    if hand_in {
        layout.button(95, 455, 0x2EE9, 0x2EEB, GumpButton::Reply, 0, button::COMPLETE);
    }
}

/// "Quest Offer" or "Quest Log", depending on which side of accepting we are on.
fn header(layout: &mut GumpLayout, offer: bool) {
    let cliloc = if offer {
        cliloc::QUEST_OFFER
    } else {
        cliloc::QUEST_LOG
    };
    layout.html_localized_colored(130, 45, 270, 16, cliloc, HEADER_WHITE, false, false);
}

/// The pair of buttons at the foot of a quest page: Accept/Refuse while it is
/// being offered, Resign/Close once it has been taken.
fn accept_or_resign(layout: &mut GumpLayout, offer: bool) {
    if offer {
        layout.button(
            95,
            455,
            0x2EE0,
            0x2EE2,
            GumpButton::Reply,
            0,
            button::ACCEPT_QUEST,
        );
        layout.button(
            313,
            455,
            0x2EF2,
            0x2EF4,
            GumpButton::Reply,
            0,
            button::REFUSE_QUEST,
        );
    } else {
        layout.button(
            95,
            455,
            0x2EF5,
            0x2EF7,
            GumpButton::Reply,
            0,
            button::RESIGN_QUEST,
        );
        layout.button(
            313,
            455,
            0x2EEC,
            0x2EEE,
            GumpButton::Reply,
            0,
            button::CLOSE_QUEST,
        );
    }
}

/// Whether a player's copy of a quest has failed.
fn failed(state: &WorldState, player: EntityId, key: &QuestKey) -> bool {
    state
        .registry
        .get::<QuestLog>(player)
        .and_then(|log| log.active_quest(key))
        .is_some_and(|entry| entry.failed)
}

/// `h:m:s`, `m:s` or `s` — ServUO's `FormatSeconds`, which drops the empty
/// leading units rather than padding them.
fn format_seconds(seconds: u32) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes}:{seconds}")
    } else if minutes > 0 {
        format!("{minutes}:{seconds}")
    } else {
        seconds.to_string()
    }
}

/// Ask a player to confirm giving up a quest — ServUO's `MondainResignGump`.
///
/// A radio pair and one OK button: the choice rides in the switch, not the
/// button, which is why the reply has to carry its switches at all.
pub(crate) fn show_resign(state: &mut WorldState, player: EntityId, key: &QuestKey) {
    let Some(&Client { connection, .. }) = state.registry.get::<Client>(player) else {
        return;
    };
    let Some(serial) = state.registry.serial_of(player) else {
        return;
    };
    let title = state
        .quests
        .get(key)
        .map_or_else(|| key.as_str(), |quest| quest.title.as_str())
        .to_owned();

    let mut layout = GumpLayout::new();
    layout.no_close();
    layout.page(0);
    layout.background(0, 0, 320, 240, 0x13BE);
    layout.image_tiled(10, 10, 300, 20, 0xA40);
    layout.image_tiled(10, 40, 300, 160, 0xA40);
    layout.image_tiled(10, 210, 300, 20, 0xA40);
    layout.alpha_region(10, 10, 300, 220);
    // 1049005 "Yes, I want to quit this quest." / 1049006 "No, I want to continue."
    layout.html_localized_colored(14, 12, 300, 20, ClilocId(1_049_000), GUMP_WHITE, false, false);
    layout.html_colored(40, 45, 260, 40, &title, GUMP_DARK_GREEN, false, false);
    layout.radio(45, 100, 0x25F8, 0x25FB, true, RESIGN_SWITCH_YES);
    layout.html_localized_colored(80, 100, 200, 20, ClilocId(1_049_005), GUMP_WHITE, false, false);
    layout.radio(45, 130, 0x25F8, 0x25FB, false, RESIGN_SWITCH_NO);
    layout.html_localized_colored(80, 130, 200, 20, ClilocId(1_049_006), GUMP_WHITE, false, false);
    layout.button(265, 220, 0x00F7, 0x00F8, GumpButton::Reply, 0, RESIGN_OK);

    let (string, lines) = layout.finish();
    let packet = ServerPacket::GumpDisplay(GumpDisplay {
        serial: GumpKey::on(serial),
        gump_id: QUEST_RESIGN_GUMP,
        at: GumpPoint::new(120, 50),
        layout: string.to_owned(),
        lines: lines.to_vec(),
    });
    state.send_packet(connection, &packet);
}

/// The switch id meaning "yes, give it up" in the resign dialog.
pub(crate) const RESIGN_SWITCH_YES: SwitchId = SwitchId(1);
/// The switch id meaning "no, keep it".
pub(crate) const RESIGN_SWITCH_NO: SwitchId = SwitchId(0);
/// How many switches the resign dialog draws — the domain a reply's switch id
/// is checked against.
pub(crate) const RESIGN_SWITCHES: usize = 2;
/// The one reply button on the resign dialog.
pub(crate) const RESIGN_OK: ButtonId = ButtonId(1);

/// Play one of the quest sounds for a player.
pub(crate) fn play(state: &mut WorldState, player: EntityId, sound: openshard_protocol::wire::SoundId) {
    state.play_sound_to(player, sound);
}

/// The context for a page of a quest a player already has.
pub(crate) fn log_context(key: &QuestKey, section: QuestSection, giver: Option<Serial>) -> QuestGumpContext {
    QuestGumpContext {
        quest: Some(key.clone()),
        section,
        offer: false,
        completed: false,
        giver,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quest_log_index_round_trips_through_its_button_id() {
        for position in [0, 1, crate::QUEST_LIMIT - 1] {
            let index = QuestLogIndex::new(position);
            assert_eq!(row_of(row_button(index)), Some(index));
        }
        assert_eq!(row_of(button::NEXT_PAGE), None);
    }

    #[test]
    #[should_panic(expected = "a quest-log index must fit in the gump button-id space")]
    fn quest_log_index_does_not_silently_truncate_to_a_button_id() {
        let _ = row_button(QuestLogIndex::new(usize::MAX));
    }

    #[test]
    fn seconds_drop_the_units_they_do_not_need() {
        assert_eq!(format_seconds(45), "45");
        assert_eq!(format_seconds(125), "2:5");
        assert_eq!(format_seconds(3725), "1:2:5");
    }
}
