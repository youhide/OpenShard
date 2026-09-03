//! Which tool drives which trade, and how long it lasts.
//!
//! The craft half of [`crate::harvest::tool_data`], and here for the same reason
//! [`crate::weapon`] is: **two crates read it.** `items` reads it where an item is
//! made, to give a fresh sewing kit the uses that make it a tool rather than a
//! prop; `crafting` reads it on a double-click, to know which trade gump to
//! open. Data keyed by graphic, default in core.
//!
//! A tool names **both** the skill it drives and the trade it opens, and the
//! second is not derivable from the first: glassblowing's main skill is Alchemy,
//! the same as the alchemist's own, so a mortar and pestle and a blowpipe are one
//! skill and two gumps. The trade is carried as the *name* its row in
//! `crafting/data/craft_systems.json` already has — `state` sits below the
//! systems and learns that trades have names, not what a system is. What keeps
//! the two fields honest is a sweep in `crafting::defs`, which resolves every
//! tool's trade and asserts the system it finds practises the tool's skill.

use openshard_protocol::item_kind::ItemKindId;
use openshard_protocol::wire::Graphic;

use crate::skill::Skill;

/// A craft tool: the trade it practises and how many attempts are in it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CraftToolData {
    /// The skill this tool trains and rolls.
    pub skill:    Skill,
    /// Which trade's gump it opens, by the name `craft_systems.json` gives that
    /// system. Two tools can share a skill — a blowpipe and a mortar are both
    /// Alchemy — so this is the half that decides the window.
    pub trade:    &'static str,
    /// The fewest uses a fresh one has.
    pub min_uses: u16,
    /// And the most.
    pub max_uses: u16,
}

/// ServUO's `BaseTool(int itemID) : this(Utility.RandomMinMax(25, 75), itemID)` —
/// every craft tool in the game takes the same default, unlike the harvest tools,
/// which each name their own.
const MIN_USES: u16 = 25;
/// The other end of it.
const MAX_USES: u16 = 75;

/// What a craft tool graphic is, or `None` for anything that is not one.
///
/// Both facings of a flippable tool are listed: a client that flips a saw sends
/// the other art, and a table that knows only one of the two makes a saw that
/// works until somebody turns it round.
#[must_use]
pub fn craft_tool(graphic: Graphic) -> Option<CraftToolData> {
    // Opened once, so the arms below stay the terse art table they read as.
    let (skill, trade) = match graphic.0 {
        // Blacksmithy: a smith hammer, tongs, or a sledge.
        0x13E3 | 0x13E4 | 0x0FBB | 0x0FBC | 0x0FB4 | 0x0FB5 => (Skill::Blacksmith, BLACKSMITHY),
        // Tailoring: a sewing kit.
        0x0F9D => (Skill::Tailoring, TAILORING),
        // Carpentry: the whole bench of them.
        0x1034 | 0x1035 // saw
        | 0x1028 | 0x1029 // dovetail saw
        | 0x1030 | 0x1031 // jointing plane
        | 0x102C | 0x102D // moulding plane
        | 0x1032 | 0x1033 // smoothing plane
        | 0x102E | 0x102F // nails
        | 0x102A // hammer
        | 0x10E4 // draw knife
        | 0x10E5 // froe
        | 0x10E6 // inshave
        | 0x10E7 => (Skill::Carpentry, CARPENTRY), // scorp
        // Tinkering: tinker's tools.
        0x1EB8 | 0x1EB9 => (Skill::Tinkering, TINKERING),
        // Alchemy: a mortar and pestle.
        0x0E9B => (Skill::Alchemy, ALCHEMY),
        // Glassblowing: a blowpipe. The same skill as the mortar above and a
        // different trade — ServUO's `DefGlassblowing.MainSkill` is `Alchemy`,
        // and the tool is what tells the two windows apart.
        //
        // **One facing only**, unlike every other flippable tool here: upstream's
        // `[Flipable(0xE8A, 0xE89)]` puts the blowpipe's other face on `0x0E89`,
        // which is already the quarter staff's art. Claiming it would make a
        // staff open the glassblowing window, so the flipped blowpipe is a
        // recorded gap rather than a collision — the same call the item registry
        // makes, where `0x0E89` belongs to the staff.
        0x0E8A => (Skill::Alchemy, GLASSBLOWING),
        // Bowcraft/Fletching: both facings of fletcher's tools.
        0x1022 | 0x1023 => (Skill::Fletching, FLETCHING),
        // Cooking: skillet, rolling pin, and flour sifter.
        0x097F | 0x1043 | 0x103E => (Skill::Cooking, COOKING),
        // Inscription: both facings of a scribe's pen.
        0x0FBF | 0x0FC0 => (Skill::Inscribe, INSCRIPTION),
        _ => return None,
    };
    Some(CraftToolData {
        skill,
        trade,
        min_uses: MIN_USES,
        max_uses: MAX_USES,
    })
}

/// The trade names, spelled exactly as `craft_systems.json` spells them.
///
/// Constants rather than literals at each arm so a typo is one compile error
/// here instead of a tool that silently opens nothing.
const BLACKSMITHY: &str = "blacksmithy";
/// Tailoring's row.
const TAILORING: &str = "tailoring";
/// Carpentry's row.
const CARPENTRY: &str = "carpentry";
/// Tinkering's row.
const TINKERING: &str = "tinkering";
/// Alchemy's row.
const ALCHEMY: &str = "alchemy";
/// Glassblowing's row — the second trade to name [`Skill::Alchemy`].
const GLASSBLOWING: &str = "glassblowing";
/// Bowcraft's row.
const FLETCHING: &str = "fletching";
/// Cooking's row.
const COOKING: &str = "cooking";
/// Inscription's row.
const INSCRIPTION: &str = "inscription";

/// The craft-tool row for a registered item kind.
///
/// This is deliberately kind-keyed. [`craft_tool`] above remains the named
/// compatibility adapter for items not yet migrated into the registry.
///
/// The two tables are hand-kept and must agree wherever both can answer. What
/// says so is `every_craft_tool_graphic_names_the_same_trade_by_kind` below, and
/// it walks every graphic [`craft_tool`] knows rather than a sample: since the
/// registry now names all of them, a tool added to one table only is a test
/// failure rather than a trade that opens for a bought tool and not a crafted
/// one.
#[must_use]
pub fn craft_tool_for_kind(kind: ItemKindId) -> Option<CraftToolData> {
    let (skill, trade) = match kind.0 {
        10 | 19 | 20 => (Skill::Blacksmith, BLACKSMITHY),
        21 => (Skill::Tailoring, TAILORING),
        22 | 26..=35 => (Skill::Carpentry, CARPENTRY),
        23 => (Skill::Tinkering, TINKERING),
        24 => (Skill::Alchemy, ALCHEMY),
        25 => (Skill::Fletching, FLETCHING),
        142..=144 => (Skill::Cooking, COOKING),
        145 => (Skill::Inscribe, INSCRIPTION),
        146 => (Skill::Alchemy, GLASSBLOWING),
        _ => return None,
    };
    Some(CraftToolData {
        skill,
        trade,
        min_uses: MIN_USES,
        max_uses: MAX_USES,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_facings_of_a_flippable_tool_are_the_same_tool() {
        // A client that turns a saw round sends the other art, and a table with
        // one of the two makes a tool that stops working when it is rotated.
        for (a, b) in [
            (0x13E3, 0x13E4), // smith hammer
            (0x0FBB, 0x0FBC), // tongs
            (0x0FB5, 0x0FB4), // sledge
            (0x1034, 0x1035), // saw
            (0x1028, 0x1029), // dovetail saw
            (0x1EB8, 0x1EB9), // tinker's tools
            (0x1022, 0x1023), // fletcher's tools
            (0x0FBF, 0x0FC0), // scribe's pen
        ] {
            let (a, b) = (Graphic(a), Graphic(b));
            assert_eq!(craft_tool(a), craft_tool(b), "{:#06X} and {:#06X}", a.0, b.0);
            assert!(craft_tool(a).is_some());
        }
    }

    #[test]
    fn a_craft_tool_is_not_a_harvest_tool_and_the_reverse() {
        // The two tables are consulted one after the other on a double-click, so
        // an overlap would make one trade shadow the other.
        for graphic in (0..=u16::MAX).map(Graphic) {
            assert!(
                craft_tool(graphic).is_none() || crate::harvest::tool_data(graphic).is_none(),
                "{:#06X} is claimed by both tables",
                graphic.0
            );
        }
    }

    #[test]
    fn a_registered_pair_of_tongs_resolves_by_kind() {
        assert_eq!(
            craft_tool_for_kind(ItemKindId(10)).map(|tool| tool.skill),
            Some(Skill::Blacksmith)
        );
        assert!(craft_tool_for_kind(ItemKindId(6)).is_none()); // spellbook
    }

    #[test]
    fn registered_primary_tools_resolve_to_each_craft_skill() {
        for (kind, skill) in [
            (ItemKindId(19), Skill::Blacksmith),
            (ItemKindId(20), Skill::Blacksmith),
            (ItemKindId(21), Skill::Tailoring),
            (ItemKindId(22), Skill::Carpentry),
            (ItemKindId(23), Skill::Tinkering),
            (ItemKindId(24), Skill::Alchemy),
            (ItemKindId(25), Skill::Fletching),
            (ItemKindId(26), Skill::Carpentry),
            (ItemKindId(27), Skill::Carpentry),
            (ItemKindId(28), Skill::Carpentry),
            (ItemKindId(29), Skill::Carpentry),
            (ItemKindId(30), Skill::Carpentry),
            (ItemKindId(31), Skill::Carpentry),
            (ItemKindId(32), Skill::Carpentry),
            (ItemKindId(33), Skill::Carpentry),
            (ItemKindId(34), Skill::Carpentry),
            (ItemKindId(35), Skill::Carpentry),
        ] {
            assert_eq!(craft_tool_for_kind(kind).map(|tool| tool.skill), Some(skill));
        }
    }

    /// Both tool tables answer one question, and until now nothing said they
    /// agreed except a spot check on the tongs.
    ///
    /// Which of the two a given item reaches is decided by whether it carries an
    /// `ItemKind`, and that in turn by where it came from — a migrated recipe, a
    /// restored save, a legacy shelf. So a tool listed in one table only is a
    /// trade that opens for a bought tool and refuses an identical crafted one,
    /// and neither answer looks wrong on its own. Every art `craft_tool` knows is
    /// now a registered kind, so this can walk the whole table rather than
    /// sampling it.
    #[test]
    fn every_craft_tool_graphic_names_the_same_trade_by_kind() {
        let mut checked = 0;
        for graphic in (0..=u16::MAX).map(Graphic) {
            let Some(by_art) = craft_tool(graphic) else {
                continue;
            };
            let (kind, _) = crate::item_definition::kind_from_drawn(crate::Drawn {
                id:  graphic,
                hue: openshard_protocol::wire::Hue::NONE,
            })
            .unwrap_or_else(|| panic!("craft tool {:#06X} is in no item definition", graphic.0));
            assert_eq!(
                craft_tool_for_kind(kind),
                Some(by_art),
                "{:#06X} is a {:?} tool by art and something else as kind {}",
                graphic.0,
                by_art.skill,
                kind.0
            );
            checked += 1;
        }
        // A sweep that matched nothing would pass in silence; the count is what
        // makes "every tool" mean the thirty-five arts the table above lists,
        // both facings of a flippable one counted separately — and the blowpipe
        // has only one, because the other is the quarter staff's.
        assert_eq!(checked, 35, "craft tool arts checked");
    }

    #[test]
    fn each_tool_names_a_trade_that_can_actually_be_practised() {
        for graphic in (0..=u16::MAX).map(Graphic) {
            let Some(tool) = craft_tool(graphic) else {
                continue;
            };
            let info = crate::skill::info(tool.skill.id()).expect("a real skill");
            // A craft skill is normally one the window cannot press: the action
            // that uses it *is* the double-click on the tool. Inscription is the
            // reference's own exception and not an oversight here — ServUO gives
            // `SkillName.Inscribe` a use callback of its own (`Inscribe.cs`,
            // "target the book you wish to copy") *and* a craft system reached
            // through the pen. Both are true of it, so this asks about the rest.
            if tool.skill != Skill::Inscribe {
                assert!(!info.usable, "{} is pressable from the window", info.name);
            }
            assert!(tool.min_uses <= tool.max_uses);
        }
    }
}
