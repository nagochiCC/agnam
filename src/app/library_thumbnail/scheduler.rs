use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::super::cover_cancel::CoverCancel;

const LIBRARY_THUMBNAIL_CACHE_LOAD_CONCURRENCY: usize = 8;
const LIBRARY_THUMBNAIL_GENERATION_CONCURRENCY: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ThumbnailSourceKind {
    DirectImage,
    BookCover,
    ArchiveEntry,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::app) struct ThumbnailTextureKey {
    pub(in crate::app) source: PathBuf,
    pub(in crate::app) kind: ThumbnailSourceKind,
}

impl ThumbnailTextureKey {
    pub(in crate::app) fn new(source: PathBuf, kind: ThumbnailSourceKind) -> Self {
        Self { source, kind }
    }
}

impl From<PathBuf> for ThumbnailTextureKey {
    fn from(source: PathBuf) -> Self {
        Self::new(source, ThumbnailSourceKind::BookCover)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::app) struct ThumbnailDemand {
    pub(in crate::app) source: PathBuf,
    pub(in crate::app) kind: ThumbnailSourceKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) enum ThumbnailDemandEvaluation {
    Ready(Vec<ThumbnailDemand>),
    AllocationPending,
}

impl ThumbnailDemand {
    pub(in crate::app) fn new(source: PathBuf, kind: ThumbnailSourceKind) -> Self {
        Self { source, kind }
    }

    pub(in crate::app) fn key(&self) -> ThumbnailTextureKey {
        ThumbnailTextureKey::new(self.source.clone(), self.kind)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::app) struct ThumbnailJob {
    pub(in crate::app) generation: u64,
    pub(in crate::app) source: PathBuf,
    pub(in crate::app) kind: ThumbnailSourceKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) struct CacheLoadJob {
    pub(in crate::app) generation: u64,
    pub(in crate::app) cohort_id: u64,
    pub(in crate::app) source: PathBuf,
    pub(in crate::app) kind: ThumbnailSourceKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) struct CacheLookupCohort {
    pub(in crate::app) generation: u64,
    pub(in crate::app) id: u64,
    pub(in crate::app) sources: Vec<ThumbnailTextureKey>,
}

#[derive(Debug, Default)]
pub(in crate::app) struct Completion {
    pub(in crate::app) accepted: bool,
    pub(in crate::app) jobs: ScheduledJobs,
}

#[derive(Debug, Default)]
pub(in crate::app) struct ScheduledJobs {
    pub(in crate::app) cache_cohort: Option<CacheLookupCohort>,
    pub(in crate::app) cache_loads: Vec<CacheLoadJob>,
    pub(in crate::app) generations: Vec<ThumbnailJob>,
}

/// GTK-independent scheduling state for the currently displayed directory.
///
/// Cache loads and source generations use separate bounded lanes. Pending work
/// in both lanes is replaced whenever visible demand changes. In-flight work is
/// tracked until completion, including work cancelled from an older generation,
/// so the bounded slot still represents a physical worker. An allocation-pending evaluation leaves
/// the last confirmed pending queues intact.
#[derive(Debug)]
pub(in crate::app) struct LibraryThumbnailController {
    generation: u64,
    cancellation_generation: Arc<AtomicU64>,
    cache_load_concurrency_limit: usize,
    generation_concurrency_limit: usize,
    cohort_id: u64,
    demand: Vec<ThumbnailDemand>,
    cache_pending: VecDeque<ThumbnailDemand>,
    cache_in_flight: HashMap<ThumbnailJob, u64>,
    cache_misses: HashSet<ThumbnailTextureKey>,
    cache_entries_available: HashSet<ThumbnailTextureKey>,
    generation_pending: VecDeque<ThumbnailDemand>,
    generation_in_flight: HashSet<ThumbnailJob>,
    generation_enabled: bool,
    ready: HashSet<ThumbnailTextureKey>,
    failed: HashSet<ThumbnailTextureKey>,
}

impl Default for LibraryThumbnailController {
    fn default() -> Self {
        Self::new(
            LIBRARY_THUMBNAIL_CACHE_LOAD_CONCURRENCY,
            LIBRARY_THUMBNAIL_GENERATION_CONCURRENCY,
        )
    }
}

impl LibraryThumbnailController {
    fn new(cache_load_concurrency_limit: usize, generation_concurrency_limit: usize) -> Self {
        Self {
            generation: 0,
            cancellation_generation: Arc::new(AtomicU64::new(0)),
            cache_load_concurrency_limit: cache_load_concurrency_limit.max(1),
            generation_concurrency_limit: generation_concurrency_limit.max(1),
            cohort_id: 0,
            demand: Vec::new(),
            cache_pending: VecDeque::new(),
            cache_in_flight: HashMap::new(),
            cache_misses: HashSet::new(),
            cache_entries_available: HashSet::new(),
            generation_pending: VecDeque::new(),
            generation_in_flight: HashSet::new(),
            generation_enabled: true,
            ready: HashSet::new(),
            failed: HashSet::new(),
        }
    }

    pub(in crate::app) fn begin_directory(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.cancellation_generation
            .store(self.generation, Ordering::Release);
        self.cohort_id = 0;
        self.demand.clear();
        self.cache_pending.clear();
        self.cache_misses.clear();
        self.cache_entries_available.clear();
        self.generation_pending.clear();
        self.generation_enabled = true;
        self.ready.clear();
        self.failed.clear();
        self.generation
    }

    pub(in crate::app) fn cancellation_token(&self) -> CoverCancel {
        CoverCancel::new(self.cancellation_generation.clone(), self.generation)
    }

    pub(in crate::app) fn update_demand(
        &mut self,
        evaluation: ThumbnailDemandEvaluation,
    ) -> ScheduledJobs {
        self.update_demand_with_generation(evaluation, true)
    }

    pub(in crate::app) fn update_demand_with_generation(
        &mut self,
        evaluation: ThumbnailDemandEvaluation,
        generation_enabled: bool,
    ) -> ScheduledJobs {
        let ThumbnailDemandEvaluation::Ready(demands) = evaluation else {
            return ScheduledJobs::default();
        };
        self.generation_enabled = generation_enabled;
        let mut seen = HashSet::new();
        let demand = demands
            .into_iter()
            .filter_map(|demand| seen.insert(demand.clone()).then_some(demand))
            .collect::<Vec<_>>();
        let previous_cache_demand = self.cache_lookup_demand(&self.demand);
        let current_cache_demand = self.cache_lookup_demand(&demand);
        let demand_changed = current_cache_demand != previous_cache_demand;
        self.demand = demand;
        if demand_changed {
            self.cohort_id = self.cohort_id.wrapping_add(1);
        }
        self.rebuild_pending();
        let mut jobs = self.take_jobs();
        if demand_changed {
            jobs.cache_cohort = Some(CacheLookupCohort {
                generation: self.generation,
                id: self.cohort_id,
                sources: self.cache_pending_sources_for_current_cohort(&jobs.cache_loads),
            });
        }
        jobs
    }

    /// A source that was previously completed can become demandable again when
    /// its prepared Texture is evicted from the view-level LRU.
    pub(in crate::app) fn forget_ready_for_missing_textures(
        &mut self,
        evaluation: &ThumbnailDemandEvaluation,
    ) {
        let ThumbnailDemandEvaluation::Ready(demands) = evaluation else {
            return;
        };
        let mut forgotten = HashSet::new();
        for demand in demands {
            let key = demand.key();
            if self.ready.remove(&key) {
                forgotten.insert(key);
            }
        }
        if !forgotten.is_empty() {
            self.demand
                .retain(|demand| !forgotten.contains(&demand.key()));
        }
    }

    pub(in crate::app) fn cache_entry_became_available(
        &mut self,
        source: &Path,
        kind: ThumbnailSourceKind,
    ) {
        let key = ThumbnailTextureKey::new(source.to_path_buf(), kind);
        self.cache_entries_available.insert(key.clone());
        self.cache_misses.remove(&key);
        self.failed.remove(&key);
        self.rebuild_pending();
    }

    pub(in crate::app) fn complete_cache_load_with_kind(
        &mut self,
        generation: u64,
        cohort_id: u64,
        source: &Path,
        kind: ThumbnailSourceKind,
        cache_hit: bool,
    ) -> Completion {
        let job = ThumbnailJob {
            generation,
            source: source.to_path_buf(),
            kind,
        };
        let was_in_flight = self.cache_in_flight.get(&job).copied() == Some(cohort_id);
        if was_in_flight {
            self.cache_in_flight.remove(&job);
        }
        let accepted = was_in_flight && generation == self.generation;
        if accepted {
            let key = ThumbnailTextureKey::new(source.to_path_buf(), kind);
            if cache_hit {
                self.cache_entries_available.remove(&key);
                self.ready.insert(key);
            } else if !self.cache_entries_available.remove(&key) {
                self.cache_misses.insert(key);
            }
        }
        self.rebuild_pending();

        Completion {
            accepted,
            jobs: self.take_jobs(),
        }
    }

    pub(in crate::app) fn complete_generation_with_kind(
        &mut self,
        generation: u64,
        source: &Path,
        kind: ThumbnailSourceKind,
        succeeded: bool,
    ) -> Completion {
        let job = ThumbnailJob {
            generation,
            source: source.to_path_buf(),
            kind,
        };
        let was_in_flight = self.generation_in_flight.remove(&job);
        let accepted = was_in_flight && generation == self.generation;
        if accepted {
            if succeeded {
                self.ready
                    .insert(ThumbnailTextureKey::new(source.to_path_buf(), kind));
            } else {
                self.failed
                    .insert(ThumbnailTextureKey::new(source.to_path_buf(), kind));
            }
        }
        self.rebuild_pending();

        Completion {
            accepted,
            jobs: self.take_jobs(),
        }
    }

    pub(in crate::app) fn cancel_generation_with_kind(
        &mut self,
        generation: u64,
        source: &Path,
        kind: ThumbnailSourceKind,
    ) -> ScheduledJobs {
        self.generation_in_flight.remove(&ThumbnailJob {
            generation,
            source: source.to_path_buf(),
            kind,
        });
        self.rebuild_pending();
        self.take_jobs()
    }

    pub(in crate::app) fn accepts_cache_load_completion_with_kind(
        &self,
        generation: u64,
        cohort_id: u64,
        source: &Path,
        kind: ThumbnailSourceKind,
    ) -> bool {
        generation == self.generation
            && self
                .cache_in_flight
                .get(&ThumbnailJob {
                    generation,
                    source: source.to_path_buf(),
                    kind,
                })
                .copied()
                == Some(cohort_id)
    }

    pub(in crate::app) fn accepts_generation_completion_with_kind(
        &self,
        generation: u64,
        source: &Path,
        kind: ThumbnailSourceKind,
    ) -> bool {
        generation == self.generation
            && self.generation_in_flight.contains(&ThumbnailJob {
                generation,
                source: source.to_path_buf(),
                kind,
            })
    }

    #[cfg(test)]
    fn complete_generation(
        &mut self,
        generation: u64,
        source: &Path,
        succeeded: bool,
    ) -> Completion {
        self.complete_generation_with_kind(
            generation,
            source,
            ThumbnailSourceKind::BookCover,
            succeeded,
        )
    }

    fn rebuild_pending(&mut self) {
        self.cache_pending = self
            .demand
            .iter()
            .filter(|demand| {
                !self.ready.contains(&demand.key()) && !self.failed.contains(&demand.key())
            })
            .filter(|demand| !self.cache_misses.contains(&demand.key()))
            .filter(|demand| !self.is_current_cache_load_in_flight(demand))
            .cloned()
            .collect();
        self.generation_pending = self
            .demand
            .iter()
            .filter(|demand| {
                !self.ready.contains(&demand.key()) && !self.failed.contains(&demand.key())
            })
            .filter(|demand| self.cache_misses.contains(&demand.key()))
            .filter(|demand| !self.is_current_generation_in_flight(demand))
            .cloned()
            .collect();
    }

    fn is_current_cache_load_in_flight(&self, demand: &ThumbnailDemand) -> bool {
        self.cache_in_flight.contains_key(&ThumbnailJob {
            generation: self.generation,
            source: demand.source.clone(),
            kind: demand.kind,
        })
    }

    fn cache_pending_sources_for_current_cohort(
        &self,
        started: &[CacheLoadJob],
    ) -> Vec<ThumbnailTextureKey> {
        started
            .iter()
            .map(|job| ThumbnailTextureKey::new(job.source.clone(), job.kind))
            .chain(self.cache_pending.iter().map(ThumbnailDemand::key))
            .collect()
    }

    fn cache_lookup_demand(&self, demand: &[ThumbnailDemand]) -> Vec<ThumbnailDemand> {
        demand
            .iter()
            .filter(|demand| {
                !self.ready.contains(&demand.key()) && !self.failed.contains(&demand.key())
            })
            .filter(|demand| !self.cache_misses.contains(&demand.key()))
            .cloned()
            .collect()
    }

    fn is_current_generation_in_flight(&self, demand: &ThumbnailDemand) -> bool {
        self.generation_in_flight.contains(&ThumbnailJob {
            generation: self.generation,
            source: demand.source.clone(),
            kind: demand.kind,
        })
    }

    fn take_jobs(&mut self) -> ScheduledJobs {
        let mut jobs = ScheduledJobs::default();
        while self.cache_in_flight.len() < self.cache_load_concurrency_limit {
            let Some(demand) = self.cache_pending.pop_front() else {
                break;
            };
            let job = ThumbnailJob {
                generation: self.generation,
                source: demand.source,
                kind: demand.kind,
            };
            if self
                .cache_in_flight
                .insert(job.clone(), self.cohort_id)
                .is_none()
            {
                jobs.cache_loads.push(CacheLoadJob {
                    generation: job.generation,
                    cohort_id: self.cohort_id,
                    source: job.source,
                    kind: job.kind,
                });
            }
        }
        while self.generation_enabled
            && self.generation_in_flight.len() < self.generation_concurrency_limit
        {
            let Some(demand) = self.generation_pending.pop_front() else {
                break;
            };
            let job = ThumbnailJob {
                generation: self.generation,
                source: demand.source,
                kind: demand.kind,
            };
            if self.generation_in_flight.insert(job.clone()) {
                jobs.generations.push(job);
            }
        }
        jobs
    }
}

#[cfg(test)]
mod tests {
    use super::super::batch::LibraryThumbnailBatch;
    use super::super::cache_hit_batch::LibraryThumbnailCacheHitBatch;
    use super::super::initial_reveal::LibraryInitialReveal;
    use super::*;
    use crate::bookshelf::thumbnail::ThumbnailData;

    #[test]
    fn begin_directory_cancels_old_generation_without_releasing_its_slot() {
        let mut controller = LibraryThumbnailController::new(1, 1);
        let old = controller.begin_directory();
        let token = controller.cancellation_token();
        let cache = controller.update_demand(ready([cover("old")]));
        let generation =
            finish_cache_load(&mut controller, old, &cache.cache_loads[0].source, false);
        assert_eq!(generation.jobs.generations.len(), 1);
        let current_generation = controller.begin_directory();
        assert!(token.cancelled());
        let current = controller.update_demand(ready([cover("new")]));
        let waiting = finish_cache_load(
            &mut controller,
            current_generation,
            &current.cache_loads[0].source,
            false,
        );
        assert!(waiting.jobs.generations.is_empty());
        let released = controller.complete_generation(old, Path::new("old"), false);
        assert!(!released.accepted);
        assert!(controller.failed.is_empty());
        assert_eq!(released.jobs.generations.len(), 1);
        let retry = controller.cancel_generation_with_kind(
            current_generation,
            Path::new("new"),
            ThumbnailSourceKind::BookCover,
        );
        assert_eq!(retry.generations.len(), 1);
        assert!(controller.failed.is_empty());
    }

    fn cover(path: &str) -> ThumbnailDemand {
        ThumbnailDemand::new(PathBuf::from(path), ThumbnailSourceKind::BookCover)
    }

    fn archive_entry(path: &str) -> ThumbnailDemand {
        ThumbnailDemand::new(PathBuf::from(path), ThumbnailSourceKind::ArchiveEntry)
    }

    #[test]
    fn cache_only_demand_loads_during_progressive_and_defers_generation() {
        let mut controller = LibraryThumbnailController::new(1, 1);
        let generation = controller.begin_directory();
        let demand = archive_entry("entry");
        let jobs = controller.update_demand_with_generation(
            ThumbnailDemandEvaluation::Ready(vec![demand.clone()]),
            false,
        );
        assert_eq!(jobs.cache_loads.len(), 1);
        assert!(jobs.generations.is_empty());

        let cache = &jobs.cache_loads[0];
        let completion = controller.complete_cache_load_with_kind(
            generation,
            cache.cohort_id,
            Path::new("entry"),
            ThumbnailSourceKind::ArchiveEntry,
            false,
        );
        assert!(completion.accepted);
        assert!(completion.jobs.generations.is_empty());

        let jobs = controller
            .update_demand_with_generation(ThumbnailDemandEvaluation::Ready(vec![demand]), true);
        assert_eq!(jobs.generations.len(), 1);
        assert_eq!(jobs.generations[0].source, PathBuf::from("entry"));
    }

    #[test]
    fn cache_only_demand_does_not_start_duplicate_workers() {
        let mut controller = LibraryThumbnailController::new(1, 1);
        controller.begin_directory();
        let evaluation = || ThumbnailDemandEvaluation::Ready(vec![archive_entry("entry")]);
        assert_eq!(
            controller
                .update_demand_with_generation(evaluation(), false)
                .cache_loads
                .len(),
            1
        );
        let duplicate = controller.update_demand_with_generation(evaluation(), false);
        assert!(duplicate.cache_loads.is_empty());
        assert!(duplicate.generations.is_empty());
    }

    #[test]
    fn progressive_backing_arrival_retries_an_older_cache_miss() {
        let mut controller = LibraryThumbnailController::new(1, 1);
        let generation = controller.begin_directory();
        let jobs = controller.update_demand_with_generation(
            ThumbnailDemandEvaluation::Ready(vec![archive_entry("entry")]),
            false,
        );
        let first = &jobs.cache_loads[0];
        controller
            .cache_entry_became_available(Path::new("entry"), ThumbnailSourceKind::ArchiveEntry);

        let completion = controller.complete_cache_load_with_kind(
            generation,
            first.cohort_id,
            Path::new("entry"),
            ThumbnailSourceKind::ArchiveEntry,
            false,
        );
        assert!(completion.accepted);
        assert_eq!(completion.jobs.cache_loads.len(), 1);
        assert!(completion.jobs.generations.is_empty());

        let retry = &completion.jobs.cache_loads[0];
        let completion = controller.complete_cache_load_with_kind(
            generation,
            retry.cohort_id,
            Path::new("entry"),
            ThumbnailSourceKind::ArchiveEntry,
            true,
        );
        assert!(completion.accepted);
        assert!(completion.jobs.cache_loads.is_empty());
        assert!(completion.jobs.generations.is_empty());
    }

    trait HasSource {
        fn source(&self) -> &Path;
    }

    impl HasSource for ThumbnailJob {
        fn source(&self) -> &Path {
            &self.source
        }
    }

    impl HasSource for CacheLoadJob {
        fn source(&self) -> &Path {
            &self.source
        }
    }

    fn sources<T: HasSource>(jobs: &[T]) -> Vec<&Path> {
        jobs.iter().map(HasSource::source).collect()
    }

    fn finish_cache_load(
        controller: &mut LibraryThumbnailController,
        generation: u64,
        source: &Path,
        cache_hit: bool,
    ) -> Completion {
        let cohort_id = controller.cache_in_flight[&ThumbnailJob {
            generation,
            source: source.to_path_buf(),
            kind: ThumbnailSourceKind::BookCover,
        }];
        controller.complete_cache_load_with_kind(
            generation,
            cohort_id,
            source,
            ThumbnailSourceKind::BookCover,
            cache_hit,
        )
    }

    fn assert_no_jobs(jobs: &ScheduledJobs) {
        assert!(jobs.cache_loads.is_empty());
        assert!(jobs.generations.is_empty());
    }

    fn ready(demands: impl IntoIterator<Item = ThumbnailDemand>) -> ThumbnailDemandEvaluation {
        ThumbnailDemandEvaluation::Ready(demands.into_iter().collect())
    }

    #[test]
    fn duplicate_paths_create_one_request() {
        let mut controller = LibraryThumbnailController::new(8, 2);
        controller.begin_directory();

        let jobs = controller.update_demand(ready([cover("/books/1.cbz"), cover("/books/1.cbz")]));

        assert_eq!(sources(&jobs.cache_loads), [Path::new("/books/1.cbz")]);
        assert!(jobs.generations.is_empty());
    }

    #[test]
    fn mixed_directory_jobs_keep_each_card_source_kind() {
        let mut controller = LibraryThumbnailController::new(8, 2);
        controller.begin_directory();
        let jobs = controller.update_demand(ready([
            ThumbnailDemand::new(
                PathBuf::from("/books/01/001.jpg"),
                ThumbnailSourceKind::DirectImage,
            ),
            ThumbnailDemand::new(
                PathBuf::from("/books/01/bonus.cbz"),
                ThumbnailSourceKind::BookCover,
            ),
            ThumbnailDemand::new(
                PathBuf::from("/books/01/special/cover.jpg"),
                ThumbnailSourceKind::BookCover,
            ),
        ]));

        assert_eq!(jobs.cache_loads.len(), 3);
        assert_eq!(
            jobs.cache_loads
                .iter()
                .map(|job| (&job.source, job.kind))
                .collect::<Vec<_>>(),
            vec![
                (
                    &PathBuf::from("/books/01/001.jpg"),
                    ThumbnailSourceKind::DirectImage,
                ),
                (
                    &PathBuf::from("/books/01/bonus.cbz"),
                    ThumbnailSourceKind::BookCover,
                ),
                (
                    &PathBuf::from("/books/01/special/cover.jpg"),
                    ThumbnailSourceKind::BookCover,
                ),
            ]
        );
    }

    #[test]
    fn same_source_with_different_kinds_keeps_distinct_completion_identity() {
        let mut controller = LibraryThumbnailController::new(8, 2);
        let generation = controller.begin_directory();
        let source = PathBuf::from("/books/shared.jpg");
        let demands = ready([
            ThumbnailDemand::new(source.clone(), ThumbnailSourceKind::BookCover),
            ThumbnailDemand::new(source.clone(), ThumbnailSourceKind::DirectImage),
        ]);

        let jobs = controller.update_demand(demands.clone());
        assert_eq!(jobs.cache_loads.len(), 2);
        assert_eq!(
            jobs.cache_loads
                .iter()
                .map(|job| job.kind)
                .collect::<HashSet<_>>(),
            HashSet::from([
                ThumbnailSourceKind::BookCover,
                ThumbnailSourceKind::DirectImage,
            ])
        );

        let cover_completion = controller.complete_cache_load_with_kind(
            generation,
            jobs.cache_loads
                .iter()
                .find(|job| job.kind == ThumbnailSourceKind::BookCover)
                .unwrap()
                .cohort_id,
            &source,
            ThumbnailSourceKind::BookCover,
            true,
        );
        assert!(cover_completion.accepted);

        let direct_jobs = controller.update_demand(demands);
        assert!(direct_jobs.cache_loads.is_empty());
        assert!(controller.ready.contains(&ThumbnailTextureKey::new(
            source.clone(),
            ThumbnailSourceKind::BookCover,
        )));
        assert!(!controller.ready.contains(&ThumbnailTextureKey::new(
            source,
            ThumbnailSourceKind::DirectImage,
        )));
    }

    #[test]
    fn cache_load_lane_uses_its_higher_concurrency_limit() {
        let mut controller = LibraryThumbnailController::new(8, 2);
        let generation = controller.begin_directory();
        let demands = (0..10).map(|index| cover(&format!("/books/{index}.cbz")));
        let jobs = controller.update_demand(ready(demands));
        assert_eq!(jobs.cache_loads.len(), 8);
        assert!(jobs.generations.is_empty());

        let completion =
            finish_cache_load(&mut controller, generation, Path::new("/books/0.cbz"), true);

        assert!(completion.accepted);
        assert_eq!(
            sources(&completion.jobs.cache_loads),
            [Path::new("/books/8.cbz")]
        );
        assert!(completion.jobs.generations.is_empty());
    }

    #[test]
    fn cache_hit_never_enters_generation_lane() {
        let mut controller = LibraryThumbnailController::new(8, 2);
        let generation = controller.begin_directory();
        controller.update_demand(ready([cover("/books/hit.cbz")]));

        let completion = finish_cache_load(
            &mut controller,
            generation,
            Path::new("/books/hit.cbz"),
            true,
        );

        assert!(completion.accepted);
        assert_no_jobs(&completion.jobs);
    }

    #[test]
    fn evicted_ready_source_becomes_a_cache_load_demand_again() {
        let mut controller = LibraryThumbnailController::new(8, 2);
        let generation = controller.begin_directory();
        let demand = ready([cover("/books/hit.cbz")]);
        controller.update_demand(demand.clone());
        finish_cache_load(
            &mut controller,
            generation,
            Path::new("/books/hit.cbz"),
            true,
        );

        controller.forget_ready_for_missing_textures(&demand);
        let jobs = controller.update_demand(demand);

        assert_eq!(sources(&jobs.cache_loads), [Path::new("/books/hit.cbz")]);
        assert!(jobs.cache_cohort.is_some());
    }

    #[test]
    fn cache_miss_moves_to_generation_lane() {
        let mut controller = LibraryThumbnailController::new(8, 2);
        let generation = controller.begin_directory();
        controller.update_demand(ready([cover("/books/miss.cbz")]));

        let completion = finish_cache_load(
            &mut controller,
            generation,
            Path::new("/books/miss.cbz"),
            false,
        );

        assert!(completion.accepted);
        assert_eq!(
            sources(&completion.jobs.generations),
            [Path::new("/books/miss.cbz")]
        );
        assert!(completion.jobs.cache_loads.is_empty());
    }

    #[test]
    fn cache_miss_path_is_not_duplicated_while_generation_is_in_flight() {
        let mut controller = LibraryThumbnailController::new(8, 2);
        let generation = controller.begin_directory();
        controller.update_demand(ready([cover("/books/miss.cbz")]));
        let miss = finish_cache_load(
            &mut controller,
            generation,
            Path::new("/books/miss.cbz"),
            false,
        );
        assert_eq!(miss.jobs.generations.len(), 1);

        let repeated =
            controller.update_demand(ready([cover("/books/miss.cbz"), cover("/books/miss.cbz")]));

        assert_no_jobs(&repeated);
        assert_eq!(controller.generation_in_flight.len(), 1);
    }

    #[test]
    fn generation_lane_remains_limited_to_two() {
        let mut controller = LibraryThumbnailController::new(8, 2);
        let generation = controller.begin_directory();
        let jobs = controller.update_demand(ready([
            cover("/books/1.cbz"),
            cover("/books/2.cbz"),
            cover("/books/3.cbz"),
        ]));
        for job in jobs.cache_loads {
            let completion = finish_cache_load(&mut controller, generation, &job.source, false);
            assert!(completion.accepted);
        }

        assert_eq!(controller.generation_in_flight.len(), 2);
        let completed_source = controller
            .generation_in_flight
            .iter()
            .next()
            .unwrap()
            .source
            .clone();
        let completion = controller.complete_generation(generation, &completed_source, true);

        assert!(completion.accepted);
        assert_eq!(completion.jobs.generations.len(), 1);
        assert_eq!(controller.generation_in_flight.len(), 2);
    }

    #[test]
    fn visible_order_replaces_old_offscreen_pending_work() {
        let mut controller = LibraryThumbnailController::new(1, 1);
        let generation = controller.begin_directory();
        let jobs = controller.update_demand(ready([
            cover("/books/1.cbz"),
            cover("/books/2.cbz"),
            cover("/books/3.cbz"),
        ]));
        assert_eq!(sources(&jobs.cache_loads), [Path::new("/books/1.cbz")]);

        let updated =
            controller.update_demand(ready([cover("/books/9.cbz"), cover("/books/8.cbz")]));
        assert_no_jobs(&updated);
        assert_eq!(
            updated.cache_cohort.unwrap().sources,
            [
                ThumbnailTextureKey::from(PathBuf::from("/books/9.cbz")),
                ThumbnailTextureKey::from(PathBuf::from("/books/8.cbz")),
            ]
        );
        let completion =
            finish_cache_load(&mut controller, generation, Path::new("/books/1.cbz"), true);

        assert_eq!(
            sources(&completion.jobs.cache_loads),
            [Path::new("/books/9.cbz")]
        );
    }

    #[test]
    fn completed_results_do_not_restart_the_same_visible_cohort() {
        let mut controller = LibraryThumbnailController::new(3, 2);
        let generation = controller.begin_directory();
        let initial = controller.update_demand(ready([
            cover("/books/1.cbz"),
            cover("/books/2.cbz"),
            cover("/books/3.cbz"),
        ]));
        let cohort_id = initial.cache_cohort.unwrap().id;

        finish_cache_load(&mut controller, generation, Path::new("/books/1.cbz"), true);
        finish_cache_load(
            &mut controller,
            generation,
            Path::new("/books/2.cbz"),
            false,
        );
        let updated =
            controller.update_demand(ready([cover("/books/2.cbz"), cover("/books/3.cbz")]));

        assert!(updated.cache_cohort.is_none());
        assert_eq!(controller.cohort_id, cohort_id);
    }

    #[test]
    fn only_final_generation_failure_is_not_requeued() {
        let mut controller = LibraryThumbnailController::new(1, 1);
        let generation = controller.begin_directory();
        controller.update_demand(ready([cover("/books/broken.cbz")]));
        let miss = finish_cache_load(
            &mut controller,
            generation,
            Path::new("/books/broken.cbz"),
            false,
        );
        assert_eq!(miss.jobs.generations.len(), 1);

        let completion =
            controller.complete_generation(generation, Path::new("/books/broken.cbz"), false);

        assert!(completion.accepted);
        let repeated = controller.update_demand(ready([cover("/books/broken.cbz")]));
        assert_no_jobs(&repeated);
    }

    #[test]
    fn generation_change_drops_pending_and_rejects_stale_cache_miss() {
        let mut controller = LibraryThumbnailController::new(2, 2);
        let old_generation = controller.begin_directory();
        let old_jobs = controller.update_demand(ready([
            cover("/old/1.cbz"),
            cover("/old/2.cbz"),
            cover("/old/3.cbz"),
        ]));
        assert_eq!(old_jobs.cache_loads.len(), 2);

        let new_generation = controller.begin_directory();
        assert_ne!(old_generation, new_generation);
        let new_jobs = controller.update_demand(ready([cover("/new/1.cbz")]));
        assert_no_jobs(&new_jobs);

        let first_stale = finish_cache_load(
            &mut controller,
            old_generation,
            Path::new("/old/1.cbz"),
            false,
        );
        assert!(!first_stale.accepted);
        assert!(first_stale.jobs.generations.is_empty());
        assert_eq!(
            sources(&first_stale.jobs.cache_loads),
            [Path::new("/new/1.cbz")]
        );

        let second_stale = finish_cache_load(
            &mut controller,
            old_generation,
            Path::new("/old/2.cbz"),
            true,
        );
        assert!(!second_stale.accepted);
        assert_no_jobs(&second_stale.jobs);
    }

    #[test]
    fn generation_change_rejects_stale_generation_success() {
        let mut controller = LibraryThumbnailController::new(1, 1);
        let old_generation = controller.begin_directory();
        controller.update_demand(ready([cover("/old/miss.cbz")]));
        let miss = finish_cache_load(
            &mut controller,
            old_generation,
            Path::new("/old/miss.cbz"),
            false,
        );
        assert_eq!(miss.jobs.generations.len(), 1);

        controller.begin_directory();
        let stale =
            controller.complete_generation(old_generation, Path::new("/old/miss.cbz"), true);

        assert!(!stale.accepted);
        assert_no_jobs(&stale.jobs);
    }

    #[test]
    fn successful_source_is_not_requested_again() {
        let mut controller = LibraryThumbnailController::new(1, 1);
        let generation = controller.begin_directory();
        controller.update_demand(ready([cover("/books/ready.cbz")]));
        assert!(
            finish_cache_load(
                &mut controller,
                generation,
                Path::new("/books/ready.cbz"),
                true,
            )
            .accepted
        );

        let repeated = controller.update_demand(ready([cover("/books/ready.cbz")]));
        assert_no_jobs(&repeated);
    }

    #[test]
    fn allocation_pending_does_not_replace_queued_demand() {
        let mut controller = LibraryThumbnailController::new(1, 1);
        let generation = controller.begin_directory();
        let jobs = controller.update_demand(ready([cover("/books/1.cbz"), cover("/books/2.cbz")]));
        assert_eq!(sources(&jobs.cache_loads), [Path::new("/books/1.cbz")]);

        let pending = controller.update_demand(ThumbnailDemandEvaluation::AllocationPending);
        assert_no_jobs(&pending);
        let completion =
            finish_cache_load(&mut controller, generation, Path::new("/books/1.cbz"), true);

        assert_eq!(
            sources(&completion.jobs.cache_loads),
            [Path::new("/books/2.cbz")]
        );
    }

    #[test]
    fn evaluated_empty_demand_still_clears_queued_work() {
        let mut controller = LibraryThumbnailController::new(1, 1);
        let generation = controller.begin_directory();
        controller.update_demand(ready([cover("/books/1.cbz"), cover("/books/2.cbz")]));

        controller.update_demand(ThumbnailDemandEvaluation::Ready(Vec::new()));
        let completion =
            finish_cache_load(&mut controller, generation, Path::new("/books/1.cbz"), true);

        assert_no_jobs(&completion.jobs);
    }

    #[test]
    fn demand_is_processed_after_allocation_becomes_ready() {
        let mut controller = LibraryThumbnailController::new(1, 1);
        controller.begin_directory();

        let pending = controller.update_demand(ThumbnailDemandEvaluation::AllocationPending);
        assert_no_jobs(&pending);
        let jobs = controller.update_demand(ready([cover("/books/visible.cbz")]));

        assert_eq!(
            sources(&jobs.cache_loads),
            [Path::new("/books/visible.cbz")]
        );
    }

    fn thumbnail(value: u8) -> ThumbnailData {
        ThumbnailData {
            pixels: vec![value, value, value],
            width: 1,
            height: 1,
            stride: 3,
        }
    }

    #[test]
    fn first_result_is_available_to_its_quiet_timeout() {
        let mut batch = LibraryThumbnailBatch::default();
        batch.begin_generation(4);

        let timers = batch
            .push(4, PathBuf::from("/books/1.cbz"), thumbnail(1))
            .unwrap();

        assert!(timers.starts_batch);
        let prepared = batch.take_for_timeout(
            timers.generation,
            timers.batch_id,
            Some(timers.quiet_revision),
        );
        assert_eq!(prepared.len(), 1);
        assert_eq!(prepared[0].0.source, Path::new("/books/1.cbz"));
    }

    #[test]
    fn later_result_invalidates_the_previous_quiet_timeout() {
        let mut batch = LibraryThumbnailBatch::default();
        batch.begin_generation(4);
        let first = batch
            .push(4, PathBuf::from("/books/1.cbz"), thumbnail(1))
            .unwrap();
        let second = batch
            .push(4, PathBuf::from("/books/2.cbz"), thumbnail(2))
            .unwrap();

        assert!(!second.starts_batch);
        assert!(
            batch
                .take_for_timeout(4, first.batch_id, Some(first.quiet_revision))
                .is_empty()
        );
        assert_eq!(
            batch
                .take_for_timeout(4, second.batch_id, Some(second.quiet_revision))
                .len(),
            2
        );
    }

    #[test]
    fn maximum_timeout_flushes_despite_continuing_arrivals() {
        let mut batch = LibraryThumbnailBatch::default();
        batch.begin_generation(5);
        let first = batch
            .push(5, PathBuf::from("/books/1.cbz"), thumbnail(1))
            .unwrap();
        batch
            .push(5, PathBuf::from("/books/2.cbz"), thumbnail(2))
            .unwrap();
        batch
            .push(5, PathBuf::from("/books/3.cbz"), thumbnail(3))
            .unwrap();

        assert_eq!(
            batch
                .take_for_timeout(first.generation, first.batch_id, None)
                .len(),
            3
        );
    }

    #[test]
    fn flush_starts_an_independent_next_batch() {
        let mut batch = LibraryThumbnailBatch::default();
        batch.begin_generation(6);
        let first = batch
            .push(6, PathBuf::from("/books/1.cbz"), thumbnail(1))
            .unwrap();
        assert_eq!(
            batch
                .take_for_timeout(6, first.batch_id, Some(first.quiet_revision))
                .len(),
            1
        );

        let second = batch
            .push(6, PathBuf::from("/books/2.cbz"), thumbnail(2))
            .unwrap();

        assert!(second.starts_batch);
        assert_ne!(second.batch_id, first.batch_id);
        assert!(batch.take_for_timeout(6, first.batch_id, None).is_empty());
        assert_eq!(
            batch
                .take_for_timeout(6, second.batch_id, Some(second.quiet_revision))
                .len(),
            1
        );
    }

    #[test]
    fn generation_change_discards_pending_and_ignores_old_timeouts() {
        let mut batch = LibraryThumbnailBatch::default();
        batch.begin_generation(7);
        let old = batch
            .push(7, PathBuf::from("/old/1.cbz"), thumbnail(1))
            .unwrap();

        batch.begin_generation(8);
        let new = batch
            .push(8, PathBuf::from("/new/1.cbz"), thumbnail(3))
            .unwrap();

        assert!(
            batch
                .take_for_timeout(7, old.batch_id, Some(old.quiet_revision))
                .is_empty()
        );
        assert!(
            batch
                .push(7, PathBuf::from("/old/2.cbz"), thumbnail(2))
                .is_none()
        );
        assert_eq!(
            batch
                .take_for_timeout(8, new.batch_id, Some(new.quiet_revision))
                .len(),
            1
        );
    }

    fn cohort(generation: u64, id: u64, sources: &[&str]) -> CacheLookupCohort {
        CacheLookupCohort {
            generation,
            id,
            sources: sources
                .iter()
                .map(|source| ThumbnailTextureKey::from(PathBuf::from(*source)))
                .collect(),
        }
    }

    #[test]
    fn all_hit_cohort_waits_for_the_last_lookup_and_flushes_once_in_visible_order() {
        let mut batch = LibraryThumbnailCacheHitBatch::default();
        batch.begin_generation(9);
        assert!(!batch.begin_cohort(cohort(9, 1, &["/books/1.cbz", "/books/2.cbz"])));

        let second = batch.complete_lookup(9, 1, PathBuf::from("/books/2.cbz"), Some(thumbnail(2)));
        assert!(second.immediate.is_empty());
        assert!(!second.schedule_frame_flush);
        assert!(batch.take_for_flush(9).is_empty());

        let first = batch.complete_lookup(9, 1, PathBuf::from("/books/1.cbz"), Some(thumbnail(1)));
        let prepared = first.immediate;
        assert_eq!(prepared.len(), 2);
        assert_eq!(prepared[0].0.source, Path::new("/books/1.cbz"));
        assert_eq!(prepared[1].0.source, Path::new("/books/2.cbz"));
        assert!(!first.schedule_frame_flush);
        assert!(batch.take_for_flush(9).is_empty());
    }

    #[test]
    fn cache_hit_batch_preserves_kind_for_the_same_source() {
        let source = PathBuf::from("/books/shared.jpg");
        let cover = ThumbnailTextureKey::new(source.clone(), ThumbnailSourceKind::BookCover);
        let image = ThumbnailTextureKey::new(source.clone(), ThumbnailSourceKind::DirectImage);
        let mut batch = LibraryThumbnailCacheHitBatch::default();
        batch.begin_generation(9);
        batch.begin_cohort(CacheLookupCohort {
            generation: 9,
            id: 1,
            sources: vec![cover.clone(), image.clone()],
        });

        batch.complete_lookup(9, 1, image.clone(), Some(thumbnail(2)));
        let update = batch.complete_lookup(9, 1, cover.clone(), Some(thumbnail(1)));

        assert_eq!(
            update
                .immediate
                .iter()
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>(),
            vec![cover, image]
        );
    }

    #[test]
    fn first_miss_flushes_held_hits_and_later_hits_use_the_frame_batch() {
        let mut batch = LibraryThumbnailCacheHitBatch::default();
        batch.begin_generation(10);
        batch.begin_cohort(cohort(
            10,
            3,
            &["/books/1.cbz", "/books/2.cbz", "/books/3.cbz"],
        ));
        let hit = batch.complete_lookup(10, 3, PathBuf::from("/books/1.cbz"), Some(thumbnail(1)));
        assert!(hit.immediate.is_empty());

        let miss = batch.complete_lookup(10, 3, PathBuf::from("/books/2.cbz"), None);
        assert_eq!(miss.immediate.len(), 1);
        assert_eq!(miss.immediate[0].0.source, Path::new("/books/1.cbz"));
        assert!(!miss.schedule_frame_flush);

        let later_hit =
            batch.complete_lookup(10, 3, PathBuf::from("/books/3.cbz"), Some(thumbnail(3)));
        assert!(later_hit.immediate.is_empty());
        assert!(later_hit.schedule_frame_flush);
        let prepared = batch.take_for_flush(10);
        assert_eq!(prepared.len(), 1);
        assert_eq!(prepared[0].0.source, Path::new("/books/3.cbz"));
    }

    #[test]
    fn miss_after_no_hits_releases_waiting_without_waiting_for_other_lookups() {
        let mut batch = LibraryThumbnailCacheHitBatch::default();
        batch.begin_generation(11);
        batch.begin_cohort(cohort(11, 4, &["/books/1.cbz", "/books/2.cbz"]));

        let miss = batch.complete_lookup(11, 4, PathBuf::from("/books/1.cbz"), None);
        assert!(miss.immediate.is_empty());
        let later_hit =
            batch.complete_lookup(11, 4, PathBuf::from("/books/2.cbz"), Some(thumbnail(2)));
        assert!(later_hit.schedule_frame_flush);
        assert_eq!(batch.take_for_flush(11).len(), 1);
    }

    #[test]
    fn new_visible_cohort_does_not_inherit_old_completion_state() {
        let mut batch = LibraryThumbnailCacheHitBatch::default();
        batch.begin_generation(11);
        batch.begin_cohort(cohort(11, 1, &["/old/1.cbz", "/old/2.cbz"]));
        batch.complete_lookup(11, 1, PathBuf::from("/old/1.cbz"), Some(thumbnail(1)));

        assert!(batch.begin_cohort(cohort(11, 2, &["/new/1.cbz", "/new/2.cbz"])));
        assert_eq!(batch.take_for_flush(11).len(), 1);
        let stale = batch.complete_lookup(11, 1, PathBuf::from("/old/2.cbz"), Some(thumbnail(2)));
        assert!(stale.schedule_frame_flush);

        let first_new =
            batch.complete_lookup(11, 2, PathBuf::from("/new/1.cbz"), Some(thumbnail(3)));
        assert!(first_new.immediate.is_empty());
        let last_new =
            batch.complete_lookup(11, 2, PathBuf::from("/new/2.cbz"), Some(thumbnail(4)));
        assert_eq!(last_new.immediate.len(), 2);
        assert_eq!(last_new.immediate[0].0.source, Path::new("/new/1.cbz"));
        assert_eq!(last_new.immediate[1].0.source, Path::new("/new/2.cbz"));
    }

    #[test]
    fn generation_change_discards_old_cohort_hits_and_completions() {
        let mut batch = LibraryThumbnailCacheHitBatch::default();
        batch.begin_generation(12);
        batch.begin_cohort(cohort(12, 1, &["/old/1.cbz", "/old/2.cbz"]));
        batch.complete_lookup(12, 1, PathBuf::from("/old/1.cbz"), Some(thumbnail(1)));

        batch.begin_generation(13);
        let stale = batch.complete_lookup(12, 1, PathBuf::from("/old/2.cbz"), Some(thumbnail(2)));
        assert!(stale.immediate.is_empty());
        assert!(!stale.schedule_frame_flush);
        assert!(batch.take_for_flush(12).is_empty());

        batch.begin_cohort(cohort(13, 1, &["/new/1.cbz"]));
        let fresh = batch.complete_lookup(13, 1, PathBuf::from("/new/1.cbz"), Some(thumbnail(3)));
        assert_eq!(fresh.immediate.len(), 1);
        assert_eq!(fresh.immediate[0].0.source, Path::new("/new/1.cbz"));
    }

    #[test]
    fn initial_reveal_waits_for_every_cache_lookup_but_not_generation() {
        let mut reveal = LibraryInitialReveal::default();
        reveal.begin_directory(20);
        let timeout = reveal.begin_render().unwrap();
        assert!(!reveal.begin_demand(&ready([cover("/books/hit.cbz"), cover("/books/miss.cbz"),])));

        assert!(!reveal.complete_lookup(20, &cover("/books/miss.cbz").key()));
        assert!(reveal.complete_lookup(20, &cover("/books/hit.cbz").key()));
        assert!(!reveal.timeout(timeout));
    }

    #[test]
    fn initial_reveal_timeout_does_not_stop_late_cache_work() {
        let mut controller = LibraryThumbnailController::new(1, 1);
        let generation = controller.begin_directory();
        let jobs = controller.update_demand(ready([cover("/books/1.cbz"), cover("/books/2.cbz")]));
        assert_eq!(jobs.cache_loads.len(), 1);

        let mut reveal = LibraryInitialReveal::default();
        reveal.begin_directory(generation);
        let timeout = reveal.begin_render().unwrap();
        reveal.begin_demand(&ready([cover("/books/1.cbz"), cover("/books/2.cbz")]));
        assert!(reveal.timeout(timeout));

        let completion =
            finish_cache_load(&mut controller, generation, Path::new("/books/1.cbz"), true);
        assert!(completion.accepted);
        assert_eq!(
            sources(&completion.jobs.cache_loads),
            [Path::new("/books/2.cbz")]
        );
        assert!(!reveal.complete_lookup(generation, &cover("/books/1.cbz").key()));
    }

    #[test]
    fn stale_initial_reveal_timeout_cannot_reveal_a_new_directory() {
        let mut reveal = LibraryInitialReveal::default();
        reveal.begin_directory(30);
        let stale = reveal.begin_render().unwrap();
        reveal.begin_demand(&ready([cover("/old/1.cbz")]));

        reveal.begin_directory(31);
        let current = reveal.begin_render().unwrap();
        reveal.begin_demand(&ready([cover("/new/1.cbz")]));

        assert!(!reveal.timeout(stale));
        assert!(reveal.timeout(current));
    }

    #[test]
    fn revealed_library_is_not_hidden_again_by_viewport_or_size_updates() {
        let mut reveal = LibraryInitialReveal::default();
        reveal.begin_directory(40);
        reveal.begin_render();
        assert!(reveal.begin_demand(&ready([])));

        assert!(!reveal.begin_demand(&ready([cover("/books/scrolled.cbz")])));
        assert!(reveal.begin_render().is_none());
    }

    #[test]
    fn reveal_flush_takes_partial_all_hit_results_without_ending_lookup_tracking() {
        let mut batch = LibraryThumbnailCacheHitBatch::default();
        batch.begin_generation(50);
        batch.begin_cohort(cohort(50, 1, &["/books/1.cbz", "/books/2.cbz"]));
        batch.complete_lookup(50, 1, PathBuf::from("/books/1.cbz"), Some(thumbnail(1)));

        let partial = batch.take_available_for_initial_reveal(50);
        assert_eq!(partial.len(), 1);
        assert_eq!(partial[0].0.source, Path::new("/books/1.cbz"));

        let remaining =
            batch.complete_lookup(50, 1, PathBuf::from("/books/2.cbz"), Some(thumbnail(2)));
        assert_eq!(remaining.immediate.len(), 1);
        assert_eq!(remaining.immediate[0].0.source, Path::new("/books/2.cbz"));
    }
}
