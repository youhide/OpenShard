//! What a slain creature's corpse holds beyond the baseline gold.
//!
//! The engine lays a corpse, moves the creature's worn gear into it and drops a
//! flat gold baseline, so a shard with no tables still loots. This is the table
//! on top of that: a per-body list of drops, rolled when the corpse is made.
//!
//! # It used to be a script's, and the rng is why that mattered
//!
//! The Community Pack held these tables and rolled them with `Math.random`,
//! writing itself an explicit exemption from the engine's replayable-tick
//! guarantee — a script being "an external input, like a network packet". The
//! tables are in the tree now and roll on
//! [`WorldState::rng`](openshard_state::WorldState), so the exemption is gone and
//! a replayed tick loots the same corpse twice.
//!
//! # Rolled where the corpse is made
//!
//! Not off the [`CorpseCreated`](crate::events::CorpseCreated) event. That event
//! remains for anything that wants to watch a death — but content in the tree is
//! not a listener, it is part of the tick, and a round trip through the bus would
//! only put a frame between the corpse and what is in it.

use openshard_protocol::wire::{
    Graphic,
    Hue,
};

/// One thing a corpse may hold.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Drop {
    /// The item tile.
    pub graphic:   Graphic,
    /// Its colour.
    pub hue:       Hue,
    /// The fewest that drop, at least one.
    pub least:     u16,
    /// The most. Equal to `least` for a fixed count.
    pub most:      u16,
    /// Whether it merges into a pile — gold, reagents, arrows — or is placed
    /// whole, like a weapon.
    pub stackable: bool,
    /// The chance it drops at all, as a percentage. `100` is always.
    pub percent:   u32,
}

include!(concat!(env!("OUT_DIR"), "/loot.rs"));

/// The table for a body, if the shard ships one.
#[must_use]
pub fn table(body: Graphic) -> Option<&'static [Drop]> {
    SHIPPED
        .binary_search_by_key(&body.0, |&(body, _)| body)
        .ok()
        .map(|at| SHIPPED[at].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_sorted_so_the_search_is_valid() {
        assert!(SHIPPED.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }

    #[test]
    fn a_body_with_no_table_loots_only_the_baseline() {
        // Not an error: most creatures have no table, and the engine's own gold
        // and worn gear are what they leave.
        assert!(table(Graphic(0xFFFF)).is_none());
    }

    #[test]
    fn a_shipped_drop_can_actually_drop() {
        for &(body, drops) in SHIPPED {
            for drop in drops {
                assert!(drop.percent > 0, "a drop of body {body} never drops");
                assert!(drop.least >= 1 && drop.least <= drop.most);
            }
        }
    }
}
