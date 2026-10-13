//! A least-recently-used map from source text to its compile result.

use std::collections::HashMap;

struct Entry<V> {
    value: V,
    used: u64,
}

/// A bounded map with least-recently-used eviction.
///
/// When it is full, the least recently used eighth goes at once, so the cost
/// of finding them is amortised over the inserts that follow. It never empties
/// itself wholesale: a working set that fits the capacity keeps hitting.
pub(crate) struct Cache<V: Clone> {
    entries: HashMap<String, Entry<V>>,
    clock: u64,
    capacity: usize,
}

impl<V: Clone> Cache<V> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            clock: 0,
            capacity: capacity.max(1),
        }
    }

    /// Raises the capacity to hold at least `entries` (never lowers it).
    pub(crate) fn reserve(&mut self, entries: usize) {
        self.capacity = self.capacity.max(entries);
    }

    /// The value for `key`, marking it as just used.
    pub(crate) fn get(&mut self, key: &str) -> Option<V> {
        self.clock += 1;
        let clock = self.clock;
        let entry = self.entries.get_mut(key)?;
        entry.used = clock;
        Some(entry.value.clone())
    }

    /// The value for `key` without marking it used.
    pub(crate) fn peek(&self, key: &str) -> Option<&V> {
        self.entries.get(key).map(|entry| &entry.value)
    }

    pub(crate) fn insert(&mut self, key: &str, value: V) {
        if self.entries.len() >= self.capacity {
            self.evict();
        }
        self.clock += 1;
        let entry = Entry {
            value,
            used: self.clock,
        };
        self.entries.insert(key.to_owned(), entry);
    }

    fn evict(&mut self) {
        let mut stamps: Vec<u64> = self.entries.values().map(|entry| entry.used).collect();
        if stamps.is_empty() {
            return;
        }
        let cut = (stamps.len() / 8).max(1);
        let (_, &mut threshold, _) = stamps.select_nth_unstable(cut - 1);
        self.entries.retain(|_, entry| entry.used > threshold);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eviction_drops_the_oldest_eighth_and_keeps_the_recently_used() {
        let mut cache = Cache::new(80);
        for i in 0..80 {
            cache.insert(&format!("k{i}"), i);
        }
        // Touch the oldest, so it is no longer the least recently used.
        assert_eq!(cache.get("k0"), Some(0));
        cache.insert("new", 100);
        assert_eq!(cache.len(), 80 - 10 + 1);
        assert_eq!(cache.peek("k0"), Some(&0), "a used entry survives");
        assert_eq!(cache.peek("k1"), None, "the oldest unused goes");
        assert_eq!(cache.peek("k79"), Some(&79));
        assert_eq!(cache.peek("new"), Some(&100));
    }

    #[test]
    fn a_working_set_that_fits_never_misses() {
        let mut cache = Cache::new(100);
        for round in 0..5 {
            for i in 0..100 {
                let key = format!("k{i}");
                if cache.get(&key).is_none() {
                    assert_eq!(round, 0, "{key} missed after the first round");
                    cache.insert(&key, i);
                }
            }
        }
    }

    #[test]
    fn reserve_only_raises_the_capacity() {
        let mut cache: Cache<u8> = Cache::new(10);
        cache.reserve(5);
        for i in 0..10 {
            cache.insert(&format!("k{i}"), 0);
        }
        assert_eq!(cache.len(), 10);
        cache.reserve(20);
        for i in 10..20 {
            cache.insert(&format!("k{i}"), 0);
        }
        assert_eq!(cache.len(), 20);
    }
}
