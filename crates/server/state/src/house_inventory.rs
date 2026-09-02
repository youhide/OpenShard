//! Derived, permission-partitioned inventory of player houses.
//!
//! The registry remains authoritative. This index is a read-only projection of
//! ground roots carrying [`LockedDown`](crate::LockedDown) and their canonical
//! contained descendants. Mutations invalidate one house in constant logical
//! work; a tick-owned budget rebuilds it before search becomes available again.

use std::collections::{
    BTreeMap,
    HashSet,
    VecDeque,
};
use std::ops::Bound;

use openshard_entities::EntityId;
use openshard_map::grid::Tile;
pub use openshard_protocol::house_inventory::{
    HouseInventoryCursor,
    HouseItemIdentity,
    MAX_HOUSE_INVENTORY_PAGE,
    MAX_HOUSE_INVENTORY_SELECTORS,
};
use openshard_protocol::serial::Serial;

use crate::components::{
    Amount,
    Container,
    CorpseBody,
    Drawn,
    House,
    ItemKind,
    ItemLocation,
    LockedDown,
    Material,
    SettledItemLocation,
    Standing,
    TradeWindow,
};
use crate::{
    WorldState,
    contained_items,
};

/// Projection work units performed by the world tick.
pub const HOUSE_INVENTORY_REBUILD_BUDGET: usize = 256;

/// Resolve one live item's searchable identity.
#[must_use]
pub fn house_item_identity(state: &WorldState, item: EntityId) -> Option<HouseItemIdentity> {
    match state.registry.get::<ItemKind>(item) {
        Some(ItemKind(kind)) => {
            Some(HouseItemIdentity::Semantic {
                kind:     *kind,
                material: state.registry.get::<Material>(item).map(|material| material.0),
            })
        }
        None => {
            state.registry.get::<Drawn>(item).map(|drawn| {
                HouseItemIdentity::Legacy {
                    graphic: drawn.id,
                    hue:     drawn.hue,
                }
            })
        }
    }
}

/// One permitted root contributing to an aggregate identity result.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HouseInventoryResult {
    pub identity:         HouseItemIdentity,
    /// Total quantity of this identity across every root the actor may search.
    pub aggregate_total:  u64,
    pub root:             Serial,
    pub root_total:       u64,
    pub first_pile:       Serial,
    pub pile_count:       usize,
    pub minimum_standing: Standing,
}

/// One bounded page of house inventory roots.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HouseInventoryPage {
    pub epoch: u64,
    pub rows:  Vec<HouseInventoryResult>,
    pub next:  Option<HouseInventoryCursor>,
}

/// Why the derived search projection could not answer now.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HouseInventoryError {
    EmptySelectors,
    TooManySelectors,
    InvalidPageSize,
    /// The house changed and its current epoch is still being rebuilt.
    Unavailable {
        epoch: u64,
    },
    /// A continuation belongs to an older projection.
    StaleEpoch {
        current: u64,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct InventoryKey {
    standing: Standing,
    identity: HouseItemIdentity,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct RootCandidate {
    entity: EntityId,
    serial: Serial,
}

#[derive(Debug)]
struct RootInventory {
    total: u64,
    piles: BTreeMap<Serial, EntityId>,
}

impl RootInventory {
    fn new() -> Self {
        Self {
            total: 0,
            piles: BTreeMap::new(),
        }
    }
}

#[derive(Debug)]
struct InventoryRow {
    total: u64,
    roots: BTreeMap<Serial, RootInventory>,
}

impl InventoryRow {
    fn new() -> Self {
        Self {
            total: 0,
            roots: BTreeMap::new(),
        }
    }
}

#[derive(Debug)]
struct ReadyProjection {
    epoch: u64,
    rows:  BTreeMap<InventoryKey, InventoryRow>,
}

#[derive(Clone, Copy, Debug)]
struct Visit {
    root:     Serial,
    standing: Standing,
    item:     EntityId,
}

#[derive(Debug)]
struct Rebuild {
    epoch:       u64,
    root_cursor: usize,
    pending:     Vec<Visit>,
    visited:     HashSet<EntityId>,
    rows:        BTreeMap<InventoryKey, InventoryRow>,
}

impl Rebuild {
    fn new(epoch: u64) -> Self {
        Self {
            epoch,
            root_cursor: 0,
            pending: Vec::new(),
            visited: HashSet::new(),
            rows: BTreeMap::new(),
        }
    }
}

#[derive(Debug)]
struct HouseProjection {
    epoch:   u64,
    roots:   Vec<RootCandidate>,
    ready:   Option<ReadyProjection>,
    rebuild: Option<Rebuild>,
    queued:  bool,
}

impl HouseProjection {
    fn new() -> Self {
        Self {
            epoch:   1,
            roots:   Vec::new(),
            ready:   None,
            rebuild: None,
            queued:  false,
        }
    }
}

/// All house projections. Persist canonical facts, never this value.
#[derive(Debug)]
pub(crate) struct HouseInventoryIndex {
    houses:  BTreeMap<Serial, HouseProjection>,
    pending: VecDeque<Serial>,
}

impl HouseInventoryIndex {
    pub(crate) fn new() -> Self {
        Self {
            houses:  BTreeMap::new(),
            pending: VecDeque::new(),
        }
    }

    fn projection_mut(&mut self, house: Serial) -> &mut HouseProjection {
        self.houses.entry(house).or_insert_with(HouseProjection::new)
    }

    fn invalidate(&mut self, house: Serial) {
        let projection = self.projection_mut(house);
        projection.epoch = projection.epoch.wrapping_add(1).max(1);
        if !projection.queued {
            projection.queued = true;
            self.pending.push_back(house);
        }
    }

    fn note_lockdown(
        &mut self,
        item: EntityId,
        serial: Option<Serial>,
        previous: Option<LockedDown>,
        current: Option<LockedDown>,
    ) {
        if let Some(previous) = previous {
            let projection = self.projection_mut(previous.house);
            projection.roots.retain(|candidate| candidate.entity != item);
            self.invalidate(previous.house);
        }
        if let (Some(current), Some(serial)) = (current, serial) {
            let projection = self.projection_mut(current.house);
            if !projection.roots.iter().any(|candidate| candidate.entity == item) {
                let at = projection
                    .roots
                    .binary_search_by_key(&serial, |candidate| candidate.serial)
                    .unwrap_or_else(|at| at);
                projection
                    .roots
                    .insert(at, RootCandidate { entity: item, serial });
            }
            self.invalidate(current.house);
        }
    }

    fn advance(&mut self, state: &WorldState, mut budget: usize) -> usize {
        let available = budget;
        while budget > 0 {
            let Some(house) = self.pending.pop_front() else {
                break;
            };
            let Some(projection) = self.houses.get_mut(&house) else {
                continue;
            };
            projection.queued = false;
            let starts = projection
                .rebuild
                .as_ref()
                .is_none_or(|rebuild| rebuild.epoch != projection.epoch);
            if starts {
                projection.rebuild = Some(Rebuild::new(projection.epoch));
                // Establishing a fresh epoch is work too. Charging it matters
                // for an empty shard with many houses: otherwise every empty
                // projection could finalize in one allegedly bounded call.
                budget -= 1;
            }

            let finished = rebuild_house(state, house, projection, &mut budget);
            if finished {
                let rebuild = projection
                    .rebuild
                    .take()
                    .expect("a finished house rebuild exists");
                projection.ready = Some(ReadyProjection {
                    epoch: rebuild.epoch,
                    rows:  rebuild.rows,
                });
            } else if !projection.queued {
                projection.queued = true;
                self.pending.push_back(house);
            }
        }
        available - budget
    }

    fn page(
        &self,
        house: Serial,
        standing: Standing,
        expected_epoch: Option<u64>,
        selectors: &[HouseItemIdentity],
        after: Option<HouseInventoryCursor>,
        limit: usize,
    ) -> Result<HouseInventoryPage, HouseInventoryError> {
        if selectors.is_empty() {
            return Err(HouseInventoryError::EmptySelectors);
        }
        if selectors.len() > MAX_HOUSE_INVENTORY_SELECTORS {
            return Err(HouseInventoryError::TooManySelectors);
        }
        if limit == 0 || limit > MAX_HOUSE_INVENTORY_PAGE {
            return Err(HouseInventoryError::InvalidPageSize);
        }
        let Some(projection) = self.houses.get(&house) else {
            return Err(HouseInventoryError::Unavailable { epoch: 0 });
        };
        if expected_epoch.is_some_and(|expected| expected != projection.epoch) {
            return Err(HouseInventoryError::StaleEpoch {
                current: projection.epoch,
            });
        }
        let Some(ready) = projection
            .ready
            .as_ref()
            .filter(|ready| ready.epoch == projection.epoch)
        else {
            return Err(HouseInventoryError::Unavailable {
                epoch: projection.epoch,
            });
        };

        let selectors: std::collections::BTreeSet<_> = selectors.iter().copied().collect();
        let mut found = Vec::with_capacity(limit.saturating_add(1));
        for identity in selectors {
            if after.is_some_and(|cursor| identity < cursor.identity) {
                continue;
            }
            let aggregate_total = accessible_rows(&ready.rows, standing, identity)
                .map(|(_, row)| row.total)
                .sum();
            if aggregate_total == 0 {
                continue;
            }
            let after_root = after
                .filter(|cursor| cursor.identity == identity)
                .map(|cursor| cursor.root);
            append_root_results(
                &ready.rows,
                standing,
                identity,
                aggregate_total,
                after_root,
                limit.saturating_add(1),
                &mut found,
            );
            if found.len() > limit {
                break;
            }
        }

        let next = (found.len() > limit).then(|| {
            let last = found[limit - 1];
            HouseInventoryCursor {
                identity: last.identity,
                root:     last.root,
            }
        });
        found.truncate(limit);
        Ok(HouseInventoryPage {
            epoch: projection.epoch,
            rows: found,
            next,
        })
    }

    fn contains(
        &self,
        house: Serial,
        epoch: u64,
        standing: Standing,
        identity: HouseItemIdentity,
        root: Serial,
        item: Serial,
    ) -> bool {
        let Some(projection) = self.houses.get(&house) else {
            return false;
        };
        let Some(ready) = projection
            .ready
            .as_ref()
            .filter(|ready| ready.epoch == epoch && ready.epoch == projection.epoch)
        else {
            return false;
        };
        accessible_rows(&ready.rows, standing, identity).any(|(_, row)| {
            row.roots
                .get(&root)
                .is_some_and(|root| root.piles.contains_key(&item))
        })
    }
}

fn accessible_rows(
    rows: &BTreeMap<InventoryKey, InventoryRow>,
    standing: Standing,
    identity: HouseItemIdentity,
) -> impl Iterator<Item = (Standing, &InventoryRow)> {
    [
        Standing::Stranger,
        Standing::Friend,
        Standing::CoOwner,
        Standing::Owner,
    ]
    .into_iter()
    .filter(move |&minimum| standing >= minimum)
    .filter_map(move |minimum| {
        rows.get(&InventoryKey {
            standing: minimum,
            identity,
        })
        .map(|row| (minimum, row))
    })
}

fn append_root_results(
    rows: &BTreeMap<InventoryKey, InventoryRow>,
    standing: Standing,
    identity: HouseItemIdentity,
    aggregate_total: u64,
    after_root: Option<Serial>,
    ceiling: usize,
    found: &mut Vec<HouseInventoryResult>,
) {
    let start = after_root.map_or(Bound::Unbounded, Bound::Excluded);
    let mut ranges: Vec<_> = accessible_rows(rows, standing, identity)
        .map(|(minimum, row)| (minimum, row.roots.range((start, Bound::Unbounded)).peekable()))
        .collect();
    while found.len() < ceiling {
        let next_root = ranges
            .iter_mut()
            .filter_map(|(_, range)| range.peek().map(|(&root, _)| root))
            .min();
        let Some(next_root) = next_root else {
            break;
        };
        let mut minimum = Standing::Owner;
        let mut root_total = 0_u64;
        let mut first_pile = None;
        let mut pile_count = 0_usize;
        for (threshold, range) in &mut ranges {
            if range.peek().is_some_and(|(&root, _)| root == next_root) {
                let (_, root) = range.next().expect("the peeked root exists");
                minimum = minimum.min(*threshold);
                root_total = root_total.saturating_add(root.total);
                first_pile = first_pile.or_else(|| root.piles.keys().next().copied());
                pile_count = pile_count.saturating_add(root.piles.len());
            }
        }
        if let Some(first_pile) = first_pile {
            found.push(HouseInventoryResult {
                identity,
                aggregate_total,
                root: next_root,
                root_total,
                first_pile,
                pile_count,
                minimum_standing: minimum,
            });
        }
    }
}

fn rebuild_house(
    state: &WorldState,
    house: Serial,
    projection: &mut HouseProjection,
    budget: &mut usize,
) -> bool {
    let rebuild = projection.rebuild.as_mut().expect("a queued rebuild exists");
    loop {
        if let Some(visit) = rebuild.pending.pop() {
            if *budget == 0 {
                rebuild.pending.push(visit);
                return false;
            }
            *budget -= 1;
            visit_item(state, rebuild, visit);
            continue;
        }
        let Some(&candidate) = projection.roots.get(rebuild.root_cursor) else {
            return true;
        };
        if *budget == 0 {
            return false;
        }
        rebuild.root_cursor += 1;
        *budget -= 1;
        if let Some(standing) = eligible_root(state, house, candidate) {
            rebuild.pending.push(Visit {
                root: candidate.serial,
                standing,
                item: candidate.entity,
            });
        }
    }
}

fn eligible_root(state: &WorldState, house: Serial, candidate: RootCandidate) -> Option<Standing> {
    if state.registry.serial_of(candidate.entity) != Some(candidate.serial)
        || state.registry.has::<CorpseBody>(candidate.entity)
        || state.registry.has::<TradeWindow>(candidate.entity)
    {
        return None;
    }
    let locked = state.registry.get::<LockedDown>(candidate.entity)?;
    if locked.house != house {
        return None;
    }
    let ItemLocation::Settled(SettledItemLocation::Ground { facet, position }) =
        *state.registry.get::<ItemLocation>(candidate.entity)?
    else {
        return None;
    };
    let house_entity = state.registry.entity_of(house)?;
    if state.registry.get::<House>(house_entity).is_none()
        || !state
            .facet_state_if_loaded(facet)?
            .houses_covering(Tile::new(position.x, position.y))
            .contains(&house_entity)
    {
        return None;
    }
    Some(match locked.secure {
        Some(Standing::Banned) | Some(Standing::Stranger) => Standing::Stranger,
        Some(standing) => standing,
        None => Standing::CoOwner,
    })
}

fn visit_item(state: &WorldState, rebuild: &mut Rebuild, visit: Visit) {
    if !rebuild.visited.insert(visit.item)
        || state.registry.has::<CorpseBody>(visit.item)
        || state.registry.has::<TradeWindow>(visit.item)
    {
        return;
    }
    let Some(serial) = state.registry.serial_of(visit.item) else {
        return;
    };
    if serial != visit.root && state.registry.has::<LockedDown>(visit.item) {
        return;
    }
    let Some(identity) = house_item_identity(state, visit.item) else {
        return;
    };
    let amount = state
        .registry
        .get::<Amount>(visit.item)
        .map_or(1, |amount| amount.0);
    if amount == 0 {
        return;
    }
    let key = InventoryKey {
        standing: visit.standing,
        identity,
    };
    let row = rebuild.rows.entry(key).or_insert_with(InventoryRow::new);
    row.total = row.total.saturating_add(u64::from(amount));
    let root = row.roots.entry(visit.root).or_insert_with(RootInventory::new);
    root.total = root.total.saturating_add(u64::from(amount));
    root.piles.insert(serial, visit.item);

    if state.registry.has::<Container>(visit.item) {
        let mut children: Vec<_> = contained_items(state, serial).map(|(item, _)| item).collect();
        children.sort_by_key(|&item| (state.registry.serial_of(item), item));
        for item in children.into_iter().rev() {
            rebuild.pending.push(Visit { item, ..visit });
        }
    }
}

impl WorldState {
    /// Replace an item's lockdown fact and update the house-root projection.
    pub fn set_item_lockdown(&mut self, item: EntityId, locked: Option<LockedDown>) -> Option<LockedDown> {
        let previous = self.registry.get::<LockedDown>(item).copied();
        if previous == locked {
            return previous;
        }
        match locked {
            Some(locked) => {
                self.registry.insert(item, locked);
            }
            None => {
                self.registry.remove::<LockedDown>(item);
                // An addon's grouping is a fact about pinned house furniture: a
                // component that has gone loose — released, or swept into a
                // collapsed house's crate — is an ordinary item again, not half
                // of an oven whose other half is now somewhere else. See
                // [`AddonPart`](crate::components::AddonPart).
                self.registry.remove::<crate::components::AddonPart>(item);
                // And with the grouping goes the loom's half-woven count and the
                // spinning wheel's timer, for the same reason: both are state
                // about an *installed* addon, and a tile in a moving crate is no
                // longer one. The count is forfeit rather than refunded, which is
                // the same bargain the whole addon makes — a collapsed house
                // hands back the deed, not the boards.
                self.registry.remove::<crate::components::LoomPhase>(item);
                self.registry.remove::<crate::components::Spinning>(item);
            }
        }
        let serial = self.registry.serial_of(item);
        self.house_inventory.note_lockdown(item, serial, previous, locked);
        previous
    }

    /// Make one house's current projection unavailable and queue a rebuild.
    pub fn invalidate_house_inventory(&mut self, house: Serial) {
        self.house_inventory.invalidate(house);
    }

    /// Invalidate the house storage domain currently containing `item`, if any.
    pub fn invalidate_house_inventory_for_item(&mut self, item: EntityId) {
        if let Some(house) = inventory_house_of_item(self, item) {
            self.house_inventory.invalidate(house);
        }
        self.refresh_craft_stock_for_item(item);
    }

    /// Spend at most `budget` work units rebuilding invalidated projections.
    pub fn advance_house_inventory_rebuilds(&mut self, budget: usize) {
        let began = std::time::Instant::now();
        let mut index = std::mem::replace(&mut self.house_inventory, HouseInventoryIndex::new());
        let spent = index.advance(self, budget);
        tracing::trace!(
            metric = "item_transaction.house_inventory_projection",
            budget,
            spent,
            elapsed_ns = began.elapsed().as_nanos(),
        );
        self.house_inventory = index;
    }

    /// Read one permission-filtered page from a ready projection.
    pub fn house_inventory_page(
        &self,
        house: Serial,
        standing: Standing,
        expected_epoch: Option<u64>,
        selectors: &[HouseItemIdentity],
        after: Option<HouseInventoryCursor>,
        limit: usize,
    ) -> Result<HouseInventoryPage, HouseInventoryError> {
        self.house_inventory
            .page(house, standing, expected_epoch, selectors, after, limit)
    }

    /// Whether a page's exact root/pile reference still belongs to this epoch.
    pub fn house_inventory_contains(
        &self,
        house: Serial,
        epoch: u64,
        standing: Standing,
        identity: HouseItemIdentity,
        root: Serial,
        item: Serial,
    ) -> bool {
        self.house_inventory
            .contains(house, epoch, standing, identity, root, item)
    }
}

fn inventory_house_of_item(state: &WorldState, mut item: EntityId) -> Option<Serial> {
    let mut visited = HashSet::new();
    while visited.insert(item) {
        if let Some(locked) = state.registry.get::<LockedDown>(item) {
            return Some(locked.house);
        }
        let SettledItemLocation::Contained(contained) =
            state.registry.get::<ItemLocation>(item).copied()?.origin()
        else {
            return None;
        };
        item = state.registry.entity_of(contained.container)?;
    }
    None
}

#[cfg(test)]
mod tests {
    use std::collections::{
        BTreeMap,
        BTreeSet,
    };

    use openshard_protocol::containers::GridSlot;
    use openshard_protocol::direction::Direction;
    use openshard_protocol::gump::GumpPoint;
    use openshard_protocol::item_kind::{
        ItemKindId,
        MaterialId,
    };
    use openshard_protocol::serial::SerialKind;
    use openshard_protocol::wire::{
        Graphic,
        Hue,
        MultiId,
    };
    use openshard_protocol::world::{
        Facet,
        Point,
    };
    use openshard_tiles::TileData;
    use openshard_uofiles::multi::Multis;
    use proptest::prelude::*;

    use super::*;
    use crate::components::{
        Contained,
        Position,
    };
    use crate::facet_rules::FacetRules;
    use crate::{
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

    fn house(state: &mut WorldState, at: Point) -> (EntityId, Serial) {
        let (entity, serial) = state.registry.spawn_with_serial(SerialKind::Item).unwrap();
        state.registry.insert(
            entity,
            House {
                multi:     MultiId(1),
                owner:     serial,
                co_owners: BTreeSet::new(),
                friends:   BTreeSet::new(),
                bans:      BTreeSet::new(),
                age:       0,
                lockdowns: 100,
            },
        );
        state.registry.insert(entity, Position(at));
        state
            .facet_state_mut(Facet(0))
            .cover_house(entity, &[Tile::new(at.x, at.y)]);
        state.invalidate_house_inventory(serial);
        (entity, serial)
    }

    fn item(
        state: &mut WorldState,
        drawn: Drawn,
        location: ItemLocation,
        container: bool,
    ) -> (EntityId, Serial) {
        let (entity, serial) = state.registry.spawn_with_serial(SerialKind::Item).unwrap();
        state.registry.insert(entity, drawn);
        if container {
            state.registry.insert(
                entity,
                Container {
                    gump: Graphic(0x003C),
                },
            );
        }
        establish_item_location(state, entity, location).unwrap();
        (entity, serial)
    }

    fn standing(code: u8) -> Standing {
        match code % 5 {
            0 => Standing::Banned,
            1 => Standing::Stranger,
            2 => Standing::Friend,
            3 => Standing::CoOwner,
            _ => Standing::Owner,
        }
    }

    #[test]
    fn an_explicit_nested_lockdown_stops_inherited_root_inventory_access() {
        let mut state = world();
        let inside = Point::new(10, 10, 0);
        let (_, house) = house(&mut state, inside);
        let root_art = Drawn {
            id:  Graphic(0x0E3C),
            hue: Hue::NONE,
        };
        let nested_art = Drawn {
            id:  Graphic(0x0E76),
            hue: Hue::NONE,
        };
        let wanted = HouseItemIdentity::Legacy {
            graphic: Graphic(0x0EED),
            hue:     Hue::NONE,
        };
        let (root, root_serial) = item(&mut state, root_art, ItemLocation::ground(Facet(0), inside), true);
        state.set_item_lockdown(
            root,
            Some(LockedDown {
                house,
                secure: Some(Standing::Friend),
            }),
        );
        let (nested, nested_serial) = item(
            &mut state,
            nested_art,
            ItemLocation::contained(Contained {
                container: root_serial,
                position:  GumpPoint::new(0, 0),
                grid:      GridSlot(0),
            }),
            true,
        );
        let (_, pile_serial) = item(
            &mut state,
            Drawn {
                id:  Graphic(0x0EED),
                hue: Hue::NONE,
            },
            ItemLocation::contained(Contained {
                container: nested_serial,
                position:  GumpPoint::new(1, 1),
                grid:      GridSlot(0),
            }),
            false,
        );

        for _ in 0..16 {
            state.advance_house_inventory_rebuilds(1);
        }
        let inherited = state
            .house_inventory_page(
                house,
                Standing::Friend,
                None,
                &[wanted],
                None,
                MAX_HOUSE_INVENTORY_PAGE,
            )
            .expect("the small projection is available");
        assert_eq!(inherited.rows.len(), 1);
        assert_eq!(inherited.rows[0].first_pile, pile_serial);
        assert_eq!(inherited.rows[0].pile_count, 1);

        state.set_item_lockdown(
            nested,
            Some(LockedDown {
                house,
                secure: Some(Standing::CoOwner),
            }),
        );
        for _ in 0..16 {
            state.advance_house_inventory_rebuilds(1);
        }
        let stopped = state
            .house_inventory_page(
                house,
                Standing::Owner,
                None,
                &[wanted],
                None,
                MAX_HOUSE_INVENTORY_PAGE,
            )
            .expect("the rebuilt projection is available");
        assert!(stopped.rows.is_empty());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// The sparse index agrees with a deliberately simple model across
        /// nested, loose, foreign, outside, excluded and permissioned roots.
        #[test]
        fn indexed_house_inventory_matches_a_slow_root_model(
            specs in prop::collection::vec(
                (any::<bool>(), 0_u8..3, 1_u8..5, any::<bool>(), 1_u16..40, any::<bool>(), 0_u8..3),
                0..16,
            ),
            actor_access in 0_u8..5,
        ) {
            let mut state = world();
            let inside = Point::new(10, 10, 0);
            let outside = Point::new(30, 30, 0);
            let (_, main_house) = house(&mut state, inside);
            let (_, foreign_house) = house(&mut state, Point::new(40, 40, 0));
            let one = HouseItemIdentity::Legacy {
                graphic: Graphic(0x0EED),
                hue: Hue::NONE,
            };
            let two = HouseItemIdentity::Semantic {
                kind: ItemKindId::new(1).unwrap(),
                material: Some(MaterialId::new(1).unwrap()),
            };
            let root_art = Drawn {
                id: Graphic(0x0E3C),
                hue: Hue::NONE,
            };
            let bag_art = Drawn {
                id: Graphic(0x0E76),
                hue: Hue::NONE,
            };
            let mut expected: BTreeMap<HouseItemIdentity, Vec<(Serial, u64)>> = BTreeMap::new();
            let actor = standing(actor_access);

            for (is_inside, lock_kind, access_code, identity_two, amount, nested, excluded) in specs {
                let at = if is_inside { inside } else { outside };
                let (root, root_serial) = item(&mut state, root_art, ItemLocation::ground(Facet(0), at), true);
                match excluded {
                    1 => {
                        state.registry.insert(root, TradeWindow);
                    }
                    2 => {
                        state.registry.insert(root, CorpseBody {
                            body: Graphic(0x0190),
                            facing: Direction::North,
                        });
                    }
                    _ => {}
                }
                let access = standing(access_code);
                match lock_kind {
                    1 => {
                        state.set_item_lockdown(root, Some(LockedDown {
                            house: main_house,
                            secure: Some(access),
                        }));
                    }
                    2 => {
                        state.set_item_lockdown(root, Some(LockedDown {
                            house: foreign_house,
                            secure: Some(access),
                        }));
                    }
                    _ => {}
                }

                let parent = if nested {
                    let (bag, bag_serial) = item(
                        &mut state,
                        bag_art,
                        ItemLocation::contained(Contained {
                            container: root_serial,
                            position: GumpPoint::new(0, 0),
                            grid: GridSlot(0),
                        }),
                        true,
                    );
                    let _ = bag;
                    bag_serial
                } else {
                    root_serial
                };
                let identity = if identity_two { two } else { one };
                let drawn = match identity {
                    HouseItemIdentity::Legacy { graphic, hue } => Drawn { id: graphic, hue },
                    HouseItemIdentity::Semantic { .. } => Drawn {
                        id: Graphic(0x0F7A),
                        hue: Hue::NONE,
                    },
                };
                let (pile, _) = item(
                    &mut state,
                    drawn,
                    ItemLocation::contained(Contained {
                        container: parent,
                        position: GumpPoint::new(1, 1),
                        grid: GridSlot(0),
                    }),
                    false,
                );
                if let HouseItemIdentity::Semantic { kind, material } = identity {
                    state.registry.insert(pile, ItemKind(kind));
                    if let Some(material) = material {
                        state.registry.insert(pile, Material(material));
                    }
                    state.invalidate_house_inventory_for_item(pile);
                }
                if amount > 1 {
                    state.registry.insert(pile, Amount(amount));
                    state.invalidate_house_inventory_for_item(pile);
                }

                if is_inside && lock_kind == 1 && excluded == 0 && actor >= access {
                    match expected.entry(identity) {
                        std::collections::btree_map::Entry::Occupied(mut row) => {
                            row.get_mut().push((root_serial, u64::from(amount)));
                        }
                        std::collections::btree_map::Entry::Vacant(row) => {
                            row.insert(vec![(root_serial, u64::from(amount))]);
                        }
                    }
                }
            }

            for roots in expected.values_mut() {
                roots.sort_by_key(|(root, _)| *root);
            }
            for _ in 0..128 {
                state.advance_house_inventory_rebuilds(1);
            }
            let page = state
                .house_inventory_page(main_house, actor, None, &[one, two], None, MAX_HOUSE_INVENTORY_PAGE)
                .expect("the bounded fixture finishes rebuilding");
            let actual: BTreeMap<_, Vec<_>> = page.rows.iter().fold(BTreeMap::new(), |mut rows, row| {
                match rows.entry(row.identity) {
                    std::collections::btree_map::Entry::Occupied(mut roots) => {
                        roots.get_mut().push((row.root, row.root_total));
                    }
                    std::collections::btree_map::Entry::Vacant(roots) => {
                        roots.insert(vec![(row.root, row.root_total)]);
                    }
                }
                rows
            });
            prop_assert_eq!(&actual, &expected);
            for row in &page.rows {
                let total: u64 = expected[&row.identity].iter().map(|(_, amount)| amount).sum();
                prop_assert_eq!(row.aggregate_total, total);
            }
        }
    }
}
