//! What a paperdoll's buttons send.
//!
//! [`interact`](crate::interact) is what this client does to an *object*; this
//! is what it does about itself. The two are apart because nothing here names a
//! thing in the world: pressing War is a stance, pressing Log Out is a
//! departure, and the four requests below are windows being asked for.
//!
//! # The frame is drawn here and answered nowhere
//!
//! A paperdoll has no server-sent layout — the client draws all of it, out of
//! `gumpart` — so none of its buttons is a `0xB1` reply the way a shard's dialog
//! is. Each one is its own packet, and which packet is the whole content of this
//! module: `0x72` for the toggle, `0xD1` for Log Out, `0x34` for the status bar
//! and the skill list, `0xD7` for Quests and Guild. The one exception is the
//! virtue menu, which *is* a `0xB1` — see [`virtue`], and it is a convention
//! rather than a window.
//!
//! # What is not here, and why
//!
//! Help (`0x9B`), Profile (`0xB8`) and the party manifest (`0xBF 0x06`) have no
//! packet in `openshard_protocol` yet, and Options is a window of the client's
//! own that does not exist. Those four buttons press and send nothing, which is
//! written down in `docs/client.md` rather than papered over with a packet the
//! shard would log as unknown.

use openshard_protocol::combat::WarMode;
use openshard_protocol::encoded::RawEncodedSerial;
use openshard_protocol::gump::{
    RawButtonId,
    RawGumpId,
    RawGumpKey,
    RawSwitchId,
};
use openshard_protocol::mobile::{
    StatusQuery,
    StatusQueryKind,
};
use openshard_protocol::serial::{
    RawSerial,
    Serial,
};
use openshard_protocol::world::LogoutRequest;

/// Ask to enter or leave war mode: the `0x72` to write to the socket.
///
/// `war` is the stance being asked *for*, not the one being reported: the
/// server answers on the same id with the stance that settled, and that answer
/// is what the toggle's picture is drawn from — see
/// [`WorldView::player`](crate::view::WorldView::player)'s `war`. A client that
/// flipped its own picture on the press would show a war stance a shard refused.
#[must_use]
pub fn war_mode(war: bool) -> Vec<u8> {
    WarMode { war }.encode()
}

/// "Log Out" was pressed: the `0xD1` to write to the socket.
///
/// A notification and not a request in any sense the client can act on alone —
/// the connection stays up until the server answers with its own `0xD1`, which
/// is what the reference waits for before leaving the world.
#[must_use]
pub fn log_out() -> Vec<u8> {
    LogoutRequest.encode()
}

/// Ask for a mobile's status bar: the `0x34` to write to the socket.
///
/// The serial is written because the packet carries one; this shard answers
/// about the connection's own character whatever it says (see
/// [`StatusQuery::serial`]), so a stranger's doll is not what this is for yet.
#[must_use]
pub fn status(mobile: Serial) -> Vec<u8> {
    query(mobile, StatusQueryKind::Status)
}

/// Ask for a mobile's skill list — the same `0x34` with the other type byte.
#[must_use]
pub fn skills(mobile: Serial) -> Vec<u8> {
    query(mobile, StatusQueryKind::Skills)
}

/// The one packet both of the above are.
fn query(mobile: Serial, kind: StatusQueryKind) -> Vec<u8> {
    StatusQuery {
        kind,
        serial: RawSerial(mobile.raw()),
    }
    .encode()
}

/// Ask for the quest log: the `0xD7` subcommand `0x32`.
///
/// `player` is our own serial, which is what the reference writes and what this
/// shard ignores — the connection already says whose quest log is meant.
#[must_use]
pub fn quest_log(player: Serial) -> Vec<u8> {
    openshard_protocol::encoded::quest_log_request(RawEncodedSerial(player.raw()))
}

/// Ask for the guild menu — the `0xD7` beside it, subcommand `0x28`.
#[must_use]
pub fn guild_menu(player: Serial) -> Vec<u8> {
    openshard_protocol::encoded::guild_menu_request(RawEncodedSerial(player.raw()))
}

/// The gump id the virtue menu is asked for under.
///
/// `PaperDollGump`'s `VirtueMenu_MouseDoubleClickEvent` replies to a window
/// nobody opened: `ReplyGump(World.Player, 0x000001CD, 1, [LocalSerial], [])`.
/// The number is a convention both references share — ServUO's
/// `VirtueGump.Initialize` registers exactly this id — and it is the only place
/// in this client where a `0xB1` is sent for a dialog the shard never displayed.
const VIRTUE_GUMP: u32 = 0x0000_01CD;

/// The button the virtue scroll presses on that imaginary window.
const VIRTUE_BUTTON: u32 = 0x0000_0001;

/// Double-clicking the virtue scroll: the `0xB1` to write to the socket.
///
/// `player` is the key the reply is filed under — the reference uses the
/// player's own serial, which is what a real shard matches on — and `subject` is
/// whose virtues are being asked about, carried as the reply's one switch.
///
/// This engine has no virtue system. The reply is not dropped for that: an
/// unrecognised `0xB1` reaches the script pack as a `GumpAnswered`
/// (`tick/staff.rs`), so a Community Pack that draws a virtue menu gets the
/// press without a line of engine code changing.
#[must_use]
pub fn virtue(player: Serial, subject: Serial) -> Vec<u8> {
    crate::talk::answer_gump(
        RawGumpKey(player.raw()),
        RawGumpId(VIRTUE_GUMP),
        RawButtonId(VIRTUE_BUTTON),
        vec![RawSwitchId(subject.raw())],
        Vec::new(),
    )
}

#[cfg(test)]
mod tests {
    use openshard_protocol::client_packet::ClientPacket;
    use openshard_protocol::encoded::EncodedSubcommand;
    use openshard_protocol::version::ClientVersion;

    use super::*;

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    fn player() -> Serial {
        Serial::new(0x0000_002A).unwrap()
    }

    /// The test every function here exists for, and the reason they are not
    /// written at their call sites: what this client puts on the wire is what
    /// this server's own dispatch decodes, through the same `ClientPacket` a
    /// real connection is routed by. A button whose packet decoded as
    /// `Unknown` would look, from the window, exactly like a button that works.
    #[test]
    fn every_button_reaches_the_server_as_the_packet_it_means() {
        let heard = |bytes: &[u8]| ClientPacket::decode(bytes, version()).expect("it decodes");

        assert!(matches!(
            heard(&war_mode(true)),
            ClientPacket::WarMode(mode) if mode.war
        ));
        assert!(matches!(
            heard(&war_mode(false)),
            ClientPacket::WarMode(mode) if !mode.war
        ));
        assert!(matches!(heard(&log_out()), ClientPacket::LogoutRequest));
        assert!(matches!(
            heard(&status(player())),
            ClientPacket::StatusQuery(query) if query.kind == StatusQueryKind::Status
        ));
        assert!(matches!(
            heard(&skills(player())),
            ClientPacket::StatusQuery(query) if query.kind == StatusQueryKind::Skills
        ));
        assert!(matches!(
            heard(&quest_log(player())),
            ClientPacket::Encoded(command)
                if command.subcommand.interpret() == EncodedSubcommand::QuestGumpRequest
        ));
        assert!(matches!(
            heard(&guild_menu(player())),
            ClientPacket::Encoded(command)
                if command.subcommand.interpret() == EncodedSubcommand::GuildGumpRequest
        ));
    }

    /// The virtue scroll, which is the one button whose packet is a reply to a
    /// window that was never opened: the id and the switch are what a shard
    /// matches on, so both are asserted rather than only the packet kind.
    #[test]
    fn the_virtue_scroll_replies_under_the_id_both_references_use() {
        let stranger = Serial::new(0x0000_00FF).unwrap();
        let ClientPacket::GumpResponse(reply) =
            ClientPacket::decode(&virtue(player(), stranger), version()).expect("0xB1 decodes")
        else {
            panic!("the virtue reply decoded as some other packet");
        };
        assert_eq!(reply.gump_id, RawGumpId(VIRTUE_GUMP));
        assert_eq!(reply.button, RawButtonId(VIRTUE_BUTTON));
        assert_eq!(
            reply.switches,
            vec![RawSwitchId(stranger.raw())],
            "whose virtues, which is not whose reply it is"
        );
    }
}
