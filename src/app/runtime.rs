use super::*;

impl App {
    pub(super) fn update_window_state(
        &mut self,
        width: i32,
        height: i32,
        maximized: bool,
        fullscreened: bool,
    ) {
        self.settings
            .update_window_state(width, height, maximized, fullscreened);
    }

    pub(super) fn is_viewer_active(&self) -> bool {
        self.shell.view == AppView::Viewer
    }

    pub(super) fn is_library_active(&self) -> bool {
        !self.is_viewer_active()
    }

    pub(super) fn effective_header_auto_hide(&self) -> bool {
        effective_header_auto_hide(
            self.shell.view,
            self.settings.header_auto_hide,
            self.shell.fullscreen,
        )
    }

    pub(super) fn effective_slider_auto_hide(&self) -> bool {
        effective_slider_auto_hide(self.settings.slider_auto_hide, self.shell.fullscreen)
    }

    pub(super) fn sync_header_mode(&mut self) {
        self.viewer
            .header
            .set_auto_hide(self.effective_header_auto_hide());
    }

    pub(super) fn sync_viewer_accelerators(&self) {
        let Some(app) = self
            .shell
            .main_window
            .as_ref()
            .and_then(|window| window.application())
            .or_else(|| gtk::gio::Application::default().and_downcast::<gtk::Application>())
        else {
            return;
        };
        actions::set_viewer_accelerators_enabled(
            &app,
            viewer_accelerators_enabled(self.shell.view, self.navigation.panel.selected()),
        );
    }

    pub(super) fn sync_library_search_action(&self) {
        if let Some(action) = &self.library.search_action {
            action.set_enabled(library_search_action_enabled(
                self.shell.view,
                self.library.model.root().is_some(),
            ));
        }
    }

    pub(super) fn set_app_view(&mut self, app_view: AppView) {
        if returning_to_library(self.shell.view, app_view) {
            self.library
                .view
                .sync_progress(self.navigation.history.entries());
        }
        self.shell.view = app_view;
        self.sync_viewer_accelerators();
        self.sync_library_search_action();
        self.sync_header_mode();
    }

    pub(super) fn page_count(&self) -> usize {
        self.viewer.session.page_count()
    }

    pub(super) fn progressive_archive_loading(&self) -> bool {
        self.document.progressive_archive.is_displayed()
    }

    pub(super) fn viewer_navigation_enabled(&self) -> bool {
        viewer_navigation_enabled(
            self.document.load.has_active_request(),
            self.document.progressive_archive.is_displayed(),
            self.document.progressive_archive.is_waiting_for_page(),
        )
    }

    pub(super) fn slider_content_available(&self) -> bool {
        self.slider_item_count() > 0
    }

    pub(super) fn slider_item_count(&self) -> usize {
        slider_item_count(
            self.document.progressive_archive.total_physical_images(),
            self.page_count(),
        )
    }

    pub(super) fn slider_current_index(&self) -> usize {
        if self.progressive_archive_loading() {
            self.document
                .progressive_archive
                .pending_physical_target()
                .or_else(|| self.viewer.session.current_physical_image_index())
                .unwrap_or(0)
        } else {
            self.viewer.session.current_index()
        }
    }

    pub(super) fn document_boundary_visible(&self) -> bool {
        self.document.boundary.is_visible()
    }

    pub(super) fn boundary_previous_label(&self) -> String {
        self.document.boundary.previous_name()
    }

    pub(super) fn boundary_next_label(&self) -> String {
        self.document.boundary.next_name()
    }

    pub(super) fn sibling_navigation_allowed(&self) -> bool {
        sibling_navigation_allowed(
            self.viewer.session.document_path(),
            self.library.model.root(),
        )
    }

    pub(super) fn is_full_spread(&self) -> bool {
        self.viewer.session.is_full_spread()
    }

    pub(super) fn refresh_textures(&mut self) -> bool {
        let preparation = self.viewer.smart_crop.handle();
        (self.viewer.right_texture, self.viewer.left_texture) = self
            .viewer
            .session
            .current_textures_with_preparation(self.settings.smart_crop, Some(&preparation));
        self.viewer.session.take_archive_resource_limit().is_some()
    }

    pub(super) fn right_align(&self) -> gtk::Align {
        if self.is_full_spread() {
            gtk::Align::Start
        } else {
            gtk::Align::Center
        }
    }

    pub(super) fn sync_viewer_position(&mut self, sender: &AppSender) {
        if self.refresh_textures() {
            self.show_document_toast(
                super::document_workflow::ARCHIVE_RESOURCE_LIMIT_MESSAGE,
                sender,
            );
        }
        self.schedule_preload(sender);
        self.update_last_session_snapshot();
        self.update_history_position(sender);
        self.refresh_breadcrumb(sender);
    }

    pub(super) fn update_last_session_snapshot(&mut self) {
        if self.progressive_archive_loading() {
            return;
        }
        let document_path = self.viewer.session.document_path().map(ToOwned::to_owned);
        update_last_session(
            &mut self.settings.last_session,
            document_path.as_deref(),
            self.viewer.session.page_count(),
            self.viewer.session.current_index(),
        );
    }

    pub(super) fn refresh_breadcrumb(&mut self, sender: &AppSender) {
        let segments = self.breadcrumb_segments();
        self.shell.breadcrumb_bar.render(&segments, sender);
    }

    pub(super) fn breadcrumb_segments(&self) -> Vec<super::breadcrumb::BreadcrumbSegment> {
        match self.shell.view {
            AppView::Library if self.library.archive.is_active() => self
                .library
                .archive
                .location()
                .map(|location| archive_segments(self.library.model.root(), location))
                .unwrap_or_default(),
            AppView::Library => self
                .library
                .model
                .root()
                .zip(self.library.model.current_directory())
                .map(|(root, current)| library_segments(root, current))
                .unwrap_or_default(),
            AppView::Viewer => self
                .viewer
                .session
                .document_path()
                .map(|path| {
                    viewer_segments(
                        self.library.model.root(),
                        path,
                        self.viewer
                            .session
                            .image_source_for_page(self.viewer.session.current_index()),
                    )
                })
                .unwrap_or_default(),
        }
    }

    pub(super) fn navigate_up(&mut self, sender: &AppSender) {
        match super::breadcrumb::parent_target(&self.breadcrumb_segments()) {
            Some(super::breadcrumb::BreadcrumbTarget::Library(path)) => {
                self.navigate_library(path, sender)
            }
            Some(super::breadcrumb::BreadcrumbTarget::Archive(location)) => {
                self.navigate_archive_contents(location, sender)
            }
            None => {}
        }
    }

    pub(super) fn can_navigate_up(&self) -> bool {
        super::breadcrumb::parent_target(&self.breadcrumb_segments()).is_some()
    }
}

fn returning_to_library(previous: AppView, next: AppView) -> bool {
    previous == AppView::Viewer && next == AppView::Library
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_to_library_transition_requires_progress_sync_before_display() {
        assert!(returning_to_library(AppView::Viewer, AppView::Library));
        assert!(!returning_to_library(AppView::Viewer, AppView::Viewer));
        assert!(!returning_to_library(AppView::Library, AppView::Library));
    }
}
