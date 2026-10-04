use super::*;
use crate::archive::cover_image_in_folder;
use crate::archive::navigation::find_sibling_file;
use std::fs;
use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn timed(operation: impl FnOnce()) -> Duration {
    let start = Instant::now();
    operation();
    start.elapsed()
}

fn measure(mut operation: impl FnMut() -> Duration) -> (u128, u128) {
    operation();
    let mut samples = (0..9).map(|_| operation()).collect::<Vec<Duration>>();
    samples.sort_unstable();
    (samples[4].as_micros(), samples[8].as_micros())
}

fn report(label: &str, size: usize, result: (u128, u128)) {
    println!(
        "{label} n={size}: median={} us max={} us",
        result.0, result.1
    );
}

#[test]
#[ignore = "manual timing fixture; elapsed time is not a pass/fail condition"]
fn history_main_thread_io_timings() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("books");
    let identity_parent = root.join("identity-work");
    let image_folder = identity_parent.join("current");
    fs::create_dir_all(&image_folder).unwrap();
    let image_path = image_folder.join("001.jpg");
    fs::write(&image_path, []).unwrap();

    let sibling_parent = root.join("sibling-work");
    fs::create_dir_all(&sibling_parent).unwrap();
    let sibling_path = sibling_parent.join("9999.cbz");
    fs::write(&sibling_path, []).unwrap();

    let image_sibling_parent = root.join("image-sibling-work");
    let current_image_folder = image_sibling_parent.join("current");
    let next_image_folder = image_sibling_parent.join("next");
    fs::create_dir_all(&current_image_folder).unwrap();
    fs::create_dir_all(&next_image_folder).unwrap();
    let current_image = current_image_folder.join("001.jpg");
    fs::write(&current_image, []).unwrap();

    let history_parent = root.join("history-work");
    fs::create_dir_all(&history_parent).unwrap();
    let mut entries = Vec::new();
    for size in [0, 10, 200, 1000, 2000] {
        let previous = entries.len();
        for index in previous..size {
            let folder = history_parent.join(format!("work-{index:04}/book"));
            fs::create_dir_all(&folder).unwrap();
            let document = folder.join("001.jpg");
            fs::write(&document, []).unwrap();
            entries.push(HistoryEntry {
                identity: HistoryIdentity::BookshelfWork(folder.clone()),
                work_path: Some(folder.clone()),
                title: "book".to_owned(),
                last_document_path: document,
                page_index: 0,
                page_count: 1,
                at_document_end: false,
                last_viewed_unix_ms: 1,
            });
        }
        for index in previous..size {
            fs::create_dir(identity_parent.join(format!("empty-{index:04}"))).unwrap();
            fs::write(sibling_parent.join(format!("{index:04}.cbz")), []).unwrap();
            fs::write(next_image_folder.join(format!("{index:04}.jpg")), []).unwrap();
        }

        report(
            "identity",
            size,
            measure(|| {
                timed(|| {
                    black_box(history_identity_for_document(&image_path, Some(&root)));
                })
            }),
        );
        report(
            "record_with_normalization",
            size,
            measure(|| {
                let mut history = HistoryStore {
                    entries: entries.clone(),
                    storage_path: PathBuf::new(),
                };
                timed(|| {
                    black_box(history.record_document_state_at(
                        &root.join("record/new.cbz"),
                        Some(&root),
                        0,
                        1,
                        false,
                        1,
                    ));
                })
            }),
        );
        report(
            "end_sibling_lookup",
            size,
            measure(|| {
                timed(|| {
                    black_box(find_sibling_file(&sibling_path, true));
                })
            }),
        );
        report(
            "end_image_folder_lookup",
            size,
            measure(|| {
                timed(|| {
                    black_box(find_sibling_file(&current_image, true));
                })
            }),
        );
        report(
            "history_cover_source_lookup",
            size,
            measure(|| {
                timed(|| {
                    black_box(cover_image_in_folder(&next_image_folder));
                })
            }),
        );
        let history = HistoryStore {
            entries: entries.clone(),
            storage_path: directory.path().join("history.ini"),
        };
        report(
            "save",
            size,
            measure(|| {
                timed(|| {
                    history.save_to_path(&history.storage_path).unwrap();
                })
            }),
        );
    }
}
