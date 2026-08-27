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

use openshard_commands::StaffCommand;
use openshard_entities::EntityId;
use openshard_map::grid::Tile;
use openshard_protocol::direction::Direction;
use openshard_protocol::serial::Serial;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::speech::{Font, SpokenMessage, TalkMode};
use openshard_protocol::target::{TargetCursor, TargetKind};
use openshard_protocol::wire::{CursorId, Graphic, Hue};
use openshard_protocol::world::{Facet, Point};
use openshard_state::components::{
    Client, HouseDoor, HouseSign, Position, SPELLBOOK_GRAPHIC, Spellbook, Staff, Stats,
};
use openshard_state::{HouseChange, TargetPurpose, WorldState};

use openshard_items as items;
use openshard_skills as skills;

/// The character that turns speech into a command. Sphere's, and what the
/// `Command::Say` handler strips before calling [`run`].
///
/// The value itself is [`openshard_commands::PREFIX`] and not a second `'.'`
/// written here: the client reads the same character to know that the line
/// being typed is a command, and the two must be the same character or one end
/// offers a vocabulary the other will not run.
pub const COMMAND_PREFIX: char = openshard_commands::PREFIX;

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
    let Some(word) = words.next() else {
        return; // a lone "." is nothing to do
    };
    let args: Vec<&str> = words.collect();

    // The vocabulary is `openshard_commands::StaffCommand` and the match below
    // is **exhaustive** on it, which is the whole point of it being an enum: a
    // command added to that table does not compile until it has an arm here,
    // and the client — which completes from the same table — cannot offer a
    // word this function would answer with "unknown".
    let Some(command) = StaffCommand::parse(word) else {
        notify(state, actor, &format!("Unknown command '{word}'."));
        return;
    };
    match command {
        StaffCommand::Gm => toggle_gm_mode(state, actor, &args),
        StaffCommand::Where => where_am_i(state, actor),
        StaffCommand::Tele => teleport_cursor(state, actor),
        StaffCommand::Sight => sight_cursor(state, actor),
        StaffCommand::Go => go_to(state, actor, &args),
        StaffCommand::Add => add_item(state, actor, &args),
        StaffCommand::AddGold => add_gold(state, actor, &args),
        StaffCommand::Key => make_key(state, actor, &args),
        StaffCommand::Poison => make_poison(state, actor, &args),
        StaffCommand::Trap => set_trap(state, actor, &args),
        StaffCommand::Spellbook => full_spellbook(state, actor),
        StaffCommand::Quests => openshard_quests::open_log_for(state, actor),
        StaffCommand::Set => set_stat(state, actor, &args),
        StaffCommand::Skill => set_skill(state, actor, &args),
        StaffCommand::House => place_house(state, actor, &args),
        StaffCommand::Deed => make_deed(state, actor, &args),
        StaffCommand::HFriend => house_list(state, actor, HouseChange::Friend),
        StaffCommand::HCoOwner => house_list(state, actor, HouseChange::CoOwner),
        StaffCommand::HDrop => house_list(state, actor, HouseChange::Drop),
        StaffCommand::HBan => house_list(state, actor, HouseChange::Ban),
        StaffCommand::HUnban => house_list(state, actor, HouseChange::Unban),
        StaffCommand::HDemolish => demolish_house(state, actor, &args),
        StaffCommand::HDesign => design_house(state, actor, &args),
        StaffCommand::Boat => launch_boat(state, actor, &args),
        StaffCommand::Sail => sail_boat(state, actor, &args),
        StaffCommand::Admin => crate::admin::open_menu(state, actor),
        StaffCommand::Save => save_world(state, actor),
        StaffCommand::Tile => describe_tile(state, actor),
        StaffCommand::SetLand => set_land(state, actor, &args),
        StaffCommand::AddStatic => add_static(state, actor, &args),
        StaffCommand::RmStatic => remove_static(state, actor, &args),
    }
}

/// `.tile` — say what the map holds under you, with the ordinal of each static.
///
/// The thing to run before [`remove_static`], because an ordinal is only an
/// identity against a stated revision — which is why the revision is on the
/// first line. `openshard-map-patch`'s `list` and `show` in one, for the world
/// the shard is actually holding rather than the one on disk.
fn describe_tile(state: &mut WorldState, actor: EntityId) {
    let Some(&Position(at)) = state.registry.get::<Position>(actor) else {
        return;
    };
    let facet = state.facet_of(actor);
    let ground = state.facet_state(facet).ground();
    let Some(world) = ground.snapshot() else {
        notify(state, actor, "This facet has no map.");
        return;
    };
    let (x, y) = (at.x, at.y);
    let mut lines = vec![match world.map().land(x, y) {
        Some(cell) => format!(
            "({x}, {y}) on facet {} at revision {}: land {} at z {}",
            facet.0,
            world.revision().get(),
            cell.tile.0,
            cell.z
        ),
        None => format!("({x}, {y}) is not on facet {}.", facet.0),
    }];
    lines.extend(world.map().statics_at(x, y).enumerate().map(|(nth, item)| {
        format!(
            "  {nth}: graphic {:#06x} at z {}, hue {}",
            item.tile.0, item.z, item.hue.0
        )
    }));
    if lines.len() == 1 {
        lines.push("  nothing stands on it".to_owned());
    }
    // The live layer is deliberately not listed: a door, a crate or a house
    // floor is an *entity*, and none of them is something a patch can name.
    if state.facet_state(facet).home().is_none() {
        lines.push("  (read from the client's files, so it cannot be edited)".to_owned());
    }
    for line in lines {
        notify(state, actor, &line);
    }
}

/// `.setland <tile id> [z]` — change the ground under you, in the world itself.
///
/// Not a decoration and not an item: this commits a patch to the shard's own
/// map, so it survives a restart and changes what every player is allowed to
/// walk on. `z` defaults to the height that is already there, which is the
/// common case — repainting a tile without moving it.
fn set_land(state: &mut WorldState, actor: EntityId, args: &[&str]) {
    let Some(tile) = args.first().and_then(parse_u16) else {
        notify(
            state,
            actor,
            "Usage: .setland <tile id> [z], e.g. .setland 0x03 5",
        );
        return;
    };
    let Some((facet, x, y)) = standing_on(state, actor) else {
        return;
    };
    let Some(was) = ground_at(state, facet, x, y) else {
        notify(state, actor, "There is no map under you.");
        return;
    };
    let z = match args.get(1) {
        Some(word) => match word.parse::<i8>() {
            Ok(z) => z,
            Err(_) => {
                notify(state, actor, "The height has to be a number from -128 to 127.");
                return;
            }
        },
        None => was.z,
    };
    let op = openshard_map::patch::PatchOp::SetLand {
        x,
        y,
        was,
        now: openshard_map::map::LandCell {
            tile: openshard_tiles::LandTileId(tile),
            z,
        },
    };
    commit_one(state, actor, facet, op, &format!("land {tile} at z {z}"));
}

/// `.addstatic <graphic> [z]` — build a static into the map under you.
///
/// The difference from `.add` is the whole point: `.add` spawns an *item*, which
/// is an entity with a serial that can be picked up and that the client is told
/// about. This puts a static into the terrain, where it is part of the world and
/// nothing on the wire mentions it — so the shard treats it as a wall, and every
/// picture keeps drawing what the player's own files say until direction E.
fn add_static(state: &mut WorldState, actor: EntityId, args: &[&str]) {
    let Some(graphic) = args.first().and_then(parse_u16) else {
        notify(
            state,
            actor,
            "Usage: .addstatic <graphic> [z], e.g. .addstatic 0x0edd",
        );
        return;
    };
    let Some((facet, x, y)) = standing_on(state, actor) else {
        return;
    };
    let Some(&Position(at)) = state.registry.get::<Position>(actor) else {
        return;
    };
    let z = match args.get(1) {
        Some(word) => match word.parse::<i8>() {
            Ok(z) => z,
            Err(_) => {
                notify(state, actor, "The height has to be a number from -128 to 127.");
                return;
            }
        },
        // Where the operator is standing, which is what "under you" means for
        // something that has a height of its own.
        None => at.z,
    };
    let op = openshard_map::patch::PatchOp::AddStatic {
        item: openshard_map::map::StaticItem {
            tile: Graphic(graphic),
            x,
            y,
            z,
            hue: Hue::NONE,
        },
    };
    commit_one(
        state,
        actor,
        facet,
        op,
        &format!("graphic {graphic:#06x} at z {z}"),
    );
}

/// `.rmstatic [nth]` — take one static out of the map under you.
///
/// The ordinal is `.tile`'s, and it is only an identity against the revision
/// that command printed: a patch committed in between renumbers what follows it.
/// That is not a race this has to guard, because both are one tick.
fn remove_static(state: &mut WorldState, actor: EntityId, args: &[&str]) {
    let nth = match args.first() {
        Some(word) => match parse_u16(word) {
            Some(nth) => nth,
            None => {
                notify(state, actor, "Usage: .rmstatic [nth], as printed by .tile");
                return;
            }
        },
        None => 0,
    };
    let Some((facet, x, y)) = standing_on(state, actor) else {
        return;
    };
    let which = openshard_map::patch::StaticId(nth);
    let built = state
        .facet_state(facet)
        .ground()
        .snapshot()
        .ok_or(openshard_map::patch::PatchError::NoGround)
        .and_then(|world| openshard_map::patch::PatchOp::remove_static(world.map(), x, y, which));
    match built {
        Ok(op) => commit_one(state, actor, facet, op, &format!("static {nth} removed")),
        Err(refusal) => notify(state, actor, &format!("Nothing to remove: {refusal}")),
    }
}

/// The facet and tile the actor is standing on.
fn standing_on(state: &mut WorldState, actor: EntityId) -> Option<(Facet, u16, u16)> {
    let &Position(at) = state.registry.get::<Position>(actor)?;
    Some((state.facet_of(actor), at.x, at.y))
}

/// The land cell at a tile of a facet, or `None` where there is no map.
fn ground_at(state: &WorldState, facet: Facet, x: u16, y: u16) -> Option<openshard_map::map::LandCell> {
    state
        .facet_state(facet)
        .ground()
        .snapshot()
        .and_then(|world| world.map().land(x, y))
}

/// Commit one op as a patch of its own, and say what happened.
///
/// One op per patch because a command line has no spelling for "and also" — the
/// model holds a list and the applier walks it in order, so a brush that batches
/// them is direction F's and needs nothing here.
///
/// The author is the operator's own account name: a history whose every entry
/// says "staff" is a history that cannot answer who raised the hill.
fn commit_one(
    state: &mut WorldState,
    actor: EntityId,
    facet: Facet,
    op: openshard_map::patch::PatchOp,
    what: &str,
) {
    let author = state
        .registry
        .get::<openshard_state::components::Name>(actor)
        .map_or_else(|| "staff".to_owned(), |name| name.0.clone());
    let Some(parent) = state
        .facet_state(facet)
        .ground()
        .snapshot()
        .map(openshard_map::snapshot::MapSnapshot::revision)
    else {
        notify(state, actor, "There is no map under you.");
        return;
    };
    let patch = openshard_map::patch::Patch::new(
        facet,
        parent,
        openshard_map::patch::PatchAuthor(author),
        // The wall clock, and it is not the tick's business: a patch's time is
        // for a person reading the history, and the *order* is the chain of
        // revisions — see `PatchTime`. Nothing in the simulation reads it.
        openshard_map::patch::PatchTime(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_secs()),
        ),
        vec![op],
    );

    match crate::mapedit::commit(state, facet, &patch) {
        Ok(revision) => {
            notify(
                state,
                actor,
                &format!(
                    "Committed: {what}. Facet {} is at revision {}.",
                    facet.0,
                    revision.get()
                ),
            );
            // Said every time, because it is true every time and the cost of
            // forgetting it is a shard that will not boot: the navigation
            // artifact is stamped against the world, and this moved the world.
            if let Some(rebake) = crate::mapedit::rebake_command(state, facet) {
                notify(
                    state,
                    actor,
                    "Long routes are off until the graph is rebuilt and the shard restarted:",
                );
                notify(state, actor, &rebake);
            }
        }
        Err(refusal) => notify(state, actor, &format!("{refusal}")),
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
    let Some(key) = items::spawn_item(state, Graphic(0x100E), Hue(0), 1, false, at, facet) else {
        notify(state, actor, "No room for a key.");
        return;
    };
    state.registry.insert(
        key,
        openshard_state::components::KeyValue::new(value).expect("non-zero staff key"),
    );
    state
        .registry
        .insert(key, openshard_state::components::Name(format!("a key ({value})")));
    notify(state, actor, &format!("A key for lock {value} is in your pack."));
}

/// `.poison <level>` — drop a bottle of poison at the operator's feet.
///
/// The four strengths are the same bottle on the wire (`0x0F0A`), so which poison
/// one holds is on the item and something has to put it there. On a live shard that
/// is the staff command; this is how the Poisoning skill
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
    let Some(potion) = items::spawn_item(state, graphic, Hue(0), 1, false, at, facet) else {
        notify(state, actor, "No room for a potion.");
        return;
    };
    state.registry.insert(
        potion,
        openshard_state::components::PoisonCharges {
            level: openshard_protocol::world::PoisonLevel::new(level),
            charges: 1,
        },
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
        &format!("You are at {}, {}, {} on facet {facet}.", at.x, at.y, at.z),
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
        None => ground_z(state, facet, x, y)
            .or_else(|| state.registry.get::<Position>(actor).map(|p| p.0.z))
            .unwrap_or(0),
    };
    state.move_to(actor, facet, Point::new(x, y, z));
    notify(state, actor, &format!("Went to {x}, {y}, {z} on facet {facet}."));
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

/// `.sight` — raise a cursor, and answer what the ray to the clicked spot meets.
///
/// The same shape as [`teleport_cursor`], and the same packet: the interesting
/// half is what the click does, in [`report_sight`].
fn sight_cursor(state: &mut WorldState, actor: EntityId) {
    let Some(&Client { connection, .. }) = state.registry.get::<Client>(actor) else {
        return;
    };
    let serial = state.registry.serial_of(actor).map_or(0, |s| s.raw());
    state.raise_target(actor, TargetPurpose::Sight);
    state.send_packet(
        connection,
        &ServerPacket::TargetCursor(TargetCursor {
            cursor_id: CursorId(serial),
            kind: TargetKind::Location,
        }),
    );
}

/// Finish a `.sight`: say what a look from the actor to `to` meets.
///
/// **The shard's own verdict, in the words the client's overlay uses.** Same
/// call — `sight::trace` — same footing, same `Doors::AsTheyStand` every real
/// caller passes. The point is that the two answers can be *read against each
/// other*: the client computes its picture from its own copy of the map and its
/// own live overlay (`docs/sight.md`'s D3), and the one thing it can be wrong
/// about is a door the shard has not told it about. This is how that stops being
/// an assumption. It changes nothing in the world.
pub(crate) fn report_sight(state: &mut WorldState, actor: EntityId, to: Point) {
    let Some(&Position(from)) = state.registry.get::<Position>(actor) else {
        return;
    };
    let facet = state.facet_of(actor);
    let trace = openshard_movement::sight::trace(
        &state.footing(facet, openshard_map::overlay::Doors::AsTheyStand),
        from,
        to,
        // The whole line: a game master asking this wants the tiles behind the
        // wall as much as the wall — "it is the second door, not the first" is
        // the answer that is hard to get any other way.
        openshard_movement::sight::Extent::WholeLine,
    );
    let mut lines = vec![format!(
        "sight ({}, {}, {}) → ({}, {}, {}): {}",
        from.x,
        from.y,
        from.z,
        to.x,
        to.y,
        to.z,
        match trace.clear() {
            true => "clear",
            false => "blocked",
        }
    )];
    // The *other* half of a refusal, and the half the ray cannot carry. A shot
    // is barred by two tests — `in_range` and then the sight line — and the
    // second is all this command used to report, so a clear look at something
    // fourteen tiles away read as permission when the bow reaches ten. The reach
    // is the one the attacker would commit to right now, off the weapon in its
    // hands: `openshard_combat::reach_of` is what the commit itself reads.
    let reach = openshard_combat::reach_of(state, actor);
    let distance = from.distance(to);
    lines.push(format!(
        "  {distance} tiles away, reach {}: {}",
        reach.get(),
        match distance <= u32::from(reach.get()) {
            true => "within",
            false => "out of reach",
        }
    ));
    // Every stop, not only the first: the first is the verdict and the rest are
    // what a person moving one tile sideways is about to meet instead.
    let stops = trace.steps.iter().filter(|step| step.stop.is_some()).count();
    lines.extend(trace.steps.iter().filter_map(|step| {
        let stop = step.stop?;
        let ray = step.ray_z;
        Some(match stop {
            openshard_movement::sight::Stop::Ground { z } => {
                format!(
                    "  ({}, {}): ground z {z} over ray {ray}",
                    step.tile.x, step.tile.y
                )
            }
            openshard_movement::sight::Stop::Static {
                graphic,
                base,
                top,
                wallish,
            } => format!(
                "  ({}, {}): {} {:#06x} z {base}..{top} over ray {ray}",
                step.tile.x,
                step.tile.y,
                match wallish {
                    true => "wall",
                    false => "platform",
                },
                graphic.0
            ),
            openshard_movement::sight::Stop::Door => {
                format!("  ({}, {}): a shut door, ray {ray}", step.tile.x, step.tile.y)
            }
        })
    }));
    if stops == 0 {
        lines.push(format!(
            "  {} tiles crossed, nothing in the way",
            trace.steps.len()
        ));
    }
    for line in lines {
        notify(state, actor, &line);
    }
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
    let backpack = openshard_state::equipped_items(state, actor_serial)
        .find(|(_, worn)| worn.layer == openshard_items::BACKPACK_LAYER)
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
    if items::spawn_item(state, graphic, Hue(0), amount, stackable, at, facet).is_some() {
        notify(
            state,
            actor,
            &format!("Spawned {amount} of {:#06x} at your feet.", graphic.0),
        );
    }
}

/// `.addgold <amount>` — put gold into the actor's own pack.
///
/// `items::give` is the same call a vendor sale makes (`vendor::sell`), so a
/// pile from this command behaves exactly like one earned in play — it merges
/// onto an existing pile in the backpack or starts a new one, with no capacity
/// check, `.spellbook`'s reason for calling `give` directly rather than
/// `give_to_backpack`.
fn add_gold(state: &mut WorldState, actor: EntityId, args: &[&str]) {
    let Some(amount) = args
        .first()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|&amount| amount > 0)
    else {
        notify(state, actor, "Usage: .addgold <amount>");
        return;
    };
    let Some(serial) = state.registry.serial_of(actor) else {
        return;
    };
    let Some(backpack) = items::backpack_of(state, serial) else {
        notify(state, actor, "You have no backpack.");
        return;
    };
    if items::give(state, backpack, items::GOLD_GRAPHIC, Hue(0), amount).is_some() {
        notify(state, actor, &format!("{amount} gold appears in your pack."));
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

/// `.skill <name> <value>` — set one of your own skills, the value in whole
/// points.
///
/// The counterpart of `.set`, and missing until now: there was a `Command::SetSkill`
/// that only tests reached, so the one way to move a skill on a running shard was
/// to train it. That makes half of this engine hard to try — a miner needs Mining
/// before a vein gives anything, and a smith needs Blacksmithy before the ore is
/// worth digging.
///
/// **Whole points, not tenths**, because "95" is what a player reads off their own
/// window and `.skill mining 950` is a trap laid for whoever types the obvious
/// thing. The engine's own unit is tenths and the conversion happens here, at the
/// one place a person types a number.
fn set_skill(state: &mut WorldState, actor: EntityId, args: &[&str]) {
    let (Some(name), Some(points)) = (args.first(), args.get(1)) else {
        notify(
            state,
            actor,
            "Usage: .skill <name> <value>, e.g. .skill mining 95",
        );
        return;
    };
    let Some(skill) = openshard_state::skill::Skill::from_name(name) else {
        notify(state, actor, &format!("There is no skill called '{name}'."));
        return;
    };
    // One decimal, since the window draws one: ".skill mining 95.5" is a value a
    // player can see and so a value they will type.
    let Some(tenths) = points.split_once('.').map_or_else(
        || points.parse::<u16>().ok().and_then(|whole| whole.checked_mul(10)),
        |(whole, fraction)| {
            let whole = whole.parse::<u16>().ok()?;
            let tenth = fraction.parse::<u16>().ok().filter(|_| fraction.len() == 1)?;
            whole.checked_mul(10)?.checked_add(tenth)
        },
    ) else {
        notify(state, actor, "That is not a skill value. Try 95, or 95.5.");
        return;
    };
    let Some(serial) = state.registry.serial_of(actor) else {
        return;
    };
    skills::set_skill(state, serial, skill.id(), tenths);
    // Read back rather than echoed: the cap is the sheet's and the shard's, so
    // what was asked for is not always what was set.
    let set = openshard_skills::skill_value(state, actor, skill);
    notify(
        state,
        actor,
        &format!("Set {} to {}.{}.", skill.info().name, set / 10, set % 10),
    );
}

/// `.house <multi id|@template> [x y z]` — put a house at your feet or at an
/// editor tile.
///
/// The staff half of housing, and the whole of H1's front door: a deed and the
/// cursor that draws the house under it are H2, and until they exist this is how
/// a house gets onto the ground at all. The id is the multi's, hex or decimal,
/// with or without the `0x4000` the wire carries — `place` masks either.
///
/// An `@template` is the JSON file stem read from `openshard-houses/` at shard
/// boot. It is placed on a customisable foundation and immediately becomes that
/// design, so the shard persists and distributes the actual component list.
/// With no coordinates this keeps `.add`'s at-your-feet convention. The map
/// editor supplies all three coordinates after showing the same shape under its
/// pointer.
fn place_house(state: &mut WorldState, actor: EntityId, args: &[&str]) {
    let Some(request) = args.first() else {
        notify(
            state,
            actor,
            "Usage: .house <multi id|@template> [x y z], e.g. .house 0x64",
        );
        return;
    };
    let template = request.strip_prefix('@').map(|name| {
        state
            .house_templates
            .get(name)
            .cloned()
            .map(|components| (name, components))
    });
    let template = match template {
        Some(Some((name, components))) => {
            let Some((foundation, components)) =
                openshard_housing::fit_design_to_foundation(&state.multis, components)
            else {
                notify(
                    state,
                    actor,
                    "This template does not fit any custom-house foundation in this client.",
                );
                return;
            };
            Some((name, foundation, components))
        }
        Some(None) => {
            notify(state, actor, "No imported house template has that name.");
            return;
        }
        None => None,
    };
    let multi = match template.as_ref() {
        Some((_, foundation, _)) => *foundation,
        None => {
            let Some(raw_multi) = parse_u16(request) else {
                notify(
                    state,
                    actor,
                    "Usage: .house <multi id|@template> [x y z], e.g. .house 0x64",
                );
                return;
            };
            openshard_protocol::wire::MultiId::from_graphic(Graphic(raw_multi))
        }
    };
    let Some(&Position(feet)) = state.registry.get::<Position>(actor) else {
        return;
    };
    let at = match args.get(1..) {
        Some([x, y, z]) => match (x.parse::<u16>(), y.parse::<u16>(), z.parse::<i8>()) {
            (Ok(x), Ok(y), Ok(z)) => Point::new(x, y, z),
            _ => {
                notify(state, actor, "House coordinates must be x y z numbers.");
                return;
            }
        },
        Some([]) => feet,
        _ => {
            notify(
                state,
                actor,
                "Usage: .house <multi id|@template> [x y z], e.g. .house @legacy-inn 1400 1600 0",
            );
            return;
        }
    };
    let facet = state.facet_of(actor);
    let Some(owner) = state.registry.serial_of(actor) else {
        return;
    };
    let template_name = template.as_ref().map(|(name, _, _)| *name);
    let placement = match template {
        Some((_, _, components)) => {
            openshard_housing::place_design(state, actor, at, facet, multi, owner, components)
        }
        None => openshard_housing::place(state, actor, at, facet, multi, owner),
    };
    match placement {
        Ok(_house) => {
            if let Some(name) = template_name {
                notify(
                    state,
                    actor,
                    &format!(
                        "The imported house {name:?} stands at ({}, {}, {}) (revision 1).",
                        at.x, at.y, at.z
                    ),
                );
            } else {
                notify(
                    state,
                    actor,
                    &format!(
                        "A house ({:#06x}) stands at ({}, {}, {}).",
                        multi.0, at.x, at.y, at.z
                    ),
                );
            }
        }
        Err(refusal) => notify(state, actor, refusal.message()),
    }
}

/// `.deed <multi id>` — put a house deed in your pack.
///
/// The other half of `.house`, and the one that exercises the *player's* path:
/// `.house` places directly, which is the staff shortcut, while a deed raises the
/// `0x99` cursor and goes through every placement rule with the house drawn under
/// the pointer. Until a vendor sells one this is the only way to hold a deed, and
/// without it the whole H2 path is unreachable on a running shard.
fn make_deed(state: &mut WorldState, actor: EntityId, args: &[&str]) {
    let Some(raw_multi) = args.first().and_then(parse_u16) else {
        notify(state, actor, "Usage: .deed <multi id>, e.g. .deed 0x64");
        return;
    };
    let Some(&Position(at)) = state.registry.get::<Position>(actor) else {
        return;
    };
    let facet = state.facet_of(actor);
    // ServUO's own deed graphic, `0x14F0` — a rolled scroll.
    let Some(deed) = items::spawn_item(state, Graphic(0x14F0), Hue(0), 1, false, at, facet) else {
        notify(state, actor, "No room for a deed.");
        return;
    };
    let multi = openshard_protocol::wire::MultiId::from_graphic(Graphic(raw_multi));
    state
        .registry
        .insert(deed, openshard_state::components::HouseDeed { multi });
    state.registry.insert(
        deed,
        openshard_state::components::Name(format!("a house deed ({:#06x})", multi.0)),
    );
    notify(state, actor, "A house deed is at your feet.");
}

/// `.hdemolish [house serial]` — pull down a named house, or the one underfoot.
///
/// The sign has this button and only shows it to the owner. Staff get a command
/// as well, because the case it is for is the one where the sign is no help: an
/// abandoned house whose owner will never open it, standing on a plot somebody
/// else wants.
fn demolish_house(state: &mut WorldState, actor: EntityId, args: &[&str]) {
    let house = match args {
        [] => {
            let Some(&openshard_state::components::Position(at)) =
                state.registry.get::<openshard_state::components::Position>(actor)
            else {
                return;
            };
            let facet = state.facet_of(actor);
            let Some(house) = openshard_housing::house_at(state, at, facet) else {
                notify(state, actor, "You are not standing in a house.");
                return;
            };
            house
        }
        [raw] => {
            let Some(serial) = parse_u32(raw).and_then(Serial::new) else {
                notify(state, actor, "Usage: .hdemolish [house serial]");
                return;
            };
            let Some(house) = state.registry.entity_of(serial) else {
                notify(state, actor, "That house no longer exists.");
                return;
            };
            if state.registry.has::<openshard_state::components::House>(house) {
                house
            } else if let Some(fixture_house) = state
                .registry
                .get::<HouseDoor>(house)
                .map(|door| door.house)
                .or_else(|| state.registry.get::<HouseSign>(house).map(|sign| sign.house))
            {
                let Some(house) = state.registry.entity_of(fixture_house) else {
                    notify(state, actor, "That house no longer exists.");
                    return;
                };
                house
            } else {
                // Map Editor's whole-house picker names a live item. Imported
                // loose doors are live items too. A house fixture took the arm
                // above, so only an unrelated door is deleted on its own.
                if state.registry.has::<openshard_state::components::Door>(house) {
                    items::consume(state, serial, 0);
                    notify(state, actor, "The door is removed.");
                } else {
                    notify(state, actor, "That object is not a house or door.");
                }
                return;
            }
        }
        _ => {
            notify(state, actor, "Usage: .hdemolish [house serial]");
            return;
        }
    };
    openshard_housing::decay::demolish(state, house);
    notify(
        state,
        actor,
        "The house comes down. What it held is in the crate.",
    );
}

/// `.boat <multi id>` — put a ship on the water at your feet.
///
/// `.house`'s shape, and the whole of B1's front door: until a shipwright sells
/// one this is the only way a ship reaches the water at all. Staff-exempt on the
/// judgements about the berth — a game master may moor in a fountain — but not
/// on the arithmetic, so a hull that would land off the edge of the world is
/// still refused. `docs/boats.md`'s B1.
fn launch_boat(state: &mut WorldState, actor: EntityId, args: &[&str]) {
    let Some(raw_multi) = args.first().and_then(parse_u16) else {
        notify(state, actor, "Usage: .boat <multi id>, e.g. .boat 0x0C");
        return;
    };
    let Some(&openshard_state::components::Position(at)) =
        state.registry.get::<openshard_state::components::Position>(actor)
    else {
        return;
    };
    let facet = state.facet_of(actor);
    let Some(owner) = state.registry.serial_of(actor) else {
        return;
    };
    let multi = openshard_protocol::wire::MultiId::from_graphic(Graphic(raw_multi));
    match openshard_boats::place(state, actor, at, facet, multi, owner) {
        Ok(_) => notify(
            state,
            actor,
            &format!("A ship ({:#06x}) is moored at your feet.", multi.0),
        ),
        Err(refusal) => notify(state, actor, refusal.message()),
    }
}

/// `.sail <direction|stop> [fast]` — steer the ship you are standing on.
///
/// **Not the tiller.** B6's tiller is an item a player speaks keywords to, and
/// it is what this stands in for until it exists: the point of the verb is that
/// the *steering* can be exercised — the cadence, the manifest, the stop against
/// a rock — without the item and the speech path being written first, which is
/// `.hdesign`'s argument one noun over.
///
/// The ship is the one under your feet, so a game master steers by standing on
/// the deck. That is also what the tiller will do, from the other end: it will
/// be an item on the ship rather than a serial anybody may name.
fn sail_boat(state: &mut WorldState, actor: EntityId, args: &[&str]) {
    let Some(&openshard_state::components::Position(at)) =
        state.registry.get::<openshard_state::components::Position>(actor)
    else {
        return;
    };
    let facet = state.facet_of(actor);
    let Some(boat) = openshard_boats::boat_at(state, at, facet) else {
        notify(state, actor, "You are not aboard a ship.");
        return;
    };

    let word = args.first().copied().unwrap_or_default();
    if word.eq_ignore_ascii_case("stop") {
        openshard_boats::furl(state, boat);
        notify(state, actor, "The ship comes to a stop.");
        return;
    }
    let Some(direction) = compass(word) else {
        notify(state, actor, "Usage: .sail <n|ne|e|se|s|sw|w|nw|stop> [fast]");
        return;
    };
    let fast = args.get(1).is_some_and(|word| word.eq_ignore_ascii_case("fast"));
    openshard_boats::set_course(state, boat, direction, fast);
    notify(state, actor, "The ship gets under way.");
}

/// A compass point as a direction, which is how a person names one and how the
/// tiller's keywords will arrive too.
fn compass(word: &str) -> Option<Direction> {
    Some(match word.to_ascii_lowercase().as_str() {
        "n" | "north" => Direction::North,
        "ne" | "northeast" => Direction::NorthEast,
        "e" | "east" => Direction::East,
        "se" | "southeast" => Direction::SouthEast,
        "s" | "south" => Direction::South,
        "sw" | "southwest" => Direction::SouthWest,
        "w" | "west" => Direction::West,
        "nw" | "northwest" => Direction::NorthWest,
        _ => return None,
    })
}

/// `.hdesign <multi id>` — give the house you are standing in another multi's
/// shape.
///
/// The whole of C1's front door, and deliberately not an editor: it proves the
/// design *seam* — the storage, the restore, the walls, the sign, the allowance
/// and `0xD8` — with components that came out of a client file, so a bug in any
/// of those is a bug in that thing rather than a bug in an editor nobody has
/// written. `docs/customisation.md`'s C1 argues the order.
///
/// It is also the only way a shard ships its own architecture today: a pack can
/// give a house a shape no `multi.mul` entry has, without editing a client file.
fn design_house(state: &mut WorldState, actor: EntityId, args: &[&str]) {
    let Some(raw_multi) = args.first().and_then(parse_u16) else {
        notify(state, actor, "Usage: .hdesign <multi id>, e.g. .hdesign 0x65");
        return;
    };
    let Some(&openshard_state::components::Position(at)) =
        state.registry.get::<openshard_state::components::Position>(actor)
    else {
        return;
    };
    let facet = state.facet_of(actor);
    let Some(house) = openshard_housing::house_at(state, at, facet) else {
        notify(state, actor, "You are not standing in a house.");
        return;
    };
    let multi = openshard_protocol::wire::MultiId::from_graphic(Graphic(raw_multi));
    // Straight out of the client files, which is the point: a design this shard
    // *invented* is C3's editor, and this one is a shape already known to draw.
    let components = state.multis.components(multi.0).to_vec();
    if components.is_empty() {
        notify(state, actor, "No multi by that id.");
        return;
    }
    match openshard_housing::design::redesign(state, actor, house, components) {
        Ok(revision) => notify(
            state,
            actor,
            &format!("The house is rebuilt to {:#06x} (revision {revision}).", multi.0),
        ),
        Err(refusal) => notify(state, actor, refusal.message()),
    }
}

/// `.hfriend`, `.hcoowner`, `.hdrop`, `.hban`, `.hunban` — change the house you
/// are standing in, by clicking whom.
///
/// A **cursor** and not a name, for `.tele`'s reason: naming a mobile needs a
/// lookup this engine has no verb for, and picking one is what the reference's
/// own house sign does. When a sign exists it is a window over exactly these
/// five calls; until then this is how the rules are reachable at all, which is
/// what `.key` and `.trap` are for their own.
fn house_list(state: &mut WorldState, actor: EntityId, change: HouseChange) {
    state.raise_target(actor, TargetPurpose::HouseList { change });
    let Some(&Client { connection, .. }) = state.registry.get::<Client>(actor) else {
        return;
    };
    let Some(serial) = state.registry.serial_of(actor) else {
        return;
    };
    state.send_packet(
        connection,
        &ServerPacket::TargetCursor(TargetCursor {
            cursor_id: CursorId(serial.raw()),
            kind: TargetKind::Object,
        }),
    );
    notify(state, actor, "Whom?");
}

/// Send the actor a private system line — the reply to a command, seen by no one
/// else. A mobile with no client (a scripted GM, say) simply gets no reply.
pub(crate) fn notify(state: &mut WorldState, actor: EntityId, text: &str) {
    state.system_message(actor, text);
}

/// The height a body put at `(x, y)` on `facet` stands at, if the facet is
/// loaded and something there can hold one.
///
/// **The world's answer and not the land's.** This used to be
/// `MapTerrain::ground_z`, which is the land tile's own average and reads no
/// static and no live cover at all — so `.go` onto a pier, a bridge, a dock or a
/// house's floor put the game master *under* it, which over facet 0 is 25,816 of
/// the 27,052 pier and bridge decks (a median ten units down; see `movement`'s
/// `arrival_survey`). The ground is still what the question is asked near, so a
/// `.go` into a building lands on its ground floor rather than climbing it.
fn ground_z(state: &WorldState, facet: Facet, x: u16, y: u16) -> Option<i8> {
    let tile = Tile::new(x, y);
    let near = state
        .map_terrain(facet)
        .and_then(|terrain| terrain.ground_z(tile))?;
    // Nothing there a body can stand on — a mountain, the open sea — still puts
    // the game master at the tile's own ground rather than refusing the command:
    // `.go` is how staff get somewhere unreachable, and `.go x y z` is how they
    // say where exactly.
    Some(
        openshard_movement::arrival_z(
            &state.footing(facet, openshard_map::overlay::Doors::AsTheyStand),
            tile,
            i32::from(near),
            openshard_movement::PLAYER_HEIGHT,
        )
        .and_then(|z| i8::try_from(z).ok())
        .unwrap_or(near),
    )
}

/// Parse a `u16` written in hex (`0x1bf2`) or decimal — item ids are quoted both.
fn parse_u16(text: &&str) -> Option<u16> {
    let text = *text;
    text.strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .map_or_else(|| text.parse().ok(), |hex| u16::from_str_radix(hex, 16).ok())
}

/// Parse a `u32` written in hex or decimal — object serials use the full word.
fn parse_u32(text: &str) -> Option<u32> {
    text.strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .map_or_else(|| text.parse().ok(), |hex| u32::from_str_radix(hex, 16).ok())
}

/// Parse a signed height, decimal only.
fn parse_i8(text: &&str) -> Option<i8> {
    text.parse().ok()
}
