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
//! **The same scissors also turn cloth into bandages**, one for one — ServUO's
//! `Cloth.Scissor` and `UncutCloth.Scissor` are both
//! `ScissorHelper(from, new Bandage(), 1)`. Bandages already reach a player
//! from a vendor or a corpse; this is the missing *route*, not a missing item.
//! It is also the weaver's own uncut cloth's first use of any kind: nothing
//! else in this engine reads that graphic at all.
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
/// Semantic kind of a pile of cut cloth — what the scissors themselves make
/// from a bolt, and now what they spend on a bandage.
const CLOTH_KIND: ItemKindId = ItemKindId(38);

/// The art cut cloth takes — what fifty-six tailoring rows, and a dozen
/// carpentry and smithing ones, actually spend.
///
/// Public beside [`HIDES_KIND`] and [`LEATHER_KIND`]: the scissors are a bridge in
/// the economy graph, and the audit reads both ends of it here.
pub const CLOTH_GRAPHIC: Graphic = Graphic(0x1766);
/// How much cloth one bolt cuts into. ServUO's `ScissorHelper(from, new Cloth(),
/// 50)`, and the number a bolt's own single-click line already quotes.
const CLOTH_PER_BOLT: u32 = 50;
/// The most bolts one cut will take, so the cloth stays inside one pile.
/// ServUO's own `60000 / amountPerOldItem` clamp, written against the same
/// [`MAX_STACK`](crate::MAX_STACK) that is the 60,000.
const MAX_BOLTS_PER_CUT: u16 = (MAX_STACK as u32 / CLOTH_PER_BOLT) as u16;

/// The art ServUO's `UncutCloth` takes — vendor stock the weaver and the
/// tailor both sell (`townsfolk.json`'s "uncut cloth" shelf lines), and this
/// engine's own crafting never makes it. A second class from `Cloth`'s own,
/// `[FlipableAttribute(0x1765, 0x1767)]`; this is the canonical facing, the
/// one the shelf shows after the vendor display-art sweep corrected it from
/// four borrowed pictures of folded cloth
/// (`evidence/2026-09-03-the-vendor-display-art-sweep.md`).
///
/// `UncutCloth.Scissor` is its own `ScissorHelper(from, new Bandage(), 1)` —
/// the same yield as [`CLOTH_GRAPHIC`] under these scissors, which is why
/// [`is_cloth_pile`] answers yes to both. Nothing else in this engine reads
/// this art at all, so this is also the route that gives the shelf a use.
const UNCUT_CLOTH_GRAPHIC: Graphic = Graphic(0x1767);
/// The other facing of the above.
const UNCUT_CLOTH_FLIPPED: Graphic = Graphic(0x1765);

/// The art a pile of hides takes. ServUO builds `BaseHides` on `0x1079` and
/// flips to `0x1078`; this engine has always drawn the flipped one, so that is
/// the registry's canonical graphic and `0x1079` its alias.
const HIDES_GRAPHIC: Graphic = Graphic(0x1078);
/// The other facing, which an item made before the registry knew about hides may
/// still be wearing.
const HIDES_FLIPPED: Graphic = Graphic(0x1079);

/// The art a clean bandage takes. ServUO's `Bandage`, item `0x0E21`.
///
/// Redeclared rather than imported: `openshard_skills::handlers::bandage`
/// names the same graphic for the healer's own double-click, but `items`
/// depends on neither `crafting` nor `skills`, so the two ends of "what a
/// bandage is drawn as" stay two literals of the one art rather than an edge
/// across that boundary.
const BANDAGE_GRAPHIC: Graphic = Graphic(0x0E21);

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
    let Some(what) = cuttable(state, target) else {
        state.localized_message(cutter, NOT_CUTTABLE, "");
        return;
    };

    // ServUO's `IsChildOf(from.Backpack)`, and the root walk is what makes it
    // recursive: hides in a bag in the pack are still in the pack, and hides in
    // a corpse on the ground — where carving leaves them — are not.
    if !carried_in_pack(state, cutter, target) {
        state.localized_message(cutter, NOT_IN_PACK, "");
        return;
    }

    let (Some(serial), Some(container), Some(drawn)) = (
        state.registry.serial_of(target),
        containing(state, target),
        state.registry.get::<Drawn>(target).copied(),
    ) else {
        return;
    };
    let held = amount_of(state, target);
    if held == 0 {
        return;
    }
    // Both cuts put what they make where the old pile was, not in the pack:
    // ServUO's `ScissorHelper` gives the new item the old one's parent, and a
    // pile cut inside a bag has no business jumping out of it.
    let (wanted, made, name) = match what {
        // A pile of hides can hold at most `MAX_STACK`, which is ServUO's own
        // 60,000 cut cap at one leather each, so the whole pile always goes in
        // one go and there is no clamp on this side.
        Cuttable::Hides(material) => {
            consume(state, serial, held);
            let wanted = u32::from(held);
            let made = give_kind(state, container, LEATHER_KIND, Some(material), wanted)
                .expect("every hide grade is a leather grade");
            (wanted, made, "leather")
        }
        // Bolts do need the clamp: fifty cloth apiece would put a full pile of
        // them fifty times over what one pile of cloth can hold.
        Cuttable::Bolt => {
            let taking = held.min(MAX_BOLTS_PER_CUT);
            consume(state, serial, taking);
            let wanted = u32::from(taking) * CLOTH_PER_BOLT;
            let made = give(state, container, CLOTH_GRAPHIC, drawn.hue, wanted);
            (wanted, made, "cloth")
        }
        // One-for-one like hides, and for the same reason: ServUO's own
        // `ScissorHelper(from, new Bandage(), 1)` caps at 60,000 cut, which is
        // `MAX_STACK` itself, so the whole pile always goes in one go.
        Cuttable::Cloth => {
            consume(state, serial, held);
            let wanted = u32::from(held);
            let made = give(state, container, BANDAGE_GRAPHIC, drawn.hue, wanted);
            (wanted, made, "bandages")
        }
    };
    state.play_sound(cutter, SNIP);
    if !made.is_complete() {
        state.system_message(
            cutter,
            &format!("Only {} of {wanted} {name} could be placed there.", made.given),
        );
    }
}

/// What the scissors found under the cursor, and therefore what comes off it.
///
/// Three entries and not three identity models, which is the point of naming
/// the answer rather than returning a material: hides are a *typed* kind
/// carrying a leather grade; a bolt is legacy art carrying only a dye and
/// nothing else answers for it; cut cloth sits between the two — a typed kind
/// like hides, but materialless like a bolt, so a dyed pile of it falls back
/// to legacy art the same way a bolt always reads. ServUO writes all three as
/// `IScissorable` implementations (`BoltOfCloth`, `Cloth`/`UncutCloth`), and
/// they are not interchangeable — a bolt has no grade, cloth has no
/// fifty-to-one yield of its own, and only hides carry a grade to lose.
enum Cuttable {
    /// A pile of hides: one leather per hide, keeping the grade.
    Hides(MaterialId),
    /// A bolt of woven cloth: fifty cloth per bolt, keeping the hue.
    Bolt,
    /// A pile of cut cloth, or ServUO's `UncutCloth` beside it: one bandage
    /// per cloth either way, keeping the hue. ServUO's `Cloth.Scissor` and
    /// `UncutCloth.Scissor`, both `ScissorHelper(from, new Bandage(), 1)`.
    Cloth,
}

/// Which of the three the scissors were pointed at, or `None` for anything
/// else.
fn cuttable(state: &WorldState, item: EntityId) -> Option<Cuttable> {
    if let Some(material) = hide_grade(state, item) {
        return Some(Cuttable::Hides(material));
    }
    if is_cloth_pile(state, item) {
        return Some(Cuttable::Cloth);
    }
    // Every facing of ServUO's `[Flipable(0xF95 … 0xF9C)]` bolt, and only for an
    // item the registry does not already call something else.
    let drawn = state.registry.get::<Drawn>(item).copied()?;
    let untyped = !state.registry.has::<ItemKind>(item);
    (untyped && (0x0F95..=0x0F9C).contains(&drawn.id.0)).then_some(Cuttable::Bolt)
}

/// Whether a pile is cloth under these scissors — what fifty-six tailoring
/// rows already spend, what the loom's own bolt becomes, and ServUO's
/// `UncutCloth` beside it, which no other system here reads at all.
///
/// Cloth carries no material grade, so unlike [`hide_grade`] there is only
/// one axis to read rather than two: a typed pile names [`CLOTH_KIND`]
/// outright, and an untyped one — vendor stock predating the registry, cut
/// cloth dyed off `Hue::NONE`, or `UncutCloth`'s own two facings, which this
/// registry has never named at all — is read back from its bare graphic,
/// because there is no material family to look a hue up within the way hides
/// look leather grades up.
fn is_cloth_pile(state: &WorldState, item: EntityId) -> bool {
    let Some(drawn) = state.registry.get::<Drawn>(item).copied() else {
        return false;
    };
    match state.registry.get::<ItemKind>(item) {
        Some(ItemKind(kind)) => *kind == CLOTH_KIND,
        None => {
            drawn.id == CLOTH_GRAPHIC || drawn.id == UNCUT_CLOTH_GRAPHIC || drawn.id == UNCUT_CLOTH_FLIPPED
        }
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use openshard_state::presentation_of;

    use super::*;

    fn world() -> WorldState {
        let tiles = openshard_tiles::TileData::empty();
        let mut facets = BTreeMap::new();
        facets.insert(
            Facet(0),
            openshard_state::FacetState::new(
                None,
                None,
                64,
                64,
                openshard_state::facet_rules::FacetRules::classic(Facet(0)),
                None,
                &tiles,
            ),
        );
        WorldState::new(
            facets,
            Facet(0),
            tiles,
            // Named rather than `Default::default()`: nothing in these tests
            // places a multi, and an empty catalogue should say so out loud.
            openshard_uofiles::multi::Multis::of([]),
            openshard_map::grid::Tile::new(0, 0),
            1,
        )
    }

    /// A cutter wearing an empty backpack. Everything nested inside a mobile's
    /// own worn pack is unconditionally `in_reach` of that mobile
    /// ([`in_reach`]'s "one's own worn pack is always in reach" branch), which
    /// is what lets these tests skip building out a real position or facet.
    fn cutter_with_backpack(state: &mut WorldState) -> (EntityId, Serial) {
        let (cutter, cutter_serial) = state
            .registry
            .spawn_with_serial(SerialKind::Mobile)
            .expect("a mobile serial");
        state.registry.insert(
            cutter,
            Body {
                id:  Graphic(0x0190),
                hue: Hue(0),
            },
        );
        // `in_reach` dereferences the cutter's own `Position` before it looks at
        // where the tool is at all, even along the shortcut above.
        state.registry.insert(cutter, Position(Point::new(0, 0, 0)));

        let (backpack, backpack_serial) = state
            .registry
            .spawn_with_serial(SerialKind::Item)
            .expect("a backpack serial");
        state.registry.insert(
            backpack,
            Drawn {
                id:  BACKPACK_GRAPHIC,
                hue: Hue::NONE,
            },
        );
        state.registry.insert(backpack, Container { gump: BACKPACK_GUMP });
        establish_item_location(
            state,
            backpack,
            ItemLocation::equipped(Equipped {
                mobile: cutter_serial,
                layer:  BACKPACK_LAYER,
            }),
        )
        .expect("the backpack is worn");

        (cutter, backpack_serial)
    }

    /// Drop a fresh item straight into `backpack`, at `grid` so two calls in the
    /// same test do not collide.
    fn place_in_pack(state: &mut WorldState, backpack: Serial, drawn: Drawn, grid: u8) -> (EntityId, Serial) {
        let (item, serial) = state
            .registry
            .spawn_with_serial(SerialKind::Item)
            .expect("an item serial");
        state.registry.insert(item, drawn);
        establish_item_location(
            state,
            item,
            ItemLocation::contained(Contained {
                container: backpack,
                position:  GumpPoint::new(60, 60),
                grid:      GridSlot(grid),
            }),
        )
        .expect("the item sits in the pack");
        (item, serial)
    }

    #[test]
    fn cutting_typed_cloth_yields_one_bandage_per_cloth() {
        let mut state = world();
        let (cutter, backpack) = cutter_with_backpack(&mut state);
        let (scissors, _) = place_in_pack(
            &mut state,
            backpack,
            Drawn {
                id:  Graphic(0x0F9F),
                hue: Hue::NONE,
            },
            0,
        );
        let (cloth, cloth_serial) = place_in_pack(
            &mut state,
            backpack,
            Drawn {
                id:  CLOTH_GRAPHIC,
                hue: Hue::NONE,
            },
            1,
        );
        // The registry's own shape for cloth cut from a bolt: a typed kind,
        // undyed.
        state.registry.insert(cloth, ItemKind(CLOTH_KIND));
        state.registry.insert(cloth, Stackable);
        initialize_stack_amount(&mut state, cloth, 7);

        cut(&mut state, cutter, scissors, Some(cloth_serial));

        assert!(
            state.registry.entity_of(cloth_serial).is_none(),
            "the whole cloth pile is spent, not merely emptied"
        );
        assert_eq!(
            count_in_container(&state, backpack, BANDAGE_GRAPHIC),
            7,
            "seven cloth make exactly seven bandages, one for one"
        );
    }

    #[test]
    fn cutting_untyped_cloth_yields_bandages_too() {
        // A pile with no `ItemKind` — cut cloth dyed off `Hue::NONE` — falls
        // back to the same legacy-art read a bolt always gets. It has to cut
        // exactly like the typed pile above, hue and all.
        let mut state = world();
        let (cutter, backpack) = cutter_with_backpack(&mut state);
        let (scissors, _) = place_in_pack(
            &mut state,
            backpack,
            Drawn {
                id:  Graphic(0x0F9F),
                hue: Hue::NONE,
            },
            0,
        );
        let dye = Hue(0x0489);
        let (cloth, cloth_serial) = place_in_pack(
            &mut state,
            backpack,
            Drawn {
                id:  CLOTH_GRAPHIC,
                hue: dye,
            },
            1,
        );
        state.registry.insert(cloth, Stackable);
        initialize_stack_amount(&mut state, cloth, 3);

        cut(&mut state, cutter, scissors, Some(cloth_serial));

        assert!(state.registry.entity_of(cloth_serial).is_none());
        assert_eq!(count_in_container(&state, backpack, BANDAGE_GRAPHIC), 3);

        let bandages = contained_items(&state, backpack)
            .map(|(entity, _)| entity)
            .find(|&entity| {
                state
                    .registry
                    .get::<Drawn>(entity)
                    .is_some_and(|drawn| drawn.id == BANDAGE_GRAPHIC)
            })
            .expect("the bandages landed in the pack");
        assert_eq!(
            state.registry.get::<Drawn>(bandages).map(|drawn| drawn.hue),
            Some(dye),
            "ServUO's ScissorHelper carries the hue by default"
        );
    }

    #[test]
    fn cutting_the_weavers_uncut_cloth_yields_bandages() {
        // `0x1767` is ServUO's real `UncutCloth` — the weaver and tailor's own
        // vendor stock, and nothing else in this engine reads that graphic at
        // all until this route exists. It has to cut exactly like the pile
        // cut from a bolt, because `UncutCloth.Scissor` is ServUO's own
        // `ScissorHelper(from, new Bandage(), 1)`, identical to `Cloth`'s.
        let mut state = world();
        let (cutter, backpack) = cutter_with_backpack(&mut state);
        let (scissors, _) = place_in_pack(
            &mut state,
            backpack,
            Drawn {
                id:  Graphic(0x0F9F),
                hue: Hue::NONE,
            },
            0,
        );
        let (cloth, cloth_serial) = place_in_pack(
            &mut state,
            backpack,
            Drawn {
                id:  UNCUT_CLOTH_GRAPHIC,
                hue: Hue::NONE,
            },
            1,
        );
        state.registry.insert(cloth, Stackable);
        initialize_stack_amount(&mut state, cloth, 20);

        cut(&mut state, cutter, scissors, Some(cloth_serial));

        assert!(
            state.registry.entity_of(cloth_serial).is_none(),
            "the whole pile of uncut cloth is spent"
        );
        assert_eq!(
            count_in_container(&state, backpack, BANDAGE_GRAPHIC),
            20,
            "twenty uncut cloth make exactly twenty bandages, one for one"
        );
    }

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
