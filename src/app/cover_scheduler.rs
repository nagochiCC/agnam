use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::cover_cancel::CoverCancel;

const CACHE_WORKER_LIMIT: usize = 8;
const GENERATION_WORKER_LIMIT: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ActiveJob<K> {
    generation: u64,
    key: K,
}

#[derive(Debug)]
pub(super) enum CoverSchedulerJob<K, P> {
    LoadCache {
        generation: u64,
        key: K,
    },
    Generate {
        generation: u64,
        key: K,
        payload: P,
        cancel: CoverCancel,
    },
}

/// GTK-independent visible-demand scheduler shared by History and Favorites.
pub(super) struct CoverLoadScheduler<K, P> {
    generation: u64,
    cancellation_generation: Arc<AtomicU64>,
    paused: bool,
    demand: Vec<K>,
    pending_cache: VecDeque<K>,
    generation_payloads: HashMap<K, P>,
    pending_generation: VecDeque<K>,
    cache_in_flight: HashSet<ActiveJob<K>>,
    generation_in_flight: HashSet<ActiveJob<K>>,
    resolved: HashSet<K>,
}

impl<K, P> Default for CoverLoadScheduler<K, P>
where
    K: Eq + Hash,
{
    fn default() -> Self {
        Self {
            generation: 0,
            cancellation_generation: Arc::new(AtomicU64::new(0)),
            paused: false,
            demand: Vec::new(),
            pending_cache: VecDeque::new(),
            generation_payloads: HashMap::new(),
            pending_generation: VecDeque::new(),
            cache_in_flight: HashSet::new(),
            generation_in_flight: HashSet::new(),
            resolved: HashSet::new(),
        }
    }
}

impl<K, P> CoverLoadScheduler<K, P>
where
    K: Clone + Eq + Hash,
    P: Clone,
{
    #[cfg(test)]
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    #[cfg(test)]
    pub(super) fn pending_cache_len(&self) -> usize {
        self.pending_cache.len()
    }

    pub(super) fn replace_demand(&mut self, demand: impl IntoIterator<Item = K>) {
        let mut seen = HashSet::new();
        self.demand = demand
            .into_iter()
            .filter(|key| seen.insert(key.clone()))
            .collect();
        let demanded = self.demand.iter().cloned().collect::<HashSet<_>>();
        self.resolved.retain(|key| demanded.contains(key));
        self.generation_payloads
            .retain(|key, _| demanded.contains(key));
        self.rebuild_pending();
    }

    pub(super) fn enqueue_generation(&mut self, key: K, payload: P) {
        if self.demand.contains(&key) && !self.resolved.contains(&key) {
            self.generation_payloads.insert(key, payload);
            self.rebuild_pending();
        }
    }

    pub(super) fn mark_resolved(&mut self, key: &K) {
        if self.demand.contains(key) {
            self.resolved.insert(key.clone());
            self.generation_payloads.remove(key);
            self.rebuild_pending();
        }
    }

    pub(super) fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.cancellation_generation
            .store(self.generation, Ordering::Release);
        self.demand.clear();
        self.pending_cache.clear();
        self.generation_payloads.clear();
        self.pending_generation.clear();
        self.resolved.clear();
    }

    pub(super) fn pause(&mut self) {
        self.paused = true;
    }

    pub(super) fn resume(&mut self) -> Vec<CoverSchedulerJob<K, P>> {
        self.paused = false;
        self.schedule_jobs()
    }

    pub(super) fn cache_finished(&mut self, generation: u64, key: &K) -> bool {
        self.cache_in_flight.remove(&ActiveJob {
            generation,
            key: key.clone(),
        });
        self.rebuild_pending();
        generation == self.generation && self.demand.contains(key)
    }

    pub(super) fn generation_finished(&mut self, generation: u64, key: &K) -> bool {
        self.generation_in_flight.remove(&ActiveJob {
            generation,
            key: key.clone(),
        });
        self.rebuild_pending();
        generation == self.generation && self.demand.contains(key)
    }

    pub(super) fn schedule_jobs(&mut self) -> Vec<CoverSchedulerJob<K, P>> {
        if self.paused {
            return Vec::new();
        }
        let mut jobs = Vec::new();
        while self.cache_in_flight.len() < CACHE_WORKER_LIMIT {
            let Some(key) = self.pending_cache.pop_front() else {
                break;
            };
            let active = ActiveJob {
                generation: self.generation,
                key: key.clone(),
            };
            if self.source_is_in_flight(&key) || !self.cache_in_flight.insert(active) {
                continue;
            }
            jobs.push(CoverSchedulerJob::LoadCache {
                generation: self.generation,
                key,
            });
        }
        while self.generation_in_flight.len() < GENERATION_WORKER_LIMIT {
            let Some(key) = self.pending_generation.pop_front() else {
                break;
            };
            let Some(payload) = self.generation_payloads.get(&key).cloned() else {
                continue;
            };
            let active = ActiveJob {
                generation: self.generation,
                key: key.clone(),
            };
            if self.source_is_in_flight(&key) || !self.generation_in_flight.insert(active) {
                continue;
            }
            jobs.push(CoverSchedulerJob::Generate {
                generation: self.generation,
                key,
                payload,
                cancel: CoverCancel::new(self.cancellation_generation.clone(), self.generation),
            });
        }
        jobs
    }

    fn rebuild_pending(&mut self) {
        self.pending_cache = self
            .demand
            .iter()
            .filter(|key| !self.resolved.contains(*key))
            .filter(|key| !self.generation_payloads.contains_key(*key))
            .filter(|key| !self.source_is_in_flight(key))
            .cloned()
            .collect();
        self.pending_generation = self
            .demand
            .iter()
            .filter(|key| !self.resolved.contains(*key))
            .filter(|key| self.generation_payloads.contains_key(*key))
            .filter(|key| !self.source_is_in_flight(key))
            .cloned()
            .collect();
    }

    fn source_is_in_flight(&self, key: &K) -> bool {
        self.cache_in_flight
            .iter()
            .chain(&self.generation_in_flight)
            .any(|job| &job.key == key)
    }
}

#[cfg(test)]
mod tests {
    use super::{CoverLoadScheduler, CoverSchedulerJob};

    #[test]
    fn invalidation_cancels_old_generation_but_waits_for_physical_completion() {
        let mut scheduler = CoverLoadScheduler::<u32, u32>::default();
        scheduler.replace_demand([1]);
        scheduler.enqueue_generation(1, 1);
        let old = scheduler.generation();
        let jobs = scheduler.schedule_jobs();
        let CoverSchedulerJob::Generate { cancel, .. } = &jobs[0] else {
            unreachable!()
        };
        scheduler.invalidate();
        assert!(cancel.cancelled());
        scheduler.replace_demand([2]);
        scheduler.enqueue_generation(2, 2);
        assert_eq!(scheduler.schedule_jobs().len(), 1); // second physical slot
        assert_eq!(scheduler.generation_in_flight.len(), 2);
        assert!(!scheduler.generation_finished(old, &1));
    }

    #[test]
    fn cache_and_generation_worker_limits_are_preserved() {
        let mut cache = CoverLoadScheduler::<u32, u32>::default();
        cache.replace_demand(0..10);
        assert_eq!(cache.schedule_jobs().len(), 8);

        let mut generation = CoverLoadScheduler::<u32, u32>::default();
        generation.replace_demand(0..3);
        for key in 0..3 {
            generation.enqueue_generation(key, key + 10);
        }
        assert_eq!(generation.schedule_jobs().len(), 2);
    }

    #[test]
    fn demand_replacement_discards_unstarted_sources() {
        let mut scheduler = CoverLoadScheduler::<u32, u32>::default();
        scheduler.replace_demand(0..10);
        let running = scheduler.schedule_jobs();
        scheduler.replace_demand([99]);
        let first = match &running[0] {
            CoverSchedulerJob::LoadCache { generation, key } => (*generation, *key),
            _ => unreachable!(),
        };
        assert!(scheduler.pending_cache_len() <= 1);
        assert!(!scheduler.cache_finished(first.0, &first.1));
        assert!(
            scheduler
                .schedule_jobs()
                .iter()
                .all(|job| matches!(job, CoverSchedulerJob::LoadCache { key: 99, .. }))
        );
    }

    #[test]
    fn pause_stops_new_jobs_and_resume_uses_current_demand_only() {
        let mut scheduler = CoverLoadScheduler::<u32, u32>::default();
        scheduler.replace_demand([1, 2]);
        scheduler.pause();
        scheduler.replace_demand([3]);
        assert!(scheduler.schedule_jobs().is_empty());
        assert!(matches!(
            scheduler.resume().as_slice(),
            [CoverSchedulerJob::LoadCache { key: 3, .. }]
        ));
    }

    #[test]
    fn stale_completion_releases_lane_without_being_accepted() {
        let mut scheduler = CoverLoadScheduler::<u32, u32>::default();
        scheduler.replace_demand([1]);
        let old_generation = scheduler.generation();
        assert_eq!(scheduler.schedule_jobs().len(), 1);
        scheduler.invalidate();
        scheduler.replace_demand([2]);

        assert!(!scheduler.cache_finished(old_generation, &1));
        assert!(matches!(
            scheduler.schedule_jobs().as_slice(),
            [CoverSchedulerJob::LoadCache { key: 2, .. }]
        ));
    }

    #[test]
    fn stale_same_source_completion_unblocks_current_generation() {
        let mut scheduler = CoverLoadScheduler::<u32, u32>::default();
        scheduler.replace_demand([1]);
        let old_generation = scheduler.generation();
        scheduler.schedule_jobs();
        scheduler.invalidate();
        scheduler.replace_demand([1]);
        assert!(scheduler.schedule_jobs().is_empty());

        assert!(!scheduler.cache_finished(old_generation, &1));
        assert!(matches!(
            scheduler.schedule_jobs().as_slice(),
            [CoverSchedulerJob::LoadCache { key: 1, .. }]
        ));
    }

    #[test]
    fn stale_generation_result_is_rejected_and_unblocks_current_source() {
        let mut scheduler = CoverLoadScheduler::<u32, u32>::default();
        scheduler.replace_demand([1]);
        scheduler.enqueue_generation(1, 10);
        let old_generation = scheduler.generation();
        assert!(matches!(
            scheduler.schedule_jobs().as_slice(),
            [CoverSchedulerJob::Generate { key: 1, .. }]
        ));
        scheduler.invalidate();
        scheduler.replace_demand([1]);
        scheduler.enqueue_generation(1, 11);
        assert!(scheduler.schedule_jobs().is_empty());

        assert!(!scheduler.generation_finished(old_generation, &1));
        assert!(matches!(
            scheduler.schedule_jobs().as_slice(),
            [CoverSchedulerJob::Generate {
                key: 1,
                payload: 11,
                ..
            }]
        ));
    }

    #[test]
    fn completion_for_an_unbound_source_is_rejected() {
        let mut scheduler = CoverLoadScheduler::<u32, u32>::default();
        scheduler.replace_demand([1]);
        let generation = scheduler.generation();
        scheduler.schedule_jobs();
        scheduler.replace_demand([2]);

        assert!(!scheduler.cache_finished(generation, &1));
    }

    #[test]
    fn resolved_source_is_requested_again_after_unbind_and_rebind() {
        let mut scheduler = CoverLoadScheduler::<u32, u32>::default();
        scheduler.replace_demand([1]);
        let generation = scheduler.generation();
        scheduler.schedule_jobs();
        assert!(scheduler.cache_finished(generation, &1));
        scheduler.mark_resolved(&1);
        assert!(scheduler.schedule_jobs().is_empty());

        scheduler.replace_demand([]);
        scheduler.replace_demand([1]);
        assert!(matches!(
            scheduler.schedule_jobs().as_slice(),
            [CoverSchedulerJob::LoadCache { key: 1, .. }]
        ));
    }
}
