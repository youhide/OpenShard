//! Carving animal carcasses with a dagger or knife.
//!
//! Carving is an item action, not a skill check: Classic UO lets a player use a
//! blade on an animal corpse and puts the meat, hides, and feathers in that
//! corpse. The corpse retains a `carved` bit so taking its contents cannot make
//! the same body yield resources again.

use openshard_protocol::item_kind::MaterialId;
use openshard_protocol::target::{
    TargetCursor,
    TargetKind,
};
use openshard_state::components::{
    CorpseBody,
    Drawn,
};

use super::*;
use crate::cut::HIDES_KIND;

/// A dagger, butcher knife, cleaver, or skinning knife may carve a carcass.
#[must_use]
pub const fn is_carving_tool(graphic: Graphic) -> bool {
    matches!(graphic.0, 0x0F52 | 0x13F6 | 0x0EC3 | 0x0EC4)
}

/// The uncooked ribs produced by an ordinary animal.
pub const RAW_RIBS: Graphic = Graphic(0x09F1);
/// The uncooked bird produced by a bird or chicken.
pub const RAW_BIRD: Graphic = Graphic(0x09B9);
/// A bird's feathers.
pub const FEATHERS: Graphic = Graphic(0x1BD1);
/// A raw fish steak, ServUO's `RawFishSteak` — what a fish is worth to a cook.
///
/// Public beside the hides: the fish is the third bridge a blade makes, and the
/// reachability audit reads both of its ends here.
pub const RAW_FISH_STEAK: Graphic = Graphic(0x097A);
/// How many steaks one fish cuts into — `ScissorHelper(from, fish, 4)`.
pub const STEAKS_PER_FISH: u32 = 4;

/// Regular leather — what all but a handful of bodies are worth.
const REGULAR_LEATHER: MaterialId = MaterialId(40);
/// Spined leather: ServUO's `HideType.Spined`.
const SPINED_LEATHER: MaterialId = MaterialId(41);
/// Horned leather: `HideType.Horned`, which upstream puts on the drakes, the
/// wyverns and the sea serpents.
const HORNED_LEATHER: MaterialId = MaterialId(42);
/// Barbed leather: `HideType.Barbed`, the dragons' own and the best hide in the
/// game.
const BARBED_LEATHER: MaterialId = MaterialId(43);

/// Tainted wool — ServUO's `TaintedWool`, what comes off a **dead** woolly
/// creature.
///
/// The distinction is upstream's and is the whole of it: shearing a live sheep
/// pays ordinary [`WOOL`](crate::shear::WOOL), and carving its carcass pays this
/// instead (`BaseCreature.OnCarve`'s `new TaintedWool(wool)`). The spinning wheel
/// knows both, so this is the second fibre the shard has ever been able to grow.
pub const TAINTED_WOOL: Graphic = Graphic(0x101F);

/// What one animal body yields when carved. These are intentionally keyed by
/// body graphic, rather than creature name: a renamed cow is still a cow, and a
/// player's chosen name must never turn their corpse into a resource table.
///
/// Public because carving is a **root of the economy** — leather enters the
/// shard here and nowhere else — and the reachability audit
/// (`openshard_world::economy`) has to be able to ask what a body is worth. Its
/// alternative is a second copy of this table written out by hand somewhere it
/// would quietly drift from this one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CarvedYield {
    /// How many racks of ribs.
    pub ribs:     u16,
    /// How many hides.
    pub hides:    u16,
    /// Which grade the hides are — ServUO's `BaseCreature.HideType`, and the
    /// only thing that makes the tailor's upper three material grades reachable
    /// at all. A separate axis from [`hides`](Self::hides): how many a body
    /// gives and how good they are are two different facts about it.
    pub hide:     MaterialId,
    /// How many feathers.
    pub feathers: u16,
    /// How much **tainted** wool — ServUO's `BaseCreature.Wool`, which only the
    /// sheep overrides and only while it is in fleece.
    ///
    /// A separate column from the fleece a live sheep gives, because it is a
    /// different item: [`crate::shear`] pays `WOOL` and this pays
    /// [`TAINTED_WOOL`]. Keyed by body like everything else here, which is
    /// exactly upstream's own `Body == 0xCF ? 3 : 0` — a shorn sheep wears a
    /// different body and its corpse carries no wool at all.
    pub wool:     u16,
    /// Whether the meat comes off as a bird rather than as ribs.
    pub bird:     bool,
}

/// What carving a body of this graphic pays, or `None` for a body nothing can be
/// cut off. Sweep it over the whole graphic space to enumerate the table.
#[must_use]
pub const fn carved_yield(body: Graphic) -> Option<CarvedYield> {
    let (ribs, hides, feathers, wool, bird) = match body.0 {
        // Birds
        0x0006 | 0x00D0 => (0, 0, 10, 0, true),
        // Small game
        0x00C9 | 0x00CD | 0x00D7 | 0x00D9 | 0x00EE => (1, 1, 0, 0, false), // cat, rabbit, rats, dog
        // Farm animals and mounts
        0x00CB | 0x0122 => (3, 3, 0, 0, false), // pig, boar
        // A sheep in fleece, and the one body on the shard that carries wool.
        0x00CF => (2, 3, 0, 3, false),
        // The same sheep shorn — a different body, no wool, and until this row it
        // could not be carved at all.
        0x00DF => (2, 3, 0, 0, false),
        0x00D1 | 0x00DC | 0x0124 => (2, 3, 0, 0, false), // goat, llamas
        0x00D8 | 0x00E7 => (10, 10, 0, 0, false),        // cow
        0x00C8 | 0x00CC | 0x00E2 | 0x00E4 | 0x0123 => (5, 5, 0, 0, false), // horses
        0x00D2 | 0x00DA | 0x00DB => (5, 5, 0, 0, false), // ostards
        // Wild game
        0x00EA | 0x00ED => (4, 4, 0, 0, false),          // hart, hind
        0x00A7 | 0x00D4 | 0x00D5 => (5, 8, 0, 0, false), // bears
        0x0017 | 0x0019 | 0x001B | 0x00E1 => (3, 4, 0, 0, false), // wolves
        0x00CA => (4, 8, 0, 0, false),                   // alligator
        0x00DD => (4, 8, 0, 0, false),                   // walrus
        0x0097 => (4, 4, 0, 0, false),                   // dolphin
        // The dragon family, and the only source of the tailor's top two hides.
        // Every one of these bodies is already spawned (`world/data/spawns.json`),
        // so this is the table catching up with the world rather than new content.
        0x000C => (19, 20, 0, 0, false), // dragon
        0x002E => (19, 40, 0, 0, false), // ancient wyrm
        0x003C => (10, 20, 0, 0, false), // drake
        0x003E => (10, 20, 0, 0, false), // wyvern
        // A sea serpent is hide and nothing else: upstream gives it no `Meat`
        // override at all, so it falls to `BaseCreature`'s zero.
        0x0096 => (0, 10, 0, 0, false),
        _ => return None,
    };
    Some(CarvedYield {
        ribs,
        hides,
        hide: hide_grade_of(body),
        feathers,
        wool,
        bird,
    })
}

/// The grade of hide a body wears — ServUO's `BaseCreature.HideType`, which
/// defaults to `Regular` and is overridden on the individual creature.
///
/// Only the exceptions are listed, and only for bodies [`carved_yield`] already
/// carves. **Keyed by body, so a body two creatures share cannot be split**:
/// ServUO's hell cat is `Spined` and its ordinary cat is not, and both are
/// `0xC9` — so `0xC9` stays regular rather than paying a housecat in monster
/// leather.
///
/// **The top two grades come from the dragon family**, which is where upstream
/// puts them: barbed on the dragons and the ancient wyrm, horned on the drakes,
/// the wyverns and the sea serpents. Until those bodies became carvable, both
/// grades — and the tailoring rows that spend them — were unreachable from
/// either end.
const fn hide_grade_of(body: Graphic) -> MaterialId {
    match body.0 {
        0x0017 => SPINED_LEATHER, // dire wolf; the grey and timber wolves are regular
        0x00CA => SPINED_LEATHER, // alligator
        0x000C | 0x002E => BARBED_LEATHER, // dragon, ancient wyrm
        0x003C | 0x003E | 0x0096 => HORNED_LEATHER, // drake, wyvern, sea serpent
        _ => REGULAR_LEATHER,
    }
}

/// Cut up a carvable *item* under the blade. Returns whether it was one.
///
/// One entry today, and the shape is the point: ServUO's `ICarvable` is an
/// interface an item implements, so this is the list of items that do, not a
/// special case for fish. `Fish.Carve` is `ScissorHelper(from, new
/// RawFishSteak(), 4)` — the whole pile at four steaks apiece, into the container
/// the fish were in.
fn cut_up(state: &mut WorldState, carver: EntityId, item: EntityId) -> bool {
    let Some(drawn) = state.registry.get::<Drawn>(item).copied() else {
        return false;
    };
    // ServUO builds a fish on `Utility.Random(0x09CC, 4)`, so all four arts are
    // one item — `harvest::FISH_GRAPHIC` is only the one this engine pays out.
    // A fish the registry has since given a kind of its own is not this: an
    // untyped art, like every other legacy identity read in this crate.
    if !(0x09CC..=0x09CF).contains(&drawn.id.0) || state.registry.has::<ItemKind>(item) {
        return false;
    }
    let held = amount_of(state, item);
    let (Some(serial), Some(container)) = (state.registry.serial_of(item), containing(state, item)) else {
        return false;
    };
    if held == 0 {
        return true;
    }
    // Four steaks to the fish needs the clamp a one-for-one cut does not: a full
    // pile of fish is four times what one pile of steaks can hold.
    let taking = held.min(MAX_STACK / STEAKS_PER_FISH as u16);
    consume(state, serial, taking);
    let wanted = u32::from(taking) * STEAKS_PER_FISH;
    let made = give(state, container, RAW_FISH_STEAK, Hue(0), wanted);
    if !made.is_complete() {
        state.system_message(
            carver,
            &format!(
                "Only {} of {wanted} fish steaks could be placed there.",
                made.given
            ),
        );
    }
    true
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
            kind:      TargetKind::Object,
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
    // Something still alive is not a carcass, and ServUO's `BladedItemTarget`
    // has one answer for the whole of that case: a sheep in fleece is `ICarvable`
    // and is shorn, everything else is told it can only skin the dead. A live
    // mobile is the one thing here carrying a `Body` — a corpse wears
    // `CorpseBody` and is drawn as an item.
    //
    // Before the reach check below, and that is load-bearing rather than tidy:
    // [`in_reach`] answers an *item's* location, and a mobile has none, so it
    // refuses every living thing as too far away however close it is standing.
    // The shear does its own reach.
    if state.registry.has::<Body>(target) {
        crate::shear::shear(state, carver, target);
        return;
    }
    if !in_reach(state, target, carver) {
        state.system_message(carver, "That is too far away.");
        return;
    }
    // The third thing the blade answers, beside the fleece and the carcass: an
    // `ICarvable` *item*. Upstream's `BladedItemTarget` runs the same three-way
    // split, and a fish is the only one of them this shard can land — fishing
    // pays them and, until this, no cooking row could ever eat one.
    if cut_up(state, carver, target) {
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
    let Some(yielded) = carved_yield(body.body) else {
        state.system_message(carver, "You cannot carve anything useful from that corpse.");
        return;
    };
    if story.carved {
        state.system_message(carver, "That carcass has already been carved.");
        return;
    }

    let mut complete = true;
    if yielded.bird {
        complete &= give(state, corpse_serial, RAW_BIRD, Hue(0), 1).is_complete();
    } else if yielded.ribs != 0 {
        complete &= give(state, corpse_serial, RAW_RIBS, Hue(0), u32::from(yielded.ribs)).is_complete();
    }
    if yielded.hides != 0 {
        // Typed rather than drawn: the grade is the point, and a hue alone would
        // leave it to whoever reads the pile next to guess the family it belongs
        // to. `give_kind` derives the art from the registry.
        complete &= give_kind(
            state,
            corpse_serial,
            HIDES_KIND,
            Some(yielded.hide),
            u32::from(yielded.hides),
        )
        .expect("every hide grade is a registered leather material")
        .is_complete();
    }
    if yielded.feathers != 0 {
        complete &= give(
            state,
            corpse_serial,
            FEATHERS,
            Hue(0),
            u32::from(yielded.feathers),
        )
        .is_complete();
    }
    if yielded.wool != 0 {
        // Tainted, not the fleece: upstream's carve pays `new TaintedWool(wool)`
        // where the shear pays `Wool`. The wheel spins both.
        complete &= give(
            state,
            corpse_serial,
            TAINTED_WOOL,
            Hue(0),
            u32::from(yielded.wool),
        )
        .is_complete();
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
        match complete {
            true => "You carve the carcass and place the resources in the corpse.",
            false => "You carve the carcass, but some resources could not be placed in the corpse.",
        },
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
    fn the_bodies_servuo_gives_a_better_hide_keep_it() {
        assert_eq!(hide_grade_of(Graphic(0x00CA)), SPINED_LEATHER, "alligator");
        assert_eq!(hide_grade_of(Graphic(0x0017)), SPINED_LEATHER, "dire wolf");
        assert_eq!(hide_grade_of(Graphic(0x0019)), REGULAR_LEATHER, "grey wolf");
        // The hell cat is `Spined` upstream and shares this body with the
        // housecat, so the table cannot tell them apart and does not try.
        assert_eq!(hide_grade_of(Graphic(0x00C9)), REGULAR_LEATHER, "cat");
    }

    #[test]
    fn only_animal_bodies_have_yields() {
        assert!(carved_yield(Graphic(0x00D8)).is_some(), "cow");
        assert!(carved_yield(Graphic(0x00D0)).is_some(), "chicken");
        assert!(carved_yield(Graphic(0x0011)).is_none(), "orc");
    }
}
