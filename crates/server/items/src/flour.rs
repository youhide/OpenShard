//! A sack of flour, opened.
//!
//! ServUO's `SackFlour.OnDoubleClick` (`Items/Consumables/Cooking.cs`), and the
//! second of the cooking chain's two missing links: the mill's craft row makes a
//! **closed** sack (`0x1039`) and every dough, cake-mix and tribal-paint row
//! spends an **open** one (`0x103A`). Nothing else in the engine turns one into
//! the other, so before this the chain broke twice — once for want of a wheat
//! field and once here, a step further along.
//!
//! The same shape of bridge as `crafting::chop` and [`cut`](crate::cut): an item
//! action with no skill, no roll and no tool. One click opens **one** sack out of
//! the pile, upstream's `if (Amount > 1) Amount--; else Delete();`, and the open
//! one lands where the closed pile was standing — in the container that held it,
//! or on the ground where it lay.

use openshard_state::components::Drawn;

use super::*;

/// A closed sack of flour, ServUO's `SackFlour`.
///
/// Public because this pair *is* the bridge, the way `smelt`'s ore and ingot
/// kinds are: `openshard_world::economy` names the edge with these two constants
/// rather than with a second spelling of the same two numbers.
pub const SACK_OF_FLOUR: Graphic = Graphic(0x1039);
/// An open sack of flour, ServUO's `SackFlourOpen` — what the cooking rows eat.
pub const OPEN_SACK_OF_FLOUR: Graphic = Graphic(0x103A);

/// "I can't reach that."
const CANNOT_REACH: ClilocId = ClilocId(1_019_045);

/// Open a double-clicked sack of flour. Returns whether the item was a sack at
/// all, so the dispatch can fall through to everything else it knows.
pub fn open_flour(state: &mut WorldState, opener: EntityId, sack: EntityId) -> bool {
    let Some(drawn) = state.registry.get::<Drawn>(sack).copied() else {
        return false;
    };
    if drawn.id != SACK_OF_FLOUR {
        return false;
    }
    // ServUO's guard is `Movable`, which on this engine is the same question as
    // "is it yours to touch": a sack behind a house's lockdown or on a vendor's
    // shelf is not, and `in_reach` is where that already lives.
    if !in_reach(state, sack, opener) {
        state.localized_message(opener, CANNOT_REACH, "");
        return true;
    }
    let Some(serial) = state.registry.serial_of(sack) else {
        return true;
    };
    // Where the closed pile is standing, because that is where the open sack
    // goes: `ScissorHelper`'s rule again, and upstream spells it out here too —
    // `Parent is Container ? DropItem : MoveToWorld(GetWorldLocation())`.
    let opened = match containing(state, sack) {
        Some(container) => give(state, container, OPEN_SACK_OF_FLOUR, Hue(0), 1).is_complete(),
        None => {
            let (Some(&Position(at)), facet) = (state.registry.get::<Position>(sack), state.facet_of(sack))
            else {
                return true;
            };
            spawn_item(state, OPEN_SACK_OF_FLOUR, Hue(0), 1, true, at, facet).is_some()
        }
    };
    // Nothing is spent when nothing was made: a full pack must not eat the sack
    // it could not open into.
    if !opened {
        state.system_message(opener, "There is no room here for an open sack of flour.");
        return true;
    }
    // One sack per click, and the pile survives with one fewer — a click that
    // opened the whole pile would turn twenty sacks into one open one.
    consume(state, serial, 1);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_sacks_are_different_items() {
        // The whole finding in one line: the mill makes the first and the dough
        // row wants the second, and they are not the same art. If these ever
        // became equal, the bridge would be a no-op that still looked present in
        // the economy report.
        assert_ne!(SACK_OF_FLOUR, OPEN_SACK_OF_FLOUR);
    }
}
