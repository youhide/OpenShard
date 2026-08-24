use super::*;

/// Turn a packet the world cares about into the command it means.
///
/// Nothing here answers the client, and nothing here reaches the world. Every
/// arm is a translation of one decoded packet into at most one [`Command`], and
/// the caller queues what comes back — so every reply still comes out of a tick,
/// which is what keeps the two ends in one order. `None` is a packet the world
/// has nothing to do about: a subcommand this shard does not act on, or a body
/// that names no object.
///
/// # The phase is matched once, and not here
///
/// Thirty of these arms used to open with `if !session.in_world()`, which is one
/// question asked thirty times and answered thirty ways — some arms logged, most
/// did not, and the next arm written was one forgotten line away from letting a
/// connection with no character queue work into the tick. `handle_world_packet`
/// matches the phase once on the way in, so a packet that reaches this function
/// is already one its connection may send, and there is no per-arm decision left
/// to forget. See `docs/connection_state.md`, S3.
///
/// That is also why this takes neither the session nor the world: with the gate
/// gone the only thing left in the arms is the packet, so it cannot reach past
/// the one it was handed.
///
/// `packet` is already decoded — `parse_packet` in `shard.rs` does that once,
/// before routing here, so a malformed packet never reaches this function at
/// all; it closes the connection at the routing step instead.
pub(crate) fn dispatch_world_packet(packet: ClientPacket, id: ConnectionId) -> Option<Command> {
    match packet {
        ClientPacket::Walk(request) => Some(Command::Walk {
            connection: id,
            request,
        }),
        // "Log Out" on the paperdoll. The client tells the server it is leaving
        // and then waits to be told it may — see `world::LogoutAck`. Queued like
        // everything else, so the answer comes out of a tick.
        ClientPacket::LogoutRequest => Some(Command::LogoutRequest { connection: id }),
        // "Where am I?" — the walk handshake's repair leg. Queued like everything
        // else, so the answer is built from the world at one instant rather than
        // from whatever a socket thread could see.
        ClientPacket::ResyncRequest => Some(Command::Resync { connection: id }),
        ClientPacket::StatusQuery(query) => Some(match query.kind {
            StatusQueryKind::Skills => Command::RequestSkills { connection: id },
            StatusQueryKind::Status => Command::RequestStatus { connection: id },
        }),
        ClientPacket::Encoded(command) => {
            // The AoS "encoded command": the paperdoll's own buttons, which are
            // not gump replies — the paperdoll is drawn client-side and has no
            // server layout to answer. Without this the Quest button does nothing
            // at all, with nothing anywhere to say why.
            match command.subcommand.interpret() {
                EncodedSubcommand::QuestGumpRequest => Some(Command::QuestLogRequest { connection: id }),
                EncodedSubcommand::GuildGumpRequest => Some(Command::GuildWindowRequest { connection: id }),
                // Named, not routed: combat has no weapon abilities. Naming it
                // means the byte layout is not re-derived the day it lands.
                EncodedSubcommand::SetAbility => None,
                EncodedSubcommand::Other(other) => {
                    debug!(subcommand = format!("0x{other:02X}"), "unhandled 0xD7");
                    None
                }
            }
        }
        ClientPacket::GumpResponse(response) => Some(Command::GumpResponse {
            connection: id,
            response,
        }),
        ClientPacket::TargetResponse(response) => Some(Command::TargetResponse {
            connection: id,
            response,
        }),
        ClientPacket::PickUpItem(pickup) => Some(Command::PickUpItem {
            connection: id,
            serial: pickup.serial,
            amount: pickup.amount.0,
        }),
        ClientPacket::DropItem(drop) => Some(Command::DropItem {
            connection: id,
            serial: drop.serial,
            destination: drop.destination(),
        }),
        ClientPacket::SecureTrade(action) => match action {
            SecureTradeAction::Cancel { container } => Some(Command::TradeCancel {
                connection: id,
                container,
            }),
            SecureTradeAction::Accept { container, accepted } => Some(Command::TradeAction {
                connection: id,
                container,
                accepted,
            }),
            // Virtual gold and platinum: an account balance this shard does not
            // keep. Gold is an item, and it trades by being dragged into the
            // window like anything else.
            SecureTradeAction::UpdateGold { .. } => None,
        },
        // The paperdoll bit comes off here, where the packet is read: it is
        // framing the client owns, and `interpret` is total, so nothing
        // downstream has to know which bit it was.
        ClientPacket::DoubleClick(click) => Some(Command::DoubleClick {
            connection: id,
            request: click.interpret(),
        }),
        // A vendor purchase, answered out of the tick like everything else.
        ClientPacket::Buy(reply) => Some(Command::Buy {
            connection: id,
            vendor: reply.vendor,
            purchases: reply.purchases,
        }),
        ClientPacket::Sell(reply) => Some(Command::Sell {
            connection: id,
            vendor: reply.vendor,
            sales: reply.sales,
        }),
        ClientPacket::Look(look) => {
            // A `0x09` naming nothing — zero, or `0xFFFF_FFFF` — is a click that
            // hit no object, which is an answer and not a reason to queue work.
            let serial = look.serial.validate()?;
            Some(Command::SingleClick {
                connection: id,
                serial,
            })
        }
        ClientPacket::PropertyQuery(query) => {
            // The AoS tooltip batch query: a client hovering wants these objects'
            // property lists. Answered out of the tick like every other reply.
            debug!(%id, count = query.serials.len(), "0xD6 tooltip query");
            Some(Command::QueryProperties {
                connection: id,
                serials: query.serials,
            })
        }
        ClientPacket::Equip(equip) => Some(Command::EquipItem {
            connection: id,
            item: equip.item,
            layer: equip.layer,
            mobile: equip.mobile,
        }),
        ClientPacket::WarMode(request) => Some(Command::WarMode {
            connection: id,
            war: request.war,
        }),
        ClientPacket::Attack(request) => Some(Command::Attack {
            connection: id,
            target: request.target,
        }),
        ClientPacket::Talk(talk) => Some(Command::Say {
            connection: id,
            mode: talk.mode,
            hue: talk.hue,
            font: talk.font,
            text: talk.text,
        }),
        // What a modern client actually sends when you type. Same `Say` as the
        // ASCII 0x03 once the words are out.
        ClientPacket::UnicodeTalk(talk) => Some(Command::Say {
            connection: id,
            mode: talk.mode,
            hue: talk.hue,
            font: talk.font,
            text: talk.text,
        }),
        // `0xBF` is a whole family of extended commands; `ExtendedRequest` has
        // already picked the one subcommand this packet carries.
        ClientPacket::Extended(request) => match request {
            // interpret() is total, so it may run right here rather than waiting
            // for a tick system to have the domain in hand — see
            // `docs/protocol_newtypes.md`'s N4 containers amendment 2. A wire 0
            // is never a legitimate spell id and queues nothing.
            ExtendedRequest::Cast(cast) => cast.spell.interpret().map(|spell| Command::RequestCast {
                connection: id,
                spell,
            }),
            ExtendedRequest::ContextMenuRequest(request) => {
                debug!(%id, serial = request.serial.0, "0xBF context-menu request");
                Some(Command::ContextMenuRequest {
                    connection: id,
                    serial: request.serial,
                })
            }
            ExtendedRequest::ContextMenuSelect(select) => Some(Command::ContextMenuSelect {
                connection: id,
                serial: select.serial,
                index: select.index,
            }),
            ExtendedRequest::StatLock(request) => {
                // The seam is where a client's byte becomes a stat: the status
                // bar has three arrows, and a packet naming a fourth is dropped
                // here rather than travelling into the tick to be ignored by a
                // `_ =>` arm nobody can see from the packet.
                match request.stat.validate() {
                    Ok(stat) => Some(Command::SetStatLock {
                        connection: id,
                        stat,
                        lock: StatLock::from_wire(request.lock.interpret()),
                    }),
                    Err(invalid) => {
                        debug!(%id, %invalid, "0xBF 0x1A named no stat");
                        None
                    }
                }
            }
            // Whole, rather than picked apart here: unlike the stat lock above
            // there is no wire value to validate into a domain one — every arm
            // names a serial the world has to look up anyway, so the seam has
            // nothing to do that the tick is not better placed to do.
            ExtendedRequest::Party(request) => Some(Command::Party {
                connection: id,
                request,
            }),
            // The serial is not validated here: whether it names a house this
            // player may see is the tick's question, and it has to look the
            // entity up either way.
            ExtendedRequest::DesignDetails(request) => Some(Command::DesignDetails {
                connection: id,
                serial: request.serial,
            }),
            // Whole, like the party request above: the facet is a byte the tick
            // has to look up anyway, and a chunk it cannot cut is answered with
            // a refusal rather than dropped — so there is nothing for a seam
            // with no world in hand to decide.
            ExtendedRequest::Chunks(request) => Some(Command::RequestChunks {
                connection: id,
                facet: request.facet,
                chunks: request.chunks,
            }),
            // And the same for the question a client with a cache asks first:
            // whether this shard can say what moved since that revision is a
            // question about a world and a log, and neither is here.
            ExtendedRequest::Changes(request) => Some(Command::RequestChanges {
                connection: id,
                facet: request.facet,
                revision: request.revision,
            }),
            ExtendedRequest::Unknown(subcommand) => {
                debug!(%id, subcommand = format!("0x{subcommand:02X}"), "unhandled 0xBF");
                None
            }
            // `ExtendedRequest` is `#[non_exhaustive]` for callers outside this
            // workspace; every variant that exists today is matched above.
            _ => unreachable!("every ExtendedRequest variant is matched above"),
        },
        ClientPacket::UseSkill(request) => Some(Command::UseSkillButton {
            connection: id,
            skill: request.skill,
        }),
        ClientPacket::SkillLock(request) => Some(Command::SetSkillLock {
            connection: id,
            skill: request.skill,
            lock: request.lock,
        }),
        // A `0x12` text command that is not "use skill" reaches here as
        // Unknown, not an error — see `ClientPacket::decode`.
        ClientPacket::Unknown { id: 0x12, .. } => {
            debug!(%id, "0x12 text command we do not act on");
            None
        }
        ClientPacket::Unknown { .. } => None,
        // `ClientPacket` is `#[non_exhaustive]` for callers outside this
        // workspace; every variant that exists today is matched above.
        _ => unreachable!("every ClientPacket variant is matched above"),
    }
}

/// The starting cities offered on the character-creation screen.
///
/// The nine classic towns a new character could wake up in on the original
/// Felucca map — the same list, inns and coordinates RunUO and ServUO have
/// shipped for two decades. Their order is what matters as much as their
/// contents: `start_location` in the create packet is a raw index into this
/// list, so position N here is the city the player picked when they clicked the
/// Nth entry. The world reads the same list back to place the spawn — it is
/// handed over at boot as part of `CharacterScreen` — so the two agree by
/// construction.
///
/// All nine are on facet 0, the only facet a new character starts on, so the
/// list is filtered to the facets this shard actually loaded: offering a city on
/// a facet with no terrain would spawn the player in nowhere. If that leaves it
/// empty — a shard that loaded no facet carrying a starting city — one city at
/// the configured start is kept, because the client refuses an empty list and
/// says so: "No city found. Something wrong with the received cities."
///
/// The description cliloc is left 0: a client older than 7.0.13.0 ignores the
/// field, and a newer one shows the city and inn names either way.
pub(crate) fn start_cities(facets: &[u8], start: (u16, u16)) -> Vec<StartLocation> {
    fn city(area: &str, name: &str, x: u16, y: u16, z: i8) -> StartLocation {
        StartLocation {
            area: area.to_owned(),
            name: name.to_owned(),
            position: Point::new(x, y, z),
            map: Facet(0),
            description_cliloc: ClilocId(0),
        }
    }

    let mut cities: Vec<StartLocation> = [
        city("Yew", "The Empath Abbey", 633, 858, 0),
        city("Minoc", "The Barnacle", 2476, 413, 15),
        city("Britain", "Sweet Dreams Inn", 1496, 1628, 10),
        city("Moonglow", "The Scholars Inn", 4408, 1168, 0),
        city("Trinsic", "The Traveler's Inn", 1845, 2745, 0),
        city("Magincia", "The Great Horns Tavern", 3734, 2222, 20),
        city("Jhelom", "The Mercenary Inn", 1374, 3826, 0),
        city("Skara Brae", "The Falconer's Inn", 618, 2234, 0),
        city("Vesper", "The Ironwood Inn", 2771, 976, 0),
    ]
    .into_iter()
    .filter(|city| facets.contains(&city.map.0))
    .collect();

    if cities.is_empty() {
        cities.push(StartLocation {
            area: "Britannia".to_owned(),
            name: "Britain".to_owned(),
            position: Point::new(start.0, start.1, 0),
            map: Facet(facets.first().copied().unwrap_or(0)),
            description_cliloc: ClilocId(0),
        });
    }
    cities
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_facet_zero_shard_offers_the_classic_towns() {
        // Facet 0 loaded — the normal case — offers the nine classic Felucca
        // cities, every one of them on map 0 with a real, non-origin position.
        let cities = start_cities(&[0], (1363, 1600));
        assert_eq!(cities.len(), 9, "the nine classic starting cities");
        assert!(
            cities.iter().any(|city| city.area == "Britain"),
            "Britain is one of them"
        );
        for city in &cities {
            assert_eq!(city.map, Facet(0), "every classic city is on Felucca");
            assert!(
                city.position.x > 0 && city.position.y > 0,
                "a real spot, not the origin"
            );
        }
    }

    #[test]
    fn a_shard_without_facet_zero_still_offers_one_city() {
        // An empty list is what makes ClassicUO refuse to open the creation
        // screen. No classic city lives on a non-zero facet, so a shard that
        // loaded only facet 1 keeps a single fallback at the configured start —
        // on a facet it actually loaded, not facet 0 it did not.
        let cities = start_cities(&[1], (1363, 1600));
        assert_eq!(cities.len(), 1, "never empty");
        assert_eq!(cities[0].position, Point::new(1363, 1600, 0));
        assert_eq!(cities[0].map, Facet(1), "on a loaded facet");
    }

    #[test]
    fn start_location_indexes_the_offered_list() {
        // The contract create_character depends on: the byte the client sends is
        // a raw index into exactly this list, so the Nth city is the one picked
        // by clicking the Nth entry. If this order ever shifts, spawns land in
        // the wrong town silently.
        let cities = start_cities(&[0], (1363, 1600));
        assert_eq!(cities[0].area, "Yew");
        assert_eq!(cities[2].area, "Britain");
        assert_eq!(cities[8].area, "Vesper");
    }
}
