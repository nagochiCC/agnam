//! Cross-controller checks for the Library thumbnail scope boundaries.

use super::archive_library::ArchiveLibraryState;
use super::library::ScanCompletion;
use super::library_thumbnail::{ThumbnailDemand, ThumbnailDemandEvaluation, ThumbnailSourceKind};
use super::navigation_panel::{NavigationPanel, NavigationPanelController};
use super::state::LibraryRuntimeState;
use crate::archive::{ArchiveContentLevel, ArchiveImageId, ArchiveLocation};
use crate::bookshelf::BookshelfDirectory;
use crate::bookshelf::thumbnail::ThumbnailData;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn demand(path: &str, kind: ThumbnailSourceKind) -> ThumbnailDemand {
    ThumbnailDemand::new(PathBuf::from(path), kind)
}

fn ready(demands: impl IntoIterator<Item = ThumbnailDemand>) -> ThumbnailDemandEvaluation {
    ThumbnailDemandEvaluation::Ready(demands.into_iter().collect())
}

fn thumbnail(value: u8) -> ThumbnailData {
    ThumbnailData {
        pixels: vec![value; 3],
        width: 1,
        height: 1,
        stride: 3,
    }
}

#[test]
fn directory_change_restarts_all_library_thumbnail_owners_together() {
    let mut library = LibraryRuntimeState::new(None);
    let old_generation = library.begin_thumbnail_scope();
    let old_demand = ready([
        demand("/A/hit.jpg", ThumbnailSourceKind::DirectImage),
        demand("/A/miss.cbz", ThumbnailSourceKind::BookCover),
    ]);
    let old_jobs = library.thumbnails.update_demand(old_demand.clone());
    let old_cohort = old_jobs.cache_cohort.unwrap();
    assert_eq!(old_cohort.generation, old_generation);
    assert_eq!(old_jobs.cache_loads.len(), 2);
    assert!(
        !library
            .thumbnail_cache_hits
            .begin_cohort(old_cohort.clone())
    );
    let timeout = library.initial_reveal.begin_render().unwrap();
    assert!(!library.initial_reveal.begin_demand(&old_demand));

    let old_miss = old_jobs
        .cache_loads
        .iter()
        .find(|job| job.source == Path::new("/A/miss.cbz"))
        .unwrap();
    let old_hit = old_jobs
        .cache_loads
        .iter()
        .find(|job| job.source == Path::new("/A/hit.jpg"))
        .unwrap();
    assert!(
        library
            .thumbnails
            .complete_cache_load_with_kind(
                old_generation,
                old_hit.cohort_id,
                &old_hit.source,
                old_hit.kind,
                true,
            )
            .accepted
    );
    assert!(
        library
            .thumbnail_cache_hits
            .complete_lookup(
                old_generation,
                old_cohort.id,
                old_hit.source.clone(),
                Some(thumbnail(0)),
            )
            .immediate
            .is_empty()
    );
    let miss = library.thumbnails.complete_cache_load_with_kind(
        old_generation,
        old_miss.cohort_id,
        &old_miss.source,
        old_miss.kind,
        false,
    );
    let old_generation_job = miss.jobs.generations.first().unwrap();
    let old_batch = library
        .thumbnail_batch
        .push_with_kind(
            old_generation,
            old_generation_job.source.clone(),
            old_generation_job.kind,
            thumbnail(1),
        )
        .unwrap();

    let current_generation = library.begin_thumbnail_scope();
    assert_ne!(current_generation, old_generation);
    let current_demand = ready([demand("/B/current.cbz", ThumbnailSourceKind::BookCover)]);
    let current_jobs = library.thumbnails.update_demand(current_demand.clone());
    let current_cohort = current_jobs.cache_cohort.unwrap();
    assert_eq!(current_cohort.generation, current_generation);
    assert!(current_jobs.cache_loads.iter().all(
        |job| job.generation == current_generation && job.source == Path::new("/B/current.cbz")
    ));
    assert!(!library.thumbnail_cache_hits.begin_cohort(current_cohort));
    assert_eq!(
        library.initial_reveal.begin_render().unwrap().generation,
        current_generation
    );

    assert!(!library.thumbnails.accepts_generation_completion_with_kind(
        old_generation,
        &old_generation_job.source,
        old_generation_job.kind,
    ));
    assert!(
        library
            .thumbnail_batch
            .take_for_timeout(old_generation, old_batch.batch_id, None)
            .is_empty()
    );
    assert!(
        library
            .thumbnail_cache_hits
            .take_for_flush(old_generation)
            .is_empty()
    );
    assert!(!library.initial_reveal.timeout(timeout));
    assert!(!library.thumbnails.accepts_cache_load_completion_with_kind(
        old_generation,
        old_cohort.id,
        Path::new("/A/hit.jpg"),
        ThumbnailSourceKind::DirectImage,
    ));
}

#[test]
fn cache_hit_and_miss_share_one_scope_and_initial_reveal_waits_for_lookups() {
    let mut library = LibraryRuntimeState::new(None);
    let generation = library.begin_thumbnail_scope();
    let timeout = library.initial_reveal.begin_render().unwrap();
    assert!(
        library
            .thumbnails
            .update_demand(ThumbnailDemandEvaluation::AllocationPending)
            .cache_loads
            .is_empty()
    );
    assert!(
        !library
            .initial_reveal
            .begin_demand(&ThumbnailDemandEvaluation::AllocationPending)
    );
    let hit = demand("/books/direct.jpg", ThumbnailSourceKind::DirectImage);
    let miss = demand("/books/book.cbz", ThumbnailSourceKind::BookCover);
    let demands = ready([hit.clone(), miss.clone()]);
    let jobs = library.thumbnails.update_demand(demands.clone());
    let cohort = jobs.cache_cohort.unwrap();
    assert_eq!(cohort.generation, generation);
    assert_eq!(cohort.sources.len(), 2);
    assert_eq!(jobs.generations.len(), 0);
    assert!(!library.thumbnail_cache_hits.begin_cohort(cohort.clone()));
    assert!(!library.initial_reveal.begin_demand(&demands));

    let miss_job = jobs
        .cache_loads
        .iter()
        .find(|job| job.source == miss.source)
        .unwrap();
    let miss_completion = library.thumbnails.complete_cache_load_with_kind(
        generation,
        miss_job.cohort_id,
        &miss.source,
        miss.kind,
        false,
    );
    assert_eq!(miss_completion.jobs.generations.len(), 1);
    let miss_lookup =
        library
            .thumbnail_cache_hits
            .complete_lookup(generation, cohort.id, miss.key(), None);
    assert!(miss_lookup.immediate.is_empty());
    assert!(
        !library
            .initial_reveal
            .complete_lookup(generation, &miss.key())
    );

    let generation_job = &miss_completion.jobs.generations[0];
    assert_eq!(generation_job.generation, generation);
    assert_eq!(generation_job.source, miss.source);
    assert!(
        library
            .thumbnails
            .complete_generation_with_kind(
                generation,
                &generation_job.source,
                generation_job.kind,
                true,
            )
            .accepted
    );
    assert!(
        library
            .thumbnail_cache_hits
            .take_available_for_initial_reveal(generation)
            .is_empty()
    );
    let timers = library
        .thumbnail_batch
        .push_with_kind(
            generation,
            generation_job.source.clone(),
            generation_job.kind,
            thumbnail(3),
        )
        .unwrap();
    let normal_batch = library
        .thumbnail_batch
        .take_for_timeout(generation, timers.batch_id, None);
    assert_eq!(normal_batch.len(), 1);
    assert_eq!(normal_batch[0].0, miss.key());

    let hit_job = jobs
        .cache_loads
        .iter()
        .find(|job| job.source == hit.source)
        .unwrap();
    let hit_completion = library.thumbnails.complete_cache_load_with_kind(
        generation,
        hit_job.cohort_id,
        &hit.source,
        hit.kind,
        true,
    );
    assert!(hit_completion.jobs.generations.is_empty());
    let hit_update = library.thumbnail_cache_hits.complete_lookup(
        generation,
        cohort.id,
        hit.key(),
        Some(thumbnail(2)),
    );
    assert!(hit_update.immediate.is_empty());
    assert!(hit_update.schedule_frame_flush);
    assert!(
        library
            .initial_reveal
            .complete_lookup(generation, &hit.key())
    );
    let cache_hit_batch = library.thumbnail_cache_hits.take_for_flush(generation);
    assert_eq!(cache_hit_batch.len(), 1);
    assert_eq!(cache_hit_batch[0].0, hit.key());
    assert!(!library.initial_reveal.timeout(timeout));
}

#[test]
fn snapshot_refresh_keeps_scope_when_unchanged_and_updates_demand_when_changed() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("books");
    std::fs::create_dir(&path).unwrap();
    let snapshot = BookshelfDirectory {
        path: path.clone(),
        is_image_document_directory: false,
        direct_files: Vec::new(),
        direct_file_metadata: HashMap::new(),
        child_shelves: Vec::new(),
    };
    let mut model = super::library::LibraryState::new(Some(path.clone()));
    model.seed_snapshot_for_test(snapshot.clone());
    assert_eq!(model.directory(), Some(&snapshot));
    let mut library = LibraryRuntimeState::new(None);
    let generation = library.begin_thumbnail_scope();
    let cached_job = library
        .thumbnails
        .update_demand(ready([demand(
            "/cached/snapshot.cbz",
            ThumbnailSourceKind::BookCover,
        )]))
        .cache_loads
        .remove(0);

    let unchanged = model.begin_current_scan().unwrap();
    assert!(unchanged.persist_snapshot);
    assert_eq!(
        model.apply_scan(unchanged.id, &unchanged.path, Ok(snapshot.clone())),
        ScanCompletion::RefreshUnchanged
    );
    assert!(library.thumbnails.accepts_cache_load_completion_with_kind(
        cached_job.generation,
        cached_job.cohort_id,
        &cached_job.source,
        cached_job.kind,
    ));
    let stable_generation = generation;

    let changed = model.begin_current_scan().unwrap();
    let mut fresh = snapshot;
    let new_book = path.join("01.cbz");
    fresh.direct_files.push(new_book.clone());
    assert_eq!(
        model.apply_scan(changed.id, &changed.path, Ok(fresh)),
        ScanCompletion::RefreshChanged
    );
    let mut demand_update = library.thumbnails.update_demand(ready([demand(
        new_book.to_str().unwrap(),
        ThumbnailSourceKind::DirectImage,
    )]));
    assert_eq!(
        demand_update.cache_cohort.take().unwrap().generation,
        stable_generation
    );
    let fresh_job = demand_update.cache_loads.remove(0);

    let stale = model.begin_current_scan().unwrap();
    let current = model.navigate(path.join("child")).unwrap();
    assert_eq!(
        model.apply_scan(
            stale.id,
            &stale.path,
            Ok(BookshelfDirectory {
                path: stale.path.clone(),
                is_image_document_directory: false,
                direct_files: vec![path.join("stale.cbz")],
                direct_file_metadata: HashMap::new(),
                child_shelves: Vec::new(),
            })
        ),
        ScanCompletion::Stale
    );
    assert_eq!(model.current_directory(), Some(current.path.as_path()));
    assert!(library.thumbnails.accepts_cache_load_completion_with_kind(
        fresh_job.generation,
        fresh_job.cohort_id,
        &fresh_job.source,
        fresh_job.kind,
    ));
}

#[test]
fn archive_contents_keep_natural_listing_and_reject_a_previous_session() {
    let root = tempfile::tempdir().unwrap();
    let archive = root.path().join("contents.cbz");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
    for entry in ["pages/10.jpg", "pages/2.jpg", "pages/1.jpg"] {
        zip.start_file(entry, zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut zip, b"x").unwrap();
    }
    zip.finish().unwrap();

    let location = ArchiveLocation::root(archive.clone());
    let level = ArchiveContentLevel::open(Default::default(), location.clone()).unwrap();
    assert_eq!(
        level
            .items
            .iter()
            .map(|item| item.path.as_path())
            .collect::<Vec<_>>(),
        [Path::new("pages")]
    );
    let mut nested_level = ArchiveContentLevel::open(Default::default(), location.clone()).unwrap();
    assert!(nested_level.navigate_directory(PathBuf::from("pages")));
    assert_eq!(
        nested_level
            .items
            .iter()
            .map(|item| item.path.as_path())
            .collect::<Vec<_>>(),
        [
            Path::new("pages/1.jpg"),
            Path::new("pages/2.jpg"),
            Path::new("pages/10.jpg")
        ]
    );

    let mut library = LibraryRuntimeState::new(None);
    let filesystem_generation = library.begin_thumbnail_scope();
    let filesystem_job = library
        .thumbnails
        .update_demand(ready([demand(
            "/books/filesystem.jpg",
            ThumbnailSourceKind::DirectImage,
        )]))
        .cache_loads
        .remove(0);
    assert_eq!(filesystem_job.generation, filesystem_generation);

    let mut archive_state = ArchiveLibraryState::default();
    let old_request = archive_state.begin_for_test(location, root.path());
    let current_location = ArchiveLocation {
        archive: archive.clone(),
        archives: Vec::new(),
        directory: PathBuf::from("pages"),
    };
    let current_request = archive_state.begin_for_test(current_location, root.path());
    assert!(!archive_state.apply(old_request.id, Ok(level)));
    assert!(archive_state.apply(current_request.id, Ok(nested_level)));
    assert_eq!(
        archive_state.location().unwrap().directory,
        PathBuf::from("pages")
    );

    let archive_generation = library.begin_thumbnail_scope();
    assert_ne!(archive_generation, filesystem_generation);
    let archive_source = ArchiveLibraryState::thumbnail_key(
        &archive,
        &ArchiveImageId {
            archives: Vec::new(),
            image: PathBuf::from("pages/1.jpg"),
        },
    );
    let archive_demand = ready([demand(
        archive_source.to_str().unwrap(),
        ThumbnailSourceKind::ArchiveEntry,
    )]);
    let progressive = archive_state.begin_progressive_thumbnails();
    let progressive_jobs = library.thumbnails.update_demand_with_generation(
        archive_demand.clone(),
        !archive_state.progressive_thumbnails_active(),
    );
    assert!(progressive_jobs.generations.is_empty());
    let archive_cache = progressive_jobs.cache_loads.first().unwrap();
    assert_eq!(archive_cache.generation, archive_generation);
    assert_eq!(archive_cache.kind, ThumbnailSourceKind::ArchiveEntry);
    let miss = library.thumbnails.complete_cache_load_with_kind(
        archive_cache.generation,
        archive_cache.cohort_id,
        &archive_cache.source,
        archive_cache.kind,
        false,
    );
    assert!(miss.jobs.generations.is_empty());
    assert!(archive_state.finish_progressive_thumbnails(current_request.id));
    let finished = library
        .thumbnails
        .update_demand_with_generation(archive_demand, true);
    assert_eq!(finished.generations.len(), 1);
    assert_eq!(finished.generations[0].generation, archive_generation);
    assert!(!library.thumbnails.accepts_cache_load_completion_with_kind(
        filesystem_job.generation,
        filesystem_job.cohort_id,
        &filesystem_job.source,
        filesystem_job.kind,
    ));
    drop(progressive);
}

#[test]
fn cover_override_surfaces_keep_independent_generations_and_dirty_state() {
    let mut library = LibraryRuntimeState::new(None);
    let library_generation = library.begin_thumbnail_scope();
    library.search_thumbnails.begin_generation();
    let old_search_jobs = library.search_thumbnails.update_demand(
        super::library_search_thumbnail::SearchThumbnailDemandEvaluation::Ready(vec![
            PathBuf::from("/cover.cbz"),
        ]),
    );
    let old_search_job = old_search_jobs.cache_loads.first().unwrap();
    let invalidated_generation = library.invalidate_cover_override(false);
    assert_ne!(invalidated_generation, library_generation);
    let after_override = library.search_thumbnails.update_demand(
        super::library_search_thumbnail::SearchThumbnailDemandEvaluation::Ready(vec![
            PathBuf::from("/cover.cbz"),
        ]),
    );
    assert!(after_override.cache_loads.is_empty());
    let stale = library.search_thumbnails.complete_cache_load(
        old_search_job.generation,
        &old_search_job.source,
        true,
    );
    assert!(!stale.accepted);
    assert_eq!(stale.jobs.cache_loads.len(), 1);
    assert_ne!(
        stale.jobs.cache_loads[0].generation,
        old_search_job.generation
    );

    let mut panels = NavigationPanelController::default();
    for panel in [
        NavigationPanel::Search,
        NavigationPanel::History,
        NavigationPanel::Favorites,
    ] {
        panels.mark_clean(panel);
    }
    panels.mark_cover_override_dirty();
    for panel in [
        NavigationPanel::Search,
        NavigationPanel::History,
        NavigationPanel::Favorites,
    ] {
        assert!(panels.is_dirty(panel));
    }
}
