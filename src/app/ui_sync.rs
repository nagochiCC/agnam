use super::*;

impl App {
    pub(super) fn sync_ui_state(&self) {
        let Some(ui) = &self.shell.ui else {
            return;
        };

        self.sync_navigation_shell_ui(ui);

        let viewer_active = self.is_viewer_active();
        let viewer_navigation_enabled = self.viewer_navigation_enabled();
        self.sync_viewer_controls_ui(ui, viewer_active, viewer_navigation_enabled);
        self.sync_library_ui(ui, viewer_active);

        let document_boundary_visible =
            self.sync_viewer_content_ui(ui, viewer_active, viewer_navigation_enabled);
        self.sync_slider_ui(ui, viewer_navigation_enabled, document_boundary_visible);
        self.sync_loading_ui(ui);
    }

    fn sync_navigation_shell_ui(&self, ui: &UiHandles) {
        ui.header_title.set_visible(false);
        ui.navigate_up_button.set_sensitive(self.can_navigate_up());
        ui.search_toggle
            .set_active(self.navigation.panel.is_selected(NavigationPanel::Search));
        ui.history_toggle
            .set_active(self.navigation.panel.is_selected(NavigationPanel::History));
        ui.favorites_toggle.set_active(
            self.navigation
                .panel
                .is_selected(NavigationPanel::Favorites),
        );
        ui.navigation_split_view
            .set_show_sidebar(self.navigation.panel.is_open());

        let show_window_buttons = !self.shell.fullscreen;
        ui.sidebar_header
            .set_show_start_title_buttons(show_window_buttons);
        ui.sidebar_header
            .set_show_end_title_buttons(show_window_buttons);
        ui.header_bar
            .set_show_start_title_buttons(show_window_buttons);
        ui.header_bar
            .set_show_end_title_buttons(show_window_buttons);
        ui.navigation_panel_title
            .set_label(match self.navigation.panel.selected() {
                Some(NavigationPanel::History) => "閲覧履歴",
                Some(NavigationPanel::Favorites) => "お気に入り",
                _ => "本棚を検索",
            });
        ui.clear_history_button
            .set_visible(self.navigation.panel.is_selected(NavigationPanel::History));
        ui.clear_history_button
            .set_sensitive(!self.navigation.history.entries().is_empty());
    }

    fn sync_viewer_controls_ui(
        &self,
        ui: &UiHandles,
        viewer_active: bool,
        viewer_navigation_enabled: bool,
    ) {
        ui.library_sort_button
            .set_visible(!viewer_active && !self.library.archive.is_active());
        ui.viewer_controls.set_visible(viewer_active);
        ui.smart_crop_toggle.set_visible(viewer_active);
        let single_page_navigation_enabled =
            self.settings.view_mode == ViewMode::Spread && viewer_navigation_enabled;
        ui.next_single_button
            .set_sensitive(single_page_navigation_enabled);
        ui.prev_single_button
            .set_sensitive(single_page_navigation_enabled);
        ui.spread_toggle.block_signal(&ui.spread_toggle_handler);
        ui.spread_toggle
            .set_active(self.settings.view_mode == ViewMode::Spread);
        ui.spread_toggle.unblock_signal(&ui.spread_toggle_handler);
        ui.smart_crop_toggle
            .block_signal(&ui.smart_crop_toggle_handler);
        ui.smart_crop_toggle.set_active(self.settings.smart_crop);
        ui.smart_crop_toggle
            .unblock_signal(&ui.smart_crop_toggle_handler);
    }

    fn sync_library_ui(&self, ui: &UiHandles, viewer_active: bool) {
        let library_status_message = self.library_status_message();
        let library_status_spinning = self.library_status_spinning();
        ui.library_content.set_visible(!viewer_active);
        ui.library_status
            .set_visible(library_status_message.is_some());
        ui.library_status_spinner
            .set_visible(library_status_spinning);
        ui.library_status_spinner
            .set_spinning(library_status_spinning);
        ui.library_status_label
            .set_label(library_status_message.as_deref().unwrap_or_default());
        ui.select_bookshelf_button
            .set_visible(self.library.model.can_select_root());
        ui.library_scroller
            .set_visible(self.library_has_visible_items());
    }

    fn sync_viewer_content_ui(
        &self,
        ui: &UiHandles,
        viewer_active: bool,
        viewer_navigation_enabled: bool,
    ) -> bool {
        ui.viewer_content.set_visible(viewer_active);
        ui.image_container.set_sensitive(viewer_navigation_enabled);
        let document_boundary_visible = self.document_boundary_visible();
        let content_fit = if self.settings.scale_up {
            gtk::ContentFit::Contain
        } else {
            gtk::ContentFit::ScaleDown
        };
        ui.left_picture.set_content_fit(content_fit);
        ui.left_picture
            .set_visible(!document_boundary_visible && self.is_full_spread());
        if ui.left_picture.paintable().as_ref()
            != self
                .viewer
                .left_texture
                .as_ref()
                .map(|texture| texture.upcast_ref::<gtk::gdk::Paintable>())
        {
            ui.left_picture
                .set_paintable(self.viewer.left_texture.as_ref());
        }
        ui.right_picture.set_halign(self.right_align());
        ui.right_picture.set_visible(!document_boundary_visible);
        ui.right_picture.set_content_fit(content_fit);
        if ui.right_picture.paintable().as_ref()
            != self
                .viewer
                .right_texture
                .as_ref()
                .map(|texture| texture.upcast_ref::<gtk::gdk::Paintable>())
        {
            ui.right_picture
                .set_paintable(self.viewer.right_texture.as_ref());
        }
        ui.document_boundary.set_visible(document_boundary_visible);
        ui.boundary_next_label
            .set_label(&self.boundary_next_label());
        ui.boundary_previous_label
            .set_label(&self.boundary_previous_label());

        document_boundary_visible
    }

    fn sync_slider_ui(
        &self,
        ui: &UiHandles,
        viewer_navigation_enabled: bool,
        document_boundary_visible: bool,
    ) {
        let slider_item_count = self.slider_item_count();
        let slider_current_index = self.slider_current_index();
        ui.page_slider_bar.set_visible(
            slider_item_count > 0 && self.viewer.slider.is_visible() && !document_boundary_visible,
        );
        ui.page_slider_bar.set_sensitive(viewer_navigation_enabled);
        ui.page_slider.block_signal(&ui.page_slider_handler);
        let slider_max = slider_item_count.saturating_sub(1) as f64;
        let adjustment = ui.page_slider.adjustment();
        if adjustment.lower() != 0.0 || adjustment.upper() != slider_max {
            ui.page_slider.set_range(0.0, slider_max);
        }
        if ui.page_slider.value() != slider_current_index as f64 {
            ui.page_slider.set_value(slider_current_index as f64);
        }
        ui.page_slider.unblock_signal(&ui.page_slider_handler);
        ui.page_slider.set_sensitive(slider_item_count > 0);
        ui.page_label.set_visible(slider_item_count > 0);
        ui.page_label
            .set_label(&page_label_text(slider_current_index, slider_item_count));
    }

    fn sync_loading_ui(&self, ui: &UiHandles) {
        let loading_visible = self.document.load.loading_visible();
        ui.loading_layer.set_visible(loading_visible);
        ui.loading_spinner.set_spinning(loading_visible);
    }
}
