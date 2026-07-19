//! Making a creature or townsperson: the one function that turns a spec into a
//! living mobile, and the event that announces it.

use openshard_entities::{EntityId, Serial, SerialKind};
use openshard_movement::Walker;
use openshard_protocol::{Direction, Facing, Notoriety, Point};
use openshard_state::components::{
    body_opens_doors, Aggression, Banker, Body, Brain, Facet, Heading, Hitpoints, MeleeDamage,
    Movement, Name, Npc, Position, Resistance, SwingSpeed,
};
use openshard_state::WorldState;
use tracing::{debug, warn};

use openshard_items as items;

use crate::banker_name;

/// How far an idle banker may drift from its post before it heads back — a couple
/// of tiles of shuffling near the counter, not a stroll out the door.
const BANKER_WANDER: u8 = 2;

/// A creature or NPC appeared in the world.
///
/// The mobile counterpart of `PlayerEntered`, for the mobiles no client drives
/// — a spawned creature. A script reads it to learn a fresh mobile's serial, the
/// name it needs to take control of it (`op_control`) or act on it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MobileSpawned {
    /// The entity.
    pub entity: EntityId,
    /// Its wire identity.
    pub serial: Serial,
    /// Where it appeared.
    pub position: Point,
}

/// Everything [`spawn`] needs — a plain bundle, so the one function that makes a
/// creature takes one argument instead of eleven.
#[derive(Debug)]
pub struct SpawnSpec {
    pub body: u16,
    pub hue: u16,
    pub hits: u16,
    pub notoriety: u8,
    pub damage: u16,
    pub resistance: u8,
    pub swing: u64,
    pub sight: u8,
    /// Whether it starts fights (2), answers them (1), or only runs (0).
    pub aggression: u8,
    pub wander: bool,
    pub position: Point,
    pub facet: u8,
    /// A name the client shows on single-click, if any. Townsfolk have one.
    pub name: Option<String>,
    /// Whether this mobile is a banker — it answers "bank".
    pub banker: bool,
    /// Worn clothing and gear, `(graphic, layer, hue)` — so it is not naked.
    pub equipment: Vec<(u16, u8, u16)>,
}

/// Put a mobile in the world. See `Command::SpawnMobile`.
///
/// The same bundle a player is built from — a body, a position, a facing, a
/// walker, hit points — minus the `Client`. That absence is the whole
/// difference between a creature and a person; everything that draws or moves
/// a mobile already treats "has a client" as the question, so a spawned one
/// falls out of the machinery already there.
pub fn spawn(state: &mut WorldState, spec: SpawnSpec) -> Option<EntityId> {
    let SpawnSpec {
        body,
        hue,
        hits,
        notoriety,
        damage,
        resistance,
        swing,
        sight,
        aggression,
        wander,
        position,
        facet,
        name,
        banker,
        equipment,
    } = spec;
    let facet = if state.facets.contains_key(&facet) {
        facet
    } else {
        warn!(facet, "unloaded facet; spawning the mobile on the default");
        state.default_facet
    };
    // Drop the mobile onto the ground, the way a client's spawner does: the
    // pack gives x/y and a rough height, and the floor it stands on — the top
    // of the static surface there, a building's raised floor and all — is the
    // map's to say. Without this a banker sinks to the given z and reads as
    // "inside a wall".
    let position = match state
        .facet_state(facet)
        .terrain
        .as_ref()
        .and_then(|t| t.stand_z(position.x, position.y, i32::from(position.z)))
        .and_then(|z| i8::try_from(z).ok())
    {
        Some(z) => Point::new(position.x, position.y, z),
        None => position,
    };
    let (entity, serial) = match state.registry.spawn_with_serial(SerialKind::Mobile) {
        Ok(pair) => pair,
        Err(error) => {
            warn!(?error, "out of mobile serials; not spawning");
            return None;
        }
    };
    let hits = hits.max(1);
    let facing = Facing::walking(Direction::South);
    state.registry.insert(entity, Body { id: body, hue });
    state.registry.insert(entity, Position(position));
    state.registry.insert(entity, Heading(facing));
    state.registry.insert(entity, Facet(facet));
    state.registry.insert(
        entity,
        Hitpoints {
            current: hits,
            max: hits,
        },
    );
    state
        .registry
        .insert(entity, Notoriety::from_bits(notoriety));
    state
        .registry
        .insert(entity, MeleeDamage { amount: damage });
    state.registry.insert(
        entity,
        Resistance {
            physical: resistance.min(100),
            ..Default::default()
        },
    );
    // Zero means "derive from dexterity", so a script that does not care about
    // pace names no number and gets the wrestling formula. A non-zero value
    // pins an exact cadence — a special creature that ignores its stats.
    if swing != 0 {
        state.registry.insert(entity, SwingSpeed { ticks: swing });
    }
    // A brain only for a creature that needs one — something that hunts or
    // wanders. A pure prop (a shopkeeper standing still) gets none and never
    // enters `think`. `Combat` it earns when it first picks a fight.
    let aggression = Aggression::from_bits(aggression);
    // A brain for anything that hunts, drifts, or must answer or flee a blow —
    // which is everything but the aggressive-but-blind prop (sight 0), the old
    // meaning of "no brain".
    if sight > 0 || wander || aggression != Aggression::Aggressive {
        state.registry.insert(
            entity,
            Brain {
                sight,
                wander,
                next_think: 0,
                guard_until: 0,
                opens_doors: body_opens_doors(body),
                aggression,
            },
        );
    }
    // A banker earns a generated name and title ("Rowena the banker") when the
    // spawn did not name it, the townsperson AI base (so it greets, faces and
    // keeps near its post), and the service mark that answers "bank".
    let name = if banker && name.is_none() {
        Some(banker_name(&mut state.rng))
    } else {
        name
    };
    if let Some(name) = name {
        state.registry.insert(entity, Name(name));
    }
    if banker {
        state.registry.insert(entity, Banker { next_greet: 0 });
        state.registry.insert(
            entity,
            Npc {
                home: position,
                wander: BANKER_WANDER,
                next_beat: 0,
            },
        );
    }
    // Dress it before the reveal, so the clothing rides in the `0x78` that
    // draws it — a naked banker is a bug that looks like nudity.
    for (graphic, layer, item_hue) in equipment {
        items::equip_worn_item(state, serial, graphic, item_hue, layer);
    }
    state
        .registry
        .insert(entity, Movement(Walker::new(position, facing)));
    state
        .facet_state_mut(facet)
        .sectors
        .insert(entity, position);
    state.reveal(entity);
    // Say who and where, so a script can take control of it: the mobile
    // counterpart of `PlayerEntered`, and how `op_control` learns a serial.
    state.bus.send(MobileSpawned {
        entity,
        serial,
        position,
    });
    debug!(%serial, body, "mobile spawned");
    Some(entity)
}
