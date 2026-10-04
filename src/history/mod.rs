mod identity;
mod persistence;
mod store;

#[cfg(test)]
mod perf_tests;

pub(crate) use identity::{
    HistoryIdentity, history_identity_for_document, is_volume_directory_name,
};
pub(crate) use store::{HistoryEntry, HistoryStore};

#[cfg(test)]
use identity::display_name;
#[cfg(test)]
use persistence::{FORMAT_VERSION, HISTORY_GROUP};

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::glib;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn store(path: PathBuf) -> HistoryStore {
        HistoryStore {
            entries: Vec::new(),
            storage_path: path,
        }
    }

    fn bookshelf_entry(identity: &Path, document: &Path, timestamp: u64) -> HistoryEntry {
        HistoryEntry {
            identity: HistoryIdentity::BookshelfWork(identity.to_path_buf()),
            work_path: Some(identity.to_path_buf()),
            title: display_name(identity, false),
            last_document_path: document.to_path_buf(),
            page_index: 0,
            page_count: 10,
            at_document_end: false,
            last_viewed_unix_ms: timestamp,
        }
    }

    #[test]
    fn adds_to_empty_history_and_groups_volumes_in_the_same_work() {
        let directory = tempfile::tempdir().unwrap();
        let mut history = store(directory.path().join("history.ini"));
        let root = Path::new("/本棚");
        let first = Path::new("/本棚/手塚治虫/ブラック・ジャック/01.rar");
        let second = Path::new("/本棚/手塚治虫/ブラック・ジャック/02.rar");

        let identity = history.record_document_at(first, Some(root), 3, 100, 10);
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].title, "ブラック・ジャック");
        assert_eq!(
            identity,
            HistoryIdentity::BookshelfWork(PathBuf::from("/本棚/手塚治虫/ブラック・ジャック"))
        );

        history.record_document_at(second, Some(root), 8, 120, 20);
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].last_document_path, second);
        assert_eq!(history.entries[0].page_index, 8);
        assert_eq!(history.entries[0].page_count, 120);
    }

    #[test]
    fn image_volume_directories_are_grouped_as_one_bookshelf_work() {
        let directory = tempfile::tempdir().unwrap();
        let mut history = store(directory.path().join("history.ini"));
        let root = Path::new("/本棚");
        let first = Path::new("/本棚/ブラック・ジャック/01巻/001.jpg");
        let second = Path::new("/本棚/ブラック・ジャック/02巻/001.jpg");

        let first_identity = history.record_document_at(first, Some(root), 3, 100, 10);
        let second_identity = history.record_document_at(second, Some(root), 8, 120, 20);

        assert_eq!(first_identity, second_identity);
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].title, "ブラック・ジャック");
        assert_eq!(history.entries[0].last_document_path, second);
        assert_eq!(history.entries[0].page_index, 8);
        assert_eq!(history.entries[0].page_count, 120);
        assert_eq!(history.entries[0].last_viewed_unix_ms, 20);
    }

    #[test]
    fn sibling_image_books_with_non_volume_names_are_grouped_by_recording_path() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sample");
        let work = root.join("folder2");
        let first = work.join("jpeg/001.jpg");
        let second = work.join("mono_numbered_png/01.png");
        fs::create_dir_all(first.parent().unwrap()).unwrap();
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(&first, []).unwrap();
        fs::write(&second, []).unwrap();
        let mut history = store(directory.path().join("history.ini"));

        let first_identity = history.record_document_at(&first, Some(&root), 3, 10, 10);
        let second_identity = history.record_document_at(&second, Some(&root), 8, 20, 20);

        assert_eq!(first_identity, HistoryIdentity::BookshelfWork(work.clone()));
        assert_eq!(second_identity, first_identity);
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].last_document_path, second);
        assert_eq!(history.entries[0].page_index, 8);
        assert_eq!(history.entries[0].page_count, 20);
    }

    #[test]
    fn cover_only_sibling_is_not_an_image_book() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("books");
        let work = root.join("work");
        let image = work.join("chapter/001.jpg");
        let cover = work.join("cover-only/cover.jpg");
        fs::create_dir_all(image.parent().unwrap()).unwrap();
        fs::create_dir_all(cover.parent().unwrap()).unwrap();
        fs::write(&image, []).unwrap();
        fs::write(&cover, []).unwrap();

        assert_eq!(
            history_identity_for_document(&image, Some(&root)),
            HistoryIdentity::BookshelfWork(image.parent().unwrap().to_path_buf())
        );
        fs::write(work.join("cover-only/001.jpg"), []).unwrap();
        assert_eq!(
            history_identity_for_document(&image, Some(&root)),
            HistoryIdentity::BookshelfWork(work)
        );
    }

    #[cfg(unix)]
    #[test]
    fn sibling_directory_symlink_is_an_image_book() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("books");
        let work = root.join("work");
        let image = work.join("chapter/001.jpg");
        let linked_images = work.join("linked-images");
        let target = directory.path().join("image-target");
        fs::create_dir_all(image.parent().unwrap()).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(&image, []).unwrap();
        fs::write(target.join("001.jpg"), []).unwrap();
        std::os::unix::fs::symlink(&target, &linked_images).unwrap();

        assert_eq!(
            history_identity_for_document(&image, Some(&root)),
            HistoryIdentity::BookshelfWork(work)
        );
    }

    #[test]
    fn root_child_image_books_keep_distinct_identities_with_filesystem_layouts() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("books");
        let first = root.join("book-a/001.jpg");
        let second = root.join("book-b/001.jpg");
        for document in [&first, &second] {
            fs::create_dir_all(document.parent().unwrap()).unwrap();
            fs::write(document, []).unwrap();
        }
        let mut history = store(directory.path().join("history.ini"));

        let first_identity = history.record_document_at(&first, Some(&root), 0, 1, 1);
        let second_identity = history.record_document_at(&second, Some(&root), 0, 1, 2);

        assert_eq!(
            first_identity,
            HistoryIdentity::BookshelfWork(root.join("book-a"))
        );
        assert_eq!(
            second_identity,
            HistoryIdentity::BookshelfWork(root.join("book-b"))
        );
        assert_ne!(first_identity, second_identity);
        assert_eq!(history.entries.len(), 2);
    }

    #[test]
    fn loading_normalizes_split_sibling_image_book_history_and_persists_it() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sample");
        let work = root.join("folder2");
        let first_folder = work.join("jpeg");
        let second_folder = work.join("mono_numbered_png");
        let first = first_folder.join("001.jpg");
        let second = second_folder.join("01.png");
        fs::create_dir_all(&first_folder).unwrap();
        fs::create_dir_all(&second_folder).unwrap();
        fs::write(&first, []).unwrap();
        fs::write(&second, []).unwrap();
        let path = directory.path().join("history.ini");
        let mut legacy = store(path.clone());
        legacy.entries = vec![
            bookshelf_entry(&second_folder, &second, 20),
            bookshelf_entry(&first_folder, &first, 10),
        ];
        legacy.save_to_path(&path).unwrap();

        let normalized =
            HistoryStore::load_with_bookshelf_root_from_path(path.clone(), Some(&root));
        assert_eq!(normalized.entries.len(), 1);
        assert_eq!(
            normalized.entries[0].identity,
            HistoryIdentity::BookshelfWork(work.clone())
        );
        assert_eq!(normalized.entries[0].last_document_path, second);

        let reloaded = HistoryStore::load_or_empty_from_path(path);
        assert_eq!(reloaded.entries, normalized.entries);
    }

    #[test]
    fn sibling_image_books_under_different_works_remain_distinct() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("books");
        let work_a = root.join("work-a");
        let work_b = root.join("work-b");
        let work_a_first = work_a.join("part-a/001.jpg");
        let work_a_second = work_a.join("part-b/001.jpg");
        let work_b_first = work_b.join("part-a/001.jpg");
        let work_b_second = work_b.join("part-b/001.jpg");
        for path in [&work_a_first, &work_a_second, &work_b_first, &work_b_second] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, []).unwrap();
        }
        let mut history = store(directory.path().join("history.ini"));

        let identity_a = history.record_document_at(&work_a_first, Some(&root), 1, 10, 10);
        let identity_b = history.record_document_at(&work_b_first, Some(&root), 2, 10, 20);

        assert_eq!(identity_a, HistoryIdentity::BookshelfWork(work_a));
        assert_eq!(identity_b, HistoryIdentity::BookshelfWork(work_b));
        assert_ne!(identity_a, identity_b);
        assert_eq!(history.entries.len(), 2);
    }

    #[test]
    fn ordinary_image_directory_is_not_promoted_to_its_parent() {
        let directory = tempfile::tempdir().unwrap();
        let mut history = store(directory.path().join("history.ini"));
        let identity = history.record_document_at(
            Path::new("/本棚/手塚治虫/ブラック・ジャック/001.jpg"),
            Some(Path::new("/本棚")),
            0,
            10,
            1,
        );

        assert_eq!(
            identity,
            HistoryIdentity::BookshelfWork(PathBuf::from("/本棚/手塚治虫/ブラック・ジャック"))
        );
        assert_eq!(history.entries[0].title, "ブラック・ジャック");
    }

    #[test]
    fn root_child_volume_directory_does_not_promote_to_bookshelf_root() {
        let directory = tempfile::tempdir().unwrap();
        let mut history = store(directory.path().join("history.ini"));
        let identity = history.record_document_at(
            Path::new("/本棚/01巻/001.jpg"),
            Some(Path::new("/本棚")),
            0,
            10,
            1,
        );

        assert_eq!(
            identity,
            HistoryIdentity::BookshelfWork(PathBuf::from("/本棚/01巻"))
        );
        assert_eq!(history.entries[0].title, "01巻");
    }

    #[test]
    fn outside_bookshelf_images_remain_document_scoped() {
        let directory = tempfile::tempdir().unwrap();
        let mut history = store(directory.path().join("history.ini"));
        let document = Path::new("/外部/ブラック・ジャック/01巻/001.jpg");
        let identity = history.record_document_at(document, Some(Path::new("/本棚")), 0, 10, 1);

        assert_eq!(identity, HistoryIdentity::Document(document.to_path_buf()));
        assert_eq!(history.entries[0].title, "001");
    }

    #[test]
    fn recognizes_only_conservative_volume_directory_names() {
        for name in [
            "01",
            "1",
            "001",
            "01巻",
            "1巻",
            "第01巻",
            "第1巻",
            "vol01",
            "vol.01",
            "vol 01",
            "Vol.01",
            "volume01",
            "volume 01",
            "volume.01",
            "VOLUME.01",
        ] {
            assert!(is_volume_directory_name(name), "{name}");
        }

        for name in [
            "ブラック・ジャック",
            "異世界居酒屋「のぶ」",
            "火の鳥",
            "ブラック・ジャック 01巻",
            "volumes",
            "vol.01 extra",
            "第1章",
            "巻1",
            "",
        ] {
            assert!(!is_volume_directory_name(name), "{name}");
        }
    }

    #[test]
    fn reopening_moves_an_existing_work_to_the_front() {
        let directory = tempfile::tempdir().unwrap();
        let mut history = store(directory.path().join("history.ini"));
        let root = Path::new("/books");
        history.record_document_at(Path::new("/books/a/1.cbz"), Some(root), 0, 10, 1);
        history.record_document_at(Path::new("/books/b/1.cbz"), Some(root), 0, 10, 2);
        history.record_document_at(Path::new("/books/a/2.cbz"), Some(root), 1, 20, 3);

        assert_eq!(history.entries.len(), 2);
        assert_eq!(history.entries[0].title, "a");
        assert_eq!(history.entries[1].title, "b");
        assert_eq!(history.entries[0].last_viewed_unix_ms, 3);
    }

    #[test]
    fn different_work_folders_are_distinct() {
        let directory = tempfile::tempdir().unwrap();
        let mut history = store(directory.path().join("history.ini"));
        let root = Path::new("/books");
        history.record_document_at(Path::new("/books/a/1.cbz"), Some(root), 0, 10, 1);
        history.record_document_at(Path::new("/books/b/1.cbz"), Some(root), 0, 10, 2);
        assert_eq!(history.entries.len(), 2);
    }

    #[test]
    fn root_documents_and_outside_documents_are_document_scoped() {
        let directory = tempfile::tempdir().unwrap();
        let mut history = store(directory.path().join("history.ini"));
        let root = Path::new("/books");
        history.record_document_at(Path::new("/books/01.cbz"), Some(root), 0, 10, 1);
        history.record_document_at(Path::new("/books/02.cbz"), Some(root), 0, 10, 2);
        history.record_document_at(
            Path::new("/outside/ブラック・ジャック 01.rar"),
            Some(root),
            0,
            10,
            3,
        );

        assert_eq!(history.entries.len(), 3);
        assert!(matches!(
            history.entries[1].identity,
            HistoryIdentity::Document(_)
        ));
        assert!(matches!(
            history.entries[2].identity,
            HistoryIdentity::Document(_)
        ));
        assert_eq!(history.entries[0].title, "ブラック・ジャック 01");
    }

    #[test]
    fn position_update_changes_timestamp_and_order_only_when_position_changes() {
        let directory = tempfile::tempdir().unwrap();
        let mut history = store(directory.path().join("history.ini"));
        let root = Path::new("/books");
        let first = history.record_document_at(Path::new("/books/a/1.cbz"), Some(root), 0, 10, 1);
        history.record_document_at(Path::new("/books/b/1.cbz"), Some(root), 0, 10, 2);

        assert!(!history.update_position_at(&first, 0, 10, 3));
        assert_eq!(history.entries[1].last_viewed_unix_ms, 1);
        assert!(history.update_position_at(&first, 5, 12, 4));
        assert_eq!(history.entries[0].identity, first);
        assert_eq!(history.entries[0].page_index, 5);
        assert_eq!(history.entries[0].page_count, 12);
        assert_eq!(history.entries[0].last_viewed_unix_ms, 4);
    }

    #[test]
    fn position_changes_gate_saves_and_removed_entries_do_not_return() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("custom").join("history.ini");
        let mut history = store(path.clone());
        let first = history.record_document_at(Path::new("/books/a/1.cbz"), None, 0, 10, 1);
        history.save_to_path(&path).unwrap();
        let original = fs::read(&path).unwrap();

        assert!(!history.update_position_state_at(&first, 0, 10, false, 2));
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(history.update_position_state_at(&first, 1, 10, false, 3));
        history.save_to_path(&path).unwrap();
        assert_eq!(
            HistoryStore::load_or_empty_from_path(path.clone()).entries[0].page_index,
            1
        );
        assert!(history.update_position_state_at(&first, 1, 11, false, 4));
        assert!(history.update_position_state_at(&first, 1, 11, true, 5));
        assert!(history.update_position_state_at(&first, 1, 11, false, 6));

        assert!(history.remove(&first));
        assert!(!history.update_position_state_at(&first, 2, 11, false, 7));
        assert!(history.entries().is_empty());

        fs::remove_dir_all(path.parent().unwrap()).unwrap();
        history.save_to_path(&path).unwrap();
        assert!(
            HistoryStore::load_or_empty_from_path(path)
                .entries()
                .is_empty()
        );
    }

    #[test]
    fn save_load_round_trip_preserves_unicode_missing_paths_and_order() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.ini");
        let mut history = store(path.clone());
        let root = Path::new("/存在しない本棚");
        history.record_document_at(
            Path::new("/存在しない本棚/作品甲/第1巻.cbz"),
            Some(root),
            7,
            99,
            100,
        );
        history.record_document_at(Path::new("/外部/作品乙 01.rar"), Some(root), 11, 88, 200);
        let expected = history.entries.clone();
        history.save_to_path(&path).unwrap();

        let loaded = HistoryStore::load_or_empty_from_path(path);
        assert_eq!(loaded.entries, expected);
        assert_eq!(loaded.entries[0].title, "作品乙 01");
    }

    #[test]
    fn document_end_state_round_trips_and_can_be_cleared_at_the_same_page() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.ini");
        let document = Path::new("/books/work/01.cbz");
        let mut history = store(path.clone());
        let identity =
            history.record_document_state_at(document, Some(Path::new("/books")), 63, 65, true, 10);
        history.save_to_path(&path).unwrap();

        let mut loaded = HistoryStore::load_or_empty_from_path(path);
        assert!(loaded.entries[0].at_document_end);
        assert_eq!(loaded.entries[0].page_index, 63);
        assert_eq!(loaded.entries[0].page_count, 65);

        assert!(loaded.update_position_state_at(&identity, 63, 65, false, 20));
        assert!(!loaded.entries[0].at_document_end);
    }

    #[test]
    fn missing_file_is_empty_and_malformed_entries_are_skipped() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing.ini");
        assert!(
            HistoryStore::load_or_empty_from_path(missing)
                .entries
                .is_empty()
        );

        let path = directory.path().join("history.ini");
        let key_file = glib::KeyFile::new();
        key_file.set_integer(HISTORY_GROUP, "format-version", FORMAT_VERSION);
        key_file.set_integer(HISTORY_GROUP, "entry-count", i32::MAX);
        key_file.set_string("Entry 0", "identity-kind", "document");
        key_file.set_string("Entry 0", "identity-path", "/valid.cbz");
        key_file.set_string("Entry 0", "title", "valid");
        key_file.set_string("Entry 0", "last-document", "/valid.cbz");
        key_file.set_string("Entry 0", "page-index", "1");
        key_file.set_string("Entry 0", "page-count", "10");
        key_file.set_string("Entry 0", "last-viewed-unix-ms", "5");
        key_file.set_string("Entry 1", "identity-kind", "document");
        key_file.set_string("Entry 1", "identity-path", "/broken.cbz");
        key_file.set_string("Entry 1", "title", "broken");
        key_file.set_string("Entry 1", "page-index", "not-a-number");
        key_file.save_to_file(&path).unwrap();

        let loaded = HistoryStore::load_or_empty_from_path(path);
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].title, "valid");
        assert!(!loaded.entries[0].at_document_end);
    }

    #[test]
    fn unreadable_key_file_falls_back_to_empty_history() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.ini");
        fs::write(&path, "not an ini file\0").unwrap();
        assert!(
            HistoryStore::load_or_empty_from_path(path)
                .entries
                .is_empty()
        );
    }

    #[test]
    fn removes_one_entry_and_missing_identity_is_safe() {
        let directory = tempfile::tempdir().unwrap();
        let mut history = store(directory.path().join("history.ini"));
        let first = history.record_document_at(Path::new("/a.cbz"), None, 0, 10, 1);
        let second = history.record_document_at(Path::new("/b.cbz"), None, 0, 10, 2);

        assert!(history.remove(&first));
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].identity, second);
        assert!(!history.remove(&first));
        assert_eq!(history.entries.len(), 1);
    }

    #[test]
    fn clears_all_entries_and_empty_clear_is_safe() {
        let directory = tempfile::tempdir().unwrap();
        let mut history = store(directory.path().join("history.ini"));
        history.record_document_at(Path::new("/a.cbz"), None, 0, 10, 1);
        history.record_document_at(Path::new("/b.cbz"), None, 0, 10, 2);

        assert!(history.clear());
        assert!(history.entries().is_empty());
        assert!(!history.clear());
    }

    #[test]
    fn save_load_after_deletion_uses_current_format() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.ini");
        let mut history = store(path.clone());
        let removed = history.record_document_at(Path::new("/a.cbz"), None, 1, 10, 1);
        let kept = history.record_document_at(Path::new("/b.cbz"), None, 2, 20, 2);
        assert!(history.remove(&removed));
        history.save_to_path(&path).unwrap();

        let key_file = glib::KeyFile::new();
        key_file
            .load_from_file(&path, glib::KeyFileFlags::NONE)
            .unwrap();
        assert_eq!(
            key_file.integer(HISTORY_GROUP, "format-version").unwrap(),
            FORMAT_VERSION
        );
        let loaded = HistoryStore::load_or_empty_from_path(path);
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].identity, kept);
    }

    #[test]
    fn previous_format_is_loaded_as_empty_history() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.ini");
        let key_file = glib::KeyFile::new();
        key_file.set_integer(HISTORY_GROUP, "format-version", 1);
        key_file.set_integer(HISTORY_GROUP, "entry-count", 1);
        key_file.set_string("Entry 0", "identity-kind", "bookshelf-work");
        key_file.set_string("Entry 0", "identity-path", "/本棚/作品/01巻");
        key_file.set_string("Entry 0", "title", "01巻");
        key_file.set_string("Entry 0", "last-document", "/本棚/作品/01巻/001.jpg");
        key_file.set_string("Entry 0", "page-index", "1");
        key_file.set_string("Entry 0", "page-count", "10");
        key_file.set_string("Entry 0", "last-viewed-unix-ms", "5");
        key_file.save_to_file(&path).unwrap();

        let mut loaded = HistoryStore::load_or_empty_from_path(path.clone());
        assert!(loaded.entries.is_empty());

        loaded.record_document_at(
            Path::new("/本棚/作品/01巻/001.jpg"),
            Some(Path::new("/本棚")),
            2,
            10,
            6,
        );
        loaded.save_to_path(&path).unwrap();
        let saved = glib::KeyFile::new();
        saved
            .load_from_file(path, glib::KeyFileFlags::NONE)
            .unwrap();
        assert_eq!(
            saved.integer(HISTORY_GROUP, "format-version").unwrap(),
            FORMAT_VERSION
        );
    }
}
