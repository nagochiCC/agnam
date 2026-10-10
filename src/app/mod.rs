mod actions;
mod archive_library;
mod archive_thumbnail_backing;
mod background;
mod breadcrumb;
mod cover_cancel;
mod cover_scheduler;
mod dispatch;
mod document_boundary;
mod document_load;
mod document_workflow;
mod drop_target;
mod favorites;
mod file_dialog;
mod header;
mod history;
mod input;
mod library;
#[cfg(test)]
mod library_scope_regression_tests;
mod library_search;
mod library_search_thumbnail;
mod library_search_view;
mod library_search_workflow;
mod library_thumbnail;
mod library_workflow;
mod list_model_sync;
mod message;
mod navigation_panel;
mod navigation_workflow;
mod preferences;
mod preview;
mod progressive_archive;
#[cfg(test)]
mod progressive_lifecycle_tests;
mod progressive_thumbnail;
mod runtime;
mod sender;
mod settings_workflow;
mod shortcuts;
mod slider;
mod startup;
mod state;
mod thumbnail_texture;
mod ui_sync;
mod viewer_workflow;
mod widgets;
mod wiring;

pub(crate) use background::spawn_background;
pub(crate) use message::Msg;
pub(crate) use sender::AppSender;

use crate::archive::navigation::{find_sibling_file, sibling_container};
use crate::archive::{
    ProgressiveArchiveCancelToken, ProgressiveArchiveImage, ProgressiveArchiveLoadOutcome,
    archive_supports_sequential_progress, load_document_from_path_with_cancel,
    load_document_from_path_with_sequential_progress,
};
#[cfg(test)]
use crate::bookshelf::cache::generate_and_cache as generate_and_cache_bookshelf_thumbnail;
use crate::bookshelf::cache::{
    generate_and_cache_direct as generate_direct_bookshelf_thumbnail,
    generate_and_cache_with_cancel as generate_and_cache_bookshelf_thumbnail_with_cancel,
    load_cached as load_cached_bookshelf_thumbnail,
    load_cached_direct as load_cached_direct_bookshelf_thumbnail,
};
use crate::bookshelf::scan_directory;
use crate::bookshelf::{LibrarySortDirection, LibrarySortKey};
use crate::document::{AssetId, Document};
use crate::favorites::{FavoriteIdentity, FavoritesStore};
use crate::history::{HistoryIdentity, HistoryStore};
use crate::settings::{
    ClickMode, LIBRARY_BOOK_HEIGHT_MAX, LIBRARY_BOOK_HEIGHT_MIN, PreviewPositionMode,
    StartupBehavior, UserSettings,
};
use crate::thumbnail::{Thumbnail, make_memory_asset_thumbnails};
use crate::viewer::{
    HoverThumbnailRequest, NearThumbnailRequest, PreloadRequest, SmartCropPreparationRequest,
    SmartCropPreparationResult, SmartCropPreparationScheduler, ThumbnailGenerationSpeed, ViewMode,
    ViewerBackgroundResult, ViewerBackgroundScheduler, ViewerSession, snap_to_view,
};
use adw::prelude::*;
use breadcrumb::{BreadcrumbBar, archive_segments, library_segments, viewer_segments};
use document_boundary::{
    BoundaryDirection, BoundaryEntryDecision, DocumentBoundaryState, boundary_entry_decision,
    document_display_name,
};
use document_load::{
    DocumentLoadController, InitialPage, LoadPurpose, SiblingLookupCompletion,
    SiblingLookupController,
};
use drop_target::resolve_dropped_path;
use favorites::{FavoritesCoverJob, FavoritesView};
use file_dialog::{show_bookshelf_folder_dialog, show_file_dialog};
use gtk::glib::clone;
use header::HeaderController;
use history::{HistoryCoverJob, HistoryView};
use input::{ClickDirection, decide_click_direction};
use library::{
    BookshelfDisplaySizes, LibraryState, LibraryView, ScanRequest, favorite_progress,
    image_folder_progress, library_progress,
};
use library_search::{LibrarySearchState, SearchBuildRequest};
use library_search_thumbnail::{
    LibrarySearchThumbnailController, SearchThumbnailDemandEvaluation, SearchThumbnailJobs,
};
use library_search_view::LibrarySearchView;
use library_thumbnail::{
    CacheLookupBatchUpdate, CacheLookupCohort, InitialRevealTimeout, LibraryInitialReveal,
    LibraryThumbnailBatch, LibraryThumbnailCacheHitBatch, LibraryThumbnailController,
    ScheduledJobs, ThumbnailDemandEvaluation,
};
use navigation_panel::{NavigationPanel, NavigationPanelController};
use preferences::SettingsDialog;
use preview::{
    PREVIOUS_PREVIEW_HOLD_DURATION, PreviewController, PreviewTarget, nearest_ready_view,
};
use progressive_archive::{PendingPageMove, ProgressiveArchiveState, ProgressiveArrival};
use progressive_thumbnail::{
    ProgressiveThumbnailController, ProgressiveThumbnailJob, ProgressiveThumbnailRequest,
    nearest_ready_physical,
};
use slider::{
    SliderController, SliderNavigationInput, page_label_text, slider_navigation_target,
    slider_page_target,
};
use startup::{StartupAction, startup_action, update_last_session};
use state::{
    AppShellState, DocumentRuntimeState, LibraryRuntimeState, NavigationRuntimeState,
    ViewerRuntimeState,
};
use std::cell::RefCell;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;
use widgets::AppWidgets;
use wiring::{UiSignalHandlers, UiWiring};

const LOADING_DISPLAY_DELAY: Duration = Duration::from_millis(200);
const LIBRARY_THUMBNAIL_DEBOUNCE: Duration = Duration::from_millis(80);
const LIBRARY_THUMBNAIL_MAX_LATENCY: Duration = Duration::from_millis(240);
const LIBRARY_INITIAL_REVEAL_TIMEOUT: Duration = Duration::from_millis(100);
const NAVIGATION_DRAWER_MIN_WIDTH: f64 = 280.0;
const NAVIGATION_DRAWER_MAX_WIDTH: f64 = 320.0;
const NAVIGATION_DRAWER_WIDTH_FRACTION: f64 = 0.3;
const APP_CSS: &str = r#"
.navigation-rail {
    background-color: var(--sidebar-bg-color);
    color: var(--sidebar-fg-color);
}

window:backdrop .navigation-rail {
    background-color: var(--sidebar-backdrop-color);
}

.loading-overlay {
    background-color: rgba(0, 0, 0, 0.55);
}

.loading-indicator {
    color: white;
}

label.viewer-page-label {
    padding: 2px 8px;
    border-radius: 7px;
    background-color: rgba(0, 0, 0, 0.55);
    color: white;
}

.document-boundary-spread {
    border-spacing: 24px;
}

.document-boundary-side {
    padding: 8px 4px;
    border-spacing: 8px;
}

label.document-boundary-name {
    font-weight: 600;
}

.document-boundary-size-small label.document-boundary-heading {
    font-size: 15px;
}

.document-boundary-size-small label.document-boundary-name {
    font-size: 20px;
}

.document-boundary-size-medium label.document-boundary-heading {
    font-size: 17px;
}

.document-boundary-size-medium label.document-boundary-name {
    font-size: 23px;
}

.document-boundary-size-large label.document-boundary-heading {
    font-size: 19px;
}

.document-boundary-size-large label.document-boundary-name {
    font-size: 26px;
}

label.library-section-title {
    font-size: 1.15em;
    font-weight: 600;
}

.document-boundary-divider {
    min-height: 72px;
    opacity: 0.4;
}

label.library-progress-title {
    color: @accent_color;
}

.library-progress-track {
    background-color: alpha(@window_fg_color, 0.15);
}

.library-progress-bar {
    background-color: @accent_bg_color;
}

.library-favorite-indicator {
    opacity: 0;
}

toolbarview.header-auto-hide headerbar {
    background-color: alpha(@headerbar_bg_color, 0.8);
    box-shadow: 0 1px 4px alpha(black, 0.18);
}

.cover-item:hover {
    background-color: rgba(120, 120, 120, 0.16);
}

button.bookshelf-card:hover {
    background-color: transparent;
    box-shadow: none;
}

.bookshelf-card-flow > flowboxchild:hover {
    background-color: rgba(120, 120, 120, 0.16);
}

button.search-result-row {
    min-height: 0;
    padding: 0;
    border-radius: 6px;
}

.cover-placeholder {
    background-color: rgba(110, 110, 110, 0.20);
    border: 1px solid rgba(110, 110, 110, 0.55);
    border-radius: 4px;
}

.cover-placeholder image {
    opacity: 0.65;
}

.folder-book-front-shadow-edge {
    background-color: alpha(black, 0.11);
}

.folder-book-front-highlight-edge {
    background-color: alpha(white, 0.11);
}

.folder-book-spine-shadow-edge {
    background-color: alpha(black, 0.26);
}

.folder-book-spine-highlight-edge {
    background-color: alpha(white, 0.3);
}

.folder-book-more {
    padding: 1px 3px;
    border-radius: 3px;
    background-color: alpha(@window_bg_color, 0.74);
    color: @window_fg_color;
    font-size: 0.85em;
}

.folder-preview-slot {
    border-radius: 4px;
    border: 1px solid rgba(70, 70, 70, 0.22);
    background-color: @window_bg_color;
}

box.breadcrumb-container {
    border-spacing: 2px;
}

headerbar.breadcrumb-header-bar > windowhandle > box {
    padding-top: 5px;
    padding-bottom: 5px;
}

window:drop(active) .drop-highlight {
    box-shadow: inset 0 0 0 2px @accent_bg_color;
}

button.breadcrumb-segment {
    min-width: 0;
    min-height: 36px;
    padding: 0 10px;
    border: none;
    border-radius: 7px;
    color: @accent_color;
    background-color: transparent;
    box-shadow: none;
}

button.breadcrumb-segment > box.breadcrumb-segment-content,
box.breadcrumb-current {
    min-width: 0;
    border: none;
    background-color: transparent;
    box-shadow: none;
}

box.breadcrumb-current {
    min-height: 36px;
    padding: 0 10px;
}

button.breadcrumb-segment:hover {
    background-color: alpha(@window_fg_color, 0.08);
}

button.breadcrumb-segment:active {
    background-color: alpha(@window_fg_color, 0.12);
}

label.breadcrumb-separator {
    margin: 0 1px;
    opacity: 0.45;
}

.history-row {
    padding: 8px;
}

"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocumentBoundaryLoad {
    Invalidate,
    Retain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppView {
    Library,
    Viewer,
}

struct ProgressiveDisplayBackup {
    app_view: AppView,
    history_identity: Option<HistoryIdentity>,
}

fn progressive_archive_enabled(path: &Path, initial_page: InitialPage) -> bool {
    initial_page != InitialPage::End && archive_supports_sequential_progress(path)
}

fn history_resume_initial_page(at_document_end: bool, page_index: usize) -> InitialPage {
    if at_document_end {
        InitialPage::LoaderDefault
    } else {
        InitialPage::Saved(page_index)
    }
}

fn history_at_document_end(
    document_path: &Path,
    identity: &HistoryIdentity,
    at_document_end: bool,
) -> bool {
    if !at_document_end {
        return false;
    }

    let HistoryIdentity::BookshelfWork(work_path) = identity else {
        return true;
    };

    find_sibling_file(document_path, true).is_none_or(|next_path| !next_path.starts_with(work_path))
}

fn viewer_navigation_enabled(
    load_active: bool,
    progressive_displayed: bool,
    waiting_for_progressive_page: bool,
) -> bool {
    (!load_active || progressive_displayed) && !waiting_for_progressive_page
}

fn slider_item_count(progressive_total: Option<usize>, logical_page_count: usize) -> usize {
    progressive_total.unwrap_or(logical_page_count)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LibraryThumbnailScope {
    None,
    Normal,
}

fn library_thumbnail_scope(app_view: AppView) -> LibraryThumbnailScope {
    match app_view {
        AppView::Viewer => LibraryThumbnailScope::None,
        AppView::Library => LibraryThumbnailScope::Normal,
    }
}

fn search_thumbnail_scope_visible(
    search_panel_selected: bool,
    search_has_meaningful_query: bool,
) -> bool {
    search_panel_selected && search_has_meaningful_query
}

fn effective_header_auto_hide(app_view: AppView, header_auto_hide: bool, fullscreen: bool) -> bool {
    app_view == AppView::Viewer && (fullscreen || header_auto_hide)
}

fn effective_slider_auto_hide(slider_auto_hide: bool, fullscreen: bool) -> bool {
    fullscreen || slider_auto_hide
}

fn viewer_accelerators_enabled(
    app_view: AppView,
    navigation_panel: Option<NavigationPanel>,
) -> bool {
    app_view == AppView::Viewer && navigation_panel != Some(NavigationPanel::Search)
}

fn widget_is_inside_toast(widget: &gtk::Widget) -> bool {
    let mut current = Some(widget.clone());
    while let Some(widget) = current {
        if widget.css_name() == "toast" {
            return true;
        }
        current = widget.parent();
    }
    false
}

fn library_search_action_enabled(app_view: AppView, has_root: bool) -> bool {
    matches!(app_view, AppView::Library | AppView::Viewer) && has_root
}

fn library_sort_menu() -> gtk::gio::Menu {
    let menu = gtk::gio::Menu::new();
    let keys = gtk::gio::Menu::new();
    keys.append(Some("ファイル名"), Some("win.library-sort-key::name"));
    keys.append(Some("更新日時"), Some("win.library-sort-key::modified"));
    keys.append(Some("作成日時"), Some("win.library-sort-key::created"));
    menu.append_section(None, &keys);

    let directions = gtk::gio::Menu::new();
    directions.append(Some("昇順"), Some("win.library-sort-direction::ascending"));
    directions.append(Some("降順"), Some("win.library-sort-direction::descending"));
    menu.append_section(None, &directions);
    menu
}

fn register_library_sort_actions(
    window: &adw::ApplicationWindow,
    settings: &UserSettings,
    sender: &AppSender,
) {
    use gtk::glib::variant::ToVariant;

    let key_action = gtk::gio::SimpleAction::new_stateful(
        "library-sort-key",
        Some(gtk::glib::VariantTy::STRING),
        &settings.library_sort_key.storage_value().to_variant(),
    );
    key_action.connect_activate({
        let sender = sender.clone();
        move |action, parameter| {
            let Some(parameter) = parameter else {
                return;
            };
            let Some(sort_key) = parameter.str().and_then(LibrarySortKey::from_storage_value)
            else {
                return;
            };
            action.set_state(parameter);
            sender.input(Msg::SetLibrarySortKey(sort_key));
        }
    });
    window.add_action(&key_action);

    let direction_action = gtk::gio::SimpleAction::new_stateful(
        "library-sort-direction",
        Some(gtk::glib::VariantTy::STRING),
        &settings.library_sort_direction.storage_value().to_variant(),
    );
    direction_action.connect_activate({
        let sender = sender.clone();
        move |action, parameter| {
            let Some(parameter) = parameter else {
                return;
            };
            let Some(direction) = parameter
                .str()
                .and_then(LibrarySortDirection::from_storage_value)
            else {
                return;
            };
            action.set_state(parameter);
            sender.input(Msg::SetLibrarySortDirection(direction));
        }
    });
    window.add_action(&direction_action);
}

pub(crate) struct App {
    settings: UserSettings,
    viewer: ViewerRuntimeState,
    document: DocumentRuntimeState,
    library: LibraryRuntimeState,
    navigation: NavigationRuntimeState,
    shell: AppShellState,
}

pub(crate) struct AppRuntime {
    app: Rc<RefCell<App>>,
    sender: AppSender,
    window: adw::ApplicationWindow,
}

impl AppRuntime {
    pub(crate) fn window(&self) -> &adw::ApplicationWindow {
        &self.window
    }

    pub(crate) fn close(self) {
        self.app.borrow_mut().shutdown();
        self.sender.close();
    }
}

struct UiHandles {
    widgets: AppWidgets,
    spread_toggle_handler: gtk::glib::SignalHandlerId,
    smart_crop_toggle_handler: gtk::glib::SignalHandlerId,
    page_slider_handler: gtk::glib::SignalHandlerId,
}

impl UiHandles {
    fn new(widgets: AppWidgets, signal_handlers: UiSignalHandlers) -> Self {
        Self {
            widgets,
            spread_toggle_handler: signal_handlers.spread_toggle,
            smart_crop_toggle_handler: signal_handlers.smart_crop_toggle,
            page_slider_handler: signal_handlers.page_slider,
        }
    }
}

impl Deref for UiHandles {
    type Target = AppWidgets;

    fn deref(&self) -> &Self::Target {
        &self.widgets
    }
}

fn sibling_navigation_allowed(current_path: Option<&Path>, bookshelf_root: Option<&Path>) -> bool {
    match (current_path, bookshelf_root) {
        (Some(current_path), Some(bookshelf_root)) => {
            sibling_container(current_path) != Some(bookshelf_root)
        }
        _ => true,
    }
}

impl App {
    fn new(sender: &AppSender) -> Self {
        let preview = PreviewController::new();
        let settings = UserSettings::load();
        let viewer = ViewerRuntimeState::new(preview, &settings, sender);
        let navigation = NavigationRuntimeState::load(&settings);
        let shell = AppShellState::new(&settings, sender);
        let bookshelf_root = settings.bookshelf_root.clone();

        Self {
            settings,
            viewer,
            document: DocumentRuntimeState::default(),
            library: LibraryRuntimeState::new(bookshelf_root),
            navigation,
            shell,
        }
    }

    fn attach_ui_components(&mut self, widgets: &AppWidgets, sender: &AppSender) {
        self.shell.main_window = Some(widgets.main_window.clone());
        self.document.toast_overlay = Some(widgets.document_toast_overlay.clone());
        self.document.viewer_content = Some(widgets.viewer_content.clone());
        self.navigation
            .panel
            .attach(widgets.navigation_panel_stack.clone());

        register_library_sort_actions(&widgets.main_window, &self.settings, sender);
        self.viewer.preview.set_parent(&widgets.page_slider);
        self.library.search_entry = Some(widgets.library_search_entry.clone());
        self.library.view.attach(
            widgets.library_items.clone(),
            widgets.library_scroller.clone(),
            sender.clone(),
        );
        self.navigation.history_view.attach(
            widgets.history_items.clone(),
            widgets.history_scroller.clone(),
        );
        self.navigation.favorites_view.attach(
            widgets.favorites_items.clone(),
            widgets.favorites_scroller.clone(),
        );
        self.library.search_view.attach(
            widgets.library_search_items.clone(),
            widgets.library_search_scroller.clone(),
            sender.clone(),
        );
        self.shell
            .breadcrumb_bar
            .attach(widgets.breadcrumb_container.clone(), sender);
        self.refresh_breadcrumb(sender);

        self.viewer.slider.set_widgets(
            widgets.slider_overlay.clone(),
            widgets.slider_reserved_container.clone(),
            widgets.page_slider_bar.clone(),
            widgets.page_label.clone(),
        );
        self.viewer
            .slider
            .place_bar(self.effective_slider_auto_hide());
        self.viewer
            .header
            .set_toolbar_view(widgets.toolbar_view.clone());

        self.library.search_action = Some(actions::register(
            &widgets.main_window,
            &self.shell.settings_dialog,
            sender.clone(),
        ));
        self.sync_library_search_action();
    }

    fn install_ui(&mut self, widgets: AppWidgets, signal_handlers: UiSignalHandlers) {
        self.shell.ui = Some(UiHandles::new(widgets, signal_handlers));
        self.sync_ui_state();
    }

    fn apply_initial_window_state(&self) {
        if self.settings.window_maximized
            && let Some(window) = &self.shell.main_window
        {
            window.maximize();
        }
    }

    fn start_startup_action(app: &Rc<RefCell<Self>>, sender: &AppSender) {
        let startup = {
            let app = app.borrow();
            startup_action(&app.settings)
        };
        match startup {
            StartupAction::SelectBookshelfRoot => sender.input(Msg::SelectBookshelfRoot),
            StartupAction::ShowBookshelf(root) => sender.input(Msg::NavigateLibrary(root)),
            StartupAction::Restore {
                bookshelf_root,
                session,
            } => app.borrow_mut().start_path_load(
                session.document_path,
                InitialPage::Saved(session.page_index),
                LoadPurpose::StartupRestore {
                    fallback_root: bookshelf_root,
                },
                sender,
            ),
        }
    }

    pub(crate) fn launch(application: &adw::Application) -> AppRuntime {
        let (sender, receiver) = AppSender::channel();
        let mut model = Self::new(&sender);

        let root = adw::ApplicationWindow::builder()
            .application(application)
            .build();
        root.set_title(Some("Agnam"));
        root.set_default_size(model.settings.window_width, model.settings.window_height);

        let main_menu = actions::main_menu();
        let library_sort_menu = library_sort_menu();
        let widgets = AppWidgets::build(&root, &main_menu, &library_sort_menu);

        model.attach_ui_components(&widgets, &sender);
        apply_application_css(&widgets.main_window);

        let UiWiring {
            signal_handlers,
            preview_pointer_controller,
        } = wiring::connect(&widgets, &sender);
        model
            .viewer
            .preview
            .add_pointer_controller(preview_pointer_controller);
        model.apply_initial_window_state();
        model.install_ui(widgets, signal_handlers);

        let app = Rc::new(RefCell::new(model));
        start_message_loop(&app, receiver, &sender);
        Self::start_startup_action(&app, &sender);

        AppRuntime {
            app,
            sender,
            window: root,
        }
    }
}

fn apply_application_css(window: &adw::ApplicationWindow) {
    let css_provider = gtk::CssProvider::new();
    css_provider.load_from_data(APP_CSS);
    gtk::style_context_add_provider_for_display(
        &gtk::prelude::WidgetExt::display(window),
        &css_provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn start_message_loop(
    app: &Rc<RefCell<App>>,
    receiver: async_channel::Receiver<Msg>,
    sender: &AppSender,
) {
    let app_weak = Rc::downgrade(app);
    let dispatch_sender = sender.clone();
    gtk::glib::MainContext::default().spawn_local(async move {
        while let Ok(message) = receiver.recv().await {
            let Some(app) = app_weak.upgrade() else {
                break;
            };
            let mut app = app.borrow_mut();
            app.dispatch(message, &dispatch_sender);
            app.sync_ui_state();
        }
    });
}

impl App {
    fn shutdown(&mut self) {
        self.navigation.history_view.shutdown();
        self.navigation.favorites_view.shutdown();
        self.viewer.preview.shutdown();
        if let Some(window) = &self.shell.main_window {
            let (width, height) = window.default_size();
            self.update_window_state(width, height, window.is_maximized(), window.is_fullscreen());
        }
        self.update_last_session_snapshot();
        self.settings.save();
    }
}

// -----------------------------------------------------------

#[cfg(test)]
mod issue76_measure;

#[cfg(test)]
mod tests {
    use super::document_load::{DocumentLoadController, InitialPage, LoadPurpose};
    use super::{
        AppView, BoundaryDirection, DocumentBoundaryState, LibraryThumbnailScope, NavigationPanel,
        effective_header_auto_hide, effective_slider_auto_hide, history_at_document_end,
        history_resume_initial_page, library_search_action_enabled, library_thumbnail_scope,
        progressive_archive_enabled, search_thumbnail_scope_visible, sibling_navigation_allowed,
        slider_item_count, viewer_accelerators_enabled, viewer_navigation_enabled,
    };
    use crate::history::{HistoryIdentity, HistoryStore, history_identity_for_document};
    use std::path::{Path, PathBuf};

    #[test]
    fn viewer_accelerators_require_viewer_without_search_selected() {
        assert!(!viewer_accelerators_enabled(AppView::Library, None));
        assert!(viewer_accelerators_enabled(AppView::Viewer, None));
        assert!(!viewer_accelerators_enabled(
            AppView::Viewer,
            Some(NavigationPanel::Search)
        ));
        assert!(viewer_accelerators_enabled(
            AppView::Viewer,
            Some(NavigationPanel::History)
        ));
    }

    #[test]
    fn library_search_action_requires_a_root_in_both_views() {
        assert!(library_search_action_enabled(AppView::Library, true));
        assert!(!library_search_action_enabled(AppView::Library, false));
        assert!(library_search_action_enabled(AppView::Viewer, true));
        assert!(!library_search_action_enabled(AppView::Viewer, false));
    }

    #[test]
    fn normal_library_thumbnail_scope_is_independent_of_the_search_panel() {
        assert_eq!(
            library_thumbnail_scope(AppView::Library),
            LibraryThumbnailScope::Normal
        );
        assert_eq!(
            library_thumbnail_scope(AppView::Viewer),
            LibraryThumbnailScope::None
        );
    }

    #[test]
    fn search_thumbnail_scope_requires_selection_and_a_meaningful_query() {
        assert!(search_thumbnail_scope_visible(true, true));
        assert!(!search_thumbnail_scope_visible(false, true));
        assert!(!search_thumbnail_scope_visible(true, false));
        assert!(!search_thumbnail_scope_visible(false, false));
    }

    #[test]
    fn header_auto_hide_uses_view_setting_and_fullscreen() {
        assert!(!effective_header_auto_hide(AppView::Library, false, false));
        assert!(!effective_header_auto_hide(AppView::Library, true, false));
        assert!(!effective_header_auto_hide(AppView::Viewer, false, false));
        assert!(effective_header_auto_hide(AppView::Viewer, true, false));
        assert!(effective_header_auto_hide(AppView::Viewer, false, true));
    }

    #[test]
    fn slider_auto_hide_uses_setting_and_fullscreen_state() {
        assert!(!effective_slider_auto_hide(false, false));
        assert!(effective_slider_auto_hide(true, false));
        assert!(effective_slider_auto_hide(false, true));
        assert!(effective_slider_auto_hide(true, true));
    }

    #[test]
    fn progressive_loading_uses_the_io_free_archive_capability_without_end_navigation() {
        assert!(progressive_archive_enabled(
            Path::new("book.rar"),
            InitialPage::LoaderDefault
        ));
        assert!(progressive_archive_enabled(
            Path::new("book.CBR"),
            InitialPage::LoaderDefault
        ));
        assert!(!progressive_archive_enabled(
            Path::new("book.cbz"),
            InitialPage::LoaderDefault
        ));
        assert!(progressive_archive_enabled(
            Path::new("book.cb7"),
            InitialPage::LoaderDefault
        ));
        assert!(progressive_archive_enabled(
            Path::new("book.7z"),
            InitialPage::LoaderDefault
        ));
        assert!(!progressive_archive_enabled(
            Path::new("book.rar"),
            InitialPage::End
        ));
        assert!(!progressive_archive_enabled(
            Path::new("book.cb7"),
            InitialPage::End
        ));
        assert!(progressive_archive_enabled(
            Path::new("book.rar"),
            InitialPage::Saved(4)
        ));
    }

    #[test]
    fn completed_history_entries_resume_from_the_loader_default() {
        assert_eq!(
            history_resume_initial_page(true, 63),
            InitialPage::LoaderDefault
        );
        assert_eq!(
            history_resume_initial_page(false, 63),
            InitialPage::Saved(63)
        );
    }

    #[test]
    fn bookshelf_work_marks_only_its_final_document_as_completed() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("books");
        let work = root.join("black-jack");
        std::fs::create_dir_all(&work).unwrap();
        let first = work.join("01.zip");
        let second = work.join("02.zip");
        let last = work.join("03.zip");
        std::fs::write(&first, []).unwrap();
        std::fs::write(&second, []).unwrap();
        std::fs::write(&last, []).unwrap();

        let identity = history_identity_for_document(&first, Some(&root));
        assert_eq!(identity, HistoryIdentity::BookshelfWork(work.clone()));
        assert!(!history_at_document_end(&first, &identity, true));
        assert!(!history_at_document_end(&second, &identity, true));
        assert!(history_at_document_end(&last, &identity, true));

        let standalone = root.join("standalone.zip");
        let standalone_identity = history_identity_for_document(&standalone, Some(&root));
        assert!(matches!(standalone_identity, HistoryIdentity::Document(_)));
        assert!(history_at_document_end(
            &standalone,
            &standalone_identity,
            true
        ));

        let mut history =
            HistoryStore::load_or_empty_from_path(directory.path().join("history.ini"));
        history.record_document(
            &second,
            Some(&root),
            19,
            20,
            history_at_document_end(&second, &identity, true),
        );
        assert_eq!(
            history_resume_initial_page(history.entries()[0].at_document_end, 19),
            InitialPage::Saved(19)
        );
        history.record_document(
            &last,
            Some(&root),
            19,
            20,
            history_at_document_end(&last, &identity, true),
        );
        assert_eq!(
            history_resume_initial_page(history.entries()[0].at_document_end, 19),
            InitialPage::LoaderDefault
        );
    }

    #[test]
    fn image_folder_chapters_use_the_same_final_document_rule() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("books");
        let work = root.join("black-jack");
        let first = work.join("01").join("001.jpg");
        let last = work.join("02").join("001.jpg");
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        std::fs::create_dir_all(last.parent().unwrap()).unwrap();
        std::fs::write(&first, []).unwrap();
        std::fs::write(&last, []).unwrap();

        let identity = history_identity_for_document(&first, Some(&root));
        assert_eq!(identity, HistoryIdentity::BookshelfWork(work));
        assert!(!history_at_document_end(&first, &identity, true));
        assert!(history_at_document_end(&last, &identity, true));
    }

    #[test]
    fn root_image_book_ignores_a_following_different_work() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("books");
        let work = root.join("black-jack");
        let other_work = root.join("fire-bird");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&other_work).unwrap();
        let first = work.join("001.jpg");
        let last = work.join("002.jpg");
        let other = other_work.join("001.jpg");
        std::fs::write(&first, []).unwrap();
        std::fs::write(&last, []).unwrap();
        std::fs::write(&other, []).unwrap();

        let identity = history_identity_for_document(&first, Some(&root));
        assert_eq!(identity, HistoryIdentity::BookshelfWork(work.clone()));
        let next = crate::archive::navigation::find_sibling_file(&last, true).unwrap();
        assert_eq!(next, other);
        assert_ne!(history_identity_for_document(&next, Some(&root)), identity);
        assert!(history_at_document_end(&last, &identity, true));
    }

    #[test]
    fn history_completion_clears_when_backtracking_from_the_final_sibling() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("books");
        let document = root.join("series/01.cbz");
        std::fs::create_dir_all(document.parent().unwrap()).unwrap();
        std::fs::write(&document, []).unwrap();
        let identity = history_identity_for_document(&document, Some(&root));
        let mut history =
            HistoryStore::load_or_empty_from_path(directory.path().join("history.ini"));

        assert!(history_at_document_end(&document, &identity, true));
        history.record_document(
            &document,
            Some(&root),
            9,
            10,
            history_at_document_end(&document, &identity, true),
        );
        assert!(history.entries()[0].at_document_end);

        assert!(history.update_position(
            &identity,
            8,
            10,
            history_at_document_end(&document, &identity, false),
        ));
        assert!(!history.entries()[0].at_document_end);
    }

    #[test]
    fn viewer_navigation_distinguishes_foreground_and_progressive_loading() {
        assert!(viewer_navigation_enabled(false, false, false));
        assert!(!viewer_navigation_enabled(true, false, false));
        assert!(viewer_navigation_enabled(true, true, false));
        assert!(!viewer_navigation_enabled(true, true, true));
    }

    #[test]
    fn slider_switches_from_progressive_physical_total_to_logical_page_count() {
        assert_eq!(slider_item_count(Some(12), 7), 12);
        assert_eq!(slider_item_count(None, 7), 7);
    }

    #[test]
    fn sibling_navigation_stops_only_for_books_directly_inside_the_bookshelf_root() {
        let root = Path::new("/books");
        assert!(!sibling_navigation_allowed(
            Some(&root.join("01.cbz")),
            Some(root)
        ));
        assert!(!sibling_navigation_allowed(
            Some(&root.join("01").join("001.jpg")),
            Some(root)
        ));

        let work = root.join("Black Jack");
        assert!(sibling_navigation_allowed(
            Some(&work.join("01.cbz")),
            Some(root)
        ));
        assert!(sibling_navigation_allowed(
            Some(&work.join("01").join("001.jpg")),
            Some(root)
        ));
        assert!(sibling_navigation_allowed(
            Some(Path::new("/outside/01.cbz")),
            Some(root)
        ));
        assert!(sibling_navigation_allowed(Some(&root.join("01.cbz")), None));
    }

    #[test]
    fn failed_retained_load_clears_loading_and_boundary_together() {
        let mut document_load = DocumentLoadController::default();
        let (load_request_id, revision) = document_load.begin(LoadPurpose::Normal);
        let mut boundary = DocumentBoundaryState::default();
        let current = PathBuf::from("/books/A.cbz");
        let lookup = boundary.begin_lookup(current.clone(), BoundaryDirection::Next);
        boundary.complete_lookup(
            lookup,
            Path::new("/books/A.cbz"),
            Some(PathBuf::from("/books/B.cbz")),
        );
        boundary.retain_for_load(load_request_id);
        document_load.show_loading(load_request_id, revision);

        let purpose = document_load.finish(load_request_id);
        assert_eq!(purpose, Some(LoadPurpose::Normal));
        assert!(boundary.finish_retained_load(load_request_id));
        assert!(!document_load.loading_visible());
        assert!(!boundary.is_visible());
    }

    #[test]
    fn stale_load_finish_does_not_clear_current_loading_or_boundary() {
        let mut document_load = DocumentLoadController::default();
        let (stale_request_id, _) = document_load.begin(LoadPurpose::Normal);
        let mut boundary = DocumentBoundaryState::default();
        let current = PathBuf::from("/books/C.cbz");
        let lookup = boundary.begin_lookup(current.clone(), BoundaryDirection::Next);
        boundary.complete_lookup(
            lookup,
            Path::new("/books/C.cbz"),
            Some(PathBuf::from("/books/D.cbz")),
        );
        boundary.retain_for_load(stale_request_id);
        let (active_request_id, revision) = document_load.begin(LoadPurpose::Normal);
        document_load.show_loading(active_request_id, revision);

        assert_eq!(document_load.finish(stale_request_id), None);
        assert!(document_load.loading_visible());
        assert!(boundary.is_visible());
    }
}
