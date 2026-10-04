use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::cover_cancel::CoverCancel;

const CACHE_LOAD_CONCURRENCY_LIMIT: usize = 8;
const GENERATION_CONCURRENCY_LIMIT: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SearchThumbnailDemandEvaluation {
    Ready(Vec<PathBuf>),
    AllocationPending,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct SearchThumbnailJob {
    pub(super) generation: u64,
    pub(super) source: PathBuf,
}

#[derive(Debug, Default)]
pub(super) struct SearchThumbnailJobs {
    pub(super) cache_loads: Vec<SearchThumbnailJob>,
    pub(super) generations: Vec<SearchThumbnailJob>,
}

#[derive(Debug, Default)]
pub(super) struct SearchThumbnailCompletion {
    pub(super) accepted: bool,
    pub(super) jobs: SearchThumbnailJobs,
}

/// GTK-independent thumbnail scheduling state for one search session.
///
/// Each result render starts a new generation while already running workers remain
/// tracked until completion, even when cancelled. Pending work follows only the latest visible cards,
/// and the two worker lanes are bounded independently. A source running for a
/// stale generation is never started again concurrently; once the stale worker
/// completes, current visible demand may schedule it for the latest generation.
#[derive(Debug)]
pub(super) struct LibrarySearchThumbnailController {
    generation: u64,
    cancellation_generation: Arc<AtomicU64>,
    session_active: bool,
    cache_load_concurrency_limit: usize,
    generation_concurrency_limit: usize,
    demand: Vec<PathBuf>,
    cache_pending: VecDeque<PathBuf>,
    generation_pending: VecDeque<PathBuf>,
    cache_in_flight: HashSet<SearchThumbnailJob>,
    generation_in_flight: HashSet<SearchThumbnailJob>,
    cache_misses: HashSet<PathBuf>,
    ready: HashSet<PathBuf>,
    failed: HashSet<PathBuf>,
}

impl Default for LibrarySearchThumbnailController {
    fn default() -> Self {
        Self::new(CACHE_LOAD_CONCURRENCY_LIMIT, GENERATION_CONCURRENCY_LIMIT)
    }
}

impl LibrarySearchThumbnailController {
    fn new(cache_load_concurrency_limit: usize, generation_concurrency_limit: usize) -> Self {
        Self {
            generation: 0,
            cancellation_generation: Arc::new(AtomicU64::new(0)),
            session_active: false,
            cache_load_concurrency_limit: cache_load_concurrency_limit.max(1),
            generation_concurrency_limit: generation_concurrency_limit.max(1),
            demand: Vec::new(),
            cache_pending: VecDeque::new(),
            generation_pending: VecDeque::new(),
            cache_in_flight: HashSet::new(),
            generation_in_flight: HashSet::new(),
            cache_misses: HashSet::new(),
            ready: HashSet::new(),
            failed: HashSet::new(),
        }
    }

    pub(super) fn begin_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.cancellation_generation
            .store(self.generation, Ordering::Release);
        self.session_active = true;
        self.demand.clear();
        self.cache_pending.clear();
        self.generation_pending.clear();
        self.generation
    }

    pub(super) fn reset_session(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.cancellation_generation
            .store(self.generation, Ordering::Release);
        self.session_active = false;
        self.demand.clear();
        self.cache_pending.clear();
        self.generation_pending.clear();
        self.cache_misses.clear();
        self.ready.clear();
        self.failed.clear();
        self.generation
    }

    pub(super) fn cancellation_token(&self) -> CoverCancel {
        CoverCancel::new(self.cancellation_generation.clone(), self.generation)
    }

    pub(super) fn update_demand(
        &mut self,
        evaluation: SearchThumbnailDemandEvaluation,
    ) -> SearchThumbnailJobs {
        let SearchThumbnailDemandEvaluation::Ready(demand) = evaluation else {
            return SearchThumbnailJobs::default();
        };
        if !self.session_active {
            return SearchThumbnailJobs::default();
        }
        let mut seen = HashSet::new();
        let demand = demand
            .into_iter()
            .filter(|source| seen.insert(source.clone()))
            .collect::<Vec<_>>();
        self.forget_ready_for_missing_textures(&demand);
        self.demand = demand;
        self.rebuild_pending();
        self.take_jobs()
    }

    fn forget_ready_for_missing_textures(&mut self, missing: &[PathBuf]) {
        for source in missing {
            if self.ready.remove(source) {
                // A generated thumbnail is persisted before it is published. Once
                // its view texture is evicted, always retry the disk cache lane.
                self.cache_misses.remove(source);
            }
        }
    }

    pub(super) fn accepts_cache_completion(&self, generation: u64, source: &Path) -> bool {
        generation == self.generation
            && self.session_active
            && self.cache_in_flight.contains(&SearchThumbnailJob {
                generation,
                source: source.to_path_buf(),
            })
    }

    pub(super) fn accepts_generation_completion(&self, generation: u64, source: &Path) -> bool {
        generation == self.generation
            && self.session_active
            && self.generation_in_flight.contains(&SearchThumbnailJob {
                generation,
                source: source.to_path_buf(),
            })
    }

    pub(super) fn complete_cache_load(
        &mut self,
        generation: u64,
        source: &Path,
        succeeded: bool,
    ) -> SearchThumbnailCompletion {
        let job = SearchThumbnailJob {
            generation,
            source: source.to_path_buf(),
        };
        let was_in_flight = self.cache_in_flight.remove(&job);
        let accepted = was_in_flight && generation == self.generation && self.session_active;
        if accepted {
            if succeeded {
                self.ready.insert(source.to_path_buf());
            } else {
                self.cache_misses.insert(source.to_path_buf());
            }
        }
        self.rebuild_pending();
        SearchThumbnailCompletion {
            accepted,
            jobs: self.take_jobs(),
        }
    }

    pub(super) fn complete_generation(
        &mut self,
        generation: u64,
        source: &Path,
        succeeded: bool,
    ) -> SearchThumbnailCompletion {
        let job = SearchThumbnailJob {
            generation,
            source: source.to_path_buf(),
        };
        let was_in_flight = self.generation_in_flight.remove(&job);
        let accepted = was_in_flight && generation == self.generation && self.session_active;
        if accepted {
            if succeeded {
                self.ready.insert(source.to_path_buf());
            } else {
                self.failed.insert(source.to_path_buf());
            }
        }
        self.rebuild_pending();
        SearchThumbnailCompletion {
            accepted,
            jobs: self.take_jobs(),
        }
    }

    pub(super) fn cancel_generation(
        &mut self,
        generation: u64,
        source: &Path,
    ) -> SearchThumbnailJobs {
        self.generation_in_flight.remove(&SearchThumbnailJob {
            generation,
            source: source.to_path_buf(),
        });
        self.rebuild_pending();
        self.take_jobs()
    }

    fn rebuild_pending(&mut self) {
        if !self.session_active {
            self.cache_pending.clear();
            self.generation_pending.clear();
            return;
        }
        self.cache_pending = self
            .demand
            .iter()
            .filter(|source| !self.ready.contains(*source) && !self.failed.contains(*source))
            .filter(|source| !self.cache_misses.contains(*source))
            .filter(|source| !self.source_is_in_flight(source))
            .cloned()
            .collect();
        self.generation_pending = self
            .demand
            .iter()
            .filter(|source| !self.ready.contains(*source) && !self.failed.contains(*source))
            .filter(|source| self.cache_misses.contains(*source))
            .filter(|source| !self.source_is_in_flight(source))
            .cloned()
            .collect();
    }

    fn source_is_in_flight(&self, source: &Path) -> bool {
        self.cache_in_flight
            .iter()
            .chain(&self.generation_in_flight)
            .any(|job| job.source == source)
    }

    fn take_jobs(&mut self) -> SearchThumbnailJobs {
        let mut jobs = SearchThumbnailJobs::default();
        while self.cache_in_flight.len() < self.cache_load_concurrency_limit {
            let Some(source) = self.cache_pending.pop_front() else {
                break;
            };
            let job = SearchThumbnailJob {
                generation: self.generation,
                source,
            };
            if !self.source_is_in_flight(&job.source) && self.cache_in_flight.insert(job.clone()) {
                jobs.cache_loads.push(job);
            }
        }
        while self.generation_in_flight.len() < self.generation_concurrency_limit {
            let Some(source) = self.generation_pending.pop_front() else {
                break;
            };
            let job = SearchThumbnailJob {
                generation: self.generation,
                source,
            };
            if !self.source_is_in_flight(&job.source)
                && self.generation_in_flight.insert(job.clone())
            {
                jobs.generations.push(job);
            }
        }
        jobs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_old_worker_keeps_physical_slot_until_completion_and_retries_same_source() {
        let mut controller = LibrarySearchThumbnailController::new(1, 1);
        let old = controller.begin_generation();
        let old_token = controller.cancellation_token();
        let cache = controller.update_demand(ready(&["same"]));
        let generation = controller.complete_cache_load(old, &cache.cache_loads[0].source, false);
        assert_eq!(generation.jobs.generations.len(), 1);
        let current = controller.begin_generation();
        assert!(old_token.cancelled());
        assert!(!controller.cancellation_token().cancelled());
        assert!(
            controller
                .update_demand(ready(&["same"]))
                .generations
                .is_empty()
        );
        let released = controller.complete_generation(old, Path::new("same"), false);
        assert!(!released.accepted);
        assert!(controller.failed.is_empty());
        assert_eq!(released.jobs.generations.len(), 1);
        assert_eq!(released.jobs.generations[0].generation, current);
        let retry = controller.cancel_generation(current, Path::new("same"));
        assert_eq!(retry.generations.len(), 1);
        assert!(controller.failed.is_empty());
    }

    #[test]
    fn repeated_generation_changes_never_replace_active_physical_workers() {
        let mut controller = LibrarySearchThumbnailController::new(2, 2);
        let old = controller.begin_generation();
        let cache = controller.update_demand(ready(&["old-a", "old-b"]));
        for job in cache.cache_loads {
            controller.complete_cache_load(old, &job.source, false);
        }
        assert_eq!(controller.generation_in_flight.len(), 2);
        for number in 0..32 {
            let generation = controller.begin_generation();
            let path = format!("current-{number}");
            let cache = controller.update_demand(ready(&[&path]));
            assert_eq!(cache.cache_loads.len(), 1);
            let waiting = controller.complete_cache_load(generation, Path::new(&path), false);
            assert!(waiting.jobs.generations.is_empty());
            assert_eq!(controller.generation_in_flight.len(), 2);
        }
        let released = controller.complete_generation(old, Path::new("old-a"), false);
        assert!(!released.accepted);
        assert_eq!(released.jobs.generations.len(), 1);
        assert_eq!(
            released.jobs.generations[0].source,
            PathBuf::from("current-31")
        );
        assert_eq!(controller.generation_in_flight.len(), 2);
    }

    fn ready(paths: &[&str]) -> SearchThumbnailDemandEvaluation {
        SearchThumbnailDemandEvaluation::Ready(paths.iter().map(PathBuf::from).collect())
    }

    fn sources(jobs: &[SearchThumbnailJob]) -> Vec<&Path> {
        jobs.iter().map(|job| job.source.as_path()).collect()
    }

    #[test]
    fn duplicate_sources_create_one_cache_request() {
        let mut controller = LibrarySearchThumbnailController::new(8, 2);
        controller.begin_generation();

        let jobs = controller.update_demand(ready(&["/a.cbz", "/a.cbz", "/b.cbz"]));

        assert_eq!(
            sources(&jobs.cache_loads),
            [Path::new("/a.cbz"), Path::new("/b.cbz")]
        );
    }

    #[test]
    fn worker_limits_are_enforced() {
        let mut controller = LibrarySearchThumbnailController::new(3, 2);
        let generation = controller.begin_generation();
        let jobs = controller.update_demand(ready(&["/0", "/1", "/2", "/3", "/4"]));
        assert_eq!(jobs.cache_loads.len(), 3);

        let mut generation_jobs = Vec::new();
        for job in jobs.cache_loads {
            let completion = controller.complete_cache_load(generation, &job.source, false);
            generation_jobs.extend(completion.jobs.generations);
        }
        assert_eq!(generation_jobs.len(), 2);
    }

    #[test]
    fn stale_completion_is_ignored_and_releases_its_worker_slot() {
        let mut controller = LibrarySearchThumbnailController::new(1, 1);
        let stale_generation = controller.begin_generation();
        let stale_job = controller
            .update_demand(ready(&["/same.cbz"]))
            .cache_loads
            .pop()
            .unwrap();

        let current_generation = controller.begin_generation();
        let jobs = controller.update_demand(ready(&["/same.cbz", "/next.cbz"]));
        assert!(jobs.cache_loads.is_empty());

        let completion = controller.complete_cache_load(stale_generation, &stale_job.source, true);
        assert!(!completion.accepted);
        assert_eq!(completion.jobs.cache_loads.len(), 1);
        assert_eq!(
            completion.jobs.cache_loads[0].generation,
            current_generation
        );
        assert_eq!(
            completion.jobs.cache_loads[0].source,
            Path::new("/same.cbz")
        );
    }

    #[test]
    fn generation_change_rejects_old_target_completion() {
        let mut controller = LibrarySearchThumbnailController::new(1, 1);
        let old_generation = controller.begin_generation();
        let job = controller
            .update_demand(ready(&["/old.cbz"]))
            .cache_loads
            .pop()
            .unwrap();
        controller.begin_generation();

        assert!(!controller.accepts_cache_completion(old_generation, &job.source));
        assert!(
            !controller
                .complete_cache_load(old_generation, &job.source, true)
                .accepted
        );
    }

    #[test]
    fn resetting_session_discards_pending_work() {
        let mut controller = LibrarySearchThumbnailController::new(1, 1);
        let generation = controller.begin_generation();
        let first = controller
            .update_demand(ready(&["/first", "/pending"]))
            .cache_loads
            .pop()
            .unwrap();

        controller.reset_session();
        let completion = controller.complete_cache_load(generation, &first.source, false);

        assert!(!completion.accepted);
        assert!(completion.jobs.cache_loads.is_empty());
        assert!(completion.jobs.generations.is_empty());
    }

    #[test]
    fn missing_view_texture_forgets_ready_and_requests_cache_again() {
        let mut controller = LibrarySearchThumbnailController::new(1, 1);
        let generation = controller.begin_generation();
        let job = controller
            .update_demand(ready(&["/ready.cbz"]))
            .cache_loads
            .pop()
            .unwrap();
        assert!(
            controller
                .complete_cache_load(generation, &job.source, true)
                .accepted
        );

        controller.begin_generation();
        let jobs = controller.update_demand(ready(&["/ready.cbz"]));
        assert_eq!(sources(&jobs.cache_loads), [Path::new("/ready.cbz")]);
        assert!(jobs.generations.is_empty());
    }

    #[test]
    fn allocation_pending_keeps_the_last_visible_queue() {
        let mut controller = LibrarySearchThumbnailController::new(1, 1);
        let generation = controller.begin_generation();
        let first = controller
            .update_demand(ready(&["/first", "/second"]))
            .cache_loads
            .pop()
            .unwrap();

        controller.update_demand(SearchThumbnailDemandEvaluation::AllocationPending);
        let completion = controller.complete_cache_load(generation, &first.source, true);

        assert_eq!(
            sources(&completion.jobs.cache_loads),
            [Path::new("/second")]
        );
    }

    #[test]
    fn cache_miss_moves_source_to_generation_without_duplicates() {
        let mut controller = LibrarySearchThumbnailController::new(8, 2);
        let generation = controller.begin_generation();
        let job = controller
            .update_demand(ready(&["/folder/book.cbz", "/folder/book.cbz"]))
            .cache_loads
            .pop()
            .unwrap();

        let completion = controller.complete_cache_load(generation, &job.source, false);

        assert_eq!(
            sources(&completion.jobs.generations),
            [job.source.as_path()]
        );
        assert!(completion.jobs.cache_loads.is_empty());
    }

    #[test]
    fn hidden_search_does_not_start_generation_after_an_in_flight_cache_miss() {
        let mut controller = LibrarySearchThumbnailController::new(1, 1);
        let generation = controller.begin_generation();
        let job = controller
            .update_demand(ready(&["/visible.cbz"]))
            .cache_loads
            .pop()
            .unwrap();

        controller.update_demand(ready(&[]));
        let completion = controller.complete_cache_load(generation, &job.source, false);

        assert!(completion.accepted);
        assert!(completion.jobs.generations.is_empty());
    }

    #[test]
    fn evicted_generated_texture_reenters_through_the_disk_cache_lane() {
        let mut controller = LibrarySearchThumbnailController::new(1, 1);
        let generation = controller.begin_generation();
        let job = controller
            .update_demand(ready(&["/visible.cbz"]))
            .cache_loads
            .pop()
            .unwrap();
        let generation_job = controller
            .complete_cache_load(generation, &job.source, false)
            .jobs
            .generations
            .pop()
            .unwrap();
        assert!(
            controller
                .complete_generation(generation, &generation_job.source, true)
                .accepted
        );

        controller.update_demand(ready(&[]));
        let resumed = controller.update_demand(ready(&["/visible.cbz"]));

        assert_eq!(sources(&resumed.cache_loads), [Path::new("/visible.cbz")]);
        assert!(resumed.generations.is_empty());
    }

    #[test]
    fn stale_generation_worker_releases_the_generation_lane() {
        let mut controller = LibrarySearchThumbnailController::new(1, 1);
        let stale_generation = controller.begin_generation();
        let cache_job = controller
            .update_demand(ready(&["/same.cbz"]))
            .cache_loads
            .pop()
            .unwrap();
        let stale_job = controller
            .complete_cache_load(stale_generation, &cache_job.source, false)
            .jobs
            .generations
            .pop()
            .unwrap();

        let current_generation = controller.begin_generation();
        assert!(
            controller
                .update_demand(ready(&["/same.cbz"]))
                .generations
                .is_empty()
        );
        let completion = controller.complete_generation(stale_generation, &stale_job.source, true);

        assert!(!completion.accepted);
        assert_eq!(completion.jobs.generations.len(), 1);
        assert_eq!(
            completion.jobs.generations[0].generation,
            current_generation
        );
        assert_eq!(completion.jobs.generations[0].source, stale_job.source);
    }
}
