//! Pets: what a tamed creature is, and what it does when it is told something.
//!
//! `npc` owns what a creature *is* — it spawns them, dresses them and gives them a
//! brain — so making one somebody's belongs here rather than in `skills`, which
//! only decides that a taming resolved. The pet's *beat* is `ai::pet_beat`, for the
//! same reason a wild creature's is `ai::think_one`: deciding a step is the AI's.

use openshard_entities::EntityId;
use openshard_protocol::world::FollowerSlots;
use openshard_state::WorldState;

/// Make a creature somebody's — or make it turn on them.
///
/// `skills` decides that a taming resolved; this is what a tamed creature *is*, and
/// that has always been this crate's business (it owns `spawn`, the dressing and
/// the brain a creature gets). ServUO's `BaseCreature.Tame`: the creature takes an
/// owner, goes friendly, stops hunting and follows.
///
/// An angered beast is the other half of the same call, because it is the same
/// decision: it simply gets a `Combat` aimed at the would-be tamer instead.
pub fn tame(
    state: &mut WorldState,
    creature: EntityId,
    tamer: EntityId,
    slots: FollowerSlots,
    angered: bool,
) {
    let Some(owner) = state.registry.serial_of(tamer) else {
        return;
    };
    if angered {
        state.registry.insert(
            creature,
            openshard_state::components::Combat::creature_engaged(owner, state.ticks),
        );
        return;
    }
    state.registry.insert(
        creature,
        openshard_state::components::Pet {
            owner,
            slots,
            order: openshard_state::components::PetOrder::Follow,
            order_target: None,
        },
    );
    // A pet is nobody's enemy: it drops whatever it was fighting, and its bar turns
    // the friendly green every client draws a controlled creature in.
    state.disengage(creature);
    state
        .registry
        .insert(creature, openshard_protocol::mobile::Notoriety::Friend);
    // It keeps its brain — a pet is a creature with an owner, not a second kind of
    // mobile — but stops hunting on its own account. A creature spawned with no
    // brain at all (a prop horse, sight zero) is *given* one here: without it
    // nothing would ever beat, and a pet that never beats never follows.
    let mut brain = state
        .registry
        .get::<openshard_state::components::Brain>(creature)
        .copied()
        .unwrap_or(openshard_state::components::Brain {
            sight: openshard_protocol::world::Sight(0),
            wander: false,
            next_think: state.ticks,
            guard_until: openshard_state::WorldTick::ZERO,
            opens_doors: false,
            aggression: openshard_protocol::world::Aggression::Defensive,
            beat_ticks: 0,
        });
    brain.aggression = openshard_protocol::world::Aggression::Defensive;
    brain.wander = false;
    state.registry.insert(creature, brain);
    state.broadcast_move(creature);
}

/// The words a pet answers to, and what each means.
///
/// ServUO's `BaseAI.OnSpeech` matches the client's *keyword ids* (`0x155`…`0x165`,
/// which the client encodes into the `0xAD` packet when it recognises a phrase);
/// this matches the words, because the parser skips that keyword block and the
/// text is what arrives either way. The ids are recorded here so the two can be
/// reconciled if the block is ever decoded.
#[rustfmt::skip]
const PET_ORDERS: &[(&str, openshard_state::components::PetOrder)] = &[
    ("kill",    openshard_state::components::PetOrder::Attack), // 0x0157 *kill
    ("attack",  openshard_state::components::PetOrder::Attack),
    ("come",    openshard_state::components::PetOrder::Come),   // 0x0155 *come
    ("follow",  openshard_state::components::PetOrder::Follow), // 0x015A *follow
    ("stay",    openshard_state::components::PetOrder::Stay),   // 0x015E *stay
    ("guard",   openshard_state::components::PetOrder::Guard),  // 0x015B *guard
    ("stop",    openshard_state::components::PetOrder::Stop),   // 0x0161 *stop
];

/// How far a pet hears its owner — ServUO's control range.
const PET_HEARING: u32 = 12;

/// A player said something near their pets: obey it, if it was an order.
///
/// The command surface is "all <order>" for everything you own within earshot, and
/// "<name> <order>" for one of them — the two forms every UO client's macros send.
/// Nothing else is touched: a word that is not an order is simply speech, and a pet
/// that is not yours never listens.
pub fn hear_pet_order(state: &mut WorldState, speaker: EntityId, text: &str) {
    let Some(owner) = state.registry.serial_of(speaker) else {
        return;
    };
    let said = text.to_lowercase();
    let Some(&(_, order)) = PET_ORDERS
        .iter()
        .find(|(word, _)| said.split_whitespace().any(|w| w == *word))
    else {
        return;
    };
    let to_all = said.split_whitespace().any(|w| w == "all");
    let Some(&openshard_state::components::Position(at)) =
        state
            .registry
            .get::<openshard_state::components::Position>(speaker)
    else {
        return;
    };
    let facet = state.facet_of(speaker);
    let mine: Vec<EntityId> = state
        .facet_state(facet)
        .sectors()
        .mobiles_near(at, PET_HEARING)
        .map(|(entity, _)| entity)
        .filter(|&entity| {
            state
                .registry
                .get::<openshard_state::components::Pet>(entity)
                .is_some_and(|pet| pet.owner == owner)
        })
        .collect();
    // "<name> come" picks one; "all come" picks every one of yours in earshot.
    let named: Vec<EntityId> = if to_all {
        mine
    } else {
        mine.into_iter()
            .filter(|&entity| {
                state
                    .registry
                    .get::<openshard_state::components::Name>(entity)
                    .is_some_and(|name| said.contains(&name.0.to_lowercase()))
            })
            .collect()
    };
    // An attack order needs something to attack: whatever the owner is fighting,
    // which is how ServUO's "all kill" works without a second cursor.
    let quarry = state
        .registry
        .get::<openshard_state::components::Combat>(speaker)
        .and_then(|combat| combat.target());
    for pet in named {
        let Some(mut current) = state
            .registry
            .get::<openshard_state::components::Pet>(pet)
            .copied()
        else {
            continue;
        };
        current.order = order;
        current.order_target = quarry;
        state.registry.insert(pet, current);
        // An order to kill points the same `Combat` the AI already drives; anything
        // else clears it, so "stop" really does stop.
        match (order, quarry) {
            (openshard_state::components::PetOrder::Attack, Some(target)) => {
                state.registry.insert(
                    pet,
                    openshard_state::components::Combat::creature_engaged(target, state.ticks),
                );
            }
            (openshard_state::components::PetOrder::Attack, None) => {}
            _ => {
                state.disengage(pet);
            }
        }
    }
}
