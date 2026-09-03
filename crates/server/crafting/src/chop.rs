//! Logs into boards.
//!
//! ServUO's `IAxe.Axe`, and the step without which the whole of Carpentry —
//! and Fletching, and half of Tinkering — is unreachable from Lumberjacking: a
//! lumberjack is paid in **logs**, and every one of those rows eats **boards**.
//! Nothing else in the engine turns one into the other, which
//! [`openshard_world::economy`] measured as 1,213 recipe rows that could never
//! run.
//!
//! **The click is the lumberjack's own cursor, not the log's.** Upstream reaches
//! this through `Services/Harvest/Core/HarvestTarget.cs`: a double-clicked axe
//! raises the harvest cursor, and that cursor answers two different clicks — a
//! tile to swing at, and (for Lumberjacking alone, with an axe in hand) an item
//! in the pack to cut up. So this is not a second double-click meaning bolted
//! onto the log; it is the second branch of a target the engine already raises.
//!
//! **Two skills, one gate.** ServUO's `TryCreateBoards(from, skill, item)` passes
//! when *either* Carpentry or Lumberjacking clears the bar — a carpenter who
//! never felled a tree can still work the wood a lumberjack brings, and the
//! lumberjack who felled it can work it without being a carpenter. The bar itself
//! is [`WOODS`]' own `req_skill`, which is the same number upstream writes a
//! second time in `Log.cs`.
//!
//! [`openshard_world::economy`]: https://docs.rs/openshard-world

use openshard_entities::EntityId;
use openshard_protocol::item_kind::{
    ItemKindId,
    MaterialId,
};
use openshard_protocol::wire::{
    ClilocId,
    Hue,
    SoundId,
};
use openshard_skills::skill_value;
use openshard_state::components::{
    Drawn,
    ItemKind,
    Material,
    Stackable,
};
use openshard_state::harvest::{
    HarvestResource,
    LOG_GRAPHIC,
    WOODS,
};
use openshard_state::{
    Skill,
    WorldState,
};

/// Semantic kind of a log — what a felled tree pays.
pub const LOG_KIND: ItemKindId = ItemKindId(3);
/// Semantic kind of a board — what the carpenter, the fletcher and the tinker
/// spend.
///
/// This pair is public because it *is* the bridge, exactly as
/// [`smelt`](crate::smelt)'s ore and ingot are: the reachability audit names the
/// edge with these two constants rather than with a second spelling of the same
/// numbers, so a renamed kind is a compile error there instead of a silently
/// missing edge.
pub const BOARD_KIND: ItemKindId = ItemKindId(36);

/// How many boards one log makes — ServUO's `ScissorHelper(from, item, 1, false)`.
///
/// The `false` is the grade rule in disguise: the board does **not** carry the
/// log's hue across, because a board's colour is its own material's. Here that is
/// automatic — [`give_kind`](openshard_items::give_kind) draws the pile from the
/// registry — and the grade is carried deliberately instead.
const BOARDS_PER_LOG: u32 = 1;

/// "You cannot work this strange and unusual wood."
const TOO_STRANGE: ClilocId = ClilocId(1_072_652);
/// "This item must be in your backpack to be used."
const NOT_IN_PACK: ClilocId = ClilocId(1_062_334);
/// The cut — ServUO plays it from the target handler on every accepted chop.
const CHOP: SoundId = SoundId(0x013E);

/// Cut a pile of logs into boards, or say why not. Returns whether the item was a
/// log at all, so the caller can fall through to the ground it was really aiming
/// at.
///
/// `tool` is re-read here rather than trusted: the cursor outlives the click that
/// raised it, and an axe dropped, sold or worn out while it was up chops nothing.
pub fn chop(state: &mut WorldState, chopper: EntityId, tool: EntityId, log: EntityId) -> bool {
    let Some(drawn) = state.registry.get::<Drawn>(log).copied() else {
        return false;
    };
    let Some(material) = log_grade(state, log, drawn) else {
        return false;
    };
    // ServUO's `m_System is Lumberjacking && m_Tool is BaseAxe`: a pickaxe raises
    // the same cursor and must not cut boards with it. Asked of the tool's own
    // harvest row, so a hatchet — which is an axe by the weapon table rather than
    // by a list — answers for itself.
    if !is_axe(state, tool) || !openshard_items::in_reach(state, tool, chopper) {
        return false;
    }
    let Some(row) = wood_row(material) else {
        return false;
    };

    // ServUO's `IsChildOf(from.Backpack)`, and recursive for the reason
    // `items::cut` gives: logs in a bag in the pack are still in the pack, and
    // logs in a corpse on the ground are not.
    if !openshard_items::carried_in_pack(state, chopper, log) {
        state.localized_message(chopper, NOT_IN_PACK, "");
        return true;
    }
    // The flat gate, and — as in `smelt` — it is not a roll: a wood beyond you is
    // not a hard cut, it is one nobody ever taught you to make. Either trade
    // clears it.
    let best =
        skill_value(state, chopper, Skill::Carpentry).max(skill_value(state, chopper, Skill::Lumberjacking));
    if row.req_skill > i32::from(best) {
        state.localized_message(chopper, TOO_STRANGE, "");
        return true;
    }

    let held = openshard_items::amount_of(state, log);
    if held == 0 {
        return true;
    }
    // One board apiece, so ServUO's `60000 / amountPerOldItem` clamp is the pile
    // cap itself and a full pile of logs always goes in one cut.
    let taking = held.min(openshard_items::MAX_STACK);
    let (Some(serial), Some(container)) = (
        state.registry.serial_of(log),
        openshard_items::containing(state, log),
    ) else {
        return true;
    };
    openshard_items::consume(state, serial, taking);
    let boards = u32::from(taking) * BOARDS_PER_LOG;
    let made = openshard_items::give_kind(state, container, BOARD_KIND, Some(material), boards)
        .expect("every wood grade a log can carry is a board grade");
    if let Some(made) = made.last {
        // Boards stack, and `give_kind` only marks what it *creates* — a merge
        // onto an existing pile leaves the marker where it was.
        state.registry.insert(made, Stackable);
    }
    state.play_sound(chopper, CHOP);
    if !made.is_complete() {
        state.system_message(
            chopper,
            &format!("Only {} of {boards} boards could be placed there.", made.given),
        );
    }
    true
}

/// Which wood a pile is, or `None` when it is not a pile of logs at all.
///
/// Both identity models, the shape [`smelt`](crate::smelt) reads ore with: a
/// typed pile names its kind and carries its [`Material`], and a pile made before
/// the registry knew about logs is read back from its art and hue **within the
/// wood table** — never from a bare global hue lookup, which answers plain iron
/// and plain wood to the same `Hue::NONE`.
fn log_grade(state: &WorldState, log: EntityId, drawn: Drawn) -> Option<MaterialId> {
    match (
        state.registry.get::<ItemKind>(log),
        state.registry.get::<Material>(log),
    ) {
        (Some(ItemKind(kind)), Some(Material(material))) if *kind == LOG_KIND => Some(*material),
        // A typed item of any other kind is that kind, whatever its art suggests.
        (Some(_), _) => None,
        (None, _) if drawn.id == LOG_GRAPHIC => hue_grade(drawn.hue),
        _ => None,
    }
}

/// The wood a legacy log's hue names, out of the harvest table that painted it.
fn hue_grade(hue: Hue) -> Option<MaterialId> {
    WOODS
        .iter()
        .find(|row| row.hue == hue)
        .and_then(|row| row.material)
}

/// The harvest row for a wood, which carries the skill it takes to work it.
fn wood_row(material: MaterialId) -> Option<&'static HarvestResource> {
    WOODS.iter().find(|row| row.material == Some(material))
}

/// Whether the tool that raised the cursor is an axe — the only tool this branch
/// of the harvest target accepts.
fn is_axe(state: &WorldState, tool: EntityId) -> bool {
    let data = match state.registry.get::<ItemKind>(tool) {
        Some(kind) => openshard_state::harvest::tool_data_for_kind(kind.0),
        None => {
            let Some(drawn) = state.registry.get::<Drawn>(tool) else {
                return false;
            };
            openshard_state::harvest::tool_data(drawn.id)
        }
    };
    data.is_some_and(|data| data.skill == Skill::Lumberjacking)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_wood_a_tree_gives_can_be_worked() {
        // The gap this module closes, as a check: a wood with no row here would
        // be a grade of log that pays a lumberjack in something no carpenter can
        // ever spend — which is precisely what the whole table did before.
        for row in WOODS {
            let material = row.material.expect("every wood row carries its grade");
            assert!(
                wood_row(material).is_some(),
                "{:?} has no working skill",
                material
            );
            assert!(
                openshard_state::presentation_of(BOARD_KIND, Some(material)).is_some(),
                "{material:?} is not a board grade",
            );
        }
    }

    #[test]
    fn the_working_skill_is_the_felling_skill() {
        // ServUO writes these numbers twice — `WOODS`' `req_skill` and `Log.cs`'s
        // `TryCreateBoards(from, 65, …)` — and they agree in both places. This
        // pins the four that are not zero, so a change to the harvest table that
        // silently moved the carpenter's bar is visible here.
        let bar = |material: u16| wood_row(MaterialId(material)).map(|row| row.req_skill);
        assert_eq!(bar(20), Some(0), "regular wood needs no skill at all");
        assert_eq!(bar(21), Some(650), "oak");
        assert_eq!(bar(22), Some(800), "ash");
        assert_eq!(bar(23), Some(950), "yew");
        assert_eq!(bar(24), Some(1000), "heartwood");
        assert_eq!(bar(26), Some(1000), "frostwood");
    }

    #[test]
    fn a_legacy_log_is_read_back_from_the_hue_the_table_painted_it() {
        // The pre-registry pile: art `0x1BDD` and a wood hue. Read within the
        // wood table rather than globally, or plain wood and plain iron — both
        // `Hue(0)` — would answer each other.
        for row in WOODS {
            assert_eq!(hue_grade(row.hue), row.material, "{:?}", row.hue);
        }
        assert_eq!(hue_grade(Hue(0x0FFF)), None, "an unpainted hue is not a wood");
    }
}
