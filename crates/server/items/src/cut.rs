//! Scissors: a pile of hides into leather.
//!
//! ServUO's `Scissors.OnDoubleClick` and the `IScissorable` its cursor lands on —
//! here, `Hides.Scissor`, which is `ScissorHelper(from, new Leather(), 1)`: the
//! whole pile at one leather per hide, keeping the hue.
//!
//! **The step without which Tailoring is unreachable from butchering**, the way
//! [`smelt`] is the step between Mining and Blacksmithy. Fifty-six of the
//! tailoring rows eat leather and nothing else in the engine made any: a player
//! carved a cow, got hides, and could do nothing with them but sell them. This is
//! the other end of [`carve`](crate::carve).
//!
//! **The grade survives the cut**, which is the whole reason hides carry a
//! `Material` rather than only a hue: barbed hides are worth cutting precisely
//! because they become barbed leather, and a cut that dropped the grade would
//! quietly turn the best hide on the shard into the cheapest leather.
//!
//! No skill, no roll, no workshop and no tool wear — cutting is an item action,
//! not a craft. ServUO charges a use only on a Siege shard (`Siege.SiegeShard`
//! guards every `CheckUsesRemaining` on the scissors), so an ordinary pair never
//! wears out and this does not pretend otherwise.
//!
//! [`smelt`]: https://docs.rs/openshard-crafting

use openshard_protocol::item_kind::{
    ItemKindId,
    MaterialId,
};
use openshard_state::item_definition::LEATHER;

use super::*;

/// Scissors, both facings of ServUO's `[Flipable(0xf9f, 0xf9e)]`.
///
/// Matched by graphic rather than through the item registry for
/// [`is_carving_tool`](crate::is_carving_tool)'s reason: the vendors' stock and
/// the tinker's recipe (`tinkering.json`, `0x0F9F`) are both legacy rows, and a
/// semantic kind for the pair would identify only the ones made after it landed.
#[must_use]
pub const fn is_scissors(graphic: Graphic) -> bool {
    matches!(graphic.0, 0x0F9F | 0x0F9E)
}

/// Semantic kind of a pile of hides — what a carved carcass yields.
pub const HIDES_KIND: ItemKindId = ItemKindId(114);
/// Semantic kind of a pile of leather — what a tailor spends.
pub const LEATHER_KIND: ItemKindId = ItemKindId(37);

/// The art a pile of hides takes. ServUO builds `BaseHides` on `0x1079` and
/// flips to `0x1078`; this engine has always drawn the flipped one, so that is
/// the registry's canonical graphic and `0x1079` its alias.
const HIDES_GRAPHIC: Graphic = Graphic(0x1078);
/// The other facing, which an item made before the registry knew about hides may
/// still be wearing.
const HIDES_FLIPPED: Graphic = Graphic(0x1079);

/// "What should I use these scissors on?"
const CUT_WHAT: ClilocId = ClilocId(502_434);
/// "Items you wish to cut must be in your backpack."
const NOT_IN_PACK: ClilocId = ClilocId(502_437);
/// "Scissors can not be used on that to produce anything."
const NOT_CUTTABLE: ClilocId = ClilocId(502_440);

/// The snip — ServUO plays it from the target handler, on every successful cut.
const SNIP: SoundId = SoundId(0x0248);

/// A double-clicked pair of scissors raises the object cursor that asks what to
/// cut. Returns whether the item was scissors at all.
pub fn use_scissors(state: &mut WorldState, cutter: EntityId, tool: EntityId) -> bool {
    let Some(graphic) = state.registry.get::<Drawn>(tool).map(|drawn| drawn.id) else {
        return false;
    };
    if !is_scissors(graphic) {
        return false;
    }
    let (Some(&Client { connection, .. }), Some(serial)) = (
        state.registry.get::<Client>(cutter),
        state.registry.serial_of(cutter),
    ) else {
        return true;
    };
    if !in_reach(state, tool, cutter) {
        return true;
    }
    state.raise_target(cutter, openshard_state::TargetPurpose::Cut { tool });
    state.localized_message(cutter, CUT_WHAT, "");
    state.send_packet(
        connection,
        &ServerPacket::TargetCursor(TargetCursor {
            cursor_id: CursorId(serial.raw()),
            kind:      TargetKind::Object,
        }),
    );
    true
}

/// Apply scissors to the object selected by [`use_scissors`].
///
/// Everything is re-checked here: the scissors and the pile can both move or be
/// spent between the two packets, and the target serial is the client's word.
pub fn cut(state: &mut WorldState, cutter: EntityId, tool: EntityId, target: Option<Serial>) {
    let tool_is_usable = state
        .registry
        .get::<Drawn>(tool)
        .is_some_and(|drawn| is_scissors(drawn.id))
        && in_reach(state, tool, cutter);
    if !tool_is_usable {
        return;
    }
    let Some(target) = target.and_then(|serial| state.registry.entity_of(serial)) else {
        state.localized_message(cutter, NOT_CUTTABLE, "");
        return;
    };
    let Some(material) = hide_grade(state, target) else {
        state.localized_message(cutter, NOT_CUTTABLE, "");
        return;
    };

    // ServUO's `IsChildOf(from.Backpack)`, and the root walk is what makes it
    // recursive: hides in a bag in the pack are still in the pack, and hides in
    // a corpse on the ground — where carving leaves them — are not.
    let in_own_pack = state
        .registry
        .serial_of(cutter)
        .and_then(|owner| backpack_of(state, owner))
        .is_some_and(|pack| state.craft_stock_root_of_item(target) == Some(pack));
    if !in_own_pack {
        state.localized_message(cutter, NOT_IN_PACK, "");
        return;
    }

    let (Some(serial), Some(container)) = (state.registry.serial_of(target), containing(state, target))
    else {
        return;
    };
    // A pile can hold at most `MAX_STACK`, which is ServUO's own 60,000 cut cap,
    // so the whole pile is always takeable in one go and there is no clamp here.
    let taking = amount_of(state, target);
    if taking == 0 {
        return;
    }
    consume(state, serial, taking);
    // The leather lands where the hides were, not in the pack: ServUO's
    // `ScissorHelper` gives the new item the old one's parent, and a pile cut
    // inside a bag has no business jumping out of it.
    let made = give_kind(state, container, LEATHER_KIND, Some(material), u32::from(taking))
        .expect("every hide grade is a leather grade");
    state.play_sound(cutter, SNIP);
    if !made.is_complete() {
        state.system_message(
            cutter,
            &format!("Only {} of {taking} leather could be placed there.", made.given),
        );
    }
}

/// Which leather grade a pile of hides would cut into, or `None` when the item is
/// not a pile of hides at all.
///
/// Both identity models, and the shape `crafting::smelt` reads ore with: a typed
/// pile names its kind and carries its `Material`, and a pile made before the
/// registry knew about hides is read back from its art and hue *within the
/// leather family* — never from a bare global hue lookup, which answers plain
/// iron and plain wood to the same `Hue::NONE`.
fn hide_grade(state: &WorldState, item: EntityId) -> Option<MaterialId> {
    let drawn = state.registry.get::<Drawn>(item).copied()?;
    match (
        state.registry.get::<ItemKind>(item),
        state.registry.get::<Material>(item),
    ) {
        (Some(ItemKind(kind)), Some(Material(material))) if *kind == HIDES_KIND => Some(*material),
        // A typed item of any other kind is that kind, whatever its art suggests.
        (Some(_), _) => None,
        (None, _) if drawn.id == HIDES_GRAPHIC || drawn.id == HIDES_FLIPPED => {
            openshard_state::material_from_legacy_hue_in_family(LEATHER, drawn.hue)
        }
        _ => None,
    }
}

/// The container an item is sitting in, or `None` if it is not in one.
fn containing(state: &WorldState, item: EntityId) -> Option<Serial> {
    match item_location(state, item)? {
        ItemLocation::Settled(SettledItemLocation::Contained(held)) => Some(held.container),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use openshard_state::presentation_of;

    use super::*;

    #[test]
    fn both_facings_of_the_scissors_are_scissors() {
        assert!(is_scissors(Graphic(0x0F9F)));
        assert!(is_scissors(Graphic(0x0F9E)));
        assert!(!is_scissors(Graphic(0x0F9D)), "a sewing kit is not scissors");
    }

    #[test]
    fn every_hide_grade_is_a_leather_grade() {
        // The cut carries the material straight across, so the two kinds must
        // accept exactly the same set. A grade one of them lacked would either
        // panic in `cut` or silently downgrade the pile.
        for material in openshard_state::item_definition::MATERIAL_DEFINITIONS
            .iter()
            .filter(|material| material.family == LEATHER)
        {
            assert!(
                presentation_of(HIDES_KIND, Some(material.id)).is_some(),
                "hides in {}",
                material.name
            );
            assert!(
                presentation_of(LEATHER_KIND, Some(material.id)).is_some(),
                "leather in {}",
                material.name
            );
        }
    }

    #[test]
    fn both_hide_arts_read_back_as_the_same_kind() {
        // The flipped facing is an alias, not a second kind: an old pile drawn
        // `0x1079` must cut exactly like one drawn `0x1078`.
        for graphic in [HIDES_GRAPHIC, HIDES_FLIPPED] {
            assert_eq!(
                openshard_state::kind_from_drawn(Drawn {
                    id:  graphic,
                    hue: Hue(0x0851),
                }),
                Some((HIDES_KIND, Some(MaterialId(43)))),
                "{graphic:?}"
            );
        }
    }
}
