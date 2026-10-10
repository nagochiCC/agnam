use super::*;
use std::cell::Cell;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

fn fixture(root: &Path, archives_per_directory: usize) -> Vec<PathBuf> {
    let mut zip_bytes = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut zip_bytes);
        zip.start_file("page.jpg", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"image bytes are irrelevant to listing")
            .unwrap();
        zip.finish().unwrap();
    }
    (0..4)
        .map(|directory| {
            let path = root.join(format!("shelf{directory}"));
            fs::create_dir_all(&path).unwrap();
            for image in 0..40 {
                fs::write(path.join(format!("page{image:03}.jpg")), b"image").unwrap();
            }
            for archive in 0..archives_per_directory {
                fs::write(
                    path.join(format!("book{archive:04}.cbz")),
                    zip_bytes.get_ref(),
                )
                .unwrap();
            }
            path
        })
        .collect()
}

fn drain_scans(
    state: &mut LibraryState,
    receiver: &async_channel::Receiver<Msg>,
    messages: &mut usize,
    rejected: &mut usize,
) {
    while let Ok(message) = receiver.try_recv() {
        if let Msg::LibraryScanFinished {
            request_id,
            path,
            result,
        } = message
        {
            *messages += 1;
            if state.apply_scan(request_id, &path, result) == library::ScanCompletion::Stale {
                *rejected += 1;
            }
        }
    }
}

fn measure_scan(label: &str, paths: &[PathBuf], requests: usize, interval: Duration) {
    let mut state = LibraryState::new(Some(paths[0].parent().unwrap().to_path_buf()));
    let (sender, receiver) = AppSender::channel();
    let (times_tx, times_rx) = mpsc::channel();
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let queue_peak = Arc::new(AtomicUsize::new(0));
    let mut issued = Vec::new();
    let mut messages = 0;
    let mut rejected = 0;
    for number in 0..requests {
        drain_scans(&mut state, &receiver, &mut messages, &mut rejected);
        let request = state.navigate(paths[number % paths.len()].clone()).unwrap();
        issued.push((request.id, Instant::now()));
        let tx = times_tx.clone();
        let sender = sender.clone();
        let active = active.clone();
        let peak = peak.clone();
        let queue_peak = queue_peak.clone();
        let receiver_for_depth = receiver.clone();
        spawn_background(move || {
            let start = Instant::now();
            let concurrent = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(concurrent, Ordering::SeqCst);
            let result = crate::bookshelf::scan_directory(&request.path).map_err(|e| e.to_string());
            let finish = Instant::now();
            sender.input(Msg::LibraryScanFinished {
                request_id: request.id,
                path: request.path,
                result,
            });
            queue_peak.fetch_max(receiver_for_depth.len(), Ordering::SeqCst);
            active.fetch_sub(1, Ordering::SeqCst);
            tx.send((request.id, start, finish)).unwrap();
        });
        let until = Instant::now() + interval;
        while Instant::now() < until {
            drain_scans(&mut state, &receiver, &mut messages, &mut rejected);
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    drop(times_tx);
    let mut completed = Vec::new();
    while completed.len() < requests {
        if let Ok(timing) = times_rx.recv_timeout(Duration::from_millis(1)) {
            completed.push(timing);
        }
        drain_scans(&mut state, &receiver, &mut messages, &mut rejected);
    }
    drain_scans(&mut state, &receiver, &mut messages, &mut rejected);
    let mut stale_ages = Vec::new();
    for &(id, _, finish) in &completed {
        if let Some((_, next_issue)) = issued.iter().find(|(candidate, _)| *candidate == id + 1) {
            if finish > *next_issue {
                stale_ages.push(finish.duration_since(*next_issue));
            }
        }
    }
    stale_ages.sort();
    let latest_id = issued.last().unwrap().0;
    let (_, latest_start, latest_finish) = completed
        .iter()
        .find(|(id, _, _)| *id == latest_id)
        .unwrap();
    let latest_issue = issued.last().unwrap().1;
    let active_old_at_latest_start = completed
        .iter()
        .filter(|(id, start, finish)| {
            *id != latest_id && *start <= *latest_start && *finish > *latest_start
        })
        .count();
    println!(
        "SCAN {label}: requests={requests} peak_jobs={} stale_overlapping={} stale_age_median_ms={:.2} stale_age_max_ms={:.2} latest_start_delay_ms={:.2} latest_finish_ms={:.2} old_active_at_latest_start={} messages={} stale_messages={} queue_peak={}",
        peak.load(Ordering::SeqCst),
        stale_ages.len(),
        stale_ages
            .get(stale_ages.len() / 2)
            .map_or(0.0, |v| v.as_secs_f64() * 1000.0),
        stale_ages.last().map_or(0.0, |v| v.as_secs_f64() * 1000.0),
        latest_start.duration_since(latest_issue).as_secs_f64() * 1000.0,
        latest_finish.duration_since(latest_issue).as_secs_f64() * 1000.0,
        active_old_at_latest_start,
        messages,
        rejected,
        queue_peak.load(Ordering::SeqCst)
    );
    assert_eq!(messages, requests);
}

fn measure_search(roots: &[PathBuf]) {
    let mut state = library_search::LibrarySearchState::default();
    let (sender, receiver) = AppSender::channel();
    let (times_tx, times_rx) = mpsc::channel();
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut issued = Vec::new();
    for root in roots {
        state.reset();
        let request = state.begin(root).unwrap();
        issued.push((request.id, Instant::now()));
        let tx = times_tx.clone();
        let sender = sender.clone();
        let active = active.clone();
        let peak = peak.clone();
        spawn_background(move || {
            let start = Instant::now();
            peak.fetch_max(active.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
            let result = crate::bookshelf::search::SearchIndex::build(&request.root)
                .map_err(|e| e.to_string());
            let finish = Instant::now();
            sender.input(Msg::LibrarySearchBuildFinished {
                request_id: request.id,
                root: request.root,
                result,
            });
            active.fetch_sub(1, Ordering::SeqCst);
            tx.send((request.id, start, finish)).unwrap();
        });
        std::thread::sleep(Duration::from_millis(2));
    }
    drop(times_tx);
    let completed: Vec<_> = times_rx.iter().collect();
    let (_, latest_start, latest_finish) = completed
        .iter()
        .find(|(id, _, _)| *id == issued.last().unwrap().0)
        .unwrap();
    let old_at_start = completed
        .iter()
        .filter(|(id, start, finish)| {
            *id != issued.last().unwrap().0 && *start <= *latest_start && *finish > *latest_start
        })
        .count();
    let stale_max = completed
        .iter()
        .filter(|(id, _, _)| *id != issued.last().unwrap().0)
        .map(|(id, _, finish)| {
            finish.saturating_duration_since(
                issued
                    .iter()
                    .find(|(issued_id, _)| *issued_id == id + 1)
                    .unwrap()
                    .1,
            )
        })
        .max()
        .unwrap();
    let mut messages = 0;
    let mut rejected = 0;
    while let Ok(message) = receiver.try_recv() {
        if let Msg::LibrarySearchBuildFinished {
            request_id,
            root,
            result,
        } = message
        {
            messages += 1;
            if !state.apply_build(request_id, &root, result) {
                rejected += 1;
            }
        }
    }
    println!(
        "SEARCH_BUILD: requests={} peak_jobs={} old_active_at_latest_start={} stale_max_ms={:.2} latest_start_delay_ms={:.2} latest_finish_ms={:.2} messages={} rejected={}",
        roots.len(),
        peak.load(Ordering::SeqCst),
        old_at_start,
        stale_max.as_secs_f64() * 1000.0,
        latest_start
            .duration_since(issued.last().unwrap().1)
            .as_secs_f64()
            * 1000.0,
        latest_finish
            .duration_since(issued.last().unwrap().1)
            .as_secs_f64()
            * 1000.0,
        messages,
        rejected
    );
    assert_eq!(messages, roots.len());
    assert_eq!(rejected, roots.len() - 1);
}

fn measure_queue() {
    for (count, consumer_ms) in [(10, 0), (100, 1), (300, 1)] {
        let (sender, receiver) = AppSender::channel();
        let mut issued = Vec::new();
        let begin = Instant::now();
        for _ in 0..count {
            issued.push(Instant::now());
            sender.input(Msg::OpenFile);
        }
        let producer_ms = begin.elapsed().as_secs_f64() * 1000.0;
        let peak = receiver.len();
        let mut max_age = Duration::ZERO;
        let mut total_age = Duration::ZERO;
        for sent in issued {
            receiver.recv_blocking().unwrap();
            let age = sent.elapsed();
            max_age = max_age.max(age);
            total_age += age;
            if consumer_ms > 0 {
                std::thread::sleep(Duration::from_millis(consumer_ms));
            }
        }
        println!(
            "QUEUE: produced={count} producer_ms={producer_ms:.2} consumer_ms={consumer_ms} peak={peak} mean_age_ms={:.2} max_age_ms={:.2}",
            total_age.as_secs_f64() * 1000.0 / count as f64,
            max_age.as_secs_f64() * 1000.0
        );
        assert_eq!(receiver.len(), 0);
    }
}

fn measure_real_cover_cost(root: &Path) {
    fs::create_dir_all(root).unwrap();
    let source = root.join("source.png");
    let image = image::RgbImage::from_fn(1800, 2400, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8])
    });
    image.save(&source).unwrap();
    let bytes = fs::read(&source).unwrap();
    let decode_start = Instant::now();
    let decoded = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .unwrap()
        .into_rgb8();
    let decode_elapsed = decode_start.elapsed();
    drop(decoded);
    let full_start = Instant::now();
    let thumbnail = crate::bookshelf::thumbnail::generate_from_bytes(&bytes).unwrap();
    let full_elapsed = full_start.elapsed();
    let cache = crate::bookshelf::cache::BookshelfThumbnailCache::new(root.join("phase-cache"));
    let cache_start = Instant::now();
    cache
        .generate_and_cache(Default::default(), &source)
        .unwrap();
    let cache_elapsed = cache_start.elapsed();
    println!(
        "COVER_PHASES: decode_ms={:.2} generate_ms={:.2} generate_cache_ms={:.2} result={}x{}",
        decode_elapsed.as_secs_f64() * 1000.0,
        full_elapsed.as_secs_f64() * 1000.0,
        cache_elapsed.as_secs_f64() * 1000.0,
        thumbnail.width,
        thumbnail.height,
    );
    let (tx, rx) = mpsc::channel();
    for n in 0..8 {
        let path = root.join(format!("cover{n}.png"));
        fs::copy(&source, &path).unwrap();
        let tx = tx.clone();
        spawn_background(move || {
            let start = Instant::now();
            let result = generate_and_cache_bookshelf_thumbnail(Default::default(), &path);
            tx.send((n, start.elapsed(), result.is_ok())).unwrap();
        });
    }
    drop(tx);
    let mut times: Vec<_> = rx.iter().collect();
    times.sort_by_key(|(_, duration, _)| *duration);
    assert!(times.iter().all(|(_, _, ok)| *ok));
    println!(
        "COVER_REAL: sources=8 png=1800x2400 size_bytes={} concurrent=8 min_ms={:.2} median_ms={:.2} max_ms={:.2}",
        fs::metadata(&source).unwrap().len(),
        times[0].1.as_secs_f64() * 1000.0,
        times[4].1.as_secs_f64() * 1000.0,
        times[7].1.as_secs_f64() * 1000.0
    );
}

fn measure_real_search_generation_block(root: &Path, cooperative_cancel: bool) {
    use super::library_search_thumbnail::{
        LibrarySearchThumbnailController, SearchThumbnailDemandEvaluation,
    };
    let source = root.join("source.png");
    let paths: Vec<_> = (0..3)
        .map(|n| {
            let path = root.join(format!("blocked{n}.png"));
            fs::copy(&source, &path).unwrap();
            path
        })
        .collect();
    let mut scheduler = LibrarySearchThumbnailController::default();
    let old = scheduler.begin_generation();
    let cache =
        scheduler.update_demand(SearchThumbnailDemandEvaluation::Ready(paths[..2].to_vec()));
    let mut old_jobs = Vec::new();
    for job in cache.cache_loads {
        old_jobs.extend(
            scheduler
                .complete_cache_load(old, &job.source, false)
                .jobs
                .generations,
        );
    }
    assert_eq!(old_jobs.len(), 2);
    let old_cancel = scheduler.cancellation_token();
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let (tx, rx) = mpsc::channel();
    let (decode_tx, decode_rx) = mpsc::channel();
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    for job in old_jobs {
        let barrier = barrier.clone();
        let tx = tx.clone();
        let decode_tx = decode_tx.clone();
        let cancel = old_cancel.clone();
        let active = active.clone();
        let peak = peak.clone();
        spawn_background(move || {
            barrier.wait();
            let start = Instant::now();
            peak.fetch_max(active.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
            let checks = Cell::new(0);
            let cancelled_phase = Cell::new(None);
            let result = generate_and_cache_bookshelf_thumbnail_with_cancel(
                Default::default(),
                &job.source,
                &|| {
                    let phase = checks.get() + 1;
                    checks.set(phase);
                    if phase == 6 {
                        // The sixth checkpoint is immediately before image decode.
                        // Return false so the codec call starts even if the main
                        // thread switches generation after receiving this signal.
                        decode_tx.send(()).unwrap();
                        false
                    } else {
                        let obsolete = cooperative_cancel && cancel.cancelled();
                        if obsolete && cancelled_phase.get().is_none() {
                            cancelled_phase.set(Some(phase));
                        }
                        obsolete
                    }
                },
            )
            .map(|thumbnail| thumbnail.is_none());
            active.fetch_sub(1, Ordering::SeqCst);
            tx.send((job, start, Instant::now(), result, cancelled_phase.get()))
                .unwrap();
        });
    }
    barrier.wait();
    for _ in 0..2 {
        decode_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    assert_eq!(active.load(Ordering::SeqCst), 2);
    let switched = Instant::now();
    let current = scheduler.begin_generation();
    let cache = scheduler.update_demand(SearchThumbnailDemandEvaluation::Ready(vec![
        paths[2].clone(),
    ]));
    let blocked = scheduler.complete_cache_load(current, &cache.cache_loads[0].source, false);
    assert!(blocked.jobs.generations.is_empty());
    let (old_job, old_start, old_finish, old_result, old_phase) = rx.recv().unwrap();
    assert!(old_result.is_ok());
    let released = scheduler.complete_generation(old, &old_job.source, true);
    assert_eq!(released.jobs.generations.len(), 1);
    let latest_job = &released.jobs.generations[0];
    let latest_started = Instant::now();
    peak.fetch_max(active.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
    let latest_ok = generate_and_cache_bookshelf_thumbnail_with_cancel(
        Default::default(),
        &latest_job.source,
        &|| cooperative_cancel && scheduler.cancellation_token().cancelled(),
    )
    .is_ok();
    active.fetch_sub(1, Ordering::SeqCst);
    let latest_finished = Instant::now();
    assert!(latest_ok);
    let remaining = rx.recv().unwrap();
    assert!(remaining.3.is_ok());
    let cancelled_jobs = usize::from(old_result.unwrap()) + usize::from(remaining.3.unwrap());
    let cancelled_after_decode =
        usize::from(old_phase == Some(7)) + usize::from(remaining.4 == Some(7));
    if cooperative_cancel {
        assert_eq!(cancelled_jobs, 2);
        assert_eq!(cancelled_after_decode, 2);
    }
    println!(
        "SEARCH_REAL_GENERATION_BLOCK: cooperative_cancel={} old_jobs=2 old_first_duration_ms={:.2} old_remaining_duration_ms={:.2} old_survival_after_switch_ms={:.2} latest_wait_to_start_ms={:.2} latest_finish_ms={:.2} latest_generation_ms={:.2} physical_peak={} current_peak=1 stale_peak=2 decode_starts=3 cancelled_jobs={} cancelled_after_decode={} completion_events=3 stale_completions=2",
        cooperative_cancel,
        old_finish.duration_since(old_start).as_secs_f64() * 1000.0,
        remaining.2.duration_since(remaining.1).as_secs_f64() * 1000.0,
        old_finish.duration_since(switched).as_secs_f64() * 1000.0,
        latest_started.duration_since(switched).as_secs_f64() * 1000.0,
        latest_finished.duration_since(switched).as_secs_f64() * 1000.0,
        latest_finished.duration_since(latest_started).as_secs_f64() * 1000.0,
        peak.load(Ordering::SeqCst),
        cancelled_jobs,
        cancelled_after_decode,
    );
}

fn measure_scheduler_lanes() {
    use super::cover_scheduler::{CoverLoadScheduler, CoverSchedulerJob};
    use super::library_search_thumbnail::{
        LibrarySearchThumbnailController, SearchThumbnailDemandEvaluation,
    };
    use super::library_thumbnail::{
        LibraryThumbnailController, ThumbnailDemand, ThumbnailDemandEvaluation, ThumbnailSourceKind,
    };
    let hold = Duration::from_millis(80);

    let mut library = LibraryThumbnailController::default();
    let old = library.begin_directory();
    let old_demand = (0..8)
        .map(|n| {
            ThumbnailDemand::new(
                PathBuf::from(format!("old{n}")),
                ThumbnailSourceKind::BookCover,
            )
        })
        .collect();
    let old_jobs = library.update_demand(ThumbnailDemandEvaluation::Ready(old_demand));
    let new = library.begin_directory();
    let latest = PathBuf::from("latest");
    let waiting = library.update_demand(ThumbnailDemandEvaluation::Ready(vec![
        ThumbnailDemand::new(latest.clone(), ThumbnailSourceKind::BookCover),
    ]));
    let switched = Instant::now();
    std::thread::sleep(hold);
    let completion = library.complete_cache_load_with_kind(
        old,
        old_jobs.cache_loads[0].cohort_id,
        &old_jobs.cache_loads[0].source,
        ThumbnailSourceKind::BookCover,
        false,
    );
    println!(
        "LANE library cache: old={} latest_initial={} latest_after_first_old={} wait_ms={:.2}",
        old_jobs.cache_loads.len(),
        waiting.cache_loads.len(),
        completion.jobs.cache_loads.len(),
        switched.elapsed().as_secs_f64() * 1000.0
    );
    assert_eq!(old_jobs.cache_loads.len(), 8);
    assert_eq!(waiting.cache_loads.len(), 0);
    assert_eq!(completion.jobs.cache_loads.len(), 1);
    assert!(!completion.accepted);
    assert_eq!(new, old + 1);

    let mut library = LibraryThumbnailController::default();
    let old = library.begin_directory();
    let demand = (0..2)
        .map(|n| {
            ThumbnailDemand::new(
                PathBuf::from(format!("oldgen{n}")),
                ThumbnailSourceKind::BookCover,
            )
        })
        .collect();
    let cache = library.update_demand(ThumbnailDemandEvaluation::Ready(demand));
    let mut generations = Vec::new();
    for job in cache.cache_loads {
        generations.extend(
            library
                .complete_cache_load_with_kind(old, job.cohort_id, &job.source, job.kind, false)
                .jobs
                .generations,
        );
    }
    let current = library.begin_directory();
    let latest = PathBuf::from("latestgen");
    let cache = library.update_demand(ThumbnailDemandEvaluation::Ready(vec![
        ThumbnailDemand::new(latest.clone(), ThumbnailSourceKind::BookCover),
    ]));
    let current_cache = &cache.cache_loads[0];
    let blocked = library.complete_cache_load_with_kind(
        current,
        current_cache.cohort_id,
        &latest,
        ThumbnailSourceKind::BookCover,
        false,
    );
    let switched = Instant::now();
    std::thread::sleep(hold);
    let released = library.complete_generation_with_kind(
        old,
        &generations[0].source,
        ThumbnailSourceKind::BookCover,
        true,
    );
    println!(
        "LANE library generation: old={} latest_initial={} latest_after_first_old={} wait_ms={:.2}",
        generations.len(),
        blocked.jobs.generations.len(),
        released.jobs.generations.len(),
        switched.elapsed().as_secs_f64() * 1000.0
    );
    assert_eq!(generations.len(), 2);
    assert!(blocked.jobs.generations.is_empty());
    assert_eq!(released.jobs.generations.len(), 1);

    let mut search = LibrarySearchThumbnailController::default();
    let old = search.begin_generation();
    let cache = search.update_demand(SearchThumbnailDemandEvaluation::Ready(
        (0..8).map(|n| PathBuf::from(format!("old{n}"))).collect(),
    ));
    search.begin_generation();
    let latest = PathBuf::from("latest");
    let waiting = search.update_demand(SearchThumbnailDemandEvaluation::Ready(vec![latest]));
    let switched = Instant::now();
    std::thread::sleep(hold);
    let released = search.complete_cache_load(old, &cache.cache_loads[0].source, false);
    println!(
        "LANE search cache: old={} latest_initial={} latest_after_first_old={} wait_ms={:.2}",
        cache.cache_loads.len(),
        waiting.cache_loads.len(),
        released.jobs.cache_loads.len(),
        switched.elapsed().as_secs_f64() * 1000.0
    );
    assert_eq!(cache.cache_loads.len(), 8);
    assert!(waiting.cache_loads.is_empty());
    assert_eq!(released.jobs.cache_loads.len(), 1);

    let mut search = LibrarySearchThumbnailController::default();
    let old = search.begin_generation();
    let cache = search.update_demand(SearchThumbnailDemandEvaluation::Ready(vec![
        "g0".into(),
        "g1".into(),
    ]));
    let mut generations = Vec::new();
    for job in cache.cache_loads {
        generations.extend(
            search
                .complete_cache_load(old, &job.source, false)
                .jobs
                .generations,
        );
    }
    let current = search.begin_generation();
    let latest = PathBuf::from("latestgen");
    let cache = search.update_demand(SearchThumbnailDemandEvaluation::Ready(vec![latest.clone()]));
    let blocked = search.complete_cache_load(current, &cache.cache_loads[0].source, false);
    let switched = Instant::now();
    std::thread::sleep(hold);
    let released = search.complete_generation(old, &generations[0].source, true);
    println!(
        "LANE search generation: old={} latest_initial={} latest_after_first_old={} wait_ms={:.2}",
        generations.len(),
        blocked.jobs.generations.len(),
        released.jobs.generations.len(),
        switched.elapsed().as_secs_f64() * 1000.0
    );
    assert_eq!(generations.len(), 2);
    assert!(blocked.jobs.generations.is_empty());
    assert_eq!(released.jobs.generations.len(), 1);

    for scope in ["history", "favorites"] {
        let mut scheduler = CoverLoadScheduler::<u32, u32>::default();
        scheduler.replace_demand(0..8);
        let old = scheduler.generation();
        let jobs = scheduler.schedule_jobs();
        scheduler.invalidate();
        scheduler.replace_demand([99]);
        let waiting = scheduler.schedule_jobs();
        let switched = Instant::now();
        std::thread::sleep(hold);
        let key = match jobs[0] {
            CoverSchedulerJob::LoadCache { key, .. } => key,
            _ => unreachable!(),
        };
        assert!(!scheduler.cache_finished(old, &key));
        let released = scheduler.schedule_jobs();
        println!(
            "LANE {scope} cache: old={} latest_initial={} latest_after_first_old={} wait_ms={:.2}",
            jobs.len(),
            waiting.len(),
            released.len(),
            switched.elapsed().as_secs_f64() * 1000.0
        );
        assert_eq!(jobs.len(), 8);
        assert!(waiting.is_empty());
        assert_eq!(released.len(), 1);

        let mut scheduler = CoverLoadScheduler::<u32, u32>::default();
        scheduler.replace_demand([0, 1]);
        scheduler.enqueue_generation(0, 0);
        scheduler.enqueue_generation(1, 1);
        let old = scheduler.generation();
        let jobs = scheduler.schedule_jobs();
        scheduler.invalidate();
        scheduler.replace_demand([99]);
        scheduler.enqueue_generation(99, 99);
        let waiting = scheduler.schedule_jobs();
        let switched = Instant::now();
        std::thread::sleep(hold);
        let key = match jobs[0] {
            CoverSchedulerJob::Generate { key, .. } => key,
            _ => unreachable!(),
        };
        assert!(!scheduler.generation_finished(old, &key));
        let released = scheduler.schedule_jobs();
        println!(
            "LANE {scope} generation: old={} latest_initial={} latest_after_first_old={} wait_ms={:.2}",
            jobs.len(),
            waiting.len(),
            released.len(),
            switched.elapsed().as_secs_f64() * 1000.0
        );
        assert_eq!(jobs.len(), 2);
        assert!(waiting.is_empty());
        assert_eq!(released.len(), 1);
    }
}

#[test]
#[ignore = "manual headless performance measurement"]
fn issue76_headless_measurement() {
    let temp = tempfile::tempdir().unwrap();
    let scan_roots = [100, 200, 400].map(|count| {
        let root = temp.path().join(format!("scan{count}"));
        fixture(&root, count)
    });
    let search_roots = [100, 200, 400].map(|count| {
        let root = temp.path().join(format!("search{count}"));
        fixture(&root, count);
        root
    });
    measure_scan("normal", &scan_roots[0], 4, Duration::from_millis(30));
    measure_scan("fast", &scan_roots[1], 12, Duration::from_millis(2));
    measure_scan("heavy", &scan_roots[2], 8, Duration::ZERO);
    measure_search(&search_roots);
    let solo_scan = fixture(&temp.path().join("solo_scan"), 400);
    let start = Instant::now();
    crate::bookshelf::scan_directory(&solo_scan[3]).unwrap();
    println!(
        "SCAN_SOLO_COLD: archives=400 duration_ms={:.2}",
        start.elapsed().as_secs_f64() * 1000.0
    );
    let solo_search = temp.path().join("solo_search");
    fixture(&solo_search, 400);
    let start = Instant::now();
    crate::bookshelf::search::SearchIndex::build(&solo_search).unwrap();
    println!(
        "SEARCH_SOLO_COLD: archives=1600 duration_ms={:.2}",
        start.elapsed().as_secs_f64() * 1000.0
    );
    measure_real_cover_cost(&temp.path().join("covers"));
    measure_real_search_generation_block(&temp.path().join("covers"), false);
    measure_queue();
    measure_scheduler_lanes();
}

#[test]
#[ignore = "manual headless Cover cancellation measurement"]
fn issue80_cover_cancel_measurement() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root).unwrap();
    let image = image::RgbImage::from_fn(1800, 2400, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8])
    });
    image.save(root.join("source.png")).unwrap();
    measure_real_search_generation_block(root, false);
    measure_real_search_generation_block(root, true);
}
