use super::*;
use std::collections::HashMap;

pub(super) struct ViewerRuntimeState {
    pub(super) session: ViewerSession,
    pub(super) background: ViewerBackgroundScheduler,
    pub(super) smart_crop: SmartCropPreparationScheduler,
    pub(super) right_texture: Option<gtk::gdk::Paintable>,
    pub(super) left_texture: Option<gtk::gdk::Paintable>,
    pub(super) preview: PreviewController,
    pub(super) slider: SliderController,
    pub(super) header: HeaderController,
}

impl ViewerRuntimeState {
    pub(super) fn new(
        preview: PreviewController,
        settings: &UserSettings,
        sender: &AppSender,
    ) -> Self {
        let smart_crop = SmartCropPreparationScheduler::new({
            let sender = sender.clone();
            move |result: SmartCropPreparationResult| {
                sender.send(Msg::SmartCropPrepared {
                    document_generation: result.document_generation,
                    asset_id: result.asset_id,
                    page_index: result.page_index,
                    success: result.success,
                })
            }
        });
        let background = ViewerBackgroundScheduler::new(
            settings.thumbnail_generation_speed,
            smart_crop.handle(),
            {
                let sender = sender.clone();
                move |result| {
                    let message = match result {
                        ViewerBackgroundResult::Preload {
                            document_generation,
                            asset_id,
                            page_index,
                            bytes,
                        } => Msg::PreloadBytesReady {
                            document_generation,
                            asset_id,
                            page_index,
                            bytes,
                        },
                        ViewerBackgroundResult::Thumbnail(result) => Msg::ThumbnailReady {
                            document_generation: result.document_generation,
                            generation: result.generation,
                            index: result.index,
                            thumbnail: result.thumbnail,
                        },
                    };
                    sender.send(message)
                }
            },
        );
        let slider_auto_hide = effective_slider_auto_hide(settings.slider_auto_hide, false);

        Self {
            session: ViewerSession::new(settings.view_mode),
            background,
            smart_crop,
            right_texture: None,
            left_texture: None,
            preview,
            slider: SliderController::new(slider_auto_hide),
            header: HeaderController::new(),
        }
    }
}

#[derive(Default)]
pub(super) struct DocumentRuntimeState {
    pub(super) load: DocumentLoadController,
    pub(super) sibling_lookup: SiblingLookupController,
    pub(super) progressive_archive: ProgressiveArchiveState,
    pub(super) progressive_thumbnails: ProgressiveThumbnailController,
    pub(super) progressive_display_backup: Option<ProgressiveDisplayBackup>,
    pub(super) boundary: DocumentBoundaryState,
    pub(super) toast_overlay: Option<adw::ToastOverlay>,
    pub(super) toast: Option<adw::Toast>,
    pub(super) viewer_content: Option<gtk::Overlay>,
}

pub(super) struct LibraryRuntimeState {
    pub(super) archive: super::archive_library::ArchiveLibraryState,
    pub(super) external_cover_requests: ExternalCoverRequests,
    pub(super) model: LibraryState,
    pub(super) search: LibrarySearchState,
    pub(super) search_action: Option<gtk::gio::SimpleAction>,
    pub(super) search_entry: Option<gtk::SearchEntry>,
    pub(super) search_view: LibrarySearchView,
    pub(super) search_thumbnails: LibrarySearchThumbnailController,
    pub(super) view: LibraryView,
    pub(super) thumbnails: LibraryThumbnailController,
    pub(super) thumbnail_cache_hits: LibraryThumbnailCacheHitBatch,
    pub(super) thumbnail_batch: LibraryThumbnailBatch,
    pub(super) initial_reveal: LibraryInitialReveal,
}

impl LibraryRuntimeState {
    pub(super) fn new(bookshelf_root: Option<PathBuf>) -> Self {
        Self {
            archive: super::archive_library::ArchiveLibraryState::default(),
            external_cover_requests: ExternalCoverRequests::default(),
            model: LibraryState::new(bookshelf_root),
            search: LibrarySearchState::default(),
            search_action: None,
            search_entry: None,
            search_view: LibrarySearchView::default(),
            search_thumbnails: LibrarySearchThumbnailController::default(),
            view: LibraryView::default(),
            thumbnails: LibraryThumbnailController::default(),
            thumbnail_cache_hits: LibraryThumbnailCacheHitBatch::default(),
            thumbnail_batch: LibraryThumbnailBatch::default(),
            initial_reveal: LibraryInitialReveal::default(),
        }
    }

    pub(super) fn begin_thumbnail_scope(&mut self) -> u64 {
        let generation = self.thumbnails.begin_directory();
        self.thumbnail_cache_hits.begin_generation(generation);
        self.thumbnail_batch.begin_generation(generation);
        self.initial_reveal.begin_directory(generation);
        generation
    }

    pub(super) fn invalidate_cover_override(&mut self, search_selected: bool) -> u64 {
        self.view.invalidate_cover_textures();
        let generation = self.begin_thumbnail_scope();
        self.search_view.invalidate_cover_textures();
        if !search_selected {
            self.search_thumbnails.begin_generation();
        }
        generation
    }
}

#[derive(Debug, Default)]
pub(super) struct ExternalCoverRequests {
    next: u64,
    active: HashMap<crate::covers::CoverBookIdentity, u64>,
}

impl ExternalCoverRequests {
    pub(super) fn begin(&mut self, identity: &crate::covers::CoverBookIdentity) -> u64 {
        self.next = self.next.wrapping_add(1);
        self.active.insert(identity.clone(), self.next);
        self.next
    }

    pub(super) fn accepts(
        &self,
        identity: &crate::covers::CoverBookIdentity,
        request_id: u64,
    ) -> bool {
        self.active.get(identity).copied() == Some(request_id)
    }

    pub(super) fn finish(
        &mut self,
        identity: &crate::covers::CoverBookIdentity,
        request_id: u64,
    ) -> bool {
        if !self.accepts(identity, request_id) {
            return false;
        }
        self.active.remove(identity);
        true
    }

    pub(super) fn cancel(&mut self, identity: &crate::covers::CoverBookIdentity) {
        self.active.remove(identity);
    }
}

#[cfg(test)]
mod external_cover_request_tests {
    use super::*;

    #[test]
    fn only_latest_request_for_same_book_is_accepted() {
        let identity = crate::covers::CoverBookIdentity::Archive(PathBuf::from("book.cbz"));
        let mut requests = ExternalCoverRequests::default();
        let first = requests.begin(&identity);
        let second = requests.begin(&identity);

        assert!(!requests.finish(&identity, first));
        assert!(requests.finish(&identity, second));
    }

    #[test]
    fn cancelling_one_book_rejects_its_result_without_affecting_another_book() {
        let folder = crate::covers::CoverBookIdentity::image_folder(Path::new("folder"));
        let archive = crate::covers::CoverBookIdentity::Archive(PathBuf::from("book.cbz"));
        let mut requests = ExternalCoverRequests::default();
        let folder_request = requests.begin(&folder);
        let archive_request = requests.begin(&archive);

        requests.cancel(&folder);

        assert!(!requests.accepts(&folder, folder_request));
        assert!(!requests.finish(&folder, folder_request));
        assert!(requests.finish(&archive, archive_request));
    }

    #[test]
    fn cancelling_internal_cover_rejects_folder_and_archive_external_results() {
        let identities = [
            crate::covers::CoverBookIdentity::image_folder(Path::new("folder")),
            crate::covers::CoverBookIdentity::Archive(PathBuf::from("book.cbz")),
        ];

        for identity in identities {
            let mut requests = ExternalCoverRequests::default();
            let request_id = requests.begin(&identity);
            requests.cancel(&identity);
            assert!(!requests.accepts(&identity, request_id));
        }
    }
}

pub(super) struct NavigationRuntimeState {
    pub(super) panel: NavigationPanelController,
    pub(super) history: HistoryStore,
    pub(super) favorites: FavoritesStore,
    pub(super) current_history_identity: Option<HistoryIdentity>,
    pub(super) history_view: HistoryView,
    pub(super) favorites_view: FavoritesView,
}

impl NavigationRuntimeState {
    pub(super) fn load(settings: &UserSettings) -> Self {
        Self {
            panel: NavigationPanelController::default(),
            history: HistoryStore::load(settings.bookshelf_root.as_deref()),
            favorites: FavoritesStore::load(),
            current_history_identity: None,
            history_view: HistoryView::default(),
            favorites_view: FavoritesView::default(),
        }
    }
}

pub(super) struct AppShellState {
    pub(super) settings_dialog: SettingsDialog,
    pub(super) view: AppView,
    pub(super) main_window: Option<adw::ApplicationWindow>,
    pub(super) fullscreen: bool,
    pub(super) breadcrumb_bar: BreadcrumbBar,
    pub(super) ui: Option<UiHandles>,
}

impl AppShellState {
    pub(super) fn new(settings: &UserSettings, sender: &AppSender) -> Self {
        Self {
            settings_dialog: SettingsDialog::new(settings, sender.clone()),
            view: AppView::Library,
            main_window: None,
            fullscreen: false,
            breadcrumb_bar: BreadcrumbBar::default(),
            ui: None,
        }
    }
}
