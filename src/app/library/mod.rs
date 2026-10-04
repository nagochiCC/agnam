mod card;
mod progress;
mod shelf_container;
mod state;
mod view;

#[cfg(test)]
pub(super) use super::Msg;

#[cfg(test)]
use card::{
    direct_file_message, direct_file_read_from_start_message, folder_message, progress_bar_width,
    progress_bar_x,
};
pub(super) use progress::{
    BookshelfDisplaySizes, favorite_progress, image_folder_progress, library_progress,
};
#[cfg(test)]
use progress::{
    FolderPreviewGeometry, LibraryProgress, folder_outer_height_for_cover_height,
    library_badge_progress,
};
pub(super) use state::{LibraryState, ScanCompletion, ScanRequest, path_is_within_root};
pub(super) use view::LibraryView;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bookshelf::BookshelfDirectory;
    use crate::favorites::FavoriteIdentity;
    use crate::history::{HistoryEntry, HistoryIdentity, HistoryStore};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    fn history_entry(path: &Path, page_index: usize, page_count: usize) -> HistoryEntry {
        HistoryEntry {
            identity: HistoryIdentity::Document(path.to_path_buf()),
            work_path: None,
            title: "test".into(),
            last_document_path: path.to_path_buf(),
            page_index,
            page_count,
            at_document_end: false,
            last_viewed_unix_ms: 0,
        }
    }

    fn work_history_entry(
        work: &Path,
        path: &Path,
        page_index: usize,
        page_count: usize,
    ) -> HistoryEntry {
        let mut entry = history_entry(path, page_index, page_count);
        entry.identity = HistoryIdentity::BookshelfWork(work.to_path_buf());
        entry.work_path = Some(work.to_path_buf());
        entry
    }

    fn empty_directory(path: &Path) -> BookshelfDirectory {
        BookshelfDirectory {
            path: path.to_path_buf(),
            is_image_document_directory: false,
            direct_files: Vec::new(),
            direct_file_metadata: HashMap::new(),
            child_shelves: Vec::new(),
        }
    }

    #[test]
    fn configured_root_initializes_the_current_directory() {
        let state = LibraryState::new(Some(PathBuf::from("/books")));

        assert_eq!(state.root(), Some(Path::new("/books")));
        assert_eq!(state.current_directory(), Some(Path::new("/books")));
    }

    #[test]
    fn folder_image_heights_produce_the_established_outer_heights() {
        for (cover_height, outer_height) in [(81, 104), (104, 130), (180, 222), (262, 312)] {
            assert_eq!(
                folder_outer_height_for_cover_height(cover_height),
                outer_height
            );
            let sizes = BookshelfDisplaySizes::new(cover_height);
            assert_eq!(sizes.folder_preview_height, outer_height);
            let geometry =
                FolderPreviewGeometry::new(sizes.folder_preview_height, sizes.book_height * 2);
            assert_eq!(geometry.cover_height, cover_height);
        }
    }

    #[test]
    fn bookshelf_card_widths_are_derived_from_cover_sizes() {
        let sizes = BookshelfDisplaySizes::new(104);

        assert_eq!(sizes.folder_preview_height, 130);
        let geometry =
            FolderPreviewGeometry::new(sizes.folder_preview_height, sizes.book_height * 2);
        assert_eq!(geometry.cover_height, 104);
    }

    #[test]
    fn changing_root_resets_the_current_directory() {
        let mut state = LibraryState::new(Some(PathBuf::from("/books")));
        assert!(state.navigate(PathBuf::from("/books/author")).is_some());

        state.set_root(Some(PathBuf::from("/new-books")));

        assert_eq!(state.current_directory(), Some(Path::new("/new-books")));
        assert!(state.directory().is_none());
    }

    #[test]
    fn child_navigation_stays_below_root() {
        let mut state = LibraryState::new(Some(PathBuf::from("/books")));

        assert!(
            state
                .navigate(PathBuf::from("/books/author/work"))
                .is_some()
        );
        assert_eq!(
            state.current_directory(),
            Some(Path::new("/books/author/work"))
        );
        assert!(state.navigate(PathBuf::from("/outside")).is_none());
        assert!(
            state
                .navigate(PathBuf::from("/books/child/../.."))
                .is_none()
        );
        assert_eq!(
            state.current_directory(),
            Some(Path::new("/books/author/work"))
        );
    }

    #[test]
    fn only_the_current_scan_result_is_applied() {
        let mut state = LibraryState::new(Some(PathBuf::from("/books")));
        let first = state.begin_current_scan().unwrap();
        assert_eq!(state.status_message(), None);
        let second = state.navigate(PathBuf::from("/books/author")).unwrap();

        assert_eq!(
            state.apply_scan(first.id, &first.path, Ok(empty_directory(&first.path))),
            ScanCompletion::Stale
        );
        assert!(state.is_scanning());
        assert_eq!(
            state.apply_scan(second.id, &second.path, Ok(empty_directory(&second.path))),
            ScanCompletion::InitialSuccess
        );
        assert!(!state.is_scanning());
        assert_eq!(state.directory().unwrap().path, second.path);
    }

    #[test]
    fn scan_errors_are_kept_without_panicking() {
        let mut state = LibraryState::new(Some(PathBuf::from("/missing")));
        let request = state.begin_current_scan().unwrap();

        assert_eq!(
            state.apply_scan(request.id, &request.path, Err("permission denied".into())),
            ScanCompletion::InitialFailed
        );
        assert!(!state.is_scanning());
        assert!(state.directory().is_none());
        assert!(
            state
                .status_message()
                .unwrap()
                .contains("permission denied")
        );
    }

    fn scan_current(state: &mut LibraryState) {
        let request = state.begin_current_scan().unwrap();
        assert_eq!(
            state.apply_scan(
                request.id,
                &request.path,
                Ok(empty_directory(&request.path))
            ),
            ScanCompletion::InitialSuccess
        );
    }

    fn navigate_and_scan(state: &mut LibraryState, path: &str) {
        let request = state.navigate(PathBuf::from(path)).unwrap();
        assert_eq!(
            state.apply_scan(
                request.id,
                &request.path,
                Ok(empty_directory(&request.path))
            ),
            ScanCompletion::InitialSuccess
        );
    }

    #[test]
    fn scan_cache_keeps_only_the_current_ancestor_chain() {
        let mut state = LibraryState::new(Some(PathBuf::from("/books")));
        scan_current(&mut state);
        navigate_and_scan(&mut state, "/books/tezuka");
        navigate_and_scan(&mut state, "/books/tezuka/blackjack");

        let mut paths = state.cached_directory_paths();
        paths.sort();
        assert_eq!(
            paths,
            ["/books", "/books/tezuka", "/books/tezuka/blackjack"].map(PathBuf::from)
        );

        state.navigate(PathBuf::from("/books/tezuka")).unwrap();
        let mut paths = state.cached_directory_paths();
        paths.sort();
        assert_eq!(paths, ["/books", "/books/tezuka"].map(PathBuf::from));

        state.navigate(PathBuf::from("/books")).unwrap();
        assert_eq!(state.cached_directory_paths(), [PathBuf::from("/books")]);
    }

    #[test]
    fn sibling_navigation_prunes_the_old_child_branch() {
        let mut state = LibraryState::new(Some(PathBuf::from("/books")));
        scan_current(&mut state);
        navigate_and_scan(&mut state, "/books/tezuka");
        navigate_and_scan(&mut state, "/books/tezuka/blackjack");

        state
            .navigate(PathBuf::from("/books/tezuka/hinotori"))
            .unwrap();

        let mut paths = state.cached_directory_paths();
        paths.sort();
        assert_eq!(paths, ["/books", "/books/tezuka"].map(PathBuf::from));
    }

    #[test]
    fn cached_navigation_remains_available_while_refreshing() {
        let mut state = LibraryState::new(Some(PathBuf::from("/books")));
        scan_current(&mut state);
        navigate_and_scan(&mut state, "/books/tezuka");
        state.navigate(PathBuf::from("/books")).unwrap();

        assert!(state.is_scanning());
        assert_eq!(state.directory().unwrap().path, PathBuf::from("/books"));
    }

    #[test]
    fn refresh_classifies_unchanged_changed_and_failure_without_losing_cache() {
        let mut state = LibraryState::new(Some(PathBuf::from("/books")));
        scan_current(&mut state);

        let request = state.begin_current_scan().unwrap();
        assert_eq!(
            state.apply_scan(
                request.id,
                &request.path,
                Ok(empty_directory(&request.path))
            ),
            ScanCompletion::RefreshUnchanged
        );

        let request = state.begin_current_scan().unwrap();
        let mut changed = empty_directory(&request.path);
        changed.direct_files.push(request.path.join("01.cbz"));
        assert_eq!(
            state.apply_scan(request.id, &request.path, Ok(changed.clone())),
            ScanCompletion::RefreshChanged
        );
        assert_eq!(state.directory(), Some(&changed));

        let request = state.begin_current_scan().unwrap();
        assert_eq!(
            state.apply_scan(request.id, &request.path, Err("offline".into())),
            ScanCompletion::RefreshFailed
        );
        assert_eq!(state.directory(), Some(&changed));
        assert_eq!(state.status_message(), None);
    }

    #[test]
    fn changing_root_drops_all_cached_scan_results() {
        let mut state = LibraryState::new(Some(PathBuf::from("/books")));
        scan_current(&mut state);
        navigate_and_scan(&mut state, "/books/tezuka");

        state.set_root(Some(PathBuf::from("/other")));

        assert!(state.cached_directory_paths().is_empty());
        assert!(state.directory().is_none());
    }

    #[test]
    fn seeded_snapshot_classifies_matching_and_changed_fresh_scans() {
        let root = PathBuf::from("/books");
        let snapshot = empty_directory(&root);
        let mut state = LibraryState::new(Some(root.clone()));
        state.seed_snapshot_for_test(snapshot.clone());

        let request = state.begin_current_scan().unwrap();
        assert!(request.persist_snapshot);
        assert_eq!(
            state.apply_scan(request.id, &request.path, Ok(snapshot.clone())),
            ScanCompletion::RefreshUnchanged
        );

        let request = state.begin_current_scan().unwrap();
        let mut changed = snapshot;
        changed.direct_files.push(root.join("new.cbz"));
        assert_eq!(
            state.apply_scan(request.id, &request.path, Ok(changed.clone())),
            ScanCompletion::RefreshChanged
        );
        assert_eq!(state.directory(), Some(&changed));
    }

    #[test]
    fn root_change_drops_snapshot_seeded_state() {
        let root = PathBuf::from("/books");
        let mut state = LibraryState::new(Some(root.clone()));
        state.seed_snapshot_for_test(empty_directory(&root));

        state.set_root(Some(PathBuf::from("/other")));

        assert!(state.directory().is_none());
        assert!(state.cached_directory_paths().is_empty());
    }

    #[test]
    fn direct_files_use_the_library_specific_open_route() {
        let path = PathBuf::from("/books/01.jpg");

        assert!(matches!(
            direct_file_message(path.clone()),
            Msg::LibraryFileSelected(selected) if selected == path
        ));
    }

    #[test]
    fn direct_file_context_menu_uses_the_read_from_start_route() {
        let path = PathBuf::from("/books/01.jpg");

        assert!(matches!(
            direct_file_read_from_start_message(path.clone()),
            Msg::LibraryFileReadFromStart(selected) if selected == path
        ));
    }

    #[test]
    fn folder_click_routes_image_books_to_viewer_and_containers_to_navigation() {
        let folder = PathBuf::from("/books/series/01");
        let entry = folder.join("001.jpg");

        assert!(matches!(
            folder_message(folder.clone(), Some(entry.clone())),
            Msg::LibraryImageFolderSelected {
                folder: selected_folder,
                entry: selected_entry,
            } if selected_folder == folder && selected_entry == entry
        ));
        assert!(matches!(
            folder_message(folder.clone(), None),
            Msg::NavigateLibrary(selected) if selected == folder
        ));
    }

    #[test]
    fn library_progress_has_a_clamped_page_fraction() {
        let first_page = LibraryProgress::new(0, 100).unwrap();
        assert_eq!(first_page.fraction(), 0.01);
        assert_eq!(first_page.page_index(), 0);

        let second_page = LibraryProgress::new(1, 100).unwrap();
        assert_eq!(second_page.fraction(), 0.02);
        assert_eq!(second_page.page_index(), 1);

        let intermediate = LibraryProgress::new(49, 100).unwrap();
        assert_eq!(intermediate.fraction(), 0.5);
        assert_eq!(intermediate.page_index(), 49);

        let final_page = LibraryProgress::new(99, 100).unwrap();
        assert_eq!(final_page.fraction(), 1.0);
        assert_eq!(final_page.page_index(), 99);

        let only_page = LibraryProgress::new(0, 1).unwrap();
        assert_eq!(only_page.fraction(), 1.0);
        assert_eq!(only_page.page_index(), 0);

        assert_eq!(LibraryProgress::new(0, 0), None);
        assert_eq!(LibraryProgress::new(100, 100), None);
        assert_eq!(LibraryProgress::new(usize::MAX, usize::MAX), None);
    }

    #[test]
    fn library_progress_uses_the_matching_documents_latest_position() {
        let path = Path::new("/books/work/01.cbz");
        let other_path = Path::new("/books/work/02.cbz");
        let mut entries = vec![history_entry(path, 239, 240)];

        let final_progress = library_progress(&entries, path).unwrap();
        assert_eq!(final_progress.fraction(), 1.0);
        assert_eq!(final_progress.page_index(), 239);
        assert_eq!(library_progress(&entries, other_path), None);

        entries[0].page_index = 49;
        let progress = library_progress(&entries, path).unwrap();
        assert_eq!(progress.fraction(), 50.0 / 240.0);
        assert_eq!(progress.page_index(), 49);
    }

    #[test]
    fn completed_book_has_no_resume_progress_or_badge() {
        let path = Path::new("/books/work/01.cbz");
        let identity = FavoriteIdentity::FileDocument(path.to_path_buf());
        let mut entry = history_entry(path, 0, 65);
        assert_eq!(
            library_badge_progress(std::slice::from_ref(&entry), &identity),
            Some(LibraryProgress::new(0, 65).unwrap())
        );

        entry.page_index = 63;
        entry.at_document_end = true;
        let entries = [entry];

        assert_eq!(library_progress(&entries, path), None);
        assert_eq!(favorite_progress(&entries, &identity), None);
        assert_eq!(library_badge_progress(&entries, &identity), None);
    }

    #[test]
    fn root_file_books_keep_separate_identities_and_badges() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("books");
        let first = root.join("archiveA.zip");
        let second = root.join("archiveB.zip");
        let first_identity = FavoriteIdentity::FileDocument(first.clone());
        let second_identity = FavoriteIdentity::FileDocument(second.clone());
        let mut history =
            HistoryStore::load_or_empty_from_path(directory.path().join("history.ini"));

        let first_history_identity = history.record_document(&first, Some(&root), 4, 20, false);
        assert_eq!(
            library_badge_progress(history.entries(), &first_identity),
            Some(LibraryProgress::new(4, 20).unwrap())
        );

        history.record_document(&first, Some(&root), 19, 20, true);
        assert_eq!(
            library_badge_progress(history.entries(), &first_identity),
            None
        );
        assert_eq!(library_progress(history.entries(), &first), None);

        let second_history_identity = history.record_document(&second, Some(&root), 2, 20, false);
        assert_ne!(first_history_identity, second_history_identity);
        assert!(matches!(
            first_history_identity,
            HistoryIdentity::Document(_)
        ));
        assert!(matches!(
            second_history_identity,
            HistoryIdentity::Document(_)
        ));
        assert_eq!(history.entries().len(), 2);
        assert_eq!(
            library_badge_progress(history.entries(), &first_identity),
            None
        );
        assert_eq!(
            library_badge_progress(history.entries(), &second_identity),
            Some(LibraryProgress::new(2, 20).unwrap())
        );
    }

    #[test]
    fn work_folder_badge_moves_to_the_latest_book_and_hides_at_end() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("books");
        let work = root.join("black-jack");
        let first = work.join("01.zip");
        let second = work.join("02.zip");
        let first_identity = FavoriteIdentity::FileDocument(first.clone());
        let second_identity = FavoriteIdentity::FileDocument(second.clone());
        let mut history =
            HistoryStore::load_or_empty_from_path(directory.path().join("history.ini"));

        history.record_document(&first, Some(&root), 3, 20, false);
        assert_eq!(
            library_badge_progress(history.entries(), &first_identity),
            Some(LibraryProgress::new(3, 20).unwrap())
        );

        history.record_document(&second, Some(&root), 5, 20, false);
        assert_eq!(
            library_badge_progress(history.entries(), &first_identity),
            None
        );
        assert_eq!(
            library_badge_progress(history.entries(), &second_identity),
            Some(LibraryProgress::new(5, 20).unwrap())
        );

        history.record_document(&second, Some(&root), 19, 20, true);
        assert_eq!(
            library_badge_progress(history.entries(), &second_identity),
            None
        );
        assert_eq!(library_progress(history.entries(), &second), None);
    }

    #[test]
    fn image_folder_progress_uses_only_the_latest_volume_in_the_same_work() {
        let work = Path::new("/books/work");
        let first = Path::new("/books/work/01巻/001.jpg");
        let second = Path::new("/books/work/02巻/001.jpg");
        let first_only = [work_history_entry(work, first, 3, 10)];
        assert_eq!(
            image_folder_progress(&first_only, Path::new("/books/work/01巻"))
                .unwrap()
                .fraction(),
            0.4
        );

        let entries = [
            work_history_entry(work, second, 8, 20),
            work_history_entry(work, first, 3, 10),
        ];

        assert_eq!(
            image_folder_progress(&entries, Path::new("/books/work/01巻")),
            None
        );
        assert_eq!(
            library_badge_progress(
                &entries,
                &FavoriteIdentity::image_folder(Path::new("/books/work/01巻")),
            ),
            None
        );
        let progress = image_folder_progress(&entries, Path::new("/books/work/02巻")).unwrap();
        assert_eq!(progress.page_index(), 8);
        assert_eq!(progress.fraction(), 0.45);
        assert_eq!(
            library_badge_progress(
                &entries,
                &FavoriteIdentity::image_folder(Path::new("/books/work/02巻")),
            ),
            Some(LibraryProgress::new(8, 20).unwrap())
        );
    }

    #[test]
    fn recorded_sibling_image_books_move_progress_to_the_latest_book() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sample");
        let work = root.join("folder2");
        let first_folder = work.join("jpeg");
        let second_folder = work.join("mono_numbered_png");
        let first = first_folder.join("001.jpg");
        let second = second_folder.join("01.png");
        std::fs::create_dir_all(&first_folder).unwrap();
        std::fs::create_dir_all(&second_folder).unwrap();
        std::fs::write(&first, []).unwrap();
        std::fs::write(&second, []).unwrap();
        let mut history =
            HistoryStore::load_or_empty_from_path(directory.path().join("history.ini"));

        history.record_document(&first, Some(&root), 3, 10, false);
        assert_eq!(
            image_folder_progress(history.entries(), &first_folder)
                .unwrap()
                .fraction(),
            0.4
        );
        assert_eq!(
            image_folder_progress(history.entries(), &second_folder),
            None
        );

        history.record_document(&second, Some(&root), 8, 20, false);
        assert_eq!(history.entries().len(), 1);
        assert_eq!(
            image_folder_progress(history.entries(), &first_folder),
            None
        );
        assert_eq!(
            image_folder_progress(history.entries(), &second_folder)
                .unwrap()
                .fraction(),
            0.45
        );
        let second_identity = FavoriteIdentity::image_folder(&second_folder);
        assert_eq!(
            library_badge_progress(history.entries(), &second_identity),
            Some(LibraryProgress::new(8, 20).unwrap())
        );

        history.record_document(&second, Some(&root), 18, 20, true);
        assert_eq!(
            library_badge_progress(history.entries(), &second_identity),
            None
        );
        assert_eq!(
            image_folder_progress(history.entries(), &second_folder),
            None
        );
    }

    #[test]
    fn image_folder_progress_keeps_different_works_independent() {
        let work_a = Path::new("/books/work-a");
        let work_b = Path::new("/books/work-b");
        let work_a_first = Path::new("/books/work-a/01巻/001.jpg");
        let work_a_second = Path::new("/books/work-a/02巻/001.jpg");
        let work_b_first = Path::new("/books/work-b/01巻/001.jpg");
        let entries = [
            work_history_entry(work_b, work_b_first, 5, 30),
            work_history_entry(work_a, work_a_second, 8, 20),
            work_history_entry(work_a, work_a_first, 3, 10),
        ];

        assert_eq!(
            image_folder_progress(&entries, Path::new("/books/work-a/01巻")),
            None
        );
        assert_eq!(
            image_folder_progress(&entries, Path::new("/books/work-a/02巻"))
                .unwrap()
                .fraction(),
            0.45
        );
        assert_eq!(
            image_folder_progress(&entries, Path::new("/books/work-b/01巻"))
                .unwrap()
                .fraction(),
            0.2
        );
    }

    #[test]
    fn standalone_image_folder_progress_remains_available() {
        let folder = Path::new("/books/01巻");
        let entries = [work_history_entry(
            folder,
            Path::new("/books/01巻/001.jpg"),
            2,
            12,
        )];

        assert_eq!(
            image_folder_progress(&entries, folder).unwrap().fraction(),
            0.25
        );
    }

    #[test]
    fn archive_progress_uses_only_the_latest_volume_in_the_same_work() {
        let first_path = Path::new("/books/work/01.cbz");
        let latest_path = Path::new("/books/work/02.cbz");
        let work = Path::new("/books/work");
        let entries = vec![
            work_history_entry(work, latest_path, 0, 120),
            work_history_entry(work, first_path, 49, 100),
        ];

        assert_eq!(library_progress(&entries, first_path), None);
        let progress = library_progress(&entries, latest_path).unwrap();
        assert_eq!(progress.fraction(), 1.0 / 120.0);
        assert_eq!(progress.page_index(), 0);
    }

    #[test]
    fn progress_bar_width_is_clamped_to_the_cover_and_has_a_visible_minimum() {
        assert_eq!(progress_bar_width(112, 0.0), 5);
        assert_eq!(progress_bar_width(112, 0.5), 56);
        assert_eq!(progress_bar_width(112, 1.0), 112);
        assert_eq!(progress_bar_width(112, 2.0), 112);
        assert_eq!(progress_bar_width(0, 0.5), 0);
    }

    #[test]
    fn progress_bar_is_positioned_from_the_right_edge() {
        assert_eq!(progress_bar_x(112, 5), 107);
        assert_eq!(progress_bar_x(112, 56), 56);
        assert_eq!(progress_bar_x(112, 112), 0);
        assert_eq!(progress_bar_x(0, 0), 0);
    }
}
