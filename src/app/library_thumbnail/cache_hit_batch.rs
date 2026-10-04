use super::scheduler::{CacheLookupCohort, ThumbnailTextureKey};
use crate::bookshelf::thumbnail::ThumbnailData;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Default)]
pub(in crate::app) struct LibraryThumbnailCacheHitBatch {
    generation: u64,
    cohort_id: u64,
    cohort_order: Vec<ThumbnailTextureKey>,
    cohort_remaining: HashSet<ThumbnailTextureKey>,
    cohort_pending: HashMap<ThumbnailTextureKey, ThumbnailData>,
    awaiting_all_hits: bool,
    flush_scheduled: bool,
    frame_pending: Vec<(ThumbnailTextureKey, ThumbnailData)>,
}

#[derive(Debug, Default)]
pub(in crate::app) struct CacheLookupBatchUpdate {
    pub(in crate::app) immediate: Vec<(ThumbnailTextureKey, ThumbnailData)>,
    pub(in crate::app) schedule_frame_flush: bool,
}

impl LibraryThumbnailCacheHitBatch {
    pub(in crate::app) fn begin_generation(&mut self, generation: u64) {
        self.generation = generation;
        self.cohort_id = 0;
        self.cohort_order.clear();
        self.cohort_remaining.clear();
        self.cohort_pending.clear();
        self.awaiting_all_hits = false;
        self.flush_scheduled = false;
        self.frame_pending.clear();
    }

    /// Starts the cache lookup group for the latest visible demand. Pending hits
    /// from the replaced cohort are released through the normal frame batch.
    pub(in crate::app) fn begin_cohort(&mut self, cohort: CacheLookupCohort) -> bool {
        if cohort.generation != self.generation {
            return false;
        }
        self.release_cohort_pending_to_frame();
        self.cohort_id = cohort.id;
        self.cohort_order = cohort.sources;
        self.cohort_remaining = self.cohort_order.iter().cloned().collect();
        self.awaiting_all_hits = !self.cohort_remaining.is_empty();
        self.reserve_frame_flush()
    }

    pub(in crate::app) fn complete_lookup(
        &mut self,
        generation: u64,
        cohort_id: u64,
        key: impl Into<ThumbnailTextureKey>,
        thumbnail: Option<ThumbnailData>,
    ) -> CacheLookupBatchUpdate {
        let key = key.into();
        if generation != self.generation {
            return CacheLookupBatchUpdate::default();
        }
        if cohort_id != self.cohort_id
            || !self.awaiting_all_hits
            || !self.cohort_remaining.remove(&key)
        {
            if let Some(thumbnail) = thumbnail {
                self.frame_pending.push((key, thumbnail));
            }
            return CacheLookupBatchUpdate {
                immediate: Vec::new(),
                schedule_frame_flush: self.reserve_frame_flush(),
            };
        }

        if let Some(thumbnail) = thumbnail {
            self.cohort_pending.insert(key, thumbnail);
            if !self.cohort_remaining.is_empty() {
                return CacheLookupBatchUpdate::default();
            }
        } else {
            self.cohort_remaining.clear();
        }

        self.awaiting_all_hits = false;
        CacheLookupBatchUpdate {
            immediate: self.take_cohort_pending(),
            schedule_frame_flush: false,
        }
    }

    fn take_cohort_pending(&mut self) -> Vec<(ThumbnailTextureKey, ThumbnailData)> {
        let mut pending = std::mem::take(&mut self.cohort_pending);
        self.cohort_order
            .iter()
            .filter_map(|key| {
                pending
                    .remove(key)
                    .map(|thumbnail| (key.clone(), thumbnail))
            })
            .collect()
    }

    fn release_cohort_pending_to_frame(&mut self) {
        let pending = self.take_cohort_pending();
        self.frame_pending.extend(pending);
        self.cohort_order.clear();
        self.cohort_remaining.clear();
        self.awaiting_all_hits = false;
    }

    fn reserve_frame_flush(&mut self) -> bool {
        if self.frame_pending.is_empty() || self.flush_scheduled {
            return false;
        }
        self.flush_scheduled = true;
        true
    }

    pub(in crate::app) fn take_for_flush(
        &mut self,
        generation: u64,
    ) -> Vec<(ThumbnailTextureKey, ThumbnailData)> {
        if generation != self.generation || !self.flush_scheduled {
            return Vec::new();
        }
        self.flush_scheduled = false;
        std::mem::take(&mut self.frame_pending)
    }

    /// Takes every cache hit currently held for either the all-hit cohort or the
    /// next frame. This is used immediately before the initially hidden library
    /// is revealed; lookup tracking remains active after a reveal timeout.
    pub(in crate::app) fn take_available_for_initial_reveal(
        &mut self,
        generation: u64,
    ) -> Vec<(ThumbnailTextureKey, ThumbnailData)> {
        if generation != self.generation {
            return Vec::new();
        }
        let mut available = self.take_cohort_pending();
        available.append(&mut self.frame_pending);
        self.flush_scheduled = false;
        available
    }
}
