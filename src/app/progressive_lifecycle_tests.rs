//! Headless App lifecycle checks using the same state owners as document_workflow.

use super::document_load::{DocumentLoadController, LoadPurpose};
use super::document_workflow::{
    accept_progressive_archive_image, append_progressive_archive_images_to_session,
    cancel_progressive_archive_session, finish_progressive_archive_session,
    retain_progressive_archive_session_for_replacement,
};
use super::progressive_archive::{PendingPageMove, ProgressiveArchiveState, ProgressiveArrival};
use super::progressive_thumbnail::{ProgressiveThumbnailController, ProgressiveThumbnailRequest};
use super::{
    AppView, LOADING_DISPLAY_DELAY, library_thumbnail_scope, slider_item_count,
    viewer_navigation_enabled,
};
use crate::archive::{
    ProgressiveArchiveCancelToken, ProgressiveArchiveImage, SequentialImageBacking,
};
use crate::document::{AssetId, Document, ImageLayout, ImageSource};
use crate::history::HistoryStore;
use crate::viewer::{ViewMode, ViewerSession};
use gtk::glib;
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct Lifecycle {
    load: DocumentLoadController,
    archive: ProgressiveArchiveState,
    viewer: ViewerSession,
    thumbnails: ProgressiveThumbnailController,
    history: HistoryStore,
    view: AppView,
    display_backup: Option<AppView>,
}

impl Lifecycle {
    fn new(history_path: PathBuf) -> Self {
        Self {
            load: DocumentLoadController::default(),
            archive: ProgressiveArchiveState::default(),
            viewer: ViewerSession::new(ViewMode::Single),
            thumbnails: ProgressiveThumbnailController::default(),
            history: HistoryStore::load_or_empty_from_path(history_path),
            view: AppView::Library,
            display_backup: None,
        }
    }

    fn start(&mut self, path: &Path) -> (u64, u64, ProgressiveArchiveCancelToken) {
        self.archive.cancel();
        let (id, revision) = self.load.begin(LoadPurpose::Normal);
        let token = self.archive.begin(id, path.to_path_buf(), 0, 1);
        (id, revision, token)
    }

    fn start_replacement(&mut self, path: &Path) -> (u64, u64, ProgressiveArchiveCancelToken) {
        assert!(retain_progressive_archive_session_for_replacement(
            &mut self.viewer,
            &mut self.archive,
            &mut self.thumbnails,
        ));
        self.display_backup = None;
        let (id, revision) = self.load.begin_replacement(LoadPurpose::Normal);
        let token = self.archive.begin(id, path.to_path_buf(), 0, 1);
        (id, revision, token)
    }

    // Drive the core transitions used by App::archive_image_extracted.
    fn extracted(&mut self, id: u64, path: &Path, image: ProgressiveArchiveImage) -> bool {
        let arrival = accept_progressive_archive_image(
            &self.load,
            &mut self.archive,
            &mut self.viewer,
            id,
            path,
            image,
        );
        let ProgressiveArrival::Contiguous(images) = arrival else {
            return false;
        };
        if self.archive.is_displayed() {
            append_progressive_archive_images_to_session(
                &mut self.viewer,
                &mut self.archive,
                &mut self.load,
                id,
                images,
            );
        } else {
            assert!(self.archive.mark_displayed(id));
            self.display_backup = Some(self.view);
            self.viewer.begin_progressive_archive_document(
                path.to_path_buf(),
                images
                    .into_iter()
                    .map(|image| (image.backing.source(), image.layout)),
                self.archive.temp_dir(id),
                self.archive.initial_index(id).unwrap(),
            );
            if self.load.is_replacement(id) {
                self.viewer.discard_progressive_rollback();
                self.display_backup = None;
            }
            self.thumbnails.reset();
            self.load.restart_loading_delay(id);
            self.view = AppView::Viewer;
        }
        true
    }

    fn finish(&mut self, id: u64, document: Document) -> bool {
        if !self.load.is_active(id) {
            return false;
        }
        if finish_progressive_archive_session(
            &mut self.viewer,
            &mut self.archive,
            &mut self.thumbnails,
            id,
            document,
        )
        .is_none()
        {
            return false;
        }
        self.display_backup = None;
        self.load.finish(id);
        let path = self.viewer.document_path().unwrap();
        self.history.record_document(
            path,
            None,
            self.viewer.current_index(),
            self.viewer.page_count(),
            self.viewer.at_document_end(),
        );
        true
    }

    fn fail(&mut self, id: u64) {
        if self.archive.is_displayed_for(id) {
            cancel_progressive_archive_session(
                &mut self.viewer,
                &mut self.archive,
                &mut self.thumbnails,
            );
            self.view = self.display_backup.take().unwrap_or(self.view);
        } else {
            self.archive.finish(id);
        }
        self.load.finish(id);
    }

    fn leave_for_library(&mut self) {
        cancel_progressive_archive_session(
            &mut self.viewer,
            &mut self.archive,
            &mut self.thumbnails,
        );
        self.load.cancel();
        self.display_backup = None;
        self.view = AppView::Library;
    }
}

fn image(index: usize, total: usize, layout: ImageLayout) -> ProgressiveArchiveImage {
    ProgressiveArchiveImage {
        physical_index: index,
        total_physical_images: total,
        backing: SequentialImageBacking::memory(glib::Bytes::from_owned(vec![index as u8])),
        layout,
    }
}

fn spilled_image(
    index: usize,
    total: usize,
    temp_dir: &Arc<tempfile::TempDir>,
) -> ProgressiveArchiveImage {
    let image = image(index, total, ImageLayout::Single);
    let path = crate::archive::sequential_image_path(temp_dir.path(), index);
    std::fs::write(&path, [index as u8]).unwrap();
    image.backing.replace_with_file(path, temp_dir.clone());
    image
}

#[test]
fn progressive_happy_lifecycle_keeps_position_until_formal_document_commit() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("book.rar");
    let mut app = Lifecycle::new(root.path().join("history.ini"));
    let (id, initial_revision, token) = app.start(&path);
    assert_eq!(LOADING_DISPLAY_DELAY.as_millis(), 200);
    assert!(!token.is_cancelled());
    assert!(!app.load.loading_visible());
    assert!(!viewer_navigation_enabled(true, false, false));
    assert!(!app.extracted(id, &path, image(1, 4, ImageLayout::Single)));
    assert_eq!(app.view, AppView::Library);

    assert!(app.extracted(id, &path, image(0, 4, ImageLayout::Single)));
    assert_eq!(app.view, AppView::Viewer);
    assert_eq!(app.viewer.physical_image_count(), 2);
    assert_eq!(
        slider_item_count(app.archive.total_physical_images(), app.viewer.page_count()),
        4
    );
    assert!(!app.load.show_loading(id, initial_revision));
    assert!(viewer_navigation_enabled(true, true, false));
    assert!(app.history.entries().is_empty());
    let document_generation = app.viewer.document_generation();
    let hover = app
        .thumbnails
        .request([ProgressiveThumbnailRequest {
            document_path: path.clone(),
            asset_id: AssetId(0),
            first_page: 0,
            layout: ImageLayout::Single,
            source: app
                .viewer
                .progressive_thumbnail_source(AssetId(0))
                .unwrap()
                .2,
            temp_dir: None,
            preview_generation: 1,
        }])
        .unwrap();
    assert!(
        app.thumbnails
            .accepts_completion(hover.session_generation, AssetId(0))
    );
    assert!(
        app.viewer
            .progressive_thumbnail_source(AssetId(3))
            .is_none()
    );

    assert!(app.archive.request_move(PendingPageMove::PhysicalImage(3)));
    assert!(!app.archive.request_move(PendingPageMove::Next));
    let pending_revision = app.load.restart_loading_delay(id).unwrap();
    assert!(!app.load.loading_visible());
    assert!(!viewer_navigation_enabled(true, true, true));
    assert!(app.load.show_loading(id, pending_revision));
    assert!(!app.extracted(id, &path, image(3, 4, ImageLayout::Single)));
    assert_eq!(app.archive.pending_physical_target(), Some(3));
    assert!(app.extracted(id, &path, image(2, 4, ImageLayout::Spread)));
    assert_eq!(app.viewer.physical_image_count(), 4);
    assert_eq!(app.viewer.current_physical_image_index(), Some(3));
    assert_eq!(app.archive.pending_move(), None);
    let resolved_position = app.viewer.current_index();
    assert!(!app.extracted(id, &path, image(2, 4, ImageLayout::Spread)));
    assert_eq!(app.viewer.current_index(), resolved_position);
    assert!(!app.load.loading_visible());
    assert!(!app.load.show_loading(id, pending_revision));
    assert_eq!(app.viewer.document_generation(), document_generation);
    assert!(
        app.viewer
            .progressive_thumbnail_source(AssetId(3))
            .is_some()
    );
    assert!(app.viewer.set_view_mode(ViewMode::Spread));
    let position_before_completion = app.viewer.current_index();

    let mut complete = Document::new(path.clone(), None);
    for layout in [
        ImageLayout::Single,
        ImageLayout::Single,
        ImageLayout::Spread,
        ImageLayout::Single,
    ] {
        complete.add_asset(
            ImageSource::Memory(glib::Bytes::from_static(b"image")),
            layout,
        );
    }
    assert!(app.finish(id, complete));
    assert_eq!(app.viewer.current_index(), position_before_completion);
    assert_eq!(app.viewer.document_generation(), document_generation);
    assert_eq!(app.viewer.view_mode(), ViewMode::Spread);
    assert_eq!(
        slider_item_count(app.archive.total_physical_images(), app.viewer.page_count()),
        5
    );
    assert_eq!(app.history.entries().len(), 1);
    assert_eq!(app.history.entries()[0].last_document_path, path);
    assert!(!app.load.has_active_request());
    assert!(!app.archive.is_displayed());
    assert!(
        !app.thumbnails
            .accepts_completion(hover.session_generation, AssetId(0))
    );
}

#[test]
fn resource_limit_failure_after_partial_display_discards_pending_loading_history_and_temp() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("broken.cb7");
    let temp_dir = Arc::new(tempfile::tempdir_in(root.path()).unwrap());
    let temp_path = temp_dir.path().to_path_buf();
    let mut app = Lifecycle::new(root.path().join("history.ini"));
    let (id, _, token) = app.start(&path);
    assert!(app.extracted(id, &path, spilled_image(0, 3, &temp_dir)));
    drop(temp_dir);
    assert!(app.archive.request_move(PendingPageMove::PhysicalImage(2)));
    let revision = app.load.restart_loading_delay(id).unwrap();
    assert!(app.load.show_loading(id, revision));
    app.fail(id);
    assert!(token.is_cancelled());
    assert!(!app.load.loading_visible());
    assert_eq!(app.archive.pending_move(), None);
    assert_eq!(app.viewer.document_path(), None);
    assert_eq!(app.view, AppView::Library);
    assert!(app.history.entries().is_empty());
    assert!(!temp_path.exists());
    assert!(!app.extracted(id, &path, image(1, 3, ImageLayout::Single)));
    assert!(!app.finish(id, Document::new(path.clone(), None)));
    assert!(!app.load.show_loading(id, revision));
}

#[test]
fn resource_limit_failure_before_display_keeps_the_previous_document_and_ends_loading() {
    let root = tempfile::tempdir().unwrap();
    let previous = root.path().join("previous.cbz");
    let replacement = root.path().join("limited.rar");
    let mut app = Lifecycle::new(root.path().join("history.ini"));
    let mut document = Document::new(previous.clone(), None);
    document.add_asset(
        ImageSource::Memory(glib::Bytes::from_static(b"previous")),
        ImageLayout::Single,
    );
    app.viewer.replace_document(document, 0);
    app.view = AppView::Viewer;

    let (id, _, _token) = app.start(&replacement);
    let revision = app.load.restart_loading_delay(id).unwrap();
    assert!(app.load.show_loading(id, revision));
    app.fail(id);

    assert_eq!(app.viewer.document_path(), Some(previous.as_path()));
    assert_eq!(app.view, AppView::Viewer);
    assert!(!app.load.loading_visible());
    assert!(!app.load.has_active_request());
    assert!(app.history.entries().is_empty());
}

#[test]
fn resource_limit_failure_before_display_keeps_library_and_ends_loading() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("limited.cbz");
    let mut app = Lifecycle::new(root.path().join("history.ini"));
    let (id, revision, _) = app.start(&path);
    assert!(app.load.show_loading(id, revision));

    app.fail(id);

    assert_eq!(app.view, AppView::Library);
    assert!(!app.load.has_active_request());
    assert!(!app.load.loading_visible());
    assert!(app.history.entries().is_empty());
    assert!(!app.load.show_loading(id, revision));
}

#[test]
fn replacement_rejects_every_old_notification_and_releases_retained_backing_on_new_display() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a.rar");
    let b = root.path().join("b.7z");
    let temp_dir = Arc::new(tempfile::tempdir_in(root.path()).unwrap());
    let temp_path = temp_dir.path().to_path_buf();
    let mut app = Lifecycle::new(root.path().join("history.ini"));
    let (old, old_revision, old_token) = app.start(&a);
    assert!(app.extracted(old, &a, spilled_image(0, 3, &temp_dir)));
    drop(temp_dir);
    assert!(app.archive.request_move(PendingPageMove::PhysicalImage(2)));
    let old_document_generation = app.viewer.document_generation();
    let (current, current_revision, current_token) = app.start_replacement(&b);
    assert!(old_token.is_cancelled());
    assert!(!current_token.is_cancelled());
    assert!(temp_path.exists());
    assert!(!app.extracted(old, &a, image(1, 3, ImageLayout::Single)));
    assert!(!app.finish(old, Document::new(a.clone(), None)));
    app.fail(old);
    assert!(!app.load.show_loading(old, old_revision));
    assert!(app.load.is_active(current));
    assert_eq!(app.viewer.document_generation(), old_document_generation);
    assert_eq!(app.viewer.document_path(), Some(a.as_path()));
    assert_eq!(app.archive.pending_move(), None);
    assert!(app.history.entries().is_empty());

    assert!(app.extracted(current, &b, image(0, 2, ImageLayout::Single)));
    assert_eq!(app.viewer.document_path(), Some(b.as_path()));
    assert_ne!(app.viewer.document_generation(), old_document_generation);
    assert!(!temp_path.exists());
    assert!(!app.load.show_loading(current, current_revision));
    assert!(!app.extracted(old, &a, image(2, 3, ImageLayout::Single)));
    assert_eq!(app.viewer.document_path(), Some(b.as_path()));
    assert_eq!(app.archive.total_physical_images(), Some(2));
    assert!(app.history.entries().is_empty());
}

#[test]
fn library_exit_cancels_worker_and_rejects_late_progress() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("book.rar");
    let temp_dir = Arc::new(tempfile::tempdir_in(root.path()).unwrap());
    let temp_path = temp_dir.path().to_path_buf();
    let mut app = Lifecycle::new(root.path().join("history.ini"));
    let (id, revision, token) = app.start(&path);
    assert!(app.extracted(id, &path, spilled_image(0, 2, &temp_dir)));
    drop(temp_dir);
    assert!(app.archive.request_move(PendingPageMove::Next));
    app.leave_for_library();
    assert!(token.is_cancelled());
    assert_eq!(app.view, AppView::Library);
    assert_eq!(app.archive.pending_move(), None);
    assert!(!app.load.loading_visible());
    assert!(!temp_path.exists());
    assert!(!app.extracted(id, &path, image(1, 2, ImageLayout::Single)));
    assert!(!app.finish(id, Document::new(path, None)));
    app.fail(id);
    assert!(!app.load.show_loading(id, revision));
    assert_eq!(app.view, AppView::Library);
    assert!(app.history.entries().is_empty());
    assert!(matches!(
        library_thumbnail_scope(app.view),
        super::LibraryThumbnailScope::Normal
    ));
}

#[test]
fn completed_document_keeps_spilled_backing_until_viewer_replaces_it() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("book.rar");
    let temp_dir = Arc::new(tempfile::tempdir_in(root.path()).unwrap());
    let temp_path = temp_dir.path().to_path_buf();
    let image_path = crate::archive::sequential_image_path(&temp_path, 0);
    let mut app = Lifecycle::new(root.path().join("history.ini"));
    let (id, _, _) = app.start(&path);
    assert!(app.extracted(id, &path, spilled_image(0, 1, &temp_dir)));
    let mut complete = Document::new(path, Some(temp_dir.clone()));
    complete.add_asset(ImageSource::File(image_path), ImageLayout::Single);
    drop(temp_dir);

    assert!(app.finish(id, complete));
    assert!(temp_path.exists());
    assert!(
        app.viewer
            .progressive_thumbnail_source(crate::document::AssetId(0))
            .is_some()
    );
    app.viewer
        .replace_document(Document::new(root.path().join("next.cbz"), None), 0);
    assert!(!temp_path.exists());
}
