//! The login conversation, from the client's chair.
//!
//! Eight packets and one reconnect stand between a socket and a body in the
//! world, and the order is not negotiable: a client that answers the character
//! list before it has been sent one is a client that hangs. This is that order,
//! as a state machine with no socket in it, so the whole conversation can be
//! driven — and got wrong — in a unit test.
//!
//! # The two sockets, and the key between them
//!
//! The login connection ends at the `0x8C` relay. What follows is a *new*
//! connection to whatever address the relay named, and the only thing tying the
//! two together is the auth key it carried. Nothing on the game connection says
//! who this is; the server matches the key. Lose it and the second socket is
//! anonymous.
//!
//! The game connection is also where compression starts — see
//! [`Stream`](crate::connection::Stream).

use std::net::SocketAddrV4;

use openshard_protocol::identity::{
    RawAccountName,
    RawCharacterName,
    RawPlaintextPassword,
};
use openshard_protocol::login::{
    AccountLogin,
    CharacterList,
    DenyReason,
    GameServerLogin,
    RawShardIndex,
    SelectShard,
    ShardList,
};
use openshard_protocol::seed::{
    RawSeedValue,
    Seed,
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::version::ClientVersion;
use openshard_protocol::wire::{
    AuthKey,
    RawCharacterSlot,
    RawClientIp,
};
use openshard_protocol::world::{
    CharacterPlay,
    PlayerStart,
};

use crate::connection::PacketId;

/// Which of several offered things to take.
///
/// A client picks a shard and a character, and both lists arrive from the
/// server — so both picks are "the one called X" or "whatever is first". A bare
/// index would be the third way and the worst: the wire's own numbering is
/// one-based for shards and zero-based for characters, which is precisely the
/// kind of detail a caller should not be holding.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Pick {
    /// The first entry the server offers. For characters, the first occupied
    /// slot: the list is padded with empty ones.
    First,
    /// The entry with this name, compared case-insensitively.
    Named(String),
}

impl Pick {
    /// Find it, among names in the order the server listed them.
    fn find<'a>(&self, names: impl Iterator<Item = &'a str>) -> Option<usize> {
        match self {
            Self::First => {
                names
                    .enumerate()
                    .find(|(_, name)| !name.is_empty())
                    .map(|(i, _)| i)
            }
            Self::Named(wanted) => {
                names
                    .enumerate()
                    .find(|(_, name)| name.eq_ignore_ascii_case(wanted))
                    .map(|(i, _)| i)
            }
        }
    }
}

/// What the client is trying to do.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Plan {
    /// The account name.
    pub account:   RawAccountName,
    /// The password, plaintext — the protocol has no other kind.
    pub password:  RawPlaintextPassword,
    /// Which shard to take from the `0xA8` list.
    pub shard:     Pick,
    /// Which character to play from the `0xA9` list.
    pub character: Pick,
}

/// Where the conversation has got to.
///
/// Public because a caller drives two sockets from it: the state says which
/// socket the next packet belongs on, and there is no other way to know.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Stage {
    /// The account login has gone out; waiting for the shard list.
    AwaitingShards,
    /// A shard has been picked; waiting for the relay.
    AwaitingRelay,
    /// The login socket is done. The game socket has not been opened.
    Relayed,
    /// The game login has gone out; waiting for the character list.
    AwaitingCharacters,
    /// A character has been picked; waiting to be put in the world.
    AwaitingStart,
    /// The body is in the world; waiting for permission to draw.
    AwaitingCompletion,
    /// In the world, drawing.
    Playing,
    /// The server refused, and said why. Nothing further will arrive.
    Refused(DenyReason),
}

/// What the caller must do about a packet.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Step {
    /// Nothing to do — a packet that moves nothing, or one this stage ignores.
    Idle,
    /// Write these bytes to the socket the packet came from.
    Send(Vec<u8>),
    /// The login socket is finished. Open a game connection to `endpoint`, in
    /// [`Stream::Compressed`](crate::connection::Stream::Compressed), and write
    /// `opening` to it first.
    Relay {
        /// Where the game server is.
        endpoint: SocketAddrV4,
        /// The seed and the `0x91`, which carry the auth key that makes the new
        /// socket this account's.
        opening:  Vec<u8>,
    },
    /// A body arrived: this is the client's own character, and where it stands.
    Entered(PlayerStart),
    /// The world is ready to be drawn. The login conversation is over.
    Playing,
    /// The login was refused. The socket is about to close from the far end.
    Refused(DenyReason),
}

/// The login conversation went somewhere it cannot continue from.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum LoginError {
    /// A packet arrived that this stage has no answer for.
    ///
    /// Not every unexpected packet: the ones a server sends unprompted —
    /// features, light, music — are ignored, because a client that fell over on
    /// them would fall over on every shard. This is for the packets that carry
    /// the conversation, arriving out of turn.
    OutOfTurn {
        /// Where the conversation was.
        stage: Stage,
        /// The id that arrived.
        id:    PacketId,
    },
    /// The shard list held nothing matching the pick.
    NoSuchShard(Pick),
    /// The character list held nothing matching the pick — including the case
    /// of an account with no characters at all, which is a legitimate answer
    /// and still leaves nothing to play.
    NoSuchCharacter(Pick),
}

impl std::fmt::Display for LoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutOfTurn { stage, id } => {
                write!(f, "{id} arrived while {stage:?}")
            }
            Self::NoSuchShard(pick) => write!(f, "no shard matching {pick:?} was offered"),
            Self::NoSuchCharacter(pick) => write!(f, "no character matching {pick:?} was offered"),
        }
    }
}

impl std::error::Error for LoginError {
}

/// Drives one login, across both sockets.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Login {
    plan:     Plan,
    version:  ClientVersion,
    stage:    Stage,
    /// Kept from the relay so the `0x91` can present it. This is the only thing
    /// linking the two sockets.
    auth_key: AuthKey,
}

impl Login {
    /// A login that has not opened a socket yet.
    #[must_use]
    pub fn new(plan: Plan, version: ClientVersion) -> Self {
        Self {
            plan,
            version,
            stage: Stage::AwaitingShards,
            auth_key: AuthKey(0),
        }
    }

    /// Where the conversation is.
    #[must_use]
    pub const fn stage(&self) -> Stage {
        self.stage
    }

    /// The first bytes to write to the login socket: the seed, then `0x80`.
    ///
    /// The seed is not a packet — it is the handshake, and it carries the
    /// version every packet the server sends back is shaped by. Sending the
    /// account login without it means being sized as an unknown client.
    #[must_use]
    pub fn open(&self) -> Vec<u8> {
        let mut bytes = Seed::encode(RawSeedValue(0x0A00_0001), self.version);
        bytes.extend(
            AccountLogin {
                account:  self.plan.account.clone(),
                password: self.plan.password.clone(),
            }
            .encode(),
        );
        bytes
    }

    /// Advance the conversation with one packet from the server.
    pub fn on_packet(&mut self, packet: &ServerPacket) -> Result<Step, LoginError> {
        // A refusal can arrive at any point in the conversation and ends it,
        // so it is answered before the stage is consulted at all.
        if let ServerPacket::LoginDenied(denied) = packet {
            self.stage = Stage::Refused(denied.reason);
            return Ok(Step::Refused(denied.reason));
        }

        match (self.stage, packet) {
            (Stage::AwaitingShards, ServerPacket::ShardList(list)) => self.pick_shard(list),
            (Stage::AwaitingRelay, ServerPacket::Relay(relay)) => {
                self.auth_key = relay.auth_key;
                self.stage = Stage::Relayed;
                Ok(Step::Relay {
                    endpoint: relay.endpoint,
                    opening:  self.open_game(),
                })
            }
            (Stage::AwaitingCharacters, ServerPacket::CharacterList(list)) => self.pick_character(list),
            (Stage::AwaitingStart, ServerPacket::PlayerStart(start)) => {
                self.stage = Stage::AwaitingCompletion;
                Ok(Step::Entered(*start))
            }
            (Stage::AwaitingCompletion, ServerPacket::LoginComplete(_)) => {
                self.stage = Stage::Playing;
                Ok(Step::Playing)
            }
            // Anything else that carries the conversation is out of turn; the
            // rest — the features mask, the light level, the music — is the
            // server talking while the client listens, and is not this type's
            // business.
            (stage, packet) => {
                match packet {
                    ServerPacket::ShardList(_)
                    | ServerPacket::Relay(_)
                    | ServerPacket::CharacterList(_)
                    | ServerPacket::PlayerStart(_)
                    | ServerPacket::LoginComplete(_) => {
                        Err(LoginError::OutOfTurn {
                            stage,
                            id: PacketId(packet.id()),
                        })
                    }
                    _ => Ok(Step::Idle),
                }
            }
        }
    }

    fn pick_shard(&mut self, list: &ShardList) -> Result<Step, LoginError> {
        let index = self
            .plan
            .shard
            .find(list.shards.iter().map(|shard| shard.name.as_str()))
            .ok_or_else(|| LoginError::NoSuchShard(self.plan.shard.clone()))?;

        self.stage = Stage::AwaitingRelay;
        // The wire numbers shards from one; `RawShardIndex` is that numbering,
        // and the server subtracts on the way back.
        Ok(Step::Send(
            SelectShard {
                index: RawShardIndex(index as u16 + 1),
            }
            .encode(),
        ))
    }

    fn pick_character(&mut self, list: &CharacterList) -> Result<Step, LoginError> {
        let slot = self
            .plan
            .character
            .find(list.characters.iter().map(|entry| entry.name.0.as_str()))
            .ok_or_else(|| LoginError::NoSuchCharacter(self.plan.character.clone()))?;
        let name = list.characters[slot].name.0.clone();

        self.stage = Stage::AwaitingStart;
        Ok(Step::Send(
            CharacterPlay {
                name:      RawCharacterName(name),
                slot:      RawCharacterSlot(slot as u32),
                // The client's own idea of its address. The server has the
                // socket's real one and does not believe this field — see
                // `RawClientIp` — so there is nothing to be gained by
                // computing it.
                client_ip: RawClientIp(0),
            }
            .encode(),
        ))
    }

    /// The seed and `0x91` that open the game socket.
    fn open_game(&mut self) -> Vec<u8> {
        self.stage = Stage::AwaitingCharacters;
        let mut bytes = Seed::encode(RawSeedValue(0x0A00_0001), self.version);
        bytes.extend(
            GameServerLogin {
                auth_key: self.auth_key,
                account:  self.plan.account.clone(),
                password: self.plan.password.clone(),
            }
            .encode(),
        );
        bytes
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use openshard_protocol::identity::CharacterName;
    use openshard_protocol::login::{
        CharacterEntry,
        CharacterListFlags,
        LoginDenied,
        PercentFull,
        Relay,
        ShardEntry,
        StartLocation,
    };
    use openshard_protocol::serial::Serial;
    use openshard_protocol::wire::{
        ClilocId,
        Graphic,
    };
    use openshard_protocol::world::{
        Facet,
        Light,
        LightLevel,
        LoginComplete,
        MapSize,
        Point,
    };

    use super::*;

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    fn plan() -> Plan {
        Plan {
            account:   RawAccountName("admin".to_owned()),
            password:  RawPlaintextPassword("hunter2".to_owned()),
            shard:     Pick::First,
            character: Pick::Named("Lord British".to_owned()),
        }
    }

    fn shard_list() -> ServerPacket {
        ServerPacket::ShardList(ShardList {
            shards: vec![ShardEntry {
                name:         "OpenShard".to_owned(),
                percent_full: PercentFull::EMPTY,
                timezone:     5,
                address:      Ipv4Addr::LOCALHOST,
            }],
        })
    }

    fn character_list() -> ServerPacket {
        // Padded the way the server pads it: five slots, one of them occupied.
        let mut characters = vec![CharacterEntry {
            name: CharacterName::new("Lord British"),
        }];
        characters.resize(5, CharacterEntry::default());
        ServerPacket::CharacterList(CharacterList {
            characters,
            starts: vec![StartLocation {
                area:               "Britain".to_owned(),
                name:               "Castle Britannia".to_owned(),
                position:           Point::new(1475, 1770, 20),
                map:                Facet(0),
                description_cliloc: ClilocId(1_075_072),
            }],
            flags: CharacterListFlags::TOOLTIPS,
        })
    }

    fn player_start() -> PlayerStart {
        PlayerStart {
            serial:   Serial::new(0x0000_002A).unwrap(),
            body:     Graphic(0x0190),
            position: Point::new(1475, 1770, 20),
            facing:   openshard_protocol::direction::Facing::walking(
                openshard_protocol::direction::Direction::South,
            ),
            map:      MapSize::BRITANNIA,
        }
    }

    #[test]
    fn the_whole_conversation_in_order() {
        let mut login = Login::new(plan(), version());

        // The opening: a seed, then the account login. The seed first is not a
        // style choice — the server sizes what it sends back from the version
        // in it.
        let opening = login.open();
        assert_eq!(opening[0], 0xEF, "the seed opens the connection");
        assert_eq!(opening[21], 0x80, "the account login follows it");

        let Ok(Step::Send(select)) = login.on_packet(&shard_list()) else {
            panic!("the shard list must be answered");
        };
        assert_eq!(select, [0xA0, 0x00, 0x01], "shards are numbered from one");
        assert_eq!(login.stage(), Stage::AwaitingRelay);

        let relay = ServerPacket::Relay(Relay {
            endpoint: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 2593),
            auth_key: AuthKey(0xDEAD_BEEF),
        });
        let Ok(Step::Relay { endpoint, opening }) = login.on_packet(&relay) else {
            panic!("the relay must send the client to the game server");
        };
        assert_eq!(endpoint.port(), 2593);
        assert_eq!(opening[0], 0xEF, "the game socket opens with a seed too");
        assert_eq!(opening[21], 0x91, "and the game login behind it");
        assert_eq!(
            &opening[22..26],
            &0xDEAD_BEEFu32.to_be_bytes(),
            "carrying the key from the relay: the only thing linking the two sockets"
        );

        let Ok(Step::Send(play)) = login.on_packet(&character_list()) else {
            panic!("the character list must be answered");
        };
        assert_eq!(play[0], 0x5D);
        assert_eq!(&play[5..17], b"Lord British");

        assert_eq!(
            login.on_packet(&ServerPacket::PlayerStart(player_start())),
            Ok(Step::Entered(player_start()))
        );
        assert_eq!(
            login.on_packet(&ServerPacket::LoginComplete(LoginComplete)),
            Ok(Step::Playing)
        );
        assert_eq!(login.stage(), Stage::Playing);
    }

    #[test]
    fn the_first_character_is_the_first_occupied_slot() {
        // The list is padded to five, so "the first character" and "slot zero"
        // are only the same thing by luck. A client that took slot zero would
        // ask to play an empty slot on any account whose first character was
        // deleted.
        let mut characters = vec![CharacterEntry::default()];
        characters.push(CharacterEntry {
            name: CharacterName::new("Shamino"),
        });
        characters.resize(5, CharacterEntry::default());
        let list = ServerPacket::CharacterList(CharacterList {
            characters,
            starts: Vec::new(),
            flags: CharacterListFlags::NONE,
        });

        let mut login = Login::new(
            Plan {
                character: Pick::First,
                ..plan()
            },
            version(),
        );
        login.stage = Stage::AwaitingCharacters;

        let Ok(Step::Send(play)) = login.on_packet(&list) else {
            panic!("the character list must be answered");
        };
        assert_eq!(&play[5..12], b"Shamino");
        assert_eq!(
            u32::from_be_bytes([play[65], play[66], play[67], play[68]]),
            1,
            "the slot is the position in the list the server sent"
        );
    }

    #[test]
    fn a_refusal_ends_the_conversation_wherever_it_arrives() {
        // 0x82 can land at any stage — bad password on the login socket, bad
        // auth key on the game socket — and always means the same thing.
        let mut login = Login::new(plan(), version());
        login.stage = Stage::AwaitingCharacters;

        let denied = ServerPacket::LoginDenied(LoginDenied {
            reason: DenyReason::BadPassword,
        });
        assert_eq!(
            login.on_packet(&denied),
            Ok(Step::Refused(DenyReason::BadPassword))
        );
        assert_eq!(login.stage(), Stage::Refused(DenyReason::BadPassword));
    }

    #[test]
    fn the_chatter_a_server_sends_unprompted_is_ignored() {
        // A shard is free to send the light level, the music and the feature
        // mask around the login; a client that treated them as out of turn
        // would fail to log in to a perfectly ordinary server.
        let mut login = Login::new(plan(), version());
        assert_eq!(
            login.on_packet(&ServerPacket::LightLevel(LightLevel { level: Light(0) })),
            Ok(Step::Idle)
        );
        assert_eq!(login.stage(), Stage::AwaitingShards, "and nothing moved");
    }

    #[test]
    fn a_conversation_packet_out_of_turn_is_an_error() {
        // The other side of the same coin: a character list before a relay
        // means this end has lost track, and answering it would send a 0x5D
        // down a socket that is not expecting one.
        let mut login = Login::new(plan(), version());
        assert_eq!(
            login.on_packet(&character_list()),
            Err(LoginError::OutOfTurn {
                stage: Stage::AwaitingShards,
                id:    PacketId(0xA9),
            })
        );
    }

    #[test]
    fn a_shard_that_was_not_offered_is_an_error() {
        let mut login = Login::new(
            Plan {
                shard: Pick::Named("Some Other Shard".to_owned()),
                ..plan()
            },
            version(),
        );
        assert_eq!(
            login.on_packet(&shard_list()),
            Err(LoginError::NoSuchShard(Pick::Named(
                "Some Other Shard".to_owned()
            )))
        );
    }

    #[test]
    fn an_account_with_no_characters_cannot_play() {
        // Five empty slots is a legitimate answer — a new account — and there
        // is still nothing to play. Silently picking slot zero would send a
        // 0x5D naming nobody.
        let list = ServerPacket::CharacterList(CharacterList {
            characters: vec![CharacterEntry::default(); 5],
            starts:     Vec::new(),
            flags:      CharacterListFlags::NONE,
        });
        let mut login = Login::new(plan(), version());
        login.stage = Stage::AwaitingCharacters;
        assert!(matches!(
            login.on_packet(&list),
            Err(LoginError::NoSuchCharacter(_))
        ));
    }
}
