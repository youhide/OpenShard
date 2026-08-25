//! Intentions a client sends after it has entered the world.
//!
//! The app names an action; this module owns the correspondence between that
//! action and the protocol encoder.  A socket driver therefore has one entry
//! point for ordinary outgoing traffic, while walking remains separate because
//! its sequence and prediction state are owned by [`crate::walk::Walk`].

use openshard_protocol::casting::SpellId;
use openshard_protocol::gump::GumpPoint;
use openshard_protocol::gump::{RawButtonId, RawGumpId, RawGumpKey, RawSwitchId};
use openshard_protocol::items::ItemAmount;
use openshard_protocol::mapedit::MapEditRequest;
use openshard_protocol::serial::Serial;
use openshard_protocol::skill::SkillLock;
use openshard_protocol::speech::TalkMode;
use openshard_protocol::target::TargetResponse;
use openshard_protocol::version::ClientVersion;
use openshard_protocol::wire::{RawLayer, RawSkillId};
use openshard_protocol::world::Point;

/// A reply to a shard-owned gump.
#[derive(Clone, Debug)]
pub struct GumpReply {
    pub key: RawGumpKey,
    pub gump_id: RawGumpId,
    pub button: RawButtonId,
    pub switches: Vec<RawSwitchId>,
    pub text_entries: Vec<(u16, String)>,
}

/// An outgoing action that has no walk-handshake state of its own.
#[derive(Clone, Debug)]
pub enum Outgoing {
    /// A line of speech, and which channel it goes on.
    ///
    /// The mode is a field rather than a constant because it decides *who
    /// hears it*: `Guild` and `Alliance` are routed by membership on the shard
    /// and never reach anybody by earshot.
    Say {
        /// What was typed.
        text: String,
        /// Which channel.
        mode: TalkMode,
    },
    AnswerGump(GumpReply),
    Use(Serial),
    Paperdoll(Serial),
    Target(TargetResponse),
    PickUp {
        item: Serial,
        amount: ItemAmount,
    },
    DropInto {
        item: Serial,
        container: Serial,
        at: GumpPoint,
    },
    DropOnGround {
        item: Serial,
        at: Point,
    },
    Equip {
        item: Serial,
        layer: RawLayer,
        mobile: Serial,
    },
    Buy {
        vendor: Serial,
        purchases: Vec<(Serial, u16)>,
    },
    Sell {
        vendor: Serial,
        sales: Vec<(Serial, u16)>,
    },
    WarMode(bool),
    Attack(Serial),
    StopAttacking,
    LogOut,
    Status(Serial),
    Skills(Serial),
    QuestLog,
    GuildMenu,
    Virtue(Serial),
    SkillLock {
        skill: RawSkillId,
        lock: SkillLock,
    },
    UseSkill(RawSkillId),
    /// Cast a spell selected from a spellbook this client opened.
    CastSpell {
        spellbook: Serial,
        spell: SpellId,
    },
    /// Ask for the tooltips of these objects, in one `0xD6`. Driven by the
    /// hover — see [`crate::properties`] for why not by everything on screen.
    QueryProperties(Vec<Serial>),
    /// A line to the party. Not speech: `0xBF 0x06`, and it reaches nobody by
    /// earshot — see [`crate::party`].
    PartySay(String),
    /// Ask the shard to raise a cursor for adding somebody to the party.
    PartyAdd,
    /// Accept the invitation this client is holding.
    PartyAccept,
    /// Decline it.
    PartyDecline,
    /// Leave the party, or turn out the member named.
    PartyRemove(Serial),
    /// Ask for a designed house's picture, in a `0xBF 0x1E`.
    ///
    /// Sent only when this client holds no shape for the revision the shard
    /// named — a design is a few hundred tiles, and the whole point of the
    /// revision riding with the draw is that the ask is rare.
    QueryDesign(Serial),
    /// Commit one bounded editor draft against the revision it was built from.
    ///
    /// The request deliberately has no author or authority field: the shard
    /// derives both from this established session before it accepts anything.
    CommitMapEdit(MapEditRequest),
}

impl Outgoing {
    /// Encode this action for `player`, whose identity only the established
    /// session knows. Quest, guild, and virtue requests use it on the wire.
    #[must_use]
    pub fn encode(self, player: Serial, version: ClientVersion) -> Vec<u8> {
        match self {
            Self::Say { text, mode } => crate::talk::say(&text, mode),
            Self::AnswerGump(reply) => crate::talk::answer_gump(
                reply.key,
                reply.gump_id,
                reply.button,
                reply.switches,
                reply.text_entries,
            ),
            Self::Use(serial) => crate::interact::use_object(serial),
            Self::Paperdoll(serial) => crate::interact::paperdoll(serial),
            Self::Target(response) => crate::target::answer(response),
            Self::PickUp { item, amount } => crate::drag::pick_up(item, amount),
            Self::DropInto { item, container, at } => crate::drag::drop_into(item, container, at, version),
            Self::DropOnGround { item, at } => crate::drag::drop_on_ground(item, at, version),
            Self::Equip { item, layer, mobile } => crate::drag::equip(item, layer, mobile),
            Self::Buy { vendor, purchases } => crate::vendor::buy(vendor, &purchases),
            Self::Sell { vendor, sales } => crate::vendor::sell(vendor, &sales),
            Self::WarMode(war) => crate::doll::war_mode(war),
            Self::Attack(mobile) => crate::combat::attack(mobile),
            Self::StopAttacking => crate::combat::stop_attacking(),
            Self::LogOut => crate::doll::log_out(),
            Self::Status(mobile) => crate::doll::status(mobile),
            Self::Skills(mobile) => crate::doll::skills(mobile),
            Self::QuestLog => crate::doll::quest_log(player),
            Self::GuildMenu => crate::doll::guild_menu(player),
            Self::Virtue(mobile) => crate::doll::virtue(player, mobile),
            Self::SkillLock { skill, lock } => crate::skill::set_lock(skill, lock),
            Self::UseSkill(skill) => crate::skill::use_skill(skill),
            Self::CastSpell { spellbook, spell } => crate::casting::cast(spellbook, spell),
            Self::QueryProperties(serials) => crate::properties::query(&serials, version),
            Self::PartySay(text) => crate::party::say(&text),
            Self::PartyAdd => crate::party::add(),
            Self::PartyAccept => crate::party::accept(),
            Self::PartyDecline => crate::party::decline(),
            Self::PartyRemove(member) => crate::party::remove(member),
            Self::QueryDesign(house) => openshard_protocol::design::DesignDetailsRequest {
                serial: openshard_protocol::serial::RawSerial(house.raw()),
            }
            .encode(),
            Self::CommitMapEdit(request) => request.encode(),
        }
    }
}

#[cfg(test)]
mod tests {
    use openshard_protocol::chunks::WorldRevision;
    use openshard_protocol::extended::ExtendedRequest;
    use openshard_protocol::mapedit::{EditLandTile, EditTile, EditX, EditY, EditZ, MapEditOp};
    use openshard_protocol::serial::Serial;
    use openshard_protocol::world::Facet;

    use super::*;

    #[test]
    fn a_world_action_uses_the_encoder_that_owns_its_packet() {
        let player = Serial::new(0x0000_002A).unwrap();
        let target = Serial::new(0x0000_002B).unwrap();
        assert_eq!(
            Outgoing::Attack(target).encode(player, ClientVersion::new(7, 0, 45, 65)),
            crate::combat::attack(target)
        );
        assert_eq!(
            Outgoing::StopAttacking.encode(player, ClientVersion::new(7, 0, 45, 65)),
            crate::combat::stop_attacking()
        );
        assert_eq!(
            Outgoing::QuestLog.encode(player, ClientVersion::new(7, 0, 45, 65)),
            crate::doll::quest_log(player)
        );
    }

    #[test]
    fn a_map_edit_action_survives_the_extended_envelope() {
        let request = MapEditRequest {
            facet: Facet(0),
            parent: WorldRevision(12),
            ops: vec![MapEditOp::SetLand {
                at: EditTile {
                    x: EditX(100),
                    y: EditY(200),
                },
                tile: EditLandTile::from_wire(9).expect("a land tile"),
                z: EditZ(-3),
            }],
        };
        let bytes = Outgoing::CommitMapEdit(request.clone())
            .encode(Serial::new(0x0000_002A).unwrap(), ClientVersion::TOL);
        assert_eq!(
            ExtendedRequest::decode(&bytes).expect("the typed action encoded one 0xBF"),
            ExtendedRequest::MapEdit(request)
        );
    }
}
