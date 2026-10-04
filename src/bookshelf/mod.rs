pub(crate) mod cache;
pub(crate) mod search;
pub(crate) mod snapshot;
mod sort;
pub(crate) mod thumbnail;

pub(crate) use sort::{LibrarySortDirection, LibrarySortKey};

use crate::archive::{
    is_bookshelf_viewable_path, is_ignored_filesystem_path, is_image_document_entry_path,
    is_image_ext,
};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs::{self, DirEntry};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const MAX_PREVIEW_CANDIDATES: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BookshelfDirectory {
    pub(crate) path: PathBuf,
    pub(crate) is_image_document_directory: bool,
    pub(crate) direct_files: Vec<PathBuf>,
    pub(crate) direct_file_metadata: HashMap<PathBuf, BookshelfItemMetadata>,
    pub(crate) child_shelves: Vec<ChildShelf>,
}

#[derive(Debug, Clone)]
pub(crate) struct ChildShelf {
    pub(crate) path: PathBuf,
    pub(crate) preview_candidates: Vec<PathBuf>,
    // Compatibility-only legacy value. Folder preview no longer displays a
    // total item count, so scan work stops once the five visible candidates
    // required by the UI are known.
    pub(crate) preview_item_count: usize,
    pub(crate) image_document_entry: Option<PathBuf>,
    pub(crate) image_document_cover: Option<PathBuf>,
    pub(crate) metadata: BookshelfItemMetadata,
}

impl PartialEq for ChildShelf {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
            && self.preview_candidates == other.preview_candidates
            && self.image_document_entry == other.image_document_entry
            && self.image_document_cover == other.image_document_cover
            && self.metadata == other.metadata
    }
}

impl Eq for ChildShelf {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BookshelfBook<'a> {
    File(&'a PathBuf),
    ImageFolder(&'a ChildShelf),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BookshelfItemMetadata {
    pub(crate) modified: Option<SystemTime>,
    pub(crate) created: Option<SystemTime>,
}

impl BookshelfDirectory {
    pub(crate) fn sorted_child_shelves(
        &self,
        key: LibrarySortKey,
        direction: LibrarySortDirection,
    ) -> Vec<&ChildShelf> {
        let mut shelves = self.child_shelves.iter().collect::<Vec<_>>();
        sort_items(
            &mut shelves,
            key,
            direction,
            |shelf| shelf.path.as_path(),
            |shelf| shelf.metadata,
        );
        shelves
    }

    pub(crate) fn sorted_books(
        &self,
        key: LibrarySortKey,
        direction: LibrarySortDirection,
    ) -> Vec<BookshelfBook<'_>> {
        let mut books = self
            .direct_files
            .iter()
            .map(BookshelfBook::File)
            .chain(
                self.child_shelves
                    .iter()
                    .filter(|shelf| shelf.image_document_entry.is_some())
                    .map(BookshelfBook::ImageFolder),
            )
            .collect::<Vec<_>>();
        sort_items(
            &mut books,
            key,
            direction,
            |book| match book {
                BookshelfBook::File(path) => path.as_path(),
                BookshelfBook::ImageFolder(shelf) => shelf.path.as_path(),
            },
            |book| match book {
                BookshelfBook::File(path) => self
                    .direct_file_metadata
                    .get(path.as_path())
                    .copied()
                    .unwrap_or_default(),
                BookshelfBook::ImageFolder(shelf) => shelf.metadata,
            },
        );
        books
    }
}

pub(crate) fn scan_directory(path: &Path) -> io::Result<BookshelfDirectory> {
    let entries = read_directory_entries(path)?;
    let is_image_document_directory = first_direct_image_path_from_entries(&entries).is_some();
    let mut direct_files = Vec::new();
    let mut direct_file_metadata = HashMap::new();
    let mut child_shelves = Vec::new();

    for entry in entries {
        let entry_path = entry.path();
        if is_ignored_filesystem_path(&entry_path) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        if file_type.is_file()
            && (is_bookshelf_item_path(&entry_path)
                || (is_image_document_directory && crate::archive::is_cover_image(&entry_path)))
        {
            direct_file_metadata.insert(entry_path.clone(), item_metadata(&entry));
            direct_files.push(entry_path);
        } else if file_type.is_dir() {
            let Ok(child_entries) = read_directory_entries(&entry_path) else {
                continue;
            };
            let preview_candidates = preview_candidates(&child_entries);
            if !preview_candidates.is_empty() {
                let image_document_entry = first_direct_image_path_from_entries(&child_entries);
                let image_document_cover = cover_image_from_entries(&child_entries);
                child_shelves.push(ChildShelf {
                    path: entry_path,
                    preview_item_count: preview_candidates.len(),
                    preview_candidates,
                    image_document_entry,
                    image_document_cover,
                    metadata: item_metadata(&entry),
                });
            }
        }
    }

    Ok(BookshelfDirectory {
        path: path.to_path_buf(),
        is_image_document_directory,
        direct_files,
        direct_file_metadata,
        child_shelves,
    })
}

fn item_metadata(entry: &DirEntry) -> BookshelfItemMetadata {
    let Ok(metadata) = entry.metadata() else {
        return BookshelfItemMetadata::default();
    };
    BookshelfItemMetadata {
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
    }
}

fn sort_items<T>(
    items: &mut [T],
    key: LibrarySortKey,
    direction: LibrarySortDirection,
    path: impl Fn(&T) -> &Path,
    metadata: impl Fn(&T) -> BookshelfItemMetadata,
) {
    items.sort_by(|left, right| {
        compare_items(
            path(left),
            metadata(left),
            path(right),
            metadata(right),
            key,
            direction,
        )
    });
}

fn compare_items(
    left_path: &Path,
    left_metadata: BookshelfItemMetadata,
    right_path: &Path,
    right_metadata: BookshelfItemMetadata,
    key: LibrarySortKey,
    direction: LibrarySortDirection,
) -> Ordering {
    let compare_names = || natural_path_compare(left_path, right_path);
    if key == LibrarySortKey::Name {
        let order = compare_names();
        return match direction {
            LibrarySortDirection::Ascending => order,
            LibrarySortDirection::Descending => order.reverse(),
        };
    }

    let (left_time, right_time) = match key {
        LibrarySortKey::Modified => (left_metadata.modified, right_metadata.modified),
        LibrarySortKey::Created => (left_metadata.created, right_metadata.created),
        LibrarySortKey::Name => unreachable!(),
    };
    match (left_time, right_time) {
        (Some(left), Some(right)) => {
            let order = left.cmp(&right);
            let order = match direction {
                LibrarySortDirection::Ascending => order,
                LibrarySortDirection::Descending => order.reverse(),
            };
            order.then_with(compare_names)
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => compare_names(),
    }
}

fn natural_path_compare(left: &Path, right: &Path) -> Ordering {
    natord::compare(
        &left
            .file_name()
            .unwrap_or(left.as_os_str())
            .to_string_lossy(),
        &right
            .file_name()
            .unwrap_or(right.as_os_str())
            .to_string_lossy(),
    )
}

fn preview_candidates(entries: &[DirEntry]) -> Vec<PathBuf> {
    let mut candidates = Vec::with_capacity(MAX_PREVIEW_CANDIDATES);
    for entry in entries {
        if candidates.len() == MAX_PREVIEW_CANDIDATES {
            break;
        }
        let entry_path = entry.path();
        if is_ignored_filesystem_path(&entry_path) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let candidate = if file_type.is_file() && is_bookshelf_item_path(&entry_path) {
            Some(entry_path)
        } else if file_type.is_dir() {
            first_viewable_file(&entry_path)
        } else {
            None
        };
        if let Some(candidate) = candidate {
            candidates.push(candidate);
        }
    }
    candidates
}

fn first_direct_image_path_from_entries(entries: &[DirEntry]) -> Option<PathBuf> {
    entries.iter().find_map(|entry| {
        let file_type = entry.file_type().ok()?;
        let path = entry.path();
        (file_type.is_file() && is_image_document_entry_path(&path)).then_some(path)
    })
}

fn cover_image_from_entries(entries: &[DirEntry]) -> Option<PathBuf> {
    let mut first_image = None;
    for entry in entries {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let path = entry.path();
        if crate::archive::is_cover_image(&path) {
            return Some(path);
        }
        if first_image.is_none() && is_image_document_entry_path(&path) {
            first_image = Some(path);
        }
    }
    first_image
}

fn first_viewable_file(path: &Path) -> Option<PathBuf> {
    let entries = read_visible_entries(path).ok()?;
    for entry in entries {
        let entry_path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_file() && is_bookshelf_item_path(&entry_path) {
            return Some(entry_path);
        }
        if file_type.is_dir()
            && let Some(candidate) = first_viewable_file(&entry_path)
        {
            return Some(candidate);
        }
    }
    None
}

pub(super) fn is_bookshelf_item_path(path: &Path) -> bool {
    if is_image_ext(path) {
        is_image_document_entry_path(path)
    } else {
        is_bookshelf_viewable_path(path).unwrap_or(false)
    }
}

fn read_directory_entries(path: &Path) -> io::Result<Vec<DirEntry>> {
    let mut entries = fs::read_dir(path)?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        natord::compare(
            &left.file_name().to_string_lossy(),
            &right.file_name().to_string_lossy(),
        )
    });
    Ok(entries)
}

pub(super) fn read_visible_entries(path: &Path) -> io::Result<Vec<DirEntry>> {
    Ok(read_directory_entries(path)?
        .into_iter()
        .filter(|entry| !is_ignored_filesystem_path(&entry.path()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::Duration;

    fn touch(path: &Path) {
        fs::write(path, []).unwrap();
    }

    fn write_zip(path: &Path, entry_names: &[&str]) {
        let mut archive = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for entry_name in entry_names {
            archive
                .start_file(*entry_name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(b"content").unwrap();
        }
        archive.finish().unwrap();
    }

    fn names(paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    fn sorted_test_names(
        items: &[(&str, Option<u64>, Option<u64>)],
        key: LibrarySortKey,
        direction: LibrarySortDirection,
    ) -> Vec<String> {
        let epoch = SystemTime::UNIX_EPOCH;
        let mut items = items
            .iter()
            .map(|(name, modified, created)| {
                (
                    PathBuf::from(name),
                    BookshelfItemMetadata {
                        modified: modified.map(|seconds| epoch + Duration::from_secs(seconds)),
                        created: created.map(|seconds| epoch + Duration::from_secs(seconds)),
                    },
                )
            })
            .collect::<Vec<_>>();
        sort_items(
            &mut items,
            key,
            direction,
            |item| item.0.as_path(),
            |item| item.1,
        );
        items
            .into_iter()
            .map(|item| item.0.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn direct_files_include_images_and_viewable_archives_in_natural_order() {
        let root = tempfile::tempdir().unwrap();
        for name in ["2.webp", "1.PNG", "3.JPG", "4.JPEG", "memo.txt"] {
            touch(&root.path().join(name));
        }
        write_zip(&root.path().join("10.ZIP"), &["page.jpg"]);
        write_zip(&root.path().join("5.CBZ"), &["nested.rar"]);

        let result = scan_directory(root.path()).unwrap();

        assert_eq!(
            names(&result.direct_files),
            ["1.PNG", "2.webp", "3.JPG", "4.JPEG", "5.CBZ", "10.ZIP"]
        );
    }

    #[test]
    fn child_shelves_distinguish_image_documents_from_container_folders() {
        let root = tempfile::tempdir().unwrap();
        let series = root.path().join("series");
        let volume = series.join("01");
        fs::create_dir_all(&volume).unwrap();
        touch(&series.join("Cover.JPEG"));
        for name in ["10.jpg", "2.jpg", "1.jpg"] {
            touch(&volume.join(name));
        }

        let root_scan = scan_directory(root.path()).unwrap();
        assert!(!root_scan.is_image_document_directory);
        assert_eq!(root_scan.child_shelves[0].path, series);
        assert_eq!(root_scan.child_shelves[0].image_document_entry, None);

        let series_scan = scan_directory(&root_scan.child_shelves[0].path).unwrap();
        assert!(!series_scan.is_image_document_directory);
        assert_eq!(
            series_scan.child_shelves[0].image_document_entry,
            Some(volume.join("1.jpg"))
        );

        let volume_scan = scan_directory(&volume).unwrap();
        assert!(volume_scan.is_image_document_directory);
    }

    #[test]
    fn cover_images_are_included_only_when_scanning_an_image_book_contents() {
        let root = tempfile::tempdir().unwrap();
        for name in [
            "cover.jpg",
            "Cover.JPEG",
            "COVER.PNG",
            "CoVeR.webp",
            "folder.jpg",
            "preview.jpg",
        ] {
            touch(&root.path().join(name));
        }
        write_zip(&root.path().join("book.cbz"), &["cover.jpg"]);

        let result = scan_directory(root.path()).unwrap();

        assert_eq!(
            names(&result.direct_files),
            [
                "COVER.PNG",
                "CoVeR.webp",
                "Cover.JPEG",
                "book.cbz",
                "cover.jpg",
                "folder.jpg",
                "preview.jpg"
            ]
        );
    }

    #[test]
    fn cover_images_are_skipped_when_building_child_shelf_previews() {
        let root = tempfile::tempdir().unwrap();
        let shelf = root.path().join("work");
        let cover_only = root.path().join("cover-only");
        fs::create_dir(&shelf).unwrap();
        fs::create_dir(&cover_only).unwrap();
        touch(&shelf.join("cover.jpg"));
        touch(&shelf.join("folder.jpg"));
        touch(&shelf.join("preview.jpg"));
        touch(&cover_only.join("COVER.PNG"));

        let result = scan_directory(root.path()).unwrap();

        assert_eq!(result.child_shelves.len(), 1);
        assert_eq!(result.child_shelves[0].path, shelf);
        assert_eq!(
            names(&result.child_shelves[0].preview_candidates),
            ["folder.jpg", "preview.jpg"]
        );
    }

    #[test]
    fn archive_content_probe_filters_non_viewable_and_broken_archives() {
        let root = tempfile::tempdir().unwrap();
        touch(&root.path().join("page.png"));
        touch(&root.path().join("memo.txt"));
        touch(&root.path().join("broken.rar"));
        write_zip(&root.path().join("images.cbz"), &["pages/001.jpg"]);
        write_zip(&root.path().join("nested.zip"), &["inner.cb7", "memo.txt"]);
        write_zip(
            &root.path().join("epub-only.zip"),
            &["book01.epub", "book02.epub"],
        );
        write_zip(
            &root.path().join("text-only.zip"),
            &["setup.exe", "readme.txt"],
        );

        let result = scan_directory(root.path()).unwrap();

        assert_eq!(
            names(&result.direct_files),
            ["images.cbz", "nested.zip", "page.png"]
        );
    }

    #[test]
    fn child_shelves_are_natural_and_require_recursive_content() {
        let root = tempfile::tempdir().unwrap();
        for name in ["work10", "work2", "work1", "empty"] {
            fs::create_dir(root.path().join(name)).unwrap();
        }
        write_zip(&root.path().join("work1/1.zip"), &["1.jpg"]);
        touch(&root.path().join("work2/1.png"));
        fs::create_dir_all(root.path().join("work10/deep/deeper")).unwrap();
        write_zip(&root.path().join("work10/deep/deeper/1.cbz"), &["1.jpg"]);
        touch(&root.path().join("empty/memo.txt"));
        write_zip(&root.path().join("empty/book.zip"), &["book.epub"]);

        let result = scan_directory(root.path()).unwrap();
        let child_paths = result
            .child_shelves
            .iter()
            .map(|shelf| shelf.path.clone())
            .collect::<Vec<_>>();

        assert_eq!(names(&child_paths), ["work1", "work2", "work10"]);
    }

    #[test]
    fn previews_replace_immediate_directories_without_reordering() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        fs::create_dir_all(work.join("3-special/deep")).unwrap();
        fs::create_dir_all(work.join("4-empty")).unwrap();
        write_zip(&work.join("1.zip"), &["1.jpg"]);
        write_zip(&work.join("2.cbz"), &["1.jpg"]);
        write_zip(&work.join("3-special/10.zip"), &["1.jpg"]);
        write_zip(&work.join("3-special/2.zip"), &["1.jpg"]);
        write_zip(&work.join("3-special/deep/1.zip"), &["1.jpg"]);
        touch(&work.join("4-empty/memo.txt"));
        write_zip(&work.join("10.zip"), &["1.jpg"]);

        let result = scan_directory(root.path()).unwrap();
        let candidates = &result.child_shelves[0].preview_candidates;

        assert_eq!(
            candidates,
            &[
                work.join("1.zip"),
                work.join("2.cbz"),
                work.join("3-special/2.zip"),
                work.join("10.zip"),
            ]
        );
        assert_eq!(result.child_shelves[0].preview_item_count, 4);
    }

    #[test]
    fn preview_candidates_are_limited_to_five_in_natural_order() {
        let root = tempfile::tempdir().unwrap();
        let shelf = root.path().join("shelf");
        fs::create_dir(&shelf).unwrap();
        for number in 1..=20 {
            write_zip(&shelf.join(format!("{number}.zip")), &["1.jpg"]);
        }

        let result = scan_directory(root.path()).unwrap();

        assert_eq!(
            names(&result.child_shelves[0].preview_candidates),
            ["1.zip", "2.zip", "3.zip", "4.zip", "5.zip"]
        );
        assert_eq!(result.child_shelves[0].preview_item_count, 5);
    }

    #[test]
    fn preview_candidates_handle_one_through_three_items() {
        let root = tempfile::tempdir().unwrap();
        for count in 1..=3 {
            let shelf = root.path().join(format!("shelf{count}"));
            fs::create_dir(&shelf).unwrap();
            for number in 1..=count {
                write_zip(&shelf.join(format!("{number}.zip")), &["1.jpg"]);
            }
        }

        let result = scan_directory(root.path()).unwrap();

        assert_eq!(result.child_shelves.len(), 3);
        for (index, shelf) in result.child_shelves.iter().enumerate() {
            assert_eq!(shelf.preview_candidates.len(), index + 1);
            assert_eq!(shelf.preview_item_count, index + 1);
        }
    }

    #[test]
    fn preview_counts_direct_children_and_uses_one_representative_per_child_folder() {
        let root = tempfile::tempdir().unwrap();
        let shelf = root.path().join("series");
        fs::create_dir(&shelf).unwrap();
        for name in ["10", "2", "1"] {
            let child = shelf.join(name);
            fs::create_dir(&child).unwrap();
            write_zip(&child.join("01.zip"), &["1.jpg"]);
            write_zip(&child.join("02.zip"), &["1.jpg"]);
        }
        touch(&shelf.join("cover.jpg"));

        let result = scan_directory(root.path()).unwrap();
        let preview = &result.child_shelves[0];

        assert_eq!(
            names(&preview.preview_candidates),
            ["01.zip", "01.zip", "01.zip"]
        );
        assert_eq!(
            preview.preview_candidates,
            [
                shelf.join("1/01.zip"),
                shelf.join("2/01.zip"),
                shelf.join("10/01.zip"),
            ]
        );
        assert_eq!(preview.preview_item_count, 3);
    }

    #[test]
    fn hidden_metadata_and_directory_symlinks_are_ignored() {
        let root = tempfile::tempdir().unwrap();
        let visible = root.path().join("visible");
        fs::create_dir(&visible).unwrap();
        write_zip(&visible.join("1.zip"), &["1.jpg"]);
        fs::create_dir(root.path().join(".hidden")).unwrap();
        write_zip(&root.path().join(".hidden/1.zip"), &["1.jpg"]);
        fs::create_dir(root.path().join("__MACOSX")).unwrap();
        write_zip(&root.path().join("__MACOSX/1.zip"), &["1.jpg"]);
        touch(&root.path().join("._book.zip"));

        #[cfg(unix)]
        std::os::unix::fs::symlink(root.path(), root.path().join("loop")).unwrap();

        let result = scan_directory(root.path()).unwrap();

        assert!(result.direct_files.is_empty());
        assert_eq!(result.child_shelves.len(), 1);
        assert_eq!(result.child_shelves[0].path, visible);
    }

    #[test]
    fn deeply_nested_content_is_found_without_a_fixed_depth() {
        let root = tempfile::tempdir().unwrap();
        let mut directory = root.path().join("shelf");
        fs::create_dir(&directory).unwrap();
        for depth in 0..64 {
            directory = directory.join(format!("level{depth}"));
            fs::create_dir(&directory).unwrap();
        }
        let book = directory.join("book.zip");
        write_zip(&book, &["1.jpg"]);

        let result = scan_directory(root.path()).unwrap();

        assert_eq!(result.child_shelves[0].preview_candidates, [book]);
    }

    #[test]
    fn filename_sort_uses_natural_order_in_both_directions() {
        let items = [
            ("10.cbz", None, None),
            ("2.cbz", None, None),
            ("1.cbz", None, None),
        ];

        assert_eq!(
            sorted_test_names(
                &items,
                LibrarySortKey::Name,
                LibrarySortDirection::Ascending
            ),
            ["1.cbz", "2.cbz", "10.cbz"]
        );
        assert_eq!(
            sorted_test_names(
                &items,
                LibrarySortKey::Name,
                LibrarySortDirection::Descending
            ),
            ["10.cbz", "2.cbz", "1.cbz"]
        );
    }

    #[test]
    fn timestamp_sort_supports_modified_and_created_in_both_directions() {
        let items = [
            ("new.cbz", Some(30), Some(10)),
            ("middle.cbz", Some(20), Some(20)),
            ("old.cbz", Some(10), Some(30)),
        ];

        assert_eq!(
            sorted_test_names(
                &items,
                LibrarySortKey::Modified,
                LibrarySortDirection::Ascending
            ),
            ["old.cbz", "middle.cbz", "new.cbz"]
        );
        assert_eq!(
            sorted_test_names(
                &items,
                LibrarySortKey::Modified,
                LibrarySortDirection::Descending
            ),
            ["new.cbz", "middle.cbz", "old.cbz"]
        );
        assert_eq!(
            sorted_test_names(
                &items,
                LibrarySortKey::Created,
                LibrarySortDirection::Ascending
            ),
            ["new.cbz", "middle.cbz", "old.cbz"]
        );
        assert_eq!(
            sorted_test_names(
                &items,
                LibrarySortKey::Created,
                LibrarySortDirection::Descending
            ),
            ["old.cbz", "middle.cbz", "new.cbz"]
        );
    }

    #[test]
    fn timestamp_ties_use_ascending_natural_name_order() {
        let items = [("10.cbz", Some(1), Some(1)), ("2.cbz", Some(1), Some(1))];

        for key in [LibrarySortKey::Modified, LibrarySortKey::Created] {
            for direction in [
                LibrarySortDirection::Ascending,
                LibrarySortDirection::Descending,
            ] {
                assert_eq!(
                    sorted_test_names(&items, key, direction),
                    ["2.cbz", "10.cbz"]
                );
            }
        }
    }

    #[test]
    fn missing_timestamps_always_sort_last() {
        let items = [
            ("missing2.cbz", None, None),
            ("known.cbz", Some(1), Some(1)),
            ("missing10.cbz", None, None),
        ];

        for key in [LibrarySortKey::Modified, LibrarySortKey::Created] {
            for direction in [
                LibrarySortDirection::Ascending,
                LibrarySortDirection::Descending,
            ] {
                assert_eq!(
                    sorted_test_names(&items, key, direction),
                    ["known.cbz", "missing2.cbz", "missing10.cbz"]
                );
            }
        }
    }

    #[test]
    fn folders_and_direct_files_use_the_same_sorting_rules() {
        let epoch = SystemTime::UNIX_EPOCH;
        let paths = [PathBuf::from("2.cbz"), PathBuf::from("10.cbz")];
        let mut direct_file_metadata = HashMap::new();
        let mut child_shelves = Vec::new();
        for (index, path) in paths.iter().enumerate() {
            let metadata = BookshelfItemMetadata {
                modified: Some(epoch + Duration::from_secs(index as u64)),
                created: None,
            };
            direct_file_metadata.insert(path.clone(), metadata);
            child_shelves.push(ChildShelf {
                path: path.clone(),
                preview_candidates: Vec::new(),
                preview_item_count: 0,
                image_document_entry: None,
                image_document_cover: None,
                metadata,
            });
        }
        let directory = BookshelfDirectory {
            path: PathBuf::from("."),
            is_image_document_directory: false,
            direct_files: paths.to_vec(),
            direct_file_metadata,
            child_shelves,
        };

        let mut direct = directory.direct_files.iter().collect::<Vec<_>>();
        sort_items(
            &mut direct,
            LibrarySortKey::Modified,
            LibrarySortDirection::Descending,
            |path| path.as_path(),
            |path| {
                directory
                    .direct_file_metadata
                    .get(path.as_path())
                    .copied()
                    .unwrap_or_default()
            },
        );
        let direct = direct.into_iter().cloned().collect::<Vec<_>>();
        let folders = directory
            .sorted_child_shelves(LibrarySortKey::Modified, LibrarySortDirection::Descending)
            .into_iter()
            .map(|shelf| shelf.path.clone())
            .collect::<Vec<_>>();
        assert_eq!(direct, folders);
        assert_eq!(names(&direct), ["10.cbz", "2.cbz"]);
    }

    #[test]
    fn sorted_books_combine_image_folders_and_direct_files() {
        let epoch = SystemTime::UNIX_EPOCH;
        let direct = PathBuf::from("2.cbz");
        let image_folder = ChildShelf {
            path: PathBuf::from("10"),
            preview_candidates: Vec::new(),
            preview_item_count: 0,
            image_document_entry: Some(PathBuf::from("10/1.jpg")),
            image_document_cover: Some(PathBuf::from("10/1.jpg")),
            metadata: BookshelfItemMetadata {
                modified: Some(epoch + Duration::from_secs(1)),
                created: None,
            },
        };
        let directory = BookshelfDirectory {
            path: PathBuf::from("."),
            is_image_document_directory: false,
            direct_files: vec![direct.clone()],
            direct_file_metadata: HashMap::from([(
                direct,
                BookshelfItemMetadata {
                    modified: Some(epoch),
                    created: None,
                },
            )]),
            child_shelves: vec![image_folder],
        };

        let books =
            directory.sorted_books(LibrarySortKey::Modified, LibrarySortDirection::Descending);

        assert_eq!(books.len(), 2);
        assert!(
            matches!(books[0], BookshelfBook::ImageFolder(shelf) if shelf.path == PathBuf::from("10"))
        );
        assert!(matches!(books[1], BookshelfBook::File(path) if path == &PathBuf::from("2.cbz")));
    }

    #[test]
    fn scan_keeps_timestamp_metadata_for_every_display_item() {
        let root = tempfile::tempdir().unwrap();
        touch(&root.path().join("book.png"));
        let shelf = root.path().join("shelf");
        fs::create_dir(&shelf).unwrap();
        touch(&shelf.join("page.png"));

        let result = scan_directory(root.path()).unwrap();

        assert!(
            result
                .direct_file_metadata
                .contains_key(&root.path().join("book.png"))
        );
        assert_eq!(result.child_shelves.len(), 1);
        assert!(result.child_shelves[0].metadata.modified.is_some());
    }

    #[test]
    fn unreadable_root_is_reported() {
        let root = tempfile::tempdir().unwrap();
        assert!(scan_directory(&root.path().join("missing")).is_err());
    }
}
