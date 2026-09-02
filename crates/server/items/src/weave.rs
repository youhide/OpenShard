//! A loom: five spools of thread into a bolt of cloth.
//!
//! ServUO's `ILoom` and the `BaseClothMaterial` that targets it — the thread and
//! yarn a spinning wheel pays out ([`spin`](crate::spin)), fed to the loom one
//! at a time. Four go in against a line each; the fifth is woven and the bolt
//! comes off.
//!
//! **The second half of the step that gives cloth a source.** A bolt is not yet
//! what a tailor spends: fifty-six tailoring rows eat `Cloth` (`0x1766`), and
//! the bolt becomes fifty of it under the scissors — see [`cut`](crate::cut).
//! The whole chain is cotton or wool → wheel → thread or yarn → loom → bolt →
//! scissors → cloth.
//!
//! The count is the loom's, not the weaver's: a player who feeds three spools
//! and logs off leaves a loom three-quarters loaded, and whoever weaves on it
//! next finishes what they started. That is ServUO's own `Phase`, and it is why
//! [`LoomPhase`] is saved where the spinning wheel's timer is not — the loom has
//! already eaten those three spools.

use openshard_state::components::{
    AddonPart,
    LoomPhase,
};

use super::*;

/// The arts a loom accepts: a spool of thread, and the three yarns. ServUO's
/// `BaseClothMaterial` subclasses, which is one abstract class and therefore one
/// predicate rather than four kinds.
///
/// Matched by graphic for [`is_scissors`](crate::is_scissors)' reason: thread and
/// yarn reach a player as vendor stock written as bare graphics, and a semantic
/// kind for them would identify only the ones bought after it landed.
#[must_use]
pub const fn is_cloth_material(graphic: Graphic) -> bool {
    matches!(
        graphic.0,
        // spool of thread · dark yarn · light yarn · light yarn unraveled
        0x0FA0 | 0x0E1D | 0x0E1E | 0x0E1F
    )
}

/// The art a woven bolt takes. ServUO flips it through eight graphics
/// (`0xF95..0xF9C`); this engine draws the first, and [`cut`](crate::cut) accepts
/// every one of them.
pub const BOLT_GRAPHIC: Graphic = Graphic(0x0F95);

/// How many materials a bolt costs — four that only load the loom, and a fifth
/// that is woven. ServUO writes it as `Phase < 4`.
const PHASES: u8 = 4;

/// "Select a loom to use that on."
const WHICH_LOOM: ClilocId = ClilocId(500_366);
/// "Try using that on a loom."
const NOT_A_LOOM: ClilocId = ClilocId(500_367);
/// "You create some cloth and put it in your backpack."
const WOVEN: ClilocId = ClilocId(500_368);
/// "That must be in your pack for you to use it."
const NOT_IN_PACK: ClilocId = ClilocId(1_042_001);
/// The first of the four lines a part-loaded loom answers with, one per phase —
/// "The bolt of cloth has just been started." through "…is almost finished."
/// They are consecutive because ServUO sends them as `1010001 + Phase++`.
const LOADED: ClilocId = ClilocId(1_010_001);

/// A double-clicked spool or ball raises the object cursor that asks which loom.
/// Returns whether the item was a cloth material at all.
pub fn use_cloth_material(state: &mut WorldState, weaver: EntityId, material: EntityId) -> bool {
    let Some(graphic) = state.registry.get::<Drawn>(material).map(|drawn| drawn.id) else {
        return false;
    };
    if !is_cloth_material(graphic) {
        return false;
    }
    if !carried_by(state, weaver, material) {
        state.localized_message(weaver, NOT_IN_PACK, "");
        return true;
    }
    let (Some(&Client { connection, .. }), Some(serial)) = (
        state.registry.get::<Client>(weaver),
        state.registry.serial_of(weaver),
    ) else {
        return true;
    };
    state.raise_target(weaver, openshard_state::TargetPurpose::Weave { material });
    state.localized_message(weaver, WHICH_LOOM, "");
    state.send_packet(
        connection,
        &ServerPacket::TargetCursor(TargetCursor {
            cursor_id: CursorId(serial.raw()),
            kind:      TargetKind::Object,
        }),
    );
    true
}

/// Feed the loom the cursor came back with.
///
/// Everything is re-checked: the spool can be traded away and the loom taken up
/// between the two packets, and the target serial is the client's word.
pub fn weave(state: &mut WorldState, weaver: EntityId, material: EntityId, target: Option<Serial>) {
    let Some(drawn) = state.registry.get::<Drawn>(material).copied() else {
        return;
    };
    if !is_cloth_material(drawn.id) {
        return; // spent, or no longer thread
    }
    let Some(loom) = target
        .and_then(|serial| state.registry.entity_of(serial))
        .and_then(|item| loom_root(state, item))
    else {
        state.localized_message(weaver, NOT_A_LOOM, "");
        return;
    };
    if !in_reach(state, loom, weaver) {
        state.localized_message(weaver, NOT_A_LOOM, "");
        return;
    }
    if !carried_by(state, weaver, material) {
        state.localized_message(weaver, NOT_IN_PACK, "");
        return;
    }
    let (Some(serial), Some(owner)) = (
        state.registry.serial_of(material),
        state.registry.serial_of(weaver),
    ) else {
        return;
    };
    let phase = state.registry.get::<LoomPhase>(loom).map_or(0, |loaded| loaded.0);
    if phase < PHASES {
        // Loading, not weaving: the spool is gone and the loom is one step on.
        consume(state, serial, 1);
        state.registry.insert(loom, LoomPhase(phase + 1));
        state.localized_message(weaver, ClilocId(LOADED.0 + u32::from(phase)), "");
        return;
    }
    // The fifth. ServUO takes the bolt's hue from *this* material, not from the
    // four already on the loom, so a weaver who finishes somebody's half-loaded
    // loom with dyed thread gets dyed cloth.
    let Some(pack) = backpack_of(state, owner) else {
        return;
    };
    let made = give(state, pack, BOLT_GRAPHIC, drawn.hue, 1);
    if !made.is_complete() {
        // Refused before the loom is charged, `craft`'s own rule: a full pack
        // costs the weaver nothing but a click.
        state.system_message(weaver, "Your pack has no room for a bolt of cloth.");
        return;
    }
    consume(state, serial, 1);
    // The loom is empty again, and carries no phase rather than a zero one: the
    // save then holds nothing for every loom nobody has touched.
    state.registry.remove::<LoomPhase>(loom);
    state.localized_message(weaver, WOVEN, "");
}

/// The addon root of a loom the player clicked, or `None` when what they clicked
/// is not one. A loom is two tiles and either of them is a valid click, which is
/// what the walk through [`AddonPart::root`] buys.
fn loom_root(state: &WorldState, item: EntityId) -> Option<EntityId> {
    let part = state.registry.get::<AddonPart>(item)?;
    part.addon.is_loom().then_some(())?;
    state.registry.entity_of(part.root)
}

/// Whether an item is in this mobile's own pack — [`spin`](crate::spin)'s check,
/// and ServUO's recursive `IsChildOf(from.Backpack)`.
fn carried_by(state: &WorldState, mobile: EntityId, item: EntityId) -> bool {
    state
        .registry
        .serial_of(mobile)
        .and_then(|owner| backpack_of(state, owner))
        .is_some_and(|pack| state.craft_stock_root_of_item(item) == Some(pack))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_loading_lines_are_consecutive_clilocs() {
        // ServUO writes them as `1010001 + Phase++`, so the four must be a run.
        // A gap would mean a loom saying "you put the thread on the loom" twice
        // and skipping a step's own line.
        for phase in 0..PHASES {
            assert_eq!(LOADED.0 + u32::from(phase), 1_010_001 + u32::from(phase));
        }
    }

    #[test]
    fn every_yarn_and_thread_art_is_a_loom_material() {
        for graphic in [Graphic(0x0FA0), Graphic(0x0E1D), Graphic(0x0E1E), Graphic(0x0E1F)] {
            assert!(is_cloth_material(graphic), "{graphic:?}");
        }
        // What a wheel eats is not what a loom eats, and a loom that accepted
        // raw cotton would skip the wheel entirely.
        assert!(!is_cloth_material(Graphic(0x0DF9)), "cotton is not thread");
        assert!(!is_cloth_material(BOLT_GRAPHIC), "a bolt is not thread");
    }
}
