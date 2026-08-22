//! Shared scheduling and residency policy for recreatable chunk products.
//!
//! This module deliberately knows neither what a product is nor how it is
//! built.  Callers keep their concrete storage and fallback rules; these two
//! values own only the duplicated queue and byte-budget decisions.

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// A bounded, coalescing hand-off between requesters and one producer.
#[derive(Debug)]
pub struct WorkQueue<K: Copy + Ord> {
    max_outstanding: usize,
    work_per_turn: usize,
    pending: BTreeSet<K>,
    in_flight: BTreeSet<K>,
}

impl<K: Copy + Ord> WorkQueue<K> {
    #[must_use]
    pub fn new(max_outstanding: usize, work_per_turn: usize) -> Option<Self> {
        (max_outstanding != 0 && work_per_turn != 0).then_some(Self {
            max_outstanding,
            work_per_turn,
            pending: BTreeSet::new(),
            in_flight: BTreeSet::new(),
        })
    }

    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    #[must_use]
    pub fn in_flight_len(&self) -> usize {
        self.in_flight.len()
    }

    #[must_use]
    pub const fn work_per_turn(&self) -> usize {
        self.work_per_turn
    }

    #[must_use]
    pub fn contains_pending(&self, key: K) -> bool {
        self.pending.contains(&key)
    }

    #[must_use]
    pub fn contains_in_flight(&self, key: K) -> bool {
        self.in_flight.contains(&key)
    }

    pub fn request(&mut self, key: K) -> bool {
        if self.pending.contains(&key) || self.in_flight.contains(&key) {
            return true;
        }
        if self.pending.len() + self.in_flight.len() >= self.max_outstanding {
            return false;
        }
        self.pending.insert(key)
    }

    pub fn reconcile(&mut self, mut wanted: impl FnMut(K) -> bool) {
        self.pending.retain(|key| wanted(*key));
    }

    pub fn pending_keys(&self) -> impl Iterator<Item = K> + '_ {
        self.pending.iter().copied()
    }

    pub fn in_flight_keys(&self) -> impl Iterator<Item = K> + '_ {
        self.in_flight.iter().copied()
    }

    pub fn drop_pending(&mut self, key: K) -> bool {
        self.pending.remove(&key)
    }

    #[must_use]
    pub fn take_for_producer(&mut self, mut order: impl FnMut(&K, &K) -> Ordering) -> Vec<K> {
        self.take_for_producer_if(|_| true, |left, right| order(left, right))
    }

    #[must_use]
    pub fn take_for_producer_if(
        &mut self,
        mut eligible: impl FnMut(K) -> bool,
        mut order: impl FnMut(&K, &K) -> Ordering,
    ) -> Vec<K> {
        let mut keys: Vec<_> = self
            .pending
            .iter()
            .copied()
            .filter(|key| eligible(*key))
            .collect();
        keys.sort_by(|left, right| order(left, right));
        keys.truncate(self.work_per_turn);
        for key in &keys {
            self.pending.remove(key);
            self.in_flight.insert(*key);
        }
        keys
    }

    /// Dispatch ordered work against a cost budget.  One job is always
    /// allowed so a product whose indivisible cost exceeds one turn's budget
    /// cannot starve forever.
    #[must_use]
    pub fn take_for_producer_by_cost(
        &mut self,
        mut order: impl FnMut(&K, &K) -> Ordering,
        mut cost: impl FnMut(K) -> usize,
    ) -> Vec<K> {
        let mut keys: Vec<_> = self.pending.iter().copied().collect();
        keys.sort_by(|left, right| order(left, right));
        let mut spent = 0usize;
        let mut take = 0usize;
        for key in &keys {
            let next = cost(*key).max(1);
            if take != 0 && spent.saturating_add(next) > self.work_per_turn {
                break;
            }
            spent = spent.saturating_add(next);
            take += 1;
        }
        keys.truncate(take);
        for key in &keys {
            self.pending.remove(key);
            self.in_flight.insert(*key);
        }
        keys
    }

    pub fn finish(&mut self, key: K) -> bool {
        self.in_flight.remove(&key)
    }

    pub fn abandon(&mut self, key: K) -> bool {
        self.finish(key)
    }

    pub fn invalidate_matching(&mut self, mut stale: impl FnMut(&K) -> bool) -> usize {
        let before = self.pending.len() + self.in_flight.len();
        self.pending.retain(|key| !stale(key));
        self.in_flight.retain(|key| !stale(key));
        before - self.pending.len() - self.in_flight.len()
    }

    pub fn clear(&mut self) -> usize {
        let removed = self.pending.len() + self.in_flight.len();
        self.pending.clear();
        self.in_flight.clear();
        removed
    }
}

#[derive(Clone, Copy, Debug)]
struct BudgetEntry {
    bytes: u64,
    last_used: u64,
}

/// The storage-independent result of one LRU maintenance pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LruEviction<K> {
    pub keys: Vec<K>,
    pub freed_bytes: u64,
    pub retained_bytes: u64,
    pub protected_over_budget_bytes: u64,
}

impl<K> Default for LruEviction<K> {
    fn default() -> Self {
        Self {
            keys: Vec::new(),
            freed_bytes: 0,
            retained_bytes: 0,
            protected_over_budget_bytes: 0,
        }
    }
}

/// A byte-accounting use clock and protected-set eviction policy.
#[derive(Debug)]
pub struct LruBudget<K: Copy + Ord> {
    max_bytes: u64,
    clock: Cell<u64>,
    entries: RefCell<BTreeMap<K, BudgetEntry>>,
    protected: BTreeSet<K>,
}

impl<K: Copy + Ord> LruBudget<K> {
    #[must_use]
    pub fn new(max_bytes: u64) -> Option<Self> {
        (max_bytes != 0).then_some(Self {
            max_bytes,
            clock: Cell::new(0),
            entries: RefCell::new(BTreeMap::new()),
            protected: BTreeSet::new(),
        })
    }

    pub fn insert(&mut self, key: K, bytes: u64) {
        let stamp = self.next_stamp();
        self.entries.borrow_mut().insert(
            key,
            BudgetEntry {
                bytes,
                last_used: stamp,
            },
        );
    }

    pub fn touch(&self, key: K) -> bool {
        let stamp = self.next_stamp();
        let mut entries = self.entries.borrow_mut();
        let Some(entry) = entries.get_mut(&key) else {
            return false;
        };
        entry.last_used = stamp;
        true
    }

    pub fn remove(&mut self, key: K) -> bool {
        self.protected.remove(&key);
        self.entries.borrow_mut().remove(&key).is_some()
    }

    pub fn clear(&mut self) {
        self.entries.get_mut().clear();
        self.protected.clear();
    }

    pub fn set_protected(&mut self, keys: impl IntoIterator<Item = K>) {
        self.protected.clear();
        self.protected.extend(keys);
    }

    pub fn set_max_bytes(&mut self, max_bytes: u64) {
        self.max_bytes = max_bytes.max(1);
    }

    #[must_use]
    pub fn retained_bytes(&self) -> u64 {
        self.entries.borrow().values().map(|entry| entry.bytes).sum()
    }

    #[must_use]
    pub fn evict_to_budget(&mut self) -> LruEviction<K> {
        self.evict_to_budget_by(|_| 0u8)
    }

    #[must_use]
    pub fn evict_to_budget_by<P: Ord>(&mut self, mut priority: impl FnMut(K) -> P) -> LruEviction<K> {
        let mut retained = self.retained_bytes();
        let mut report = LruEviction {
            retained_bytes: retained,
            ..LruEviction::default()
        };
        if retained <= self.max_bytes {
            return report;
        }
        let mut candidates: Vec<_> = self
            .entries
            .borrow()
            .iter()
            .filter_map(|(key, entry)| {
                (!self.protected.contains(key)).then_some((priority(*key), entry.last_used, *key))
            })
            .collect();
        candidates.sort_unstable();
        for (_, _, key) in candidates {
            if retained <= self.max_bytes {
                break;
            }
            let Some(entry) = self.entries.get_mut().remove(&key) else {
                continue;
            };
            retained = retained.saturating_sub(entry.bytes);
            report.keys.push(key);
            report.freed_bytes = report.freed_bytes.saturating_add(entry.bytes);
        }
        report.retained_bytes = retained;
        report.protected_over_budget_bytes = retained.saturating_sub(self.max_bytes);
        report
    }

    fn next_stamp(&self) -> u64 {
        let stamp = self.clock.get().wrapping_add(1);
        self.clock.set(stamp);
        stamp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_coalesces_bounds_and_releases_work() {
        let mut queue = WorkQueue::new(2, 1).unwrap();
        assert!(queue.request(2));
        assert!(queue.request(2));
        assert!(queue.request(1));
        assert!(!queue.request(3));
        assert_eq!(queue.take_for_producer(Ord::cmp), vec![1]);
        assert!(queue.finish(1));
        queue.reconcile(|key| key != 2);
        assert_eq!(queue.pending_len(), 0);
    }

    #[test]
    fn lru_budget_keeps_protected_entries_and_returns_storage_keys() {
        let mut budget = LruBudget::new(20).unwrap();
        budget.insert(1, 10);
        budget.insert(2, 10);
        budget.insert(3, 10);
        assert!(budget.touch(1));
        budget.set_protected([2]);
        let report = budget.evict_to_budget();
        assert_eq!(report.keys, vec![3]);
        assert_eq!(report.retained_bytes, 20);
    }
}
