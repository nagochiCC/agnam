use super::library_thumbnail::ThumbnailSourceKind;
use crate::archive::{
    ArchiveContentLevel, ArchiveImageId, ArchiveLocation, ProgressiveArchiveImage,
};
use crate::bookshelf::search::SearchIndex;
use crate::bookshelf::thumbnail::ThumbnailData;
use crate::bookshelf::{BookshelfDirectory, LibrarySortDirection, LibrarySortKey};
use crate::covers::{CoverBookIdentity, PreparedExternalCover};
use crate::document::{AssetId, Document};
use crate::favorites::FavoriteIdentity;
use crate::history::HistoryIdentity;
use crate::settings::{ClickMode, PreviewPositionMode, StartupBehavior};
use crate::thumbnail::Thumbnail;
use crate::viewer::{ThumbnailGenerationSpeed, ViewMode};
use gtk::glib;
use std::path::PathBuf;

#[derive(Debug)]
pub(crate) enum Msg {
    OpenFile,
    NavigateUp,
    SelectBookshelfRoot,
    BookshelfRootSelected(PathBuf),
    ToggleSearchPanel,
    ToggleHistoryPanel,
    ToggleFavoritesPanel,
    CloseNavigationPanel,
    NavigationDrawerClosed,
    SetLibrarySortKey(LibrarySortKey),
    SetLibrarySortDirection(LibrarySortDirection),
    HistoryCoverCacheLoadFinished {
        generation: u64,
        source: PathBuf,
        thumbnail: Option<ThumbnailData>,
    },
    HistoryCoverGenerationFinished {
        generation: u64,
        source: PathBuf,
        // Ok(Some) succeeded, Err failed, Ok(None) was cancelled. All three
        // arrive so the scheduler can release the physical worker slot.
        result: Result<Option<ThumbnailData>, String>,
    },
    HistoryCoverDemandChanged,
    ResumeHistory {
        identity: HistoryIdentity,
        path: PathBuf,
        page_index: usize,
        at_document_end: bool,
    },
    RemoveHistory(HistoryIdentity),
    ConfirmClearHistory,
    ClearHistory,
    FavoritesCoverCacheLoadFinished {
        generation: u64,
        identity: FavoriteIdentity,
        source: Option<PathBuf>,
        thumbnail: Option<ThumbnailData>,
    },
    FavoritesCoverGenerationFinished {
        generation: u64,
        identity: FavoriteIdentity,
        source: PathBuf,
        result: Result<Option<ThumbnailData>, String>,
    },
    FavoritesCoverDemandChanged,
    OpenFavorite(FavoriteIdentity),
    FavoriteOpenResolved {
        identity: FavoriteIdentity,
        entry_path: Option<PathBuf>,
    },
    ToggleFavorite(FavoriteIdentity),
    RemoveFavorite(FavoriteIdentity),
    LibrarySearchQueryChanged(String),
    LibrarySearchBuildFinished {
        request_id: u64,
        root: PathBuf,
        result: Result<SearchIndex, String>,
    },
    LibrarySearchThumbnailViewportChanged,
    LibrarySearchThumbnailCacheLoadFinished {
        generation: u64,
        source: PathBuf,
        thumbnail: Option<ThumbnailData>,
    },
    LibrarySearchThumbnailGenerationFinished {
        generation: u64,
        source: PathBuf,
        result: Result<Option<ThumbnailData>, String>,
    },
    LibrarySearchFolderSelected {
        path: PathBuf,
        image_document_entry: Option<PathBuf>,
    },
    LibrarySearchArchiveSelected(PathBuf),
    LibraryFileSelected(PathBuf),
    LibraryFileReadFromStart(PathBuf),
    LibraryImageFolderSelected {
        folder: PathBuf,
        entry: PathBuf,
    },
    LibraryImageFolderReadFromStart(PathBuf),
    OpenArchiveContents(PathBuf),
    NavigateArchiveContents(ArchiveLocation),
    ArchiveContentsLoaded {
        request_id: u64,
        result: Result<ArchiveContentLevel, String>,
    },
    ArchiveContentThumbnailReady {
        session_id: u64,
        source: PathBuf,
        backed: bool,
        thumbnail: ThumbnailData,
    },
    ArchiveContentThumbnailsFinished {
        session_id: u64,
    },
    ArchiveContentThumbnailResourceLimit {
        session_id: u64,
    },
    ArchiveImageSelected {
        archive: PathBuf,
        id: ArchiveImageId,
    },
    SetFolderImageCover {
        folder: PathBuf,
        image: PathBuf,
    },
    SetArchiveImageCover {
        archive: PathBuf,
        id: ArchiveImageId,
    },
    ClearCoverOverride(CoverBookIdentity),
    SelectExternalCover(CoverBookIdentity),
    ExternalCoverSelected {
        identity: CoverBookIdentity,
        request_id: u64,
        path: PathBuf,
    },
    ExternalCoverPrepared {
        identity: CoverBookIdentity,
        request_id: u64,
        result: Result<PreparedExternalCover, String>,
    },
    NavigateLibrary(PathBuf),
    LibraryScanFinished {
        request_id: u64,
        path: PathBuf,
        result: Result<BookshelfDirectory, String>,
    },
    LibraryThumbnailViewportChanged,
    LibraryThumbnailCacheLoadFinished {
        generation: u64,
        cohort_id: u64,
        source: PathBuf,
        kind: ThumbnailSourceKind,
        thumbnail: Option<ThumbnailData>,
    },
    LibraryThumbnailGenerationFinished {
        generation: u64,
        source: PathBuf,
        kind: ThumbnailSourceKind,
        result: Result<Option<ThumbnailData>, String>,
    },
    FlushLibraryThumbnailCacheHits {
        generation: u64,
    },
    LibraryInitialRevealTimeout {
        generation: u64,
        revision: u64,
    },
    FlushLibraryThumbnails {
        generation: u64,
        batch_id: u64,
        quiet_revision: Option<u64>,
    },
    PathDropped(PathBuf),
    PathSelected(PathBuf),
    SiblingPathLookupFinished {
        request_id: u64,
        adjacent_path: Option<PathBuf>,
    },
    BoundaryPathLookupFinished {
        request_id: u64,
        adjacent_path: Option<PathBuf>,
    },
    OpenDocument {
        load_request_id: u64,
        document: Document,
        initial_index: usize,
    },
    ArchiveImageExtracted {
        load_request_id: u64,
        path: PathBuf,
        image: ProgressiveArchiveImage,
    },
    ShowLoading {
        load_request_id: u64,
        revision: u64,
    },
    LoadRequestFinished {
        load_request_id: u64,
    },
    ArchiveResourceLimitReached {
        load_request_id: u64,
    },
    FocusViewerAfterDocumentLoad,
    DocumentToastDismissed,
    NextPage,
    PrevPage,
    NextSinglePage,
    PrevSinglePage,
    NextFile,
    PrevFile,
    ToggleViewMode,
    SetClickMode(ClickMode),
    SetShowDocumentBoundaryPage(bool),
    SetScaleUp(bool),
    SetSmartCrop(bool),
    SetViewMode(ViewMode),
    SetSliderAutoHide(bool),
    SetHeaderAutoHide(bool),
    SetThumbnailsEnabled(bool),
    SetArchiveExpansionLimit(crate::archive::ArchiveExpansionLimit),
    SetThumbnailGenerationSpeed(ThumbnailGenerationSpeed),
    UpdateSliderVisibility {
        distance_from_bottom: f64,
        pointer_over_bar: bool,
    },
    UpdateHeaderVisibility {
        distance_from_top: f64,
        pointer_over_header: bool,
    },
    FullscreenChanged(bool),
    SetSliderDragging(bool),
    PreloadBytesReady {
        document_generation: u64,
        asset_id: AssetId,
        page_index: usize,
        bytes: glib::Bytes,
    },
    SmartCropPrepared {
        document_generation: u64,
        asset_id: AssetId,
        page_index: usize,
        success: bool,
    },
    DecodeBytes {
        document_generation: u64,
        asset_id: AssetId,
        page_index: usize,
    },
    ThumbnailReady {
        document_generation: u64,
        generation: u64,
        index: usize,
        thumbnail: Thumbnail,
    },
    ProgressiveThumbnailFinished {
        session_generation: u64,
        preview_generation: u64,
        asset_id: AssetId,
        first_page: usize,
        layout: crate::document::ImageLayout,
        thumbnails: Option<Vec<(usize, Thumbnail)>>,
    },
    HoverPreview {
        value: f64,
        pointer_x: i32,
        slider_width: i32,
        slider_height: i32,
    },
    PreviewHoldExpired {
        hover_revision: u64,
    },
    HidePreview,
    SetPreviewPosition(PreviewPositionMode),
    SetLibraryBookHeight(i32),
    SetStartupBehavior(StartupBehavior),
    WindowStateChanged {
        width: i32,
        height: i32,
        maximized: bool,
        fullscreened: bool,
    },
    ImageClick {
        button: u32,
        x: f64,
        width: i32,
    },
    SetPage(f64),
    SetPageFromSliderPointer(f64),
    Scroll {
        dy: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DispatchDomain {
    Navigation,
    History,
    Favorites,
    Library,
    LibrarySearch,
    Document,
    Viewer,
    SettingsWindow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MsgInputPolicy {
    Unrestricted,
    ViewerOnly,
    ViewerNavigation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MsgRoute {
    pub(super) domain: DispatchDomain,
    pub(super) input_policy: MsgInputPolicy,
}

impl MsgInputPolicy {
    pub(super) fn allows(self, viewer_active: bool, viewer_navigation_enabled: bool) -> bool {
        match self {
            Self::Unrestricted => true,
            Self::ViewerOnly => viewer_active,
            Self::ViewerNavigation => viewer_active && viewer_navigation_enabled,
        }
    }
}

impl Msg {
    pub(super) fn route(&self) -> MsgRoute {
        use DispatchDomain::{
            Document, Favorites, History, Library, LibrarySearch, Navigation, SettingsWindow,
            Viewer,
        };
        use MsgInputPolicy::{Unrestricted, ViewerNavigation, ViewerOnly};

        match self {
            Self::OpenFile
            | Self::NavigateUp
            | Self::ToggleSearchPanel
            | Self::ToggleHistoryPanel
            | Self::ToggleFavoritesPanel
            | Self::CloseNavigationPanel
            | Self::NavigationDrawerClosed => MsgRoute {
                domain: Navigation,
                input_policy: Unrestricted,
            },
            Self::HistoryCoverCacheLoadFinished { .. }
            | Self::HistoryCoverGenerationFinished { .. }
            | Self::HistoryCoverDemandChanged
            | Self::ResumeHistory { .. }
            | Self::RemoveHistory(_)
            | Self::ConfirmClearHistory
            | Self::ClearHistory => MsgRoute {
                domain: History,
                input_policy: Unrestricted,
            },
            Self::FavoritesCoverCacheLoadFinished { .. }
            | Self::FavoritesCoverGenerationFinished { .. }
            | Self::FavoritesCoverDemandChanged
            | Self::OpenFavorite(_)
            | Self::FavoriteOpenResolved { .. }
            | Self::ToggleFavorite(_)
            | Self::RemoveFavorite(_) => MsgRoute {
                domain: Favorites,
                input_policy: Unrestricted,
            },
            Self::SelectBookshelfRoot
            | Self::BookshelfRootSelected(_)
            | Self::SetLibrarySortKey(_)
            | Self::SetLibrarySortDirection(_)
            | Self::LibraryFileSelected(_)
            | Self::LibraryFileReadFromStart(_)
            | Self::LibraryImageFolderSelected { .. }
            | Self::LibraryImageFolderReadFromStart(_)
            | Self::OpenArchiveContents(_)
            | Self::NavigateArchiveContents(_)
            | Self::ArchiveContentsLoaded { .. }
            | Self::ArchiveContentThumbnailReady { .. }
            | Self::ArchiveContentThumbnailsFinished { .. }
            | Self::ArchiveContentThumbnailResourceLimit { .. }
            | Self::ArchiveImageSelected { .. }
            | Self::SetFolderImageCover { .. }
            | Self::SetArchiveImageCover { .. }
            | Self::ClearCoverOverride(_)
            | Self::SelectExternalCover(_)
            | Self::ExternalCoverSelected { .. }
            | Self::ExternalCoverPrepared { .. }
            | Self::NavigateLibrary(_)
            | Self::LibraryScanFinished { .. }
            | Self::LibraryThumbnailViewportChanged
            | Self::LibraryThumbnailCacheLoadFinished { .. }
            | Self::LibraryThumbnailGenerationFinished { .. }
            | Self::FlushLibraryThumbnailCacheHits { .. }
            | Self::LibraryInitialRevealTimeout { .. }
            | Self::FlushLibraryThumbnails { .. } => MsgRoute {
                domain: Library,
                input_policy: Unrestricted,
            },
            Self::LibrarySearchQueryChanged(_)
            | Self::LibrarySearchBuildFinished { .. }
            | Self::LibrarySearchThumbnailViewportChanged
            | Self::LibrarySearchThumbnailCacheLoadFinished { .. }
            | Self::LibrarySearchThumbnailGenerationFinished { .. }
            | Self::LibrarySearchFolderSelected { .. }
            | Self::LibrarySearchArchiveSelected(_) => MsgRoute {
                domain: LibrarySearch,
                input_policy: Unrestricted,
            },
            Self::PathDropped(_)
            | Self::PathSelected(_)
            | Self::SiblingPathLookupFinished { .. }
            | Self::BoundaryPathLookupFinished { .. }
            | Self::OpenDocument { .. }
            | Self::ArchiveImageExtracted { .. }
            | Self::ShowLoading { .. }
            | Self::LoadRequestFinished { .. }
            | Self::ArchiveResourceLimitReached { .. }
            | Self::FocusViewerAfterDocumentLoad
            | Self::DocumentToastDismissed => MsgRoute {
                domain: Document,
                input_policy: Unrestricted,
            },
            Self::NextPage
            | Self::PrevPage
            | Self::NextSinglePage
            | Self::PrevSinglePage
            | Self::NextFile
            | Self::PrevFile
            | Self::ImageClick { .. }
            | Self::SetPage(_)
            | Self::SetPageFromSliderPointer(_)
            | Self::Scroll { .. } => MsgRoute {
                domain: Viewer,
                input_policy: ViewerNavigation,
            },
            Self::UpdateSliderVisibility { .. }
            | Self::SetSliderDragging(_)
            | Self::HoverPreview { .. } => MsgRoute {
                domain: Viewer,
                input_policy: ViewerOnly,
            },
            Self::ToggleViewMode
            | Self::UpdateHeaderVisibility { .. }
            | Self::PreloadBytesReady { .. }
            | Self::SmartCropPrepared { .. }
            | Self::DecodeBytes { .. }
            | Self::ThumbnailReady { .. }
            | Self::ProgressiveThumbnailFinished { .. }
            | Self::PreviewHoldExpired { .. }
            | Self::HidePreview => MsgRoute {
                domain: Viewer,
                input_policy: Unrestricted,
            },
            Self::SetClickMode(_)
            | Self::SetShowDocumentBoundaryPage(_)
            | Self::SetScaleUp(_)
            | Self::SetSmartCrop(_)
            | Self::SetViewMode(_)
            | Self::SetSliderAutoHide(_)
            | Self::SetHeaderAutoHide(_)
            | Self::SetThumbnailsEnabled(_)
            | Self::SetArchiveExpansionLimit(_)
            | Self::SetThumbnailGenerationSpeed(_)
            | Self::FullscreenChanged(_)
            | Self::SetPreviewPosition(_)
            | Self::SetLibraryBookHeight(_)
            | Self::SetStartupBehavior(_)
            | Self::WindowStateChanged { .. } => MsgRoute {
                domain: SettingsWindow,
                input_policy: Unrestricted,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(msg: &Msg, viewer_active: bool, navigation_enabled: bool) -> bool {
        msg.route()
            .input_policy
            .allows(viewer_active, navigation_enabled)
    }

    fn assert_route(msg: Msg, domain: DispatchDomain, input_policy: MsgInputPolicy) {
        assert_eq!(
            msg.route(),
            MsgRoute {
                domain,
                input_policy,
            }
        );
    }

    #[test]
    fn viewer_navigation_requires_an_active_navigable_viewer() {
        assert!(!allowed(&Msg::NextPage, false, true));
        assert!(allowed(&Msg::NextPage, true, true));
        assert!(!allowed(&Msg::NextPage, true, false));
    }

    #[test]
    fn viewer_only_and_unrestricted_messages_have_distinct_policies() {
        let slider = Msg::SetSliderDragging(true);
        assert!(!allowed(&slider, false, false));
        assert!(allowed(&slider, true, false));

        assert!(allowed(&Msg::OpenFile, false, false));
        assert!(allowed(&Msg::OpenFile, true, false));
        assert!(allowed(&Msg::NavigateUp, false, false));
    }

    #[test]
    fn representative_messages_keep_their_domain_and_input_policy_pairs() {
        use DispatchDomain::{
            Document, Favorites, History, Library, LibrarySearch, Navigation, SettingsWindow,
            Viewer,
        };
        use MsgInputPolicy::{Unrestricted, ViewerNavigation, ViewerOnly};

        assert_route(Msg::OpenFile, Navigation, Unrestricted);
        assert_route(Msg::NavigateUp, Navigation, Unrestricted);
        assert_route(Msg::ConfirmClearHistory, History, Unrestricted);
        assert_route(Msg::FavoritesCoverDemandChanged, Favorites, Unrestricted);
        assert_route(Msg::LibraryThumbnailViewportChanged, Library, Unrestricted);
        assert_route(
            Msg::LibrarySearchThumbnailViewportChanged,
            LibrarySearch,
            Unrestricted,
        );
        assert_route(Msg::FocusViewerAfterDocumentLoad, Document, Unrestricted);
        assert_route(Msg::NextPage, Viewer, ViewerNavigation);
        assert_route(Msg::SetPageFromSliderPointer(1.0), Viewer, ViewerNavigation);
        assert_route(Msg::SetSliderDragging(true), Viewer, ViewerOnly);
        assert_route(Msg::ToggleViewMode, Viewer, Unrestricted);
        assert_route(Msg::FullscreenChanged(true), SettingsWindow, Unrestricted);
    }
}
