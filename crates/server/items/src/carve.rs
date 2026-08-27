//! Carving animal carcasses with a dagger or knife.
//!
//! Carving is an item action, not a skill check: Classic UO lets a player use a
//! blade on an animal corpse and puts the meat, hides, and feathers in that
//! corpse. The corpse retains a `carved` bit so taking its contents cannot make
//! the same body yield resources again.

use super::*;
use openshard_protocol::target::{TargetCursor, TargetKind};
use openshard_state::components::{CorpseBody, Drawn};

/// A dagger, butcher knife, cleaver, or skinning knife may carve a carcass.
#[must_use]
pub const fn is_carving_tool(graphic: Graphic) -> bool {
    matches!(graphic.0, 0x0F52 | 0x13F6 | 0x0EC3 | 0x0EC4)
}

/// The uncooked ribs produced by an ordinary animal.
const RAW_RIBS: Graphic = Graphic(0x09F1);
/// The uncooked bird produced by a bird or chicken.
const RAW_BIRD: Graphic = Graphic(0x09B9);
/// The pile of hides a tanner turns into leather.
const HIDES: Graphic = Graphic(0x1078);
/// A bird's feathers.
const FEATHERS: Graphic = Graphic(0x1BD1);

/// What one animal body yields when carved. These are intentionally keyed by
/// body graphic, rather than creature name: a renamed cow is still a cow, and a
/// player's chosen name must never turn their corpse into a resource table.
#[derive(Clone, Copy)]
struct Yield {
    ribs: u16,
    hides: u16,
    feathers: u16,
    bird: bool,
}

const fn yield_of(body: Graphic) -> Option<Yield> {
    let (ribs, hides, feathers, bird) = match body.0 {
        // Birds
        0x0006 | 0x00D0 => (0, 0, 10, true),
        // Small game
        0x00C9 | 0x00CD | 0x00D7 | 0x00D9 | 0x00EE => (1, 1, 0, false), // cat, rabbit, rats, dog
        // Farm animals and mounts
        0x00CB | 0x0122 => (3, 3, 0, false),                   // pig, boar
        0x00CF | 0x00D1 | 0x00DC | 0x0124 => (2, 3, 0, false), // sheep, goat, llamas
        0x00D8 | 0x00E7 => (10, 10, 0, false),                 // cow
        0x00C8 | 0x00CC | 0x00E2 | 0x00E4 | 0x0123 => (5, 5, 0, false), // horses
        0x00D2 | 0x00DA | 0x00DB => (5, 5, 0, false),          // ostards
        // Wild game
        0x00EA | 0x00ED => (4, 4, 0, false),          // hart, hind
        0x00A7 | 0x00D4 | 0x00D5 => (5, 8, 0, false), // bears
        0x0017 | 0x0019 | 0x001B | 0x00E1 => (3, 4, 0, false), // wolves
        0x00CA => (4, 8, 0, false),                   // alligator
        0x00DD => (4, 8, 0, false),                   // walrus
        0x0097 => (4, 4, 0, false),                   // dolphin
        _ => return None,
    };
    Some(Yield {
        ribs,
        hides,
        feathers,
        bird,
    })
}

/// A double-clicked blade raises the object cursor used to select a carcass.
/// Returns whether the item was a carving blade at all.
pub fn use_carving_tool(state: &mut WorldState, carver: EntityId, tool: EntityId) -> bool {
    let Some(graphic) = state.registry.get::<Drawn>(tool).map(|drawn| drawn.id) else {
        return false;
    };
    if !is_carving_tool(graphic) {
        return false;
    }
    let (Some(&Client { connection, .. }), Some(serial)) = (
        state.registry.get::<Client>(carver),
        state.registry.serial_of(carver),
    ) else {
        return true;
    };
    if !in_reach(state, tool, carver) {
        return true;
    }
    state.raise_target(carver, openshard_state::TargetPurpose::Carve { tool });
    state.system_message(carver, "What do you wish to carve?");
    state.send_packet(
        connection,
        &ServerPacket::TargetCursor(TargetCursor {
            cursor_id: CursorId(serial.raw()),
            kind: TargetKind::Object,
        }),
    );
    true
}

/// Apply a blade to an object selected by [`use_carving_tool`].
///
/// Every mutable fact is re-checked here: the tool and corpse can both move or
/// disappear between the two packets, and the target serial is supplied by the
/// client.
pub fn carve(state: &mut WorldState, carver: EntityId, tool: EntityId, target: Option<Serial>) {
    let Some(target) = target.and_then(|serial| state.registry.entity_of(serial)) else {
        state.system_message(carver, "That is not a carcass you can carve.");
        return;
    };
    let tool_is_usable = state
        .registry
        .get::<Drawn>(tool)
        .is_some_and(|drawn| is_carving_tool(drawn.id))
        && in_reach(state, tool, carver);
    if !tool_is_usable {
        return;
    }
    if !in_reach(state, target, carver) {
        state.system_message(carver, "That is too far away.");
        return;
    }
    let (Some(body), Some(story), Some(corpse_serial)) = (
        state.registry.get::<CorpseBody>(target).copied(),
        state.registry.get::<Corpse>(target).cloned(),
        state.registry.serial_of(target),
    ) else {
        state.system_message(carver, "That is not a carcass you can carve.");
        return;
    };
    let Some(yielded) = yield_of(body.body) else {
        state.system_message(carver, "You cannot carve anything useful from that corpse.");
        return;
    };
    if story.carved {
        state.system_message(carver, "That carcass has already been carved.");
        return;
    }

    if yielded.bird {
        let _ = give(state, corpse_serial, RAW_BIRD, Hue(0), 1);
    } else if yielded.ribs != 0 {
        let _ = give(state, corpse_serial, RAW_RIBS, Hue(0), u32::from(yielded.ribs));
    }
    if yielded.hides != 0 {
        let _ = give(state, corpse_serial, HIDES, Hue(0), u32::from(yielded.hides));
    }
    if yielded.feathers != 0 {
        let _ = give(
            state,
            corpse_serial,
            FEATHERS,
            Hue(0),
            u32::from(yielded.feathers),
        );
    }
    state.registry.insert(
        target,
        Corpse {
            carved: true,
            ..story
        },
    );
    state.system_message(
        carver,
        "You carve the carcass and place the resources in the corpse.",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dagger_is_a_carving_tool() {
        assert!(is_carving_tool(Graphic(0x0F52)));
        assert!(!is_carving_tool(Graphic(0x0F5C)));
    }

    #[test]
    fn only_animal_bodies_have_yields() {
        assert!(yield_of(Graphic(0x00D8)).is_some(), "cow");
        assert!(yield_of(Graphic(0x00D0)).is_some(), "chicken");
        assert!(yield_of(Graphic(0x0011)).is_none(), "orc");
    }
}
