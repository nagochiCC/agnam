use std::collections::BTreeMap;

struct CacheEntry<T> {
    value: T,
    accounted_bytes: usize,
    last_used: u64,
}

pub(super) struct ThumbnailCache<T> {
    entries: BTreeMap<usize, CacheEntry<T>>,
    budget: usize,
    accounted_bytes: usize,
    recency: u64,
    warmup_saturated: bool,
}

impl<T> ThumbnailCache<T> {
    pub(super) fn new(budget: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            budget,
            accounted_bytes: 0,
            recency: 0,
            warmup_saturated: false,
        }
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.accounted_bytes = 0;
        self.recency = 0;
        self.warmup_saturated = false;
    }

    pub(super) fn contains(&self, index: usize) -> bool {
        self.entries.contains_key(&index)
    }

    pub(super) fn warmup_saturated(&self) -> bool {
        self.warmup_saturated
    }

    /// Returns true only when this insertion first fills the warmup budget.
    pub(super) fn insert<F>(
        &mut self,
        index: usize,
        value: T,
        accounted_bytes: usize,
        is_pinned: F,
    ) -> bool
    where
        F: Fn(usize) -> bool,
    {
        self.recency = self.recency.wrapping_add(1);
        if let Some(previous) = self.entries.insert(
            index,
            CacheEntry {
                value,
                accounted_bytes,
                last_used: self.recency,
            },
        ) {
            self.accounted_bytes = self
                .accounted_bytes
                .saturating_sub(previous.accounted_bytes);
        }
        self.accounted_bytes = self.accounted_bytes.saturating_add(accounted_bytes);

        let newly_saturated = !self.warmup_saturated && self.accounted_bytes >= self.budget;
        self.warmup_saturated |= self.accounted_bytes >= self.budget;
        while self.accounted_bytes > self.budget {
            let Some(oldest) = self
                .entries
                .iter()
                .filter(|(index, _)| !is_pinned(**index))
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(index, _)| *index)
            else {
                break;
            };
            if let Some(evicted) = self.entries.remove(&oldest) {
                self.accounted_bytes = self.accounted_bytes.saturating_sub(evicted.accounted_bytes);
            }
        }
        newly_saturated
    }

    pub(super) fn touch(&mut self, index: usize) {
        let Some(entry) = self.entries.get_mut(&index) else {
            return;
        };
        self.recency = self.recency.wrapping_add(1);
        entry.last_used = self.recency;
    }

    pub(super) fn get(&self, index: usize) -> Option<&T> {
        self.entries.get(&index).map(|entry| &entry.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_within_budget_are_not_evicted() {
        let mut cache = ThumbnailCache::new(11);

        assert!(!cache.insert(1, 'a', 4, |_| false));
        assert!(!cache.insert(2, 'b', 6, |_| false));

        assert!(cache.contains(1));
        assert!(cache.contains(2));
        assert_eq!(cache.accounted_bytes, 10);
    }

    #[test]
    fn reaching_the_budget_marks_warmup_saturated_without_eviction() {
        let mut cache = ThumbnailCache::new(10);
        cache.insert(1, 'a', 4, |_| false);

        assert!(cache.insert(2, 'b', 6, |_| false));
        assert!(cache.contains(1));
        assert!(cache.contains(2));
        assert!(cache.warmup_saturated());
    }

    #[test]
    fn exceeding_the_budget_evicts_the_oldest_entry() {
        let mut cache = ThumbnailCache::new(10);
        cache.insert(1, 'a', 4, |_| false);
        cache.insert(2, 'b', 4, |_| false);

        assert!(cache.insert(3, 'c', 4, |_| false));

        assert!(!cache.contains(1));
        assert!(cache.contains(2));
        assert!(cache.contains(3));
        assert_eq!(cache.accounted_bytes, 8);
    }

    #[test]
    fn actual_use_keeps_an_entry_ahead_of_an_older_unused_entry() {
        let mut cache = ThumbnailCache::new(10);
        cache.insert(1, 'a', 4, |_| false);
        cache.insert(2, 'b', 4, |_| false);
        cache.touch(1);

        cache.insert(3, 'c', 4, |_| false);

        assert!(cache.contains(1));
        assert!(!cache.contains(2));
        assert!(cache.contains(3));
    }

    #[test]
    fn pinned_entries_can_temporarily_exceed_the_soft_budget() {
        let mut cache = ThumbnailCache::new(10);
        cache.insert(1, 'a', 8, |_| false);

        assert!(cache.insert(2, 'b', 8, |index| index == 1 || index == 2));

        assert!(cache.contains(1));
        assert!(cache.contains(2));
        assert_eq!(cache.accounted_bytes, 16);
    }

    #[test]
    fn pinned_current_preview_is_kept_before_an_unpinned_entry() {
        let mut cache = ThumbnailCache::new(10);
        cache.insert(1, 'a', 6, |_| false);
        cache.insert(2, 'b', 4, |_| false);

        cache.insert(3, 'c', 4, |index| index == 1);

        assert!(cache.contains(1));
        assert!(!cache.contains(2));
        assert!(cache.contains(3));
        assert_eq!(cache.accounted_bytes, 10);
    }

    #[test]
    fn clear_resets_entries_accounting_recency_and_warmup_state() {
        let mut cache = ThumbnailCache::new(10);
        cache.insert(1, 'a', 10, |_| false);
        cache.touch(1);

        cache.clear();

        assert!(cache.entries.is_empty());
        assert_eq!(cache.accounted_bytes, 0);
        assert_eq!(cache.recency, 0);
        assert!(!cache.warmup_saturated());
    }
}
