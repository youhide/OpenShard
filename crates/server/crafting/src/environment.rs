//! The workshop: what has to be standing near a crafter before the tool works.
//!
//! ServUO's `DefBlacksmithy.CheckAnvilAndForge` and `CraftItem`'s
//! `m_HeatSources` / `m_Ovens` / `m_Mills` / `m_WaterSources` tables.
//!
//! **Both halves matter, and only one of them is obvious.** A forge is sometimes
//! an *item* the decoration pass placed and sometimes a *static* baked into the
//! map, and Britannia has both kinds in the same buildings — ServUO scans the item
//! list and the static tiles separately for exactly that reason. Reading only the
//! items would refuse a craft at half the forges in the game, and the refusal
//! would look like a broken recipe rather than a missing scan.
//!
//! What is deliberately *not* copied is ServUO's line-of-sight test on each
//! candidate. Its own reason for it is a forge on the far side of a wall two tiles
//! away, which is a corner case; the cost is a Bresenham ray per tile per craft,
//! and the z band below already throws out the forge on the floor above.

use openshard_entities::EntityId;
use openshard_movement::Tile;
use openshard_state::WorldState;
use openshard_state::components::{Drawn, Position};

use crate::system::Needs;
use openshard_protocol::wire::Graphic;

/// How far a workshop reaches — ServUO's `range` argument, 2 everywhere it is
/// called.
pub const WORKSHOP_RANGE: u32 = 2;

/// How far above or below a crafter a facility may sit and still count. ServUO's
/// `(from.Z + 16) < item.Z || (item.Z + 16) < from.Z`, which is what keeps the
/// forge on the floor above out of it.
const Z_BAND: i32 = 16;

/// What is standing around a crafter.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Facilities {
    /// A forge.
    pub forge: bool,
    /// An anvil.
    pub anvil: bool,
    /// Any fire — every forge is one, and so is a campfire.
    pub heat: bool,
    /// An oven.
    pub oven: bool,
    /// A flour mill.
    pub mill: bool,
    /// Water.
    pub water: bool,
}

impl Facilities {
    /// Whether these satisfy what a recipe wants.
    #[must_use]
    pub const fn satisfy(self, needs: Needs) -> bool {
        (!needs.forge || self.forge)
            && (!needs.anvil || self.anvil)
            && (!needs.heat || self.heat)
            && (!needs.oven || self.oven)
            && (!needs.mill || self.mill)
            && (!needs.water || self.water)
    }

    /// Fold one tile id in, whichever list it belongs to.
    fn add(&mut self, graphic: Graphic) {
        let id = graphic.0;
        self.forge |= is_forge(id);
        self.anvil |= is_anvil(id);
        // Every forge is a fire, which ServUO says by listing the forge ranges in
        // `m_HeatSources` as well; kept as the same union rather than two tables
        // that have to agree.
        self.heat |= is_heat(id);
        self.oven |= is_oven(id);
        self.mill |= is_mill(id);
        self.water |= is_water(id);
    }
}

/// What is within reach of `crafter`.
///
/// Returns everything found rather than answering one question, because a smith
/// asks about two facilities at once and asking twice would walk the same tiles
/// twice.
#[must_use]
pub fn around(state: &WorldState, crafter: EntityId) -> Facilities {
    let mut found = Facilities::default();
    let Some(&Position(at)) = state.registry.get::<Position>(crafter) else {
        return found;
    };
    let facet = state.facet_of(crafter);

    // The decoration the converter placed, and anything a player or a script has
    // dropped since. `nearby` is a Chebyshev box, which is the shape ServUO's
    // `GetItemsInRange` walks too.
    for (item, pos) in state.facet_state(facet).sectors.nearby(at, WORKSHOP_RANGE) {
        if !in_z_band(i32::from(at.z), i32::from(pos.z)) {
            continue;
        }
        if let Some(graphic) = state.registry.get::<Drawn>(item) {
            found.add(graphic.id);
        }
    }

    // And the map's own. A great many of Britannia's forges are static tiles that
    // no entity stands for, so this half is not an optimisation — without it a
    // smith cannot work in most of the shops that have a forge drawn in them.
    let Some(terrain) = state.facets.get(&facet).and_then(|f| f.terrain.as_deref()) else {
        return found;
    };
    let mut statics = Vec::new();
    let low = u32::from(at.x).saturating_sub(WORKSHOP_RANGE);
    let high = u32::from(at.x) + WORKSHOP_RANGE;
    for x in low..=high {
        let top = u32::from(at.y).saturating_sub(WORKSHOP_RANGE);
        let bottom = u32::from(at.y) + WORKSHOP_RANGE;
        for y in top..=bottom {
            let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y)) else {
                continue;
            };
            statics.clear();
            terrain.statics_at(Tile::new(x, y), &mut statics);
            for (id, z) in &statics {
                if in_z_band(i32::from(at.z), i32::from(*z)) {
                    found.add(Graphic(*id));
                }
            }
        }
    }
    found
}

/// Whether a facility at `there` is on the crafter's own floor.
const fn in_z_band(here: i32, there: i32) -> bool {
    here + Z_BAND >= there && there + Z_BAND >= here
}

/// A forge — ServUO's `4017`, the large-forge range, and the elven one.
const fn is_forge(id: u16) -> bool {
    id == 4017 || (id >= 6522 && id <= 6569) || id == 0x2DD8
}

/// An anvil — the two facings, and the two elven ones.
const fn is_anvil(id: u16) -> bool {
    id == 4015 || id == 4016 || id == 0x2DD5 || id == 0x2DD6
}

/// Any fire — `CraftItem.m_HeatSources`, as inclusive pairs.
fn is_heat(id: u16) -> bool {
    const HEAT: &[(u16, u16)] = &[
        (0x0461, 0x048E), // sandstone oven / fireplace
        (0x092B, 0x096C), // stone oven / fireplace
        (0x0DE3, 0x0DE9), // campfire
        (0x0FAC, 0x0FAC), // firepit
        (0x184A, 0x184C), // heating stand, left
        (0x184E, 0x1850), // heating stand, right
        (0x398C, 0x399F), // a fire field, which is a fire like any other
        (0x2DDB, 0x2DDC), // elven stove
        (0x19AA, 0x19BB), // brazier
        (0x197A, 0x19A9), // large forge
        (0x0FB1, 0x0FB1), // small forge
    ];
    in_ranges(id, HEAT)
}

/// An oven — `m_Ovens`.
fn is_oven(id: u16) -> bool {
    const OVENS: &[(u16, u16)] = &[(0x0461, 0x046F), (0x092B, 0x093F), (0x2DDB, 0x2DDC)];
    in_ranges(id, OVENS)
}

/// A flour mill — `m_Mills`, which ServUO keeps as loose ids rather than ranges.
fn is_mill(id: u16) -> bool {
    const MILLS: &[u16] = &[
        0x1920, 0x1921, 0x1922, 0x1923, 0x1924, 0x1295, 0x1926, 0x1928, 0x192C, 0x192D, 0x192E, 0x129F,
        0x1930, 0x1931, 0x1932, 0x1934,
    ];
    MILLS.contains(&id)
}

/// Water — `m_WaterSources`.
fn is_water(id: u16) -> bool {
    const WATER: &[(u16, u16)] = &[
        (0x0B41, 0x0B44),
        (0x0E7B, 0x0E7B),
        (0x0FFA, 0x0FFA),
        (0x154D, 0x154D),
        (0x2AC0, 0x2AC5),
    ];
    in_ranges(id, WATER)
}

/// Whether an id falls in any of a set of inclusive pairs.
fn in_ranges(id: u16, ranges: &[(u16, u16)]) -> bool {
    ranges.iter().any(|(lo, hi)| id >= *lo && id <= *hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_forge_and_anvil_ids_are_the_references_own() {
        // Pinned next to the constants, the `NO_SHOOT` lesson: a flag or an id
        // that is only ever read through a helper is an id nobody notices is
        // wrong until a whole system quietly refuses to work.
        assert!(is_anvil(4015) && is_anvil(4016));
        assert!(is_forge(4017));
        assert!(is_forge(6522) && is_forge(6569), "the large-forge range");
        assert!(!is_forge(6521) && !is_forge(6570));
        assert!(!is_anvil(4017) && !is_forge(4015));
    }

    #[test]
    fn a_forge_is_also_a_fire_but_an_anvil_is_not() {
        let mut found = Facilities::default();
        found.add(Graphic(4017));
        assert!(found.forge);
        assert!(!found.anvil);

        let mut found = Facilities::default();
        found.add(Graphic(0x0FB1)); // small forge, in the heat table
        assert!(found.heat);
    }

    #[test]
    fn a_smithy_wants_both_and_neither_alone_will_do() {
        let smithy = Needs::smithy();
        let mut found = Facilities::default();
        found.add(Graphic(4017));
        assert!(!found.satisfy(smithy), "a forge with nothing to hammer on");
        found.add(Graphic(4015));
        assert!(found.satisfy(smithy));
    }

    #[test]
    fn nothing_is_wanted_by_default() {
        assert!(Facilities::default().satisfy(Needs::none()));
    }

    #[test]
    fn a_facility_on_the_floor_above_does_not_count() {
        assert!(in_z_band(0, 0));
        assert!(in_z_band(0, 16) && in_z_band(0, -16));
        assert!(!in_z_band(0, 17) && !in_z_band(0, -17));
    }
}
