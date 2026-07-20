use super::*;

/// Turn a packet the world cares about into a command. `false` closes.
///
/// Nothing here answers the client. Every reply comes out of a tick, which is
/// what keeps the two ends in one order.
pub(crate) fn dispatch(
    session: &mut Session,
    world: &mut World,
    packet: &[u8],
    id: ConnectionId,
    saved: &HashMap<(String, String), CharacterRecord>,
    access: AccessLevel,
) -> bool {
    match packet.first().copied() {
        Some(CharacterPlay::ID) => {
            let Ok(play) = CharacterPlay::decode(packet) else {
                warn!(%id, "malformed 0x5D");
                return false;
            };
            let account = session.login.account().unwrap_or_default().to_owned();
            // A stored character enters on its saved serial, spot and look; one
            // the database has never seen — a config-only character on a fresh
            // shard — enters fresh at the start.
            let key = (account.to_lowercase(), play.name.to_lowercase());
            let record = saved.get(&key);
            let facet = record.map_or(0, |record| record.facet);
            let (serial, position, appearance, sheet) = match record {
                Some(record) => (
                    Some(record.serial),
                    Some(Point::new(record.x, record.y, record.z)),
                    Some(Appearance {
                        body: record.body,
                        hue: record.hue,
                    }),
                    Some(CharacterSheet {
                        strength: record.strength,
                        dexterity: record.dexterity,
                        intelligence: record.intelligence,
                        skills: record
                            .skills
                            .iter()
                            .map(|s| (s.id, s.value, SkillLock::from_bits(s.lock)))
                            .collect(),
                        effects: record.effects.clone(),
                    }),
                ),
                None => (None, None, None, None),
            };
            session.in_world = true;
            // Tell the gateway framer this client's version now, before any
            // in-world packet whose length depends on it (the drop packet). The
            // game connection never stated its version; this is the auth-key-linked
            // one the login carried across. Character select is the last quiet
            // moment before world traffic starts.
            let _ = session.control.send(session.login.version());
            world.queue(Command::Enter {
                connection: id,
                version: session.login.version(),
                account,
                name: play.name,
                serial,
                position,
                facet,
                appearance,
                sheet,
                access,
            });
            true
        }
        Some(WalkRequest::ID) => {
            if !session.in_world {
                debug!(%id, "0x02 before entering the world");
                return true;
            }
            let Ok(request) = WalkRequest::decode(packet) else {
                warn!(%id, "malformed 0x02");
                return false;
            };
            world.queue(Command::Walk {
                connection: id,
                request,
            });
            true
        }
        Some(0x34) => {
            // A status/skills query: a magic word (4), a type byte, and a serial.
            // Type 0x05 asks for the skill list — the client sends it when the
            // skill window opens — and type 0x04 (and the rest) for the status
            // bar. ServUO's `MobileQuery`. Answering every query with the status,
            // as before, left the skill window empty.
            if session.in_world {
                if packet.get(5) == Some(&0x05) {
                    world.queue(Command::RequestSkills { connection: id });
                } else {
                    world.queue(Command::RequestStatus { connection: id });
                }
            }
            true
        }
        Some(GumpResponse::ID) => {
            if !session.in_world {
                return true;
            }
            let Ok(response) = GumpResponse::decode(packet) else {
                warn!(%id, "malformed 0xB1");
                return false;
            };
            world.queue(Command::GumpResponse {
                connection: id,
                response,
            });
            true
        }
        Some(TargetResponse::ID) => {
            if !session.in_world {
                return true;
            }
            let Ok(response) = TargetResponse::decode(packet) else {
                warn!(%id, "malformed 0x6C");
                return false;
            };
            world.queue(Command::TargetResponse {
                connection: id,
                response,
            });
            true
        }
        Some(PickUpItem::ID) => {
            if !session.in_world {
                return true;
            }
            let Ok(pickup) = PickUpItem::decode(packet) else {
                warn!(%id, "malformed 0x07");
                return false;
            };
            world.queue(Command::PickUpItem {
                connection: id,
                serial: pickup.serial,
                amount: pickup.amount,
            });
            true
        }
        Some(DropItem::ID) => {
            if !session.in_world {
                return true;
            }
            let Ok(drop) = DropItem::decode(packet) else {
                warn!(%id, "malformed 0x08");
                return false;
            };
            world.queue(Command::DropItem {
                connection: id,
                serial: drop.serial,
                position: drop.position,
                container: drop.container,
            });
            true
        }
        Some(DoubleClick::ID) => {
            if !session.in_world {
                return true;
            }
            let Ok(click) = DoubleClick::decode(packet) else {
                warn!(%id, "malformed 0x06");
                return false;
            };
            world.queue(Command::DoubleClick {
                connection: id,
                serial: click.serial,
            });
            true
        }
        Some(0x3B) => {
            // A vendor purchase, answered out of the tick like everything else.
            if !session.in_world {
                return true;
            }
            let Ok(reply) = openshard_protocol::BuyReply::decode(packet) else {
                warn!(%id, "malformed 0x3B");
                return false;
            };
            world.queue(Command::Buy {
                connection: id,
                vendor: reply.vendor,
                purchases: reply.purchases,
            });
            true
        }
        Some(0x9F) => {
            if !session.in_world {
                return true;
            }
            let Ok(reply) = openshard_protocol::SellReply::decode(packet) else {
                warn!(%id, "malformed 0x9F");
                return false;
            };
            world.queue(Command::Sell {
                connection: id,
                vendor: reply.vendor,
                sales: reply.sales,
            });
            true
        }
        Some(LookRequest::ID) => {
            if !session.in_world {
                return true;
            }
            let Ok(look) = LookRequest::decode(packet) else {
                warn!(%id, "malformed 0x09");
                return false;
            };
            world.queue(Command::SingleClick {
                connection: id,
                serial: look.serial,
            });
            true
        }
        Some(PropertyQueryRequest::ID) => {
            // The AoS tooltip batch query: a client hovering wants these objects'
            // property lists. Answered out of the tick like every other reply.
            if !session.in_world {
                return true;
            }
            let Ok(query) = PropertyQueryRequest::decode(packet) else {
                warn!(%id, "malformed 0xD6");
                return false;
            };
            debug!(%id, count = query.serials.len(), "0xD6 tooltip query");
            world.queue(Command::QueryProperties {
                connection: id,
                serials: query.serials,
            });
            true
        }
        Some(EquipItemRequest::ID) => {
            if !session.in_world {
                return true;
            }
            let Ok(equip) = EquipItemRequest::decode(packet) else {
                warn!(%id, "malformed 0x13");
                return false;
            };
            world.queue(Command::EquipItem {
                connection: id,
                item: equip.item,
                layer: equip.layer,
                mobile: equip.mobile,
            });
            true
        }
        Some(WarModeRequest::ID) => {
            if !session.in_world {
                return true;
            }
            let Ok(request) = WarModeRequest::decode(packet) else {
                warn!(%id, "malformed 0x72");
                return false;
            };
            world.queue(Command::WarMode {
                connection: id,
                war: request.war,
            });
            true
        }
        Some(AttackRequest::ID) => {
            if !session.in_world {
                return true;
            }
            let Ok(request) = AttackRequest::decode(packet) else {
                warn!(%id, "malformed 0x05");
                return false;
            };
            world.queue(Command::Attack {
                connection: id,
                target: request.target,
            });
            true
        }
        Some(TalkRequest::ID) => {
            if !session.in_world {
                return true;
            }
            let Ok(talk) = TalkRequest::decode(packet) else {
                warn!(%id, "malformed 0x03");
                return false;
            };
            world.queue(Command::Say {
                connection: id,
                mode: talk.mode,
                hue: talk.hue,
                font: talk.font,
                text: talk.text,
            });
            true
        }
        Some(UnicodeTalkRequest::ID) => {
            // What a modern client actually sends when you type. Same `Say` as the
            // ASCII 0x03 once the words are out.
            if !session.in_world {
                return true;
            }
            let Ok(talk) = UnicodeTalkRequest::decode(packet) else {
                warn!(%id, "malformed 0xAD");
                return false;
            };
            world.queue(Command::Say {
                connection: id,
                mode: talk.mode,
                hue: talk.hue,
                font: talk.font,
                text: talk.text,
            });
            true
        }
        Some(CastSpellRequest::ID) => {
            // `0xBF` is a whole family of extended commands; only the cast
            // subcommand is one we act on, and `decode` says which it is.
            if !session.in_world {
                return true;
            }
            match CastSpellRequest::decode(packet) {
                Ok(Some(cast)) => world.queue(Command::RequestCast {
                    connection: id,
                    spell: cast.spell,
                }),
                // Not a cast — the same `0xBF` envelope carries the context-menu
                // request and selection, told apart by their own subcommand word.
                Ok(None) => {
                    if let Ok(Some(request)) = ContextMenuRequest::decode(packet) {
                        debug!(%id, serial = request.serial, "0xBF context-menu request");
                        world.queue(Command::ContextMenuRequest {
                            connection: id,
                            serial: request.serial,
                        });
                    } else if let Ok(Some(select)) = ContextMenuSelect::decode(packet) {
                        world.queue(Command::ContextMenuSelect {
                            connection: id,
                            serial: select.serial,
                            index: select.index,
                        });
                    }
                }
                Err(_) => {
                    warn!(%id, "malformed 0xBF cast");
                    return false;
                }
            }
            true
        }
        Some(SkillLockRequest::ID) => {
            if !session.in_world {
                return true;
            }
            match SkillLockRequest::decode(packet) {
                Ok(request) => world.queue(Command::SetSkillLock {
                    connection: id,
                    skill: request.skill,
                    lock: request.lock,
                }),
                Err(_) => {
                    warn!(%id, "malformed 0x3A skill lock");
                    return false;
                }
            }
            true
        }
        _ => true,
    }
}

/// The starting cities offered on the character-creation screen.
///
/// The nine classic towns a new character could wake up in on the original
/// Felucca map — the same list, inns and coordinates RunUO and ServUO have
/// shipped for two decades. Their order is what matters as much as their
/// contents: `start_location` in the create packet is a raw index into this
/// list, so position N here is the city the player picked when they clicked the
/// Nth entry. `create_character` reads the same list back to place the spawn, so
/// the two agree by construction.
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
    fn city(area: &str, name: &str, x: i32, y: i32, z: i32) -> StartLocation {
        StartLocation {
            area: area.to_owned(),
            name: name.to_owned(),
            position: (x, y, z),
            map: 0,
            description_cliloc: 0,
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
    .filter(|city| facets.contains(&(city.map as u8)))
    .collect();

    if cities.is_empty() {
        cities.push(StartLocation {
            area: "Britannia".to_owned(),
            name: "Britain".to_owned(),
            position: (i32::from(start.0), i32::from(start.1), 0),
            map: i32::from(facets.first().copied().unwrap_or(0)),
            description_cliloc: 0,
        });
    }
    cities
}

/// Create a character on the authenticated account, then enter the world with
/// it — the two halves of what a `0x00`/`0xF8` packet asks for.
///
/// Returns `false` only to drop the connection: a malformed packet, or one with
/// no game login behind it to say whose character this is. A *refused* creation
/// — a full account, an empty or duplicate name — keeps the connection. Sphere
/// answers that with the same `0x82` a login error uses, and the client stays on
/// the creation screen to try again.
pub(crate) fn create_character(
    session: &mut Session,
    login: &mut LoginServer<DevAccounts>,
    world: &mut World,
    packet: &[u8],
    id: ConnectionId,
) -> bool {
    let create = match CreateCharacter::decode(packet) {
        Ok(create) => create,
        Err(error) => {
            warn!(%id, %error, "malformed create-character");
            return false;
        }
    };
    let Some(account) = session.login.account().map(str::to_owned) else {
        warn!(%id, "create-character before a game login");
        return false;
    };

    let name = create.name.trim().to_owned();
    match login.accounts.create_character(&account, &name) {
        Ok(_slot) => info!(%id, account, name, "character created"),
        Err(reason) => {
            warn!(%id, account, name, ?reason, "character creation refused");
            let _ = session.send_packet(encode_login_denied(reason));
            return true;
        }
    }

    // Place the character in the city they picked. `start_location` indexes the
    // very list `start_cities` built and the character-list packet offered, so a
    // valid pick names a real city; only a client sending an out-of-range index
    // falls back to the default facet and a fresh spawn.
    let (facet, position) = match login.starts.get(create.start_location as usize) {
        Some(city) => (
            city.map as u8,
            Some(Point::new(
                city.position.0 as u16,
                city.position.1 as u16,
                city.position.2 as i8,
            )),
        ),
        None => (0, None),
    };

    session.in_world = true;
    let access = login.accounts.access_level(&account);
    world.queue(Command::Enter {
        connection: id,
        version: session.login.version(),
        account,
        name,
        // A brand-new character: a fresh serial, spawned in the chosen city. The
        // tick will journal it, so it is in the database — and in the character
        // list — by the next time the player logs in.
        serial: None,
        position,
        facet,
        appearance: Some(Appearance {
            body: create.body(),
            hue: create.skin_hue,
        }),
        // The stats and skills the player chose on the creation screen. The
        // client sends whole points; skills are stored in tenths, so a chosen 50
        // becomes 500. New skills start unlocked (training up).
        sheet: Some(CharacterSheet {
            strength: u16::from(create.strength),
            dexterity: u16::from(create.dexterity),
            intelligence: u16::from(create.intelligence),
            skills: create
                .skills
                .iter()
                .filter(|choice| choice.value > 0)
                .map(|choice| (choice.skill, u16::from(choice.value) * 10, SkillLock::Up))
                .collect(),
            // A new character is clean.
            effects: Vec::new(),
        }),
        access,
    });
    true
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
            assert_eq!(city.map, 0, "every classic city is on Felucca");
            assert!(
                city.position.0 > 0 && city.position.1 > 0,
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
        assert_eq!(cities[0].position, (1363, 1600, 0));
        assert_eq!(cities[0].map, 1, "on a loaded facet");
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
