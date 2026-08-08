//! Staff commands: `.`-prefixed speech from a privileged mobile.
//!
//! Sphere's convention, kept — a game master types `.add`, `.tele`, `.set` into
//! the ordinary speech box, and the world runs it instead of putting it over
//! their head. The gate (is this mobile a game master?) is the caller's, in the
//! `Command::Say` handler: this module trusts that a call means the actor cleared
//! it, and only parses and acts. Everything here is a world mutation the tick is
//! already the right place for, so a command is applied exactly like any other —
//! server-authoritative, no client round-trip.
//!
//! The commands lean on the systems that already own their rules — `items` spawns
//! the item, `skills` re-caps the stat — rather than reaching into the registry
//! themselves, the same "emit, don't reimplement" the rest of the world follows.

use openshard_entities::EntityId;
use openshard_movement::Tile;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::speech::{Font, SpokenMessage, TalkMode};
use openshard_protocol::target::{TargetCursor, TargetKind};
use openshard_protocol::wire::{CursorId, Graphic, Hue};
use openshard_protocol::world::{Facet, Point};
use openshard_state::components::{Client, Equipped, Position, SPELLBOOK_GRAPHIC, Spellbook, Staff, Stats};
use openshard_state::{TargetPurpose, WorldState};

use openshard_items as items;
use openshard_skills as skills;

/// The character that turns speech into a command. Sphere's, and what the
/// `Command::Say` handler strips before calling [`run`].
pub const COMMAND_PREFIX: char = '.';

/// The hue and font a command reply is drawn in — a muted grey, the client's
/// usual system-message colour, so it reads as the server talking, not a mobile.
const SYSTEM_HUE: Hue = Hue::SYSTEM;
const SYSTEM_FONT: Font = Font::DEFAULT;

/// Run a staff command for `actor`, already checked to hold the authority. `rest`
/// is the speech with the leading [`COMMAND_PREFIX`] removed.
///
/// Unknown or malformed commands answer the actor privately rather than doing
/// anything — a game master mistypes like anyone else, and a silent no-op looks
/// like a broken shard.
pub fn run(state: &mut WorldState, actor: EntityId, rest: &str) {
    let mut words = rest.split_whitespace();
    let Some(command) = words.next() else {
        return; // a lone "." is nothing to do
    };
    let args: Vec<&str> = words.collect();

    match command.to_lowercase().as_str() {
        "gm" => toggle_gm_mode(state, actor, &args),
        "where" => where_am_i(state, actor),
        "tele" => teleport_cursor(state, actor),
        "go" => go_to(state, actor, &args),
        "add" => add_item(state, actor, &args),
        "key" => make_key(state, actor, &args),
        "poison" => make_poison(state, actor, &args),
        "trap" => set_trap(state, actor, &args),
        "spellbook" => full_spellbook(state, actor),
        "quests" => openshard_quests::open_log_for(state, actor),
        "set" => set_stat(state, actor, &args),
        "admin" => crate::admin::open_menu(state, actor),
        "save" => save_world(state, actor),
        other => notify(state, actor, &format!("Unknown command '{other}'.")),
    }
}

/// `.key <value>` — put a key in your pack, and lock what you turn it on.
///
/// The one way to exercise locks on today's data, and worth having for that reason
/// alone: neither reference's Britannia locks a single door. ServUO's own decoration
/// data has exactly one `Locked` entry in the whole game and it is a container in
/// Malas, so the lock rules would otherwise be reachable only through a pack that
/// nobody has written yet.
///
/// Turning the key on an unlocked door or chest locks it; turning it on one locked to
/// the same value unlocks it. That is ServUO's `Key.OnTarget`, which does both.
fn make_key(state: &mut WorldState, actor: EntityId, args: &[&str]) {
    let value: u32 = match args.first().map(|v| v.parse()) {
        Some(Ok(value)) if value != 0 => value,
        _ => {
            notify(state, actor, "Usage: .key <value>, where value is not zero.");
            return;
        }
    };
    // Dropped at the operator's feet rather than into the pack: `.add` does the same,
    // and it keeps this out of the backpack-lookup business.
    let Some(&Position(at)) = state.registry.get::<Position>(actor) else {
        return;
    };
    let facet = state.facet_of(actor);
    // ServUO's iron key, `0x100E`.
    let Some(key) = items::spawn_item(state, Graphic(0x100E), Hue(0), 1, false, at, facet.0) else {
        notify(state, actor, "No room for a key.");
        return;
    };
    state
        .registry
        .insert(key, openshard_state::components::KeyValue(value));
    state
        .registry
        .insert(key, openshard_state::components::Name(format!("a key ({value})")));
    notify(state, actor, &format!("A key for lock {value} is in your pack."));
}

/// `.poison <level>` — drop a bottle of poison at the operator's feet.
///
/// The four strengths are the same bottle on the wire (`0x0F0A`), so which poison
/// one holds is on the item and something has to put it there. On a live shard that
/// is the pack's alchemist through `op_set_poison`; this is how the Poisoning skill
/// is tested on a shard with no pack at all.
fn make_poison(state: &mut WorldState, actor: EntityId, args: &[&str]) {
    let level: u8 = match args.first().map(|v| v.parse::<u8>()) {
        Some(Ok(level)) if level <= 4 => level,
        _ => {
            notify(state, actor, "Usage: .poison <level 0-4>.");
            return;
        }
    };
    let Some(&Position(at)) = state.registry.get::<Position>(actor) else {
        return;
    };
    let facet = state.facet_of(actor);
    let graphic = openshard_state::components::POISON_POTION_GRAPHIC;
    let Some(potion) = items::spawn_item(state, graphic, Hue(0), 1, false, at, facet.0) else {
        notify(state, actor, "No room for a potion.");
        return;
    };
    state.registry.insert(
        potion,
        openshard_state::components::PoisonCharges { level, charges: 1 },
    );
    let names = [
        "a lesser poison potion",
        "a poison potion",
        "a greater poison potion",
        "a deadly poison potion",
        "a lethal poison potion",
    ];
    state.registry.insert(
        potion,
        openshard_state::components::Name(names[usize::from(level)].to_owned()),
    );
    notify(state, actor, "A poison potion is at your feet.");
}

/// `.trap <kind> <power>` — raise a cursor and put a trap on a container.
///
/// Neither reference traps anything in Britannia's own data (ServUO's whole
/// decoration set has one locked container and no trapped one), so like `.key` this
/// exists to make the rule reachable: without it Remove Trap would be a skill with
/// nothing in the world to use it on.
fn set_trap(state: &mut WorldState, actor: EntityId, args: &[&str]) {
    use openshard_state::components::TrapKind;
    let kind = match args.first().copied() {
        Some("magic") => TrapKind::Magic,
        Some("explosion") => TrapKind::Explosion,
        Some("dart") => TrapKind::Dart,
        Some("poison") => TrapKind::Poison,
        _ => {
            notify(
                state,
                actor,
                "Usage: .trap <magic|explosion|dart|poison> [power].",
            );
            return;
        }
    };
    let power: u16 = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(30);
    state.raise_target(actor, openshard_state::TargetPurpose::SetTrap { kind, power });
    if let Some((connection, serial)) = connection_and_serial(state, actor) {
        state.send_packet(
            connection,
            &ServerPacket::TargetCursor(TargetCursor {
                cursor_id: CursorId(serial),
                kind: TargetKind::Object,
            }),
        );
    }
    notify(state, actor, "Which container shall I trap?");
}

/// A connection and wire serial for a mobile, for the cursor commands.
fn connection_and_serial(
    state: &WorldState,
    actor: EntityId,
) -> Option<(openshard_gateway::ConnectionId, u32)> {
    let client = state.registry.get::<openshard_state::components::Client>(actor)?;
    let serial = state.registry.serial_of(actor)?;
    Some((client.connection, serial.raw()))
}

/// `.gm [on|off]` — turn staff mode on or off, or toggle it.
///
/// Sphere's `.GM`, and the reason it exists: its `PLEVEL` says who may command
/// and its `PRIV_GM` flag says who is currently held to none of the game's rules,
/// and the two are separate so a game master can *play*. With the mode off a
/// staff character tires under its load and cannot see the dead, exactly as a
/// player does — which is the only way to test those rules from a staff account.
/// The commands keep working either way: they are gated on the authority, which
/// this never touches.
///
/// The screen is rebuilt on the spot ([`WorldState::refresh_around`], the same
/// call death and resurrection make), so ghosts appear or are forgotten as the
/// mode flips rather than at the next step.
fn toggle_gm_mode(state: &mut WorldState, actor: EntityId, args: &[&str]) {
    let on = match args.first().map(|word| word.to_lowercase()) {
        None => !state.is_staff(actor),
        Some(word) => match word.as_str() {
            "on" | "1" | "true" | "yes" => true,
            "off" | "0" | "false" | "no" => false,
            _ => {
                notify(state, actor, "Usage: .gm [on|off]");
                return;
            }
        },
    };
    if on {
        state.registry.insert(actor, Staff);
    } else {
        state.registry.remove::<Staff>(actor);
    }
    notify(state, actor, if on { "GM mode ON" } else { "GM mode OFF" });
    state.refresh_around(actor);
}

/// `.save` — force an immediate world save. No pause: the snapshot is an instant
/// memcpy the tick takes and a task nobody waits on writes, so the world keeps
/// running. Everyone is told it happened — a nod to the old shards' "please wait"
/// without the wait. The tick does the actual snapshot; this only asks and
/// announces.
fn save_world(state: &mut WorldState, actor: EntityId) {
    let connections: Vec<_> = state.players.keys().copied().collect();
    for connection in connections {
        let packet = ServerPacket::SpokenMessage(SpokenMessage {
            serial: None, // the system talking, not a mobile
            graphic: None,
            mode: TalkMode::Regular,
            hue: SYSTEM_HUE,
            font: SYSTEM_FONT,
            name: "System".to_owned(),
            text: "The world is being saved.".to_owned(),
        });
        state.send_packet(connection, &packet);
    }
    state.save_requested = true;
    notify(state, actor, "World save requested.");
}

/// Tell the actor where it is standing.
fn where_am_i(state: &mut WorldState, actor: EntityId) {
    let Some(&Position(at)) = state.registry.get::<Position>(actor) else {
        return;
    };
    let facet = state.facet_of(actor);
    notify(
        state,
        actor,
        &format!("You are at {}, {}, {} on facet {}.", at.x, at.y, at.z, facet.0),
    );
}

/// `.go <x> <y> [z] [facet]` — jump to coordinates, landing on the ground when
/// no z is given. Sphere's `.go`. The instant teleport with a cursor is `.tele`.
///
/// The facet argument is what makes a second facet reachable at all: nothing
/// else in the shard moves a mobile between them, so without it the whole
/// cross-facet path would be code no one could run.
fn go_to(state: &mut WorldState, actor: EntityId, args: &[&str]) {
    let (Some(x), Some(y)) = (args.first().and_then(parse_u16), args.get(1).and_then(parse_u16)) else {
        notify(state, actor, "Usage: .go <x> <y> [z] [facet]");
        return;
    };
    let facet = match args.get(3).and_then(parse_u16) {
        Some(named) => {
            let named = named as u8;
            if !state.facets.contains_key(&Facet(named)) {
                notify(state, actor, &format!("Facet {named} is not loaded."));
                return;
            }
            Facet(named)
        }
        None => state.facet_of(actor),
    };
    // An explicit z wins; otherwise drop onto whatever the ground is there, and a
    // facet with no map (development mode) keeps the actor's current height.
    let z = match args.get(2).and_then(parse_i8) {
        Some(z) => z,
        None => ground_z(state, facet.0, x, y)
            .or_else(|| state.registry.get::<Position>(actor).map(|p| p.0.z))
            .unwrap_or(0),
    };
    state.move_to(actor, facet, Point::new(x, y, z));
    notify(
        state,
        actor,
        &format!("Went to {x}, {y}, {z} on facet {}.", facet.0),
    );
}

/// `.tele` — Sphere's cursor teleport: raise a targeting cursor, and jump to the
/// spot the game master clicks. The click comes back as a `0x6C` the world routes
/// to [`crate::gm::teleport_to`].
fn teleport_cursor(state: &mut WorldState, actor: EntityId) {
    let Some(&Client { connection, .. }) = state.registry.get::<Client>(actor) else {
        return;
    };
    let serial = state.registry.serial_of(actor).map_or(0, |s| s.raw());
    // Remember this game master is targeting for a teleport, so the click knows
    // what it is for.
    state.raise_target(actor, TargetPurpose::Teleport);
    state.send_packet(
        connection,
        &ServerPacket::TargetCursor(TargetCursor {
            cursor_id: CursorId(serial),
            kind: TargetKind::Location,
        }),
    );
}

/// Finish a `.tele`: the game master clicked a spot; jump there. Called from the
/// world's `0x6C` handler with the clicked location.
pub(crate) fn teleport_to(state: &mut WorldState, actor: EntityId, to: Point) {
    // Staff pass this by `is_staff`, so a game master with the mode *off* is
    // refused here — which is the only way to check the rule from the kind of
    // account that can set one up.
    if !state.may_teleport(actor, to) {
        notify(state, actor, "You cannot teleport from or to that place.");
        return;
    }
    state.teleport(actor, to);
    notify(
        state,
        actor,
        &format!("Teleported to {}, {}, {}.", to.x, to.y, to.z),
    );
}

/// `.spellbook` — drop a full spellbook (every Magery spell) into the actor's
/// pack, so a tester can cast anything without buying each scroll. The mage's
/// book off the shelf is empty; this is the staff shortcut.
fn full_spellbook(state: &mut WorldState, actor: EntityId) {
    let Some(actor_serial) = state.registry.serial_of(actor) else {
        return;
    };
    let backpack = state
        .registry
        .query::<Equipped>()
        .find(|(_, worn)| worn.mobile == actor_serial && worn.layer == openshard_items::BACKPACK_LAYER)
        .and_then(|(entity, _)| state.registry.serial_of(entity));
    let Some(backpack) = backpack else {
        notify(state, actor, "You have no backpack.");
        return;
    };
    if let Some(book) = items::give(state, backpack, SPELLBOOK_GRAPHIC, Hue(0), 1) {
        state.registry.insert(book, Spellbook::full());
        notify(state, actor, "A full spellbook appears in your pack.");
    }
}

/// `.add <graphic> [amount]` — drop an item at the actor's feet. Hex (`0x1bf2`)
/// or decimal, because item ids are quoted both ways.
fn add_item(state: &mut WorldState, actor: EntityId, args: &[&str]) {
    let Some(graphic) = args.first().and_then(parse_u16).map(Graphic) else {
        notify(state, actor, "Usage: .add <graphic> [amount]");
        return;
    };
    let amount = args.get(1).and_then(parse_u16).unwrap_or(1).max(1);
    let Some(&Position(at)) = state.registry.get::<Position>(actor) else {
        return;
    };
    let facet = state.facet_of(actor);
    // A stack only if more than one was asked for; a single item is not stackable
    // by decree here — the graphic decides that in real gameplay, but a spawned
    // pile the operator named is stackable so the count takes.
    let stackable = amount > 1;
    if items::spawn_item(state, graphic, Hue(0), amount, stackable, at, facet.0).is_some() {
        notify(
            state,
            actor,
            &format!("Spawned {amount} of {:#06x} at your feet.", graphic.0),
        );
    }
}

/// `.set <str|dex|int> <value>` — change one stat, re-capping hits and mana
/// through the skills system that owns that rule.
fn set_stat(state: &mut WorldState, actor: EntityId, args: &[&str]) {
    let (Some(stat), Some(value)) = (args.first(), args.get(1).and_then(parse_u16)) else {
        notify(state, actor, "Usage: .set <str|dex|int> <value>");
        return;
    };
    let Some(serial) = state.registry.serial_of(actor) else {
        return;
    };
    let current = state.registry.get::<Stats>(actor).copied().unwrap_or(Stats {
        strength: 0,
        dexterity: 0,
        intelligence: 0,
    });
    let (strength, dexterity, intelligence) = match stat.to_lowercase().as_str() {
        "str" | "strength" => (value, current.dexterity, current.intelligence),
        "dex" | "dexterity" => (current.strength, value, current.intelligence),
        "int" | "intelligence" => (current.strength, current.dexterity, value),
        other => {
            notify(
                state,
                actor,
                &format!("Unknown stat '{other}'. Use str, dex or int."),
            );
            return;
        }
    };
    skills::set_stats(state, serial, strength, dexterity, intelligence);
    notify(state, actor, &format!("Set {stat} to {value}."));
}

/// Send the actor a private system line — the reply to a command, seen by no one
/// else. A mobile with no client (a scripted GM, say) simply gets no reply.
pub(crate) fn notify(state: &mut WorldState, actor: EntityId, text: &str) {
    state.system_message(actor, text);
}

/// The ground height at `(x, y)` on `facet`, if the facet has a map loaded.
fn ground_z(state: &WorldState, facet: u8, x: u16, y: u16) -> Option<i8> {
    state
        .facet_state(Facet(facet))
        .terrain
        .as_ref()
        .and_then(|terrain| terrain.ground_z(Tile::new(x, y)))
}

/// Parse a `u16` written in hex (`0x1bf2`) or decimal — item ids are quoted both.
fn parse_u16(text: &&str) -> Option<u16> {
    let text = *text;
    text.strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .map_or_else(|| text.parse().ok(), |hex| u16::from_str_radix(hex, 16).ok())
}

/// Parse a signed height, decimal only.
fn parse_i8(text: &&str) -> Option<i8> {
    text.parse().ok()
}
