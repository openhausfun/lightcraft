//! A cost-bounded least-recently-used map.

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering};

static TICK: AtomicU64 = AtomicU64::new(1);

/// A process-wide use counter: entries of different caches (LRUs or not) compare by recency, so
/// a memory budget can evict the least recently used entry across all of them.
pub fn next_tick() -> u64 {
    TICK.fetch_add(1, Ordering::Relaxed)
}

pub struct Lru<K, V> {
    map: HashMap<K, (V, usize, u64)>,
    /// Use tick ([`next_tick`], shared by every LRU) → key, oldest first.
    order: BTreeMap<u64, K>,
    cost: usize,
    budget: usize,
}

impl<K: Eq + Hash + Clone, V> Lru<K, V> {
    /// An LRU holding at most `budget` cost units (e.g. bytes). The most recent entry is always
    /// kept, even if it alone exceeds the budget.
    pub fn new(budget: usize) -> Self {
        Lru { map: HashMap::new(), order: BTreeMap::new(), cost: 0, budget }
    }

    fn touch(&mut self, k: &K) {
        let t = next_tick();
        if let Some(e) = self.map.get_mut(k) {
            self.order.remove(&e.2);
            e.2 = t;
            self.order.insert(t, k.clone());
        }
    }

    pub fn get(&mut self, k: &K) -> Option<&V> {
        if !self.map.contains_key(k) {
            return None;
        }
        self.touch(k);
        self.map.get(k).map(|e| &e.0)
    }

    pub fn peek(&self, k: &K) -> Option<&V> {
        self.map.get(k).map(|e| &e.0)
    }

    /// Use tick of the least recently used entry.
    pub fn oldest_tick(&self) -> Option<u64> {
        self.order.first_key_value().map(|(t, _)| *t)
    }

    /// Remove the least recently used entry; returns its cost.
    pub fn pop_oldest(&mut self) -> Option<usize> {
        let (_, k) = self.order.pop_first()?;
        let (_, c, _) = self.map.remove(&k)?;
        self.cost -= c;
        Some(c)
    }

    pub fn contains(&self, k: &K) -> bool {
        self.map.contains_key(k)
    }

    pub fn insert(&mut self, k: K, v: V, cost: usize) {
        self.remove(&k);
        let t = next_tick();
        self.order.insert(t, k.clone());
        self.map.insert(k, (v, cost, t));
        self.cost += cost;
        while self.cost > self.budget && self.map.len() > 1 {
            let Some((_, old)) = self.order.pop_first() else { break };
            if let Some((_, c, _)) = self.map.remove(&old) {
                self.cost -= c;
            }
        }
    }

    pub fn remove(&mut self, k: &K) -> Option<V> {
        let (v, c, t) = self.map.remove(k)?;
        self.order.remove(&t);
        self.cost -= c;
        Some(v)
    }

    pub fn retain(&mut self, mut f: impl FnMut(&K) -> bool) {
        let drop: Vec<K> = self.map.keys().filter(|k| !f(k)).cloned().collect();
        for k in drop {
            self.remove(&k);
        }
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
        self.cost = 0;
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    pub fn cost(&self) -> usize {
        self.cost
    }
    pub fn budget(&self) -> usize {
        self.budget
    }
    pub fn set_budget(&mut self, budget: usize) {
        self.budget = budget;
    }
}
