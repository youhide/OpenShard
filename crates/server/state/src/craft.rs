//! Which tool drives which trade, and how long it lasts.
//!
//! The craft half of [`crate::harvest::tool_data`], and here for the same reason
//! [`crate::weapon`] is: **two crates read it.** `items` reads it where an item is
//! made, to give a fresh sewing kit the uses that make it a tool rather than a
//! prop; `crafting` reads it on a double-click, to know which trade gump to
//! open. Data keyed by graphic, default in core.
//!
//! A tool is named by the *skill* it drives rather than by a craft system, because
//! `state` sits below the systems and must not learn their shape. Each system owns
//! exactly one main skill, so the mapping is complete either way round.

use openshard_protocol::wire::Graphic;

use crate::skill::Skill;

/// A craft tool: the trade it practises and how many attempts are in it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CraftToolData {
    /// The trade this tool opens.
    pub skill: Skill,
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
    let skill = match graphic.0 {
        // Blacksmithy: a smith hammer, tongs, or a sledge.
        0x13E3 | 0x13E4 | 0x0FBB | 0x0FBC | 0x0FB4 | 0x0FB5 => Skill::Blacksmith,
        // Tailoring: a sewing kit.
        0x0F9D => Skill::Tailoring,
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
        | 0x10E7 => Skill::Carpentry, // scorp
        // Tinkering: tinker's tools.
        0x1EB8 | 0x1EB9 => Skill::Tinkering,
        // Alchemy: a mortar and pestle.
        0x0E9B => Skill::Alchemy,
        // Bowcraft/Fletching: both facings of fletcher's tools.
        0x1022 | 0x1023 => Skill::Fletching,
        _ => return None,
    };
    Some(CraftToolData {
        skill,
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
    fn each_tool_names_a_trade_that_can_actually_be_practised() {
        for graphic in (0..=u16::MAX).map(Graphic) {
            let Some(tool) = craft_tool(graphic) else {
                continue;
            };
            let info = crate::skill::info(tool.skill.id()).expect("a real skill");
            // Every craft skill is one the window cannot press: the action that
            // uses it *is* the double-click on the tool.
            assert!(!info.usable, "{} is pressable from the window", info.name);
            assert!(tool.min_uses <= tool.max_uses);
        }
    }
}
