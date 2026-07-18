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
use openshard_protocol::{encode_message, Point};
use openshard_state::components::{Client, Movement, Position, Stats};
use openshard_state::WorldState;

use openshard_items as items;
use openshard_skills as skills;

/// The character that turns speech into a command. Sphere's, and what the
/// `Command::Say` handler strips before calling [`run`].
pub const COMMAND_PREFIX: char = '.';

/// The hue and font a command reply is drawn in — a muted grey, the client's
/// usual system-message colour, so it reads as the server talking, not a mobile.
const SYSTEM_HUE: u16 = 0x03B2;
const SYSTEM_FONT: u16 = 3;

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
        "where" => where_am_i(state, actor),
        "tele" | "go" => teleport(state, actor, &args),
        "add" => add_item(state, actor, &args),
        "set" => set_stat(state, actor, &args),
        other => notify(state, actor, &format!("Unknown command '{other}'.")),
    }
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
        &format!("You are at {}, {}, {} on facet {facet}.", at.x, at.y, at.z),
    );
}

/// `.tele <x> <y> [z]` — move the actor there, landing on the ground when no z is
/// given. The move goes through the same interest refresh a walk does, so every
/// screen it should appear on and leave is updated.
fn teleport(state: &mut WorldState, actor: EntityId, args: &[&str]) {
    let (Some(x), Some(y)) = (
        args.first().and_then(parse_u16),
        args.get(1).and_then(parse_u16),
    ) else {
        notify(state, actor, "Usage: .tele <x> <y> [z]");
        return;
    };
    let facet = state.facet_of(actor);
    // An explicit z wins; otherwise drop onto whatever the ground is there, and a
    // facet with no map (development mode) keeps the actor's current height.
    let z = match args.get(2).and_then(parse_i8) {
        Some(z) => z,
        None => ground_z(state, facet, x, y)
            .or_else(|| state.registry.get::<Position>(actor).map(|p| p.0.z))
            .unwrap_or(0),
    };
    let to = Point::new(x, y, z);

    state.registry.insert(actor, Position(to));
    // The walker carries its own copy of the position; leave it in step or the
    // next walk starts from the old tile.
    if let Some(Movement(mut walker)) = state.registry.get::<Movement>(actor).copied() {
        walker.position = to;
        state.registry.insert(actor, Movement(walker));
    }
    state.facet_state_mut(facet).sectors.insert(actor, to);
    state.refresh_around(actor);
    notify(state, actor, &format!("Teleported to {x}, {y}, {z}."));
}

/// `.add <graphic> [amount]` — drop an item at the actor's feet. Hex (`0x1bf2`)
/// or decimal, because item ids are quoted both ways.
fn add_item(state: &mut WorldState, actor: EntityId, args: &[&str]) {
    let Some(graphic) = args.first().and_then(parse_u16) else {
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
    if items::spawn_item(state, graphic, 0, amount, stackable, at, facet).is_some() {
        notify(
            state,
            actor,
            &format!("Spawned {amount} of {graphic:#06x} at your feet."),
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
    let current = state
        .registry
        .get::<Stats>(actor)
        .copied()
        .unwrap_or(Stats {
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
    skills::set_stats(state, serial.raw(), strength, dexterity, intelligence);
    notify(state, actor, &format!("Set {stat} to {value}."));
}

/// Send the actor a private system line — the reply to a command, seen by no one
/// else. A mobile with no client (a scripted GM, say) simply gets no reply.
fn notify(state: &mut WorldState, actor: EntityId, text: &str) {
    let Some(&Client { connection, .. }) = state.registry.get::<Client>(actor) else {
        return;
    };
    let packet = encode_message(
        u32::MAX, // the system serial, so the client draws it as a server message
        0xFFFF,
        0, // regular mode
        SYSTEM_HUE,
        SYSTEM_FONT,
        "System",
        text,
    );
    state.send(connection, packet);
}

/// The ground height at `(x, y)` on `facet`, if the facet has a map loaded.
fn ground_z(state: &WorldState, facet: u8, x: u16, y: u16) -> Option<i8> {
    state
        .facet_state(facet)
        .terrain
        .as_ref()
        .and_then(|terrain| terrain.ground_z(x, y))
}

/// Parse a `u16` written in hex (`0x1bf2`) or decimal — item ids are quoted both.
fn parse_u16(text: &&str) -> Option<u16> {
    let text = *text;
    text.strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .map_or_else(
            || text.parse().ok(),
            |hex| u16::from_str_radix(hex, 16).ok(),
        )
}

/// Parse a signed height, decimal only.
fn parse_i8(text: &&str) -> Option<i8> {
    text.parse().ok()
}
