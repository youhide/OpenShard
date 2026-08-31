//! What an item *is*, by its graphic, the moment one is made.
//!
//! A few core graphics carry state that nothing else can supply: an instrument has
//! a number of tunes left in it, a pickaxe a number of swings, and a bottle of
//! poison a poison. None can come from a table at read time — a half-used lute and
//! a fresh one share a graphic — so it has to be put on the item when the item is
//! created, and this is the one place that happens.
//!
//! Without it a shop's instruments are silent props and its poison potions are
//! empty glass: the Community Pack's vendors already stock both (the converter
//! reads ServUO's own `SB*.cs`), and the skills that use them would refuse every
//! one.

use openshard_entities::{
    EntityId,
    Registry,
};
use openshard_protocol::item_kind::ItemTag;
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::PoisonLevel;
use openshard_state::WorldState;
use openshard_state::components::{
    Container,
    Decays,
    Instrument,
    ItemKind,
    Name,
    POISON_POTION_GRAPHIC,
    PoisonCharges,
    RUNEBOOK_GRAPHIC,
    Runebook,
    SPELLBOOK_GRAPHIC,
    Spellbook,
    Tool,
};
use openshard_state::craft::{
    craft_tool,
    craft_tool_for_kind,
};
use openshard_state::harvest::{
    tool_data,
    tool_data_for_kind,
};
use openshard_state::instrument::{
    INSTRUMENT_MAX_USES,
    INSTRUMENT_MIN_USES,
    instrument_data,
    instrument_data_for_kind,
};
use openshard_state::item_definition::{
    has_tag,
    item_definition,
};

/// An inclusive fresh-use range in the form the world's generator consumes.
#[derive(Clone, Copy, Debug)]
struct UsesRange {
    min:   u16,
    width: std::num::NonZeroU32,
}

impl UsesRange {
    fn inclusive(min: u16, max: u16) -> Option<Self> {
        let width = u32::from(max).checked_sub(u32::from(min))?.checked_add(1)?;
        Some(Self {
            min,
            width: std::num::NonZeroU32::new(width)?,
        })
    }

    fn roll(self, rng: &mut openshard_state::Rng) -> u16 {
        let uses = u32::from(self.min) + rng.below(self.width.get());
        u16::try_from(uses).expect("a draw inside an inclusive u16 use range fits u16")
    }
}

fn fresh_uses(rng: &mut openshard_state::Rng, min: u16, max: u16) -> u16 {
    UsesRange::inclusive(min, max)
        .expect("a fresh item's use range is ordered")
        .roll(rng)
}

/// Give a freshly made item whatever its graphic implies.
///
/// Called from every place an item is created from a graphic — a vendor's shelf, a
/// script's spawn, a staff `.add` — so there is no path that makes a lute nobody
/// can play.
pub fn apply_core_defaults(state: &mut WorldState, item: EntityId, graphic: Graphic) {
    let kind = state.registry.get::<ItemKind>(item).map(|kind| kind.0);
    if let Some(gump) = state
        .registry
        .get::<ItemKind>(item)
        .and_then(|kind| item_definition(kind.0))
        .and_then(|definition| definition.container_gump)
    {
        // The gump is a definition fact, not a special case in F1. Attach it
        // here so every semantic constructor produces an openable container.
        state.registry.insert(item, Container { gump });
        state.registry.remove::<Decays>(item);
        return;
    }
    if kind.is_some_and(|kind| has_tag(kind, ItemTag::Spellbook))
        || (kind.is_none() && graphic == SPELLBOOK_GRAPHIC)
    {
        // The F1 creator, vendors and scripts all enter through the same item
        // factory. A spellbook is not merely its `0x0EFA` drawing: without this
        // component it cannot open or learn a scroll.
        state.registry.insert(item, Spellbook::default());
        return;
    }
    let instrument = match kind {
        Some(kind) => instrument_data_for_kind(kind).is_some(),
        None => instrument_data(graphic).is_some(),
    };
    if instrument {
        // Rolled between ServUO's two bounds on the world's own generator, so a
        // shelf of lutes replays and no two are identical.
        let uses_left = fresh_uses(&mut state.rng, INSTRUMENT_MIN_USES, INSTRUMENT_MAX_USES);
        state.registry.insert(item, Instrument { uses_left });
        return;
    }
    let harvest_tool = match kind {
        Some(kind) => tool_data_for_kind(kind),
        None => tool_data(graphic),
    };
    if let Some(data) = harvest_tool {
        // A pickaxe off the shelf holds a fixed number of swings, rolled the same
        // way — without it a bought pick has no `Tool` at all and the first swing
        // never wears it down.
        let uses_left = fresh_uses(&mut state.rng, data.min_uses, data.max_uses);
        state.registry.insert(item, Tool { uses_left });
        return;
    }
    let craft_tool = match kind {
        Some(kind) => craft_tool_for_kind(kind),
        None => craft_tool(graphic),
    };
    if let Some(data) = craft_tool {
        // The same for a smith's tongs or a tailor's sewing kit. The two tables
        // are separate because a pickaxe and a saw answer different questions —
        // which ground, which trade — and one of them ends up on `Skill` either
        // way, so merging them would only hide the difference.
        let uses_left = fresh_uses(&mut state.rng, data.min_uses, data.max_uses);
        state.registry.insert(item, Tool { uses_left });
        return;
    }
    if graphic == POISON_POTION_GRAPHIC {
        let level = PoisonLevel::new(poison_level_of(state, item));
        state.registry.insert(item, PoisonCharges { level, charges: 1 });
        return;
    }
    if kind.is_some_and(|kind| has_tag(kind, ItemTag::Runebook))
        || (kind.is_none() && graphic == RUNEBOOK_GRAPHIC)
    {
        // An empty book with its charges. A bought one is blank — the
        // destinations are the owner's to bind — but it has to *be* a runebook
        // from the moment it exists, or a book off a shelf is a graphic that
        // refuses to open, which is the bug the spellbook mask once had.
        state.registry.insert(
            item,
            Runebook {
                charges: SHELF_RUNEBOOK_CHARGES,
                max_charges: SHELF_RUNEBOOK_CHARGES,
                ..Runebook::default()
            },
        );
    }
}

/// The charges a runebook that nobody crafted comes with — ServUO's own
/// constructor default, which its vendors and its loot both use. A crafted one
/// gets `5 + quality + Inscribe/30` instead, so the scribe's work shows.
const SHELF_RUNEBOOK_CHARGES: u8 = 6;

/// Put a saved use count back on an item, as whichever of the two kinds its
/// graphic says it is.
///
/// Both an instrument and a harvesting tool are ServUO's `IUsesRemaining` and both
/// ride the same saved column, so the graphic is what decides which component it
/// comes back as. Restoring the wrong one would leave a pickaxe that plays music.
pub fn restore_uses(state: &mut WorldState, item: EntityId, graphic: Graphic, uses_left: u16) {
    let instrument = match state.registry.get::<ItemKind>(item) {
        Some(kind) => instrument_data_for_kind(kind.0).is_some(),
        None => instrument_data(graphic).is_some(),
    };
    if instrument {
        state.registry.insert(item, Instrument { uses_left });
    } else {
        let harvest_tool = match state.registry.get::<ItemKind>(item) {
            Some(kind) => tool_data_for_kind(kind.0).is_some(),
            None => tool_data(graphic).is_some(),
        };
        let craft_tool = match state.registry.get::<ItemKind>(item) {
            Some(kind) => craft_tool_for_kind(kind.0).is_some(),
            None => craft_tool(graphic).is_some(),
        };
        if harvest_tool || craft_tool {
            state.registry.insert(item, Tool { uses_left });
        }
    }
}

/// How many uses an item has left, whichever of the two kinds it is — what the
/// save sweep writes.
///
/// Takes the registry rather than the world because that is all the save sweep
/// has: `item_record` runs over a borrowed `&Registry` while the rest of the world
/// is being read beside it.
#[must_use]
pub fn uses_left(registry: &Registry, item: EntityId) -> Option<u16> {
    if let Some(instrument) = registry.get::<Instrument>(item) {
        return Some(instrument.uses_left);
    }
    registry.get::<Tool>(item).map(|tool| tool.uses_left)
}

/// Which poison a bottle holds, read off its label.
///
/// All four strengths are the same graphic (`0x0F0A`), so the graphic cannot say —
/// but the *name* can, and a stocked bottle has one, because the converter carries
/// ServUO's own item labels through. A bottle with no label is the middling one,
/// which is what "a poison potion" means when nobody says otherwise.
fn poison_level_of(state: &WorldState, item: EntityId) -> u8 {
    let Some(name) = state.registry.get::<Name>(item) else {
        return 1;
    };
    let name = name.0.to_lowercase();
    if name.contains("lesser") {
        0
    } else if name.contains("greater") {
        2
    } else if name.contains("deadly") {
        3
    } else if name.contains("lethal") {
        4
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_fresh_use_ranges_are_ordered_and_bound_every_draw() {
        let harvest = tool_data(Graphic(0x0E86)).expect("a pickaxe");
        let craft = craft_tool(Graphic(0x0F9D)).expect("a sewing kit");
        let ranges = [
            (INSTRUMENT_MIN_USES, INSTRUMENT_MAX_USES),
            (harvest.min_uses, harvest.max_uses),
            (craft.min_uses, craft.max_uses),
        ];
        let mut rng = openshard_state::Rng::new(7);
        for (min, max) in ranges {
            let range = UsesRange::inclusive(min, max).expect("a shipped range");
            for _ in 0..1000 {
                assert!((min..=max).contains(&range.roll(&mut rng)));
            }
        }

        assert!(
            UsesRange::inclusive(2, 1).is_none(),
            "a reversed range is not a roll"
        );
        let whole = UsesRange::inclusive(u16::MIN, u16::MAX).expect("the full u16 range fits u32");
        assert_eq!(whole.width.get(), u32::from(u16::MAX) + 1);
    }
}
