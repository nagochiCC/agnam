use super::*;

pub(super) const ARCHIVE_RESOURCE_LIMIT_MESSAGE: &str =
    "安全上限に達したため、アーカイブの読み込みを停止しました";

#[derive(Debug, PartialEq, Eq)]
enum ResourceLimitPresentation {
    HistoryDialog,
    FavoriteDialog,
    WindowDialog,
}

fn resource_limit_presentation(purpose: &LoadPurpose, view: AppView) -> ResourceLimitPresentation {
    match (purpose, view) {
        (LoadPurpose::History { .. }, _) => ResourceLimitPresentation::HistoryDialog,
        (LoadPurpose::Favorite { .. }, _) => ResourceLimitPresentation::FavoriteDialog,
        (_, AppView::Library | AppView::Viewer) => ResourceLimitPresentation::WindowDialog,
    }
}

fn active_resource_limit_presentation(
    load: &DocumentLoadController,
    load_request_id: u64,
    view: AppView,
) -> Option<ResourceLimitPresentation> {
    load.purpose(load_request_id)
        .map(|purpose| resource_limit_presentation(purpose, view))
}

fn load_failure_message(load_request_id: u64, error: &crate::error::AppError) -> Msg {
    if matches!(error, crate::error::AppError::ArchiveResourceLimit(_)) {
        Msg::ArchiveResourceLimitReached { load_request_id }
    } else {
        Msg::LoadRequestFinished { load_request_id }
    }
}

pub(super) fn accept_progressive_archive_image(
    load: &DocumentLoadController,
    archive: &mut ProgressiveArchiveState,
    session: &mut ViewerSession,
    load_request_id: u64,
    path: &Path,
    image: ProgressiveArchiveImage,
) -> ProgressiveArrival {
    if !load.is_active(load_request_id) {
        return ProgressiveArrival::Stale;
    }
    let arrival = archive.accept(load_request_id, path, image);
    if !matches!(arrival, ProgressiveArrival::Stale)
        && archive.is_displayed()
        && let Some(temp_dir) = archive.temp_dir(load_request_id)
    {
        session.spill_progressive_document(temp_dir);
    }
    arrival
}

pub(super) fn append_progressive_archive_images_to_session(
    session: &mut ViewerSession,
    archive: &mut ProgressiveArchiveState,
    load: &mut DocumentLoadController,
    load_request_id: u64,
    images: Vec<ProgressiveArchiveImage>,
) -> bool {
    if images.is_empty()
        || !session.append_progressive_archive_images(
            images
                .into_iter()
                .map(|image| (image.backing.source(), image.layout)),
        )
    {
        return false;
    }

    let pending_moved = match archive.pending_move() {
        Some(PendingPageMove::Next) => session.next_page(),
        Some(PendingPageMove::NextSingle) => session.next_single_page(),
        Some(PendingPageMove::PhysicalImage(target)) => {
            if target >= session.physical_image_count() {
                false
            } else {
                session.set_physical_image(target);
                true
            }
        }
        None => false,
    };
    if archive.resolve_pending_move(pending_moved).is_some() {
        load.restart_loading_delay(load_request_id);
    }
    true
}

pub(super) fn finish_progressive_archive_session(
    session: &mut ViewerSession,
    archive: &mut ProgressiveArchiveState,
    thumbnails: &mut ProgressiveThumbnailController,
    load_request_id: u64,
    document: Document,
) -> Option<std::collections::HashSet<AssetId>> {
    if !archive.is_displayed_for(load_request_id) || !session.finish_progressive_document(document)
    {
        return None;
    }
    let already_generated = thumbnails.finish();
    archive.finish(load_request_id);
    Some(already_generated)
}

pub(super) fn cancel_progressive_archive_session(
    session: &mut ViewerSession,
    archive: &mut ProgressiveArchiveState,
    thumbnails: &mut ProgressiveThumbnailController,
) -> bool {
    thumbnails.reset();
    let displayed = archive.cancel();
    if displayed {
        session.cancel_progressive_document();
    }
    displayed
}

pub(super) fn retain_progressive_archive_session_for_replacement(
    session: &mut ViewerSession,
    archive: &mut ProgressiveArchiveState,
    thumbnails: &mut ProgressiveThumbnailController,
) -> bool {
    thumbnails.reset();
    if !archive.cancel() {
        return false;
    }
    let retained = session.retain_progressive_document_for_replacement();
    debug_assert!(retained);
    retained
}

impl App {
    pub(super) fn load_document(
        &mut self,
        document: Document,
        initial_index: usize,
        sender: &AppSender,
    ) {
        let display_name =
            (!document.pages.is_empty()).then(|| document_display_name(&document.path));
        self.navigation.current_history_identity = None;
        self.viewer
            .session
            .replace_document(document, initial_index);
        self.viewer
            .background
            .activate_document(self.viewer.session.document_generation());
        self.viewer
            .smart_crop
            .activate_document(self.viewer.session.document_generation());
        self.viewer
            .slider
            .update_page_label_width(self.viewer.session.page_count());
        if self.viewer.session.page_count() > 0 {
            self.record_current_document_in_history(sender);
        }
        self.enter_document_viewer_presentation(self.viewer.session.page_count() > 0, sender);
        self.schedule_thumbnail_generation(sender);
        self.announce_document_display(display_name.as_deref(), sender);
    }

    fn enter_document_viewer_presentation(
        &mut self,
        slider_content_available: bool,
        sender: &AppSender,
    ) {
        self.document.progressive_thumbnails.reset();
        self.viewer.preview.reset_for_document();
        self.viewer
            .slider
            .reevaluate_visibility(self.effective_slider_auto_hide(), slider_content_available);
        self.hide_preview_if_slider_hidden();
        self.set_app_view(AppView::Viewer);
        self.library
            .thumbnails
            .update_demand(ThumbnailDemandEvaluation::Ready(Vec::new()));
        self.sync_viewer_position(sender);
    }

    fn announce_document_display(&mut self, display_name: Option<&str>, sender: &AppSender) {
        let Some(display_name) = display_name else {
            return;
        };
        self.show_document_toast(display_name, sender);
        Self::focus_viewer_after_document_load(sender);
    }

    fn begin_progressive_archive_display(
        &mut self,
        load_request_id: u64,
        path: PathBuf,
        images: Vec<ProgressiveArchiveImage>,
        sender: &AppSender,
    ) {
        if images.is_empty()
            || !self
                .document
                .progressive_archive
                .mark_displayed(load_request_id)
        {
            return;
        }
        let initial_index = self
            .document
            .progressive_archive
            .initial_index(load_request_id)
            .unwrap_or(0);

        self.document.progressive_display_backup = Some(ProgressiveDisplayBackup {
            app_view: self.shell.view,
            history_identity: self.navigation.current_history_identity.take(),
        });
        let temp_dir = self.document.progressive_archive.temp_dir(load_request_id);
        self.viewer.session.begin_progressive_archive_document(
            path.clone(),
            images
                .into_iter()
                .map(|image| (image.backing.source(), image.layout)),
            temp_dir,
            initial_index,
        );
        if self.document.load.is_replacement(load_request_id) {
            self.viewer.session.discard_progressive_rollback();
            self.document.progressive_display_backup = None;
        }
        self.viewer
            .background
            .activate_document(self.viewer.session.document_generation());
        self.viewer
            .smart_crop
            .activate_document(self.viewer.session.document_generation());
        self.document.load.restart_loading_delay(load_request_id);
        self.document.boundary.finish_retained_load(load_request_id);
        self.viewer
            .slider
            .update_page_label_width(self.slider_item_count());
        self.enter_document_viewer_presentation(self.slider_content_available(), sender);
        let display_name = document_display_name(&path);
        self.announce_document_display(Some(&display_name), sender);
    }

    fn append_progressive_archive_images(
        &mut self,
        load_request_id: u64,
        images: Vec<ProgressiveArchiveImage>,
        sender: &AppSender,
    ) {
        if !append_progressive_archive_images_to_session(
            &mut self.viewer.session,
            &mut self.document.progressive_archive,
            &mut self.document.load,
            load_request_id,
            images,
        ) {
            return;
        }
        self.sync_viewer_position(sender);
        self.refresh_progressive_hover_mapping(sender);
    }

    pub(super) fn archive_image_extracted(
        &mut self,
        load_request_id: u64,
        path: PathBuf,
        image: ProgressiveArchiveImage,
        sender: &AppSender,
    ) {
        let arrival = accept_progressive_archive_image(
            &self.document.load,
            &mut self.document.progressive_archive,
            &mut self.viewer.session,
            load_request_id,
            &path,
            image,
        );
        let ProgressiveArrival::Contiguous(images) = arrival else {
            return;
        };
        if self.document.progressive_archive.is_displayed() {
            self.append_progressive_archive_images(load_request_id, images, sender);
        } else {
            self.begin_progressive_archive_display(load_request_id, path, images, sender);
        }
    }

    pub(super) fn finish_progressive_archive_load(
        &mut self,
        load_request_id: u64,
        document: Document,
        sender: &AppSender,
    ) -> bool {
        let Some(already_generated) = finish_progressive_archive_session(
            &mut self.viewer.session,
            &mut self.document.progressive_archive,
            &mut self.document.progressive_thumbnails,
            load_request_id,
            document,
        ) else {
            return false;
        };

        self.document.progressive_display_backup = None;
        self.finish_load_request(load_request_id);
        self.viewer
            .slider
            .update_page_label_width(self.viewer.session.page_count());
        self.viewer.slider.reevaluate_visibility(
            self.effective_slider_auto_hide(),
            self.viewer.session.page_count() > 0,
        );
        self.hide_preview_if_slider_hidden();
        self.sync_viewer_position(sender);
        if self.viewer.session.page_count() > 0 {
            self.record_current_document_in_history(sender);
            self.settings.save();
        }
        self.schedule_thumbnail_generation_skipping(already_generated, sender);
        self.rebuild_hover_preview_for_view_change(sender);
        true
    }

    pub(super) fn cancel_progressive_archive_display(&mut self) -> bool {
        let displayed = cancel_progressive_archive_session(
            &mut self.viewer.session,
            &mut self.document.progressive_archive,
            &mut self.document.progressive_thumbnails,
        );
        if !displayed {
            self.document.progressive_display_backup = None;
            return false;
        }

        self.viewer
            .background
            .activate_document(self.viewer.session.document_generation());
        self.viewer
            .smart_crop
            .activate_document(self.viewer.session.document_generation());
        self.dismiss_document_toast();
        let restored_app_view =
            if let Some(backup) = self.document.progressive_display_backup.take() {
                self.navigation.current_history_identity = backup.history_identity;
                backup.app_view
            } else {
                self.shell.view
            };
        self.refresh_textures();
        self.viewer.preview.reset_for_document();
        self.viewer.slider.reevaluate_visibility(
            self.effective_slider_auto_hide(),
            self.viewer.session.page_count() > 0,
        );
        self.set_app_view(restored_app_view);
        true
    }

    fn retain_progressive_archive_display_for_replacement(&mut self) -> bool {
        let retained = retain_progressive_archive_session_for_replacement(
            &mut self.viewer.session,
            &mut self.document.progressive_archive,
            &mut self.document.progressive_thumbnails,
        );
        if !retained {
            return false;
        }

        self.document.progressive_display_backup = None;
        self.dismiss_document_toast();
        retained
    }

    pub(super) fn show_document_toast(&mut self, display_name: &str, sender: &AppSender) {
        self.dismiss_document_toast();

        let Some(overlay) = self.document.toast_overlay.as_ref() else {
            return;
        };

        let toast = adw::Toast::new(display_name);
        toast.set_timeout(3);
        toast.set_use_markup(false);
        toast.connect_dismissed({
            let sender = sender.clone();
            move |_| sender.input(Msg::DocumentToastDismissed)
        });
        overlay.add_toast(toast.clone());
        self.document.toast = Some(toast);
    }

    pub(super) fn dismiss_document_toast(&mut self) {
        if let Some(toast) = self.document.toast.take() {
            toast.dismiss();
        }
    }

    pub(super) fn show_archive_resource_limit_dialog(&self, message: &str) {
        let Some(window) = self.shell.main_window.as_ref() else {
            return;
        };
        let dialog = adw::AlertDialog::new(Some("アーカイブを開けません"), Some(message));
        dialog.add_response("close", "閉じる");
        dialog.set_close_response("close");
        dialog.present(Some(window));
    }

    fn focus_viewer_after_document_load(sender: &AppSender) {
        let sender = sender.clone();
        gtk::glib::idle_add_local_once(move || {
            sender.input(Msg::FocusViewerAfterDocumentLoad);
        });
    }

    pub(super) fn focus_viewer(&self) {
        if self.is_viewer_active() {
            if let Some(viewer_content) = &self.document.viewer_content {
                viewer_content.grab_focus();
            }
        }
    }

    pub(super) fn restore_viewer_focus_after_toast(&self) {
        if !self.is_viewer_active() {
            return;
        }
        let Some(window) = &self.shell.main_window else {
            return;
        };
        let focus = gtk::prelude::GtkWindowExt::focus(window);
        if focus.as_ref().is_some_and(widget_is_inside_toast) {
            self.focus_viewer();
        } else if focus.is_none() {
            self.focus_viewer();
        }
    }

    pub(super) fn next_file(&mut self, sender: &AppSender) {
        if !self.sibling_navigation_allowed() {
            return;
        }
        if self.document_boundary_visible() {
            self.navigate_from_boundary(
                BoundaryDirection::Next,
                InitialPage::LoaderDefault,
                sender,
            );
            return;
        }
        self.cross_document_edge(BoundaryDirection::Next, true, sender);
    }

    pub(super) fn prev_file(&mut self, at_end: bool, sender: &AppSender) {
        if !self.sibling_navigation_allowed() {
            return;
        }
        if self.document_boundary_visible() {
            let initial_page = if at_end {
                InitialPage::End
            } else {
                InitialPage::LoaderDefault
            };
            self.navigate_from_boundary(BoundaryDirection::Prev, initial_page, sender);
            return;
        }
        self.cross_document_edge(BoundaryDirection::Prev, !at_end, sender);
    }

    pub(super) fn cross_document_edge(
        &mut self,
        direction: BoundaryDirection,
        explicit_file_navigation: bool,
        sender: &AppSender,
    ) {
        if !self.sibling_navigation_allowed() {
            return;
        }
        match boundary_entry_decision(
            self.settings.show_document_boundary_page,
            explicit_file_navigation,
        ) {
            BoundaryEntryDecision::LookupBoundary => {
                self.start_document_boundary_lookup(direction, sender)
            }
            BoundaryEntryDecision::OpenDirectly => match direction {
                BoundaryDirection::Next => self.open_sibling_file(true, false, sender),
                BoundaryDirection::Prev => {
                    self.open_sibling_file(false, !explicit_file_navigation, sender)
                }
            },
        }
    }

    fn start_document_boundary_lookup(&mut self, direction: BoundaryDirection, sender: &AppSender) {
        let Some(current_path) = self.viewer.session.document_path().map(ToOwned::to_owned) else {
            return;
        };
        let request_id = self
            .document
            .boundary
            .begin_lookup(current_path.clone(), direction);
        let next = direction == BoundaryDirection::Next;
        let sender = sender.clone();
        spawn_background(move || {
            let adjacent_path = find_sibling_file(&current_path, next);
            sender.input(Msg::BoundaryPathLookupFinished {
                request_id,
                adjacent_path,
            });
        });
    }

    pub(super) fn document_boundary_lookup_finished(
        &mut self,
        request_id: u64,
        adjacent_path: Option<PathBuf>,
    ) {
        let Some(current_path) = self.viewer.session.document_path() else {
            return;
        };
        if self
            .document
            .boundary
            .complete_lookup(request_id, current_path, adjacent_path)
        {
            self.dismiss_document_toast();
            self.hide_hover_preview();
            self.viewer
                .slider
                .reevaluate_visibility(self.effective_slider_auto_hide(), false);
        }
    }

    pub(super) fn navigate_from_boundary(
        &mut self,
        direction: BoundaryDirection,
        initial_page: InitialPage,
        sender: &AppSender,
    ) {
        let Some(target) = self.document.boundary.target(direction) else {
            return;
        };
        if target.is_held_document {
            self.close_document_boundary();
        } else {
            self.start_boundary_path_load(target.path, initial_page, sender);
        }
    }

    fn close_document_boundary(&mut self) {
        if !self.document_boundary_visible() {
            return;
        }
        self.document.boundary.close();
        self.viewer.slider.reevaluate_visibility(
            self.effective_slider_auto_hide(),
            self.viewer.session.page_count() > 0,
        );
    }

    pub(super) fn invalidate_document_boundary(&mut self) {
        let was_visible = self.document_boundary_visible();
        self.document.boundary.invalidate();
        if was_visible {
            self.viewer.slider.reevaluate_visibility(
                self.effective_slider_auto_hide(),
                self.viewer.session.page_count() > 0,
            );
        }
        self.hide_hover_preview();
    }

    fn begin_load_request(
        &mut self,
        purpose: LoadPurpose,
        document_boundary: DocumentBoundaryLoad,
        sender: &AppSender,
    ) -> u64 {
        self.begin_load_request_with_replacement(purpose, document_boundary, false, sender)
    }

    fn begin_replacement_load_request(
        &mut self,
        purpose: LoadPurpose,
        document_boundary: DocumentBoundaryLoad,
        sender: &AppSender,
    ) -> u64 {
        self.begin_load_request_with_replacement(purpose, document_boundary, true, sender)
    }

    fn begin_load_request_with_replacement(
        &mut self,
        purpose: LoadPurpose,
        document_boundary: DocumentBoundaryLoad,
        replacement: bool,
        sender: &AppSender,
    ) -> u64 {
        if replacement {
            self.retain_progressive_archive_display_for_replacement();
        } else {
            self.cancel_progressive_archive_display();
        }
        self.document.sibling_lookup.invalidate();
        self.close_navigation_panel(sender);
        let (load_request_id, loading_revision) = if replacement {
            self.document.load.begin_replacement(purpose)
        } else {
            self.document.load.begin(purpose)
        };
        match document_boundary {
            DocumentBoundaryLoad::Invalidate => self.invalidate_document_boundary(),
            DocumentBoundaryLoad::Retain => {
                if !self.document.boundary.retain_for_load(load_request_id) {
                    self.invalidate_document_boundary();
                }
            }
        }
        self.hide_hover_preview();

        Self::schedule_loading(load_request_id, loading_revision, sender);

        load_request_id
    }

    pub(super) fn schedule_loading(load_request_id: u64, revision: u64, sender: &AppSender) {
        let sender = sender.clone();
        gtk::glib::timeout_add_local_once(LOADING_DISPLAY_DELAY, move || {
            sender.input(Msg::ShowLoading {
                load_request_id,
                revision,
            });
        });
    }

    pub(super) fn show_loading(&mut self, load_request_id: u64, revision: u64) {
        if self.document.load.show_loading(load_request_id, revision) {
            self.hide_hover_preview();
        }
    }

    pub(super) fn finish_load_request(&mut self, load_request_id: u64) -> Option<LoadPurpose> {
        let purpose = self.document.load.finish(load_request_id)?;
        let boundary_closed = self.document.boundary.finish_retained_load(load_request_id);
        if boundary_closed {
            self.viewer.slider.reevaluate_visibility(
                self.effective_slider_auto_hide(),
                self.viewer.session.page_count() > 0,
            );
        }
        Some(purpose)
    }

    pub(super) fn load_request_failed(&mut self, load_request_id: u64, sender: &AppSender) {
        self.finish_failed_load_request(load_request_id, sender, None);
    }

    fn finish_failed_load_request(
        &mut self,
        load_request_id: u64,
        sender: &AppSender,
        resource_limit_message: Option<&str>,
    ) -> bool {
        let progressive_display_cancelled = self
            .document
            .progressive_archive
            .is_displayed_for(load_request_id)
            .then(|| self.cancel_progressive_archive_display())
            .unwrap_or(false);
        if !progressive_display_cancelled {
            self.document.progressive_archive.finish(load_request_id);
        }
        let Some(purpose) = self.finish_load_request(load_request_id) else {
            return false;
        };
        if progressive_display_cancelled {
            self.refresh_breadcrumb(sender);
        }
        if let Some(message) = resource_limit_message {
            match resource_limit_presentation(&purpose, self.shell.view) {
                ResourceLimitPresentation::HistoryDialog => {
                    let (identity, path) = purpose.history_target().expect("matched above");
                    return self.show_history_resource_limit_dialog(
                        identity.clone(),
                        path.to_owned(),
                        message,
                        sender,
                    );
                }
                ResourceLimitPresentation::FavoriteDialog => {
                    let identity = purpose.favorite_target().expect("matched above");
                    return self.show_favorite_resource_limit_dialog(
                        identity.clone(),
                        message,
                        sender,
                    );
                }
                ResourceLimitPresentation::WindowDialog => {}
            }
        } else {
            if let Some((identity, path)) = purpose.history_target() {
                return self.show_history_open_failed_dialog(
                    identity.clone(),
                    path.to_owned(),
                    sender,
                );
            }
            if let Some(identity) = purpose.favorite_target() {
                return self.show_favorite_open_failed_dialog(identity.clone(), sender);
            }
        }
        if let Some(root) = purpose.fallback_root().map(ToOwned::to_owned) {
            self.navigate_library(root, sender);
        }
        false
    }

    pub(super) fn archive_resource_limit_failed(
        &mut self,
        load_request_id: u64,
        sender: &AppSender,
    ) {
        if active_resource_limit_presentation(&self.document.load, load_request_id, self.shell.view)
            .is_none()
        {
            return;
        }
        if !self.finish_failed_load_request(
            load_request_id,
            sender,
            Some(ARCHIVE_RESOURCE_LIMIT_MESSAGE),
        ) {
            self.show_archive_resource_limit_dialog(ARCHIVE_RESOURCE_LIMIT_MESSAGE);
        }
    }

    pub(super) fn start_path_load(
        &mut self,
        path: std::path::PathBuf,
        initial_page: InitialPage,
        purpose: LoadPurpose,
        sender: &AppSender,
    ) {
        let load_request_id =
            self.begin_load_request(purpose, DocumentBoundaryLoad::Invalidate, sender);
        let progressive = self.prepare_progressive_archive(load_request_id, &path, initial_page);
        Self::spawn_path_load(load_request_id, path, initial_page, progressive, sender);
    }

    pub(super) fn start_archive_image_load(
        &mut self,
        path: PathBuf,
        id: crate::archive::ArchiveImageId,
        sender: &AppSender,
    ) {
        self.close_navigation_panel(sender);
        let load_request_id = self.begin_load_request(
            LoadPurpose::Normal,
            DocumentBoundaryLoad::Invalidate,
            sender,
        );
        let sender = sender.clone();
        spawn_background(move || match load_document_from_path(&path) {
            Ok((document, _)) => {
                let Some(initial_index) =
                    document.first_page_for_archive_image(&id.archives, &id.image)
                else {
                    sender.input(Msg::LoadRequestFinished { load_request_id });
                    return;
                };
                sender.input(Msg::OpenDocument {
                    load_request_id,
                    document,
                    initial_index,
                });
            }
            Err(error) => {
                eprintln!("archive内画像の読み込みに失敗しました: {error}");
                sender.input(load_failure_message(load_request_id, &error));
            }
        });
    }

    fn start_boundary_path_load(
        &mut self,
        path: std::path::PathBuf,
        initial_page: InitialPage,
        sender: &AppSender,
    ) {
        let load_request_id =
            self.begin_load_request(LoadPurpose::Normal, DocumentBoundaryLoad::Retain, sender);
        let progressive = self.prepare_progressive_archive(load_request_id, &path, initial_page);
        Self::spawn_path_load(load_request_id, path, initial_page, progressive, sender);
    }

    pub(super) fn prepare_progressive_archive(
        &mut self,
        load_request_id: u64,
        path: &Path,
        initial_page: InitialPage,
    ) -> Option<ProgressiveArchiveCancelToken> {
        if !progressive_archive_enabled(path, initial_page) {
            return None;
        }
        let (initial_index, required_initial_pages) = match initial_page {
            InitialPage::LoaderDefault => (0, 1),
            InitialPage::Saved(index) => (index, index.saturating_add(2)),
            InitialPage::End => return None,
        };
        Some(self.document.progressive_archive.begin(
            load_request_id,
            path.to_path_buf(),
            initial_index,
            required_initial_pages,
        ))
    }

    pub(super) fn spawn_path_load(
        load_request_id: u64,
        path: std::path::PathBuf,
        initial_page: InitialPage,
        progressive: Option<ProgressiveArchiveCancelToken>,
        sender: &AppSender,
    ) {
        let sender = sender.clone();
        spawn_background(move || {
            let result = if let Some(cancel_token) = progressive.as_ref() {
                let progress_path = path.clone();
                let progress_sender = sender.clone();
                let notification_token = cancel_token.clone();
                load_document_from_path_with_sequential_progress(
                    &path,
                    cancel_token,
                    move |image| {
                        if !notification_token.is_cancelled() {
                            progress_sender.input(Msg::ArchiveImageExtracted {
                                load_request_id,
                                path: progress_path.clone(),
                                image,
                            });
                        }
                    },
                )
            } else {
                load_document_from_path(&path).map(ProgressiveArchiveLoadOutcome::Complete)
            };

            match result {
                Ok(ProgressiveArchiveLoadOutcome::Complete((document, initial_index)))
                    if progressive
                        .as_ref()
                        .is_none_or(|cancel_token| !cancel_token.is_cancelled()) =>
                {
                    let initial_index = initial_page.resolve(initial_index, document.pages.len());
                    sender.input(Msg::OpenDocument {
                        load_request_id,
                        document,
                        initial_index,
                    });
                }
                Ok(ProgressiveArchiveLoadOutcome::Complete(_))
                | Ok(ProgressiveArchiveLoadOutcome::Cancelled) => {}
                Err(_)
                    if progressive
                        .as_ref()
                        .is_some_and(ProgressiveArchiveCancelToken::is_cancelled) => {}
                Err(error) => {
                    eprintln!("ファイルの読み込みに失敗しました: {error}");
                    sender.input(load_failure_message(load_request_id, &error));
                }
            }
        });
    }

    fn open_sibling_file(&mut self, next: bool, at_end: bool, sender: &AppSender) {
        let Some(current_filepath) = self.viewer.session.document_path().map(ToOwned::to_owned)
        else {
            return;
        };
        let request_id = self.document.sibling_lookup.begin(
            current_filepath.clone(),
            self.viewer.session.document_generation(),
            at_end,
        );
        let sender = sender.clone();
        spawn_background(move || {
            let adjacent_path = find_sibling_file(&current_filepath, next);
            sender.input(Msg::SiblingPathLookupFinished {
                request_id,
                adjacent_path,
            });
        });
    }

    pub(super) fn sibling_path_lookup_finished(
        &mut self,
        request_id: u64,
        adjacent_path: Option<PathBuf>,
        sender: &AppSender,
    ) {
        if !self.is_viewer_active() {
            self.document.sibling_lookup.invalidate();
            return;
        }
        let Some(current_path) = self.viewer.session.document_path() else {
            self.document.sibling_lookup.invalidate();
            return;
        };
        let completion = self.document.sibling_lookup.complete(
            request_id,
            current_path,
            self.viewer.session.document_generation(),
            adjacent_path,
        );
        let SiblingLookupCompletion::Found(target) = completion else {
            return;
        };
        let initial_page = if target.start_at_end {
            InitialPage::End
        } else {
            InitialPage::LoaderDefault
        };
        let load_request_id = self.begin_replacement_load_request(
            LoadPurpose::Normal,
            DocumentBoundaryLoad::Invalidate,
            sender,
        );
        let progressive =
            self.prepare_progressive_archive(load_request_id, &target.path, initial_page);
        Self::spawn_path_load(
            load_request_id,
            target.path,
            initial_page,
            progressive,
            sender,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_limit_reuses_history_and_favorite_failure_dialogs() {
        let path = PathBuf::from("book.cbz");
        let history = LoadPurpose::History {
            identity: crate::history::history_identity_for_document(&path, None),
            path: path.clone(),
        };
        let favorite = LoadPurpose::Favorite {
            identity: crate::favorites::FavoriteIdentity::from_document_path(&path).unwrap(),
        };
        assert_eq!(
            resource_limit_presentation(&history, AppView::Library),
            ResourceLimitPresentation::HistoryDialog
        );
        assert_eq!(
            resource_limit_presentation(&favorite, AppView::Viewer),
            ResourceLimitPresentation::FavoriteDialog
        );
        assert_eq!(
            resource_limit_presentation(&LoadPurpose::Normal, AppView::Library),
            ResourceLimitPresentation::WindowDialog
        );
    }

    #[test]
    fn normal_resource_limit_uses_visible_window_dialog_and_only_active_request_can_present() {
        for view in [AppView::Library, AppView::Viewer] {
            let mut load = DocumentLoadController::default();
            let (id, revision) = load.begin(LoadPurpose::Normal);
            assert_eq!(
                active_resource_limit_presentation(&load, id, view),
                Some(ResourceLimitPresentation::WindowDialog)
            );
            assert!(load.show_loading(id, revision));
            assert_eq!(load.finish(id), Some(LoadPurpose::Normal));
            assert!(!load.loading_visible());
            assert_eq!(active_resource_limit_presentation(&load, id, view), None);
            assert_eq!(load.finish(id), None);
        }

        let mut load = DocumentLoadController::default();
        let (cancelled, _) = load.begin(LoadPurpose::Normal);
        load.cancel();
        assert_eq!(
            active_resource_limit_presentation(&load, cancelled, AppView::Library),
            None
        );
        let (stale, _) = load.begin(LoadPurpose::Normal);
        let (current, _) = load.begin(LoadPurpose::Normal);
        assert_eq!(
            active_resource_limit_presentation(&load, stale, AppView::Viewer),
            None
        );
        assert_eq!(
            active_resource_limit_presentation(&load, current, AppView::Viewer),
            Some(ResourceLimitPresentation::WindowDialog)
        );
    }

    #[test]
    fn archive_image_and_regular_loads_route_resource_limit_by_error_kind() {
        let limit = crate::error::AppError::ArchiveResourceLimit(
            crate::archive::ResourceLimitKind::EntryCount,
        );
        assert!(matches!(
            load_failure_message(12, &limit),
            Msg::ArchiveResourceLimitReached {
                load_request_id: 12
            }
        ));
        assert!(matches!(
            load_failure_message(12, &crate::error::AppError::Archive("broken".into())),
            Msg::LoadRequestFinished {
                load_request_id: 12
            }
        ));
    }
}
