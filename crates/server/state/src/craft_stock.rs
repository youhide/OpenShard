//! Dense recursive item stock paid on mutation rather than on a craft request.

use std::collections::{
    BTreeMap,
    HashMap,
    HashSet,
};

use openshard_entities::EntityId;
use openshard_protocol::craft::{
    CRAFT_KEY_COUNT,
    CRAFT_STOCK_SELECTORS,
    CraftKey,
    MAX_CRAFT_SOURCE_ITEMS,
};
use openshard_protocol::serial::Serial;

use crate::{
    Amount,
    Container,
    Drawn,
    ItemKind,
    ItemLocation,
    Material,
    SettledItemLocation,
    WorldState,
    contained_items,
};

/// One canonically revalidated pile in an ordered stock bucket.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CraftStockPile {
    pub item:   EntityId,
    pub serial: Serial,
    pub amount: u16,
}

#[derive(Clone, Debug)]
struct StockRow {
    total: u32,
    piles: BTreeMap<Serial, EntityId>,
}

#[derive(Clone, Debug)]
enum RootProjection {
    Ready {
        revision:   u64,
        item_count: usize,
        rows:       Vec<StockRow>,
    },
    TooComplex {
        revision: u64,
    },
}

/// Why an indexed source cannot answer a bounded realtime request.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CraftStockError {
    Missing,
    TooComplex,
}

#[derive(Debug)]
pub(crate) struct CraftStockIndex {
    next_revision: u64,
    roots:         HashMap<Serial, RootProjection>,
}

impl CraftStockIndex {
    pub(crate) fn new() -> Self {
        Self {
            next_revision: 1,
            roots:         HashMap::new(),
        }
    }

    fn replace(&mut self, root: Serial, projection: Option<RootProjection>) {
        match projection {
            Some(projection) => {
                self.roots.insert(root, projection);
            }
            None => {
                self.roots.remove(&root);
            }
        }
    }
}

impl WorldState {
    /// Root of the canonical containment tree which currently owns `item`.
    #[must_use]
    pub fn craft_stock_root_of_item(&self, mut item: EntityId) -> Option<Serial> {
        let mut visited = HashSet::new();
        while visited.insert(item) {
            let location = self.registry.get::<ItemLocation>(item).copied()?;
            match location.origin() {
                SettledItemLocation::Contained(contained) => {
                    item = self.registry.entity_of(contained.container)?;
                }
                SettledItemLocation::Ground { .. } | SettledItemLocation::Equipped(_) => {
                    return self.registry.serial_of(item);
                }
            }
        }
        None
    }

    /// Rebuild one recursive root after a canonical mutation. The walk stops at
    /// the realtime ceiling and records an unavailable projection rather than
    /// allowing a later request to rediscover an adversarial subtree.
    pub fn refresh_craft_stock_root(&mut self, root: Serial) {
        let revision = self.craft_stock.next_revision;
        self.craft_stock.next_revision = self.craft_stock.next_revision.wrapping_add(1).max(1);
        let projection = build_root(self, root, revision);
        self.craft_stock.replace(root, projection);
    }

    /// Refresh the root which currently contains `item`, if it has one.
    pub fn refresh_craft_stock_for_item(&mut self, item: EntityId) {
        if let Some(root) = self.craft_stock_root_of_item(item) {
            self.refresh_craft_stock_root(root);
        }
    }

    /// Compact totals without a root or pile walk.
    pub fn craft_stock_amounts(&self, root: Serial) -> Result<(u64, Vec<u32>), CraftStockError> {
        match self.craft_stock.roots.get(&root) {
            Some(RootProjection::Ready { revision, rows, .. }) => {
                Ok((*revision, rows.iter().map(|row| row.total).collect()))
            }
            Some(RootProjection::TooComplex { .. }) => Err(CraftStockError::TooComplex),
            None => Err(CraftStockError::Missing),
        }
    }

    /// Ordered union of exact key buckets. Canonical identity, amount and root
    /// membership are revalidated before a pile leaves the derived index.
    pub fn craft_stock_piles(
        &self,
        root: Serial,
        keys: &[CraftKey],
    ) -> Result<(u64, Vec<CraftStockPile>), CraftStockError> {
        let projection = self
            .craft_stock
            .roots
            .get(&root)
            .ok_or(CraftStockError::Missing)?;
        let RootProjection::Ready { revision, rows, .. } = projection else {
            return Err(CraftStockError::TooComplex);
        };
        let mut candidates = BTreeMap::new();
        for key in keys {
            let Some(row) = rows.get(usize::from(key.0)) else {
                continue;
            };
            for (&serial, &item) in &row.piles {
                candidates.insert(serial, item);
            }
        }
        Ok((
            *revision,
            candidates
                .into_iter()
                .filter_map(|(serial, item)| {
                    let amount = amount_if_in_root(self, root, item, serial)?;
                    Some(CraftStockPile { item, serial, amount })
                })
                .collect(),
        ))
    }

    /// Indexed subtree size used by mutation-cost diagnostics and tests.
    pub fn craft_stock_item_count(&self, root: Serial) -> Result<usize, CraftStockError> {
        match self.craft_stock.roots.get(&root) {
            Some(RootProjection::Ready { item_count, .. }) => Ok(*item_count),
            Some(RootProjection::TooComplex { .. }) => Err(CraftStockError::TooComplex),
            None => Err(CraftStockError::Missing),
        }
    }

    /// Current diagnostic revision, including an over-ceiling projection.
    #[must_use]
    pub fn craft_stock_revision(&self, root: Serial) -> Option<u64> {
        match self.craft_stock.roots.get(&root)? {
            RootProjection::Ready { revision, .. } | RootProjection::TooComplex { revision } => {
                Some(*revision)
            }
        }
    }
}

fn build_root(state: &WorldState, root: Serial, revision: u64) -> Option<RootProjection> {
    let root_entity = state.registry.entity_of(root)?;
    if !state.registry.has::<Container>(root_entity) {
        return None;
    }
    let mut rows = (0..CRAFT_KEY_COUNT)
        .map(|_| {
            StockRow {
                total: 0,
                piles: BTreeMap::new(),
            }
        })
        .collect::<Vec<_>>();
    let mut visited = HashSet::from([root_entity]);
    let mut pending = vec![root];
    let mut count = 0usize;
    while let Some(container) = pending.pop() {
        let mut children: Vec<_> = contained_items(state, container).map(|(item, _)| item).collect();
        children.sort_by_key(|&item| (state.registry.serial_of(item), item));
        for item in children.into_iter().rev() {
            if !visited.insert(item) {
                continue;
            }
            count += 1;
            if count > MAX_CRAFT_SOURCE_ITEMS {
                return Some(RootProjection::TooComplex { revision });
            }
            if let Some((serial, amount)) = state.registry.serial_of(item).map(|serial| {
                (
                    serial,
                    state.registry.get::<Amount>(item).map_or(1, |amount| amount.0),
                )
            }) {
                for (key, selector) in CRAFT_STOCK_SELECTORS.iter().enumerate() {
                    if selector_matches(state, item, *selector) {
                        let row = &mut rows[key];
                        row.total = row.total.saturating_add(u32::from(amount));
                        row.piles.insert(serial, item);
                    }
                }
            }
            if state.registry.has::<Container>(item) {
                if let Some(serial) = state.registry.serial_of(item) {
                    pending.push(serial);
                }
            }
        }
    }
    Some(RootProjection::Ready {
        revision,
        item_count: count,
        rows,
    })
}

fn selector_matches(
    state: &WorldState,
    item: EntityId,
    selector: openshard_protocol::craft::CraftStockSelector,
) -> bool {
    let drawn_matches = state
        .registry
        .get::<Drawn>(item)
        .is_some_and(|drawn| drawn.id == selector.graphic && drawn.hue == selector.hue);
    match selector.kind {
        Some(kind) => {
            state.registry.get::<ItemKind>(item) == Some(&ItemKind(kind))
                && state.registry.get::<Material>(item).map(|material| material.0) == selector.material
                || state.registry.get::<ItemKind>(item).is_none() && drawn_matches
        }
        None => drawn_matches,
    }
}

fn amount_if_in_root(state: &WorldState, root: Serial, item: EntityId, serial: Serial) -> Option<u16> {
    if state.registry.serial_of(item) != Some(serial) || state.craft_stock_root_of_item(item) != Some(root) {
        return None;
    }
    Some(state.registry.get::<Amount>(item).map_or(1, |amount| amount.0))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use openshard_map::grid::Tile;
    use openshard_protocol::containers::GridSlot;
    use openshard_protocol::gump::GumpPoint;
    use openshard_protocol::item_kind::{
        ItemKindId,
        MaterialId,
    };
    use openshard_protocol::serial::SerialKind;
    use openshard_protocol::wire::{
        Graphic,
        Hue,
    };
    use openshard_protocol::world::{
        Facet,
        Point,
    };
    use openshard_tiles::TileData;
    use openshard_uofiles::multi::Multis;
    use proptest::prelude::*;

    use super::*;
    use crate::facet_rules::FacetRules;
    use crate::{
        Amount,
        Contained,
        FacetState,
        ItemLocation,
        establish_item_location,
    };

    fn world() -> WorldState {
        let tiles = TileData::empty();
        let mut facets = BTreeMap::new();
        facets.insert(
            Facet(0),
            FacetState::new(None, None, 64, 64, FacetRules::classic(Facet(0)), None, &tiles),
        );
        WorldState::new(facets, Facet(0), tiles, Multis::of([]), Tile::new(0, 0), 1)
    }

    fn root_item(state: &mut WorldState, x: u16) -> (EntityId, Serial) {
        let (item, serial) = state.registry.spawn_with_serial(SerialKind::Item).unwrap();
        state.registry.insert(
            item,
            Drawn {
                id:  Graphic(0x0E75),
                hue: Hue::NONE,
            },
        );
        state.registry.insert(
            item,
            Container {
                gump: Graphic(0x003C),
            },
        );
        establish_item_location(state, item, ItemLocation::ground(Facet(0), Point::new(x, 1, 0))).unwrap();
        (item, serial)
    }

    fn pile(state: &mut WorldState, parent: Serial, amount: u16, semantic: bool) -> (EntityId, Serial) {
        let (item, serial) = state.registry.spawn_with_serial(SerialKind::Item).unwrap();
        state.registry.insert(
            item,
            Drawn {
                id:  Graphic(0x1BF2),
                hue: Hue::NONE,
            },
        );
        if semantic {
            state.registry.insert(item, ItemKind(ItemKindId(1)));
            state.registry.insert(item, Material(MaterialId(1)));
        }
        if amount > 1 {
            state.registry.insert(item, Amount(amount));
        }
        establish_item_location(
            state,
            item,
            ItemLocation::contained(Contained {
                container: parent,
                position:  GumpPoint::new(0, 0),
                grid:      GridSlot(0),
            }),
        )
        .unwrap();
        (item, serial)
    }

    fn ingot_key() -> CraftKey {
        openshard_protocol::craft::craft_key_for(
            Some((ItemKindId(1), Some(MaterialId(1)))),
            Graphic(0x1BF2),
            Hue::NONE,
        )
        .unwrap()
    }

    #[test]
    fn moving_a_nested_branch_reprojects_each_root_once_and_keeps_ordered_piles() {
        let mut state = world();
        let (_, first) = root_item(&mut state, 1);
        let (_, second) = root_item(&mut state, 2);
        let (bag, bag_serial) = root_item(&mut state, 3);
        crate::relocate_item(
            &mut state,
            bag,
            ItemLocation::contained(Contained {
                container: first,
                position:  GumpPoint::new(0, 0),
                grid:      GridSlot(0),
            }),
        )
        .unwrap();
        let (_, high) = pile(&mut state, bag_serial, 7, true);
        let (_, low) = pile(&mut state, bag_serial, 5, false);
        let (_, piles) = state.craft_stock_piles(first, &[ingot_key()]).unwrap();
        assert_eq!(
            piles.iter().map(|pile| pile.serial).collect::<Vec<_>>(),
            vec![high, low]
        );
        assert_eq!(
            state.craft_stock_amounts(first).unwrap().1[usize::from(ingot_key().0)],
            12
        );

        crate::relocate_item(
            &mut state,
            bag,
            ItemLocation::contained(Contained {
                container: second,
                position:  GumpPoint::new(0, 0),
                grid:      GridSlot(0),
            }),
        )
        .unwrap();
        assert_eq!(
            state.craft_stock_amounts(first).unwrap().1[usize::from(ingot_key().0)],
            0
        );
        assert_eq!(
            state.craft_stock_amounts(second).unwrap().1[usize::from(ingot_key().0)],
            12
        );
    }

    #[test]
    fn a_root_over_the_realtime_item_ceiling_is_unavailable() {
        let mut state = world();
        let (_, root) = root_item(&mut state, 1);
        for _ in 0..=MAX_CRAFT_SOURCE_ITEMS {
            pile(&mut state, root, 1, true);
        }
        assert_eq!(
            state.craft_stock_item_count(root),
            Err(CraftStockError::TooComplex)
        );
        assert_eq!(
            state.craft_stock_piles(root, &[ingot_key()]),
            Err(CraftStockError::TooComplex)
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn dense_stock_totals_match_a_slow_recursive_oracle(
            specs in prop::collection::vec((1_u16..=1_000, any::<bool>(), any::<bool>()), 0..40),
        ) {
            let mut state = world();
            let (_, root) = root_item(&mut state, 1);
            let (bag, bag_serial) = root_item(&mut state, 2);
            crate::relocate_item(
                &mut state,
                bag,
                ItemLocation::contained(Contained {
                    container: root,
                    position: GumpPoint::new(0, 0),
                    grid: GridSlot(0),
                }),
            ).unwrap();
            let mut expected = 0u32;
            for (amount, semantic, nested) in specs {
                pile(&mut state, if nested { bag_serial } else { root }, amount, semantic);
                expected += u32::from(amount);
            }
            let (_, totals) = state.craft_stock_amounts(root).unwrap();
            prop_assert_eq!(totals[usize::from(ingot_key().0)], expected);
            let (_, piles) = state.craft_stock_piles(root, &[ingot_key()]).unwrap();
            prop_assert_eq!(piles.iter().map(|pile| u32::from(pile.amount)).sum::<u32>(), expected);
            prop_assert!(piles.windows(2).all(|pair| pair[0].serial < pair[1].serial));
        }
    }
}
