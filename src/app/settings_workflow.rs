use super::*;
use crate::bookshelf::{LibrarySortDirection, LibrarySortKey};

impl App {
    pub(super) fn set_preview_position_mode(&mut self, mode: PreviewPositionMode) {
        if self.settings.preview_position == mode {
            return;
        }
        self.settings.preview_position = mode;
        self.viewer.preview.refresh_position(mode);
        self.shell.settings_dialog.sync(&self.settings);
        self.settings.save();
    }

    pub(super) fn set_startup_behavior(&mut self, behavior: StartupBehavior) {
        if self.settings.startup_behavior == behavior {
            return;
        }
        self.settings.startup_behavior = behavior;
        self.shell.settings_dialog.sync(&self.settings);
        self.settings.save();
    }

    pub(super) fn set_library_book_height(&mut self, height: i32, sender: &AppSender) {
        if !(LIBRARY_BOOK_HEIGHT_MIN..=LIBRARY_BOOK_HEIGHT_MAX).contains(&height)
            || self.settings.library_book_height == height
        {
            return;
        }
        self.settings.library_book_height = height;
        self.library_sizes_changed(sender);
    }

    fn library_sizes_changed(&mut self, sender: &AppSender) {
        self.shell.settings_dialog.sync(&self.settings);
        self.settings.save();
        if self.is_library_active() {
            self.render_current_library_content(sender);
        }
    }

    pub(super) fn set_library_sort_key(&mut self, sort_key: LibrarySortKey, sender: &AppSender) {
        if self.settings.library_sort_key == sort_key {
            return;
        }
        self.settings.library_sort_key = sort_key;
        self.settings.save();
        if self.is_library_active() {
            self.render_current_library_content(sender);
        }
    }

    pub(super) fn set_library_sort_direction(
        &mut self,
        direction: LibrarySortDirection,
        sender: &AppSender,
    ) {
        if self.settings.library_sort_direction == direction {
            return;
        }
        self.settings.library_sort_direction = direction;
        self.settings.save();
        if self.is_library_active() {
            self.render_current_library_content(sender);
        }
    }

    pub(super) fn toggle_view_mode(&mut self, sender: &AppSender) {
        let mode = match self.settings.view_mode {
            ViewMode::Single => ViewMode::Spread,
            ViewMode::Spread => ViewMode::Single,
        };
        self.set_view_mode(mode, sender);
    }

    pub(super) fn set_view_mode(&mut self, mode: ViewMode, sender: &AppSender) {
        if self.settings.view_mode == mode && self.viewer.session.view_mode() == mode {
            return;
        }
        self.settings.view_mode = mode;
        let has_target = self.viewer.session.set_view_mode(mode);
        if has_target {
            self.document.boundary.cancel_lookup();
            self.sync_viewer_position(sender);
        }
        self.rebuild_hover_preview_for_view_change(sender);
        self.shell.settings_dialog.sync(&self.settings);
        self.settings.save();
    }

    pub(super) fn set_click_mode(&mut self, mode: ClickMode) {
        if self.settings.click_mode == mode {
            return;
        }
        self.settings.click_mode = mode;
        self.shell.settings_dialog.sync(&self.settings);
        self.settings.save();
    }

    pub(super) fn set_show_document_boundary_page(&mut self, enabled: bool) {
        if self.settings.show_document_boundary_page == enabled {
            return;
        }
        self.settings.show_document_boundary_page = enabled;
        if !enabled {
            self.invalidate_document_boundary();
        }
        self.shell.settings_dialog.sync(&self.settings);
        self.settings.save();
    }

    pub(super) fn set_scale_up(&mut self, scale_up: bool) {
        if self.settings.scale_up == scale_up {
            return;
        }
        self.settings.scale_up = scale_up;
        self.shell.settings_dialog.sync(&self.settings);
        self.settings.save();
    }

    pub(super) fn set_smart_crop(&mut self, enabled: bool, sender: &AppSender) {
        if self.settings.smart_crop == enabled {
            return;
        }
        self.settings.smart_crop = enabled;
        self.viewer.session.clear_textures();
        if self.is_viewer_active() && self.viewer.session.document().is_some() {
            self.sync_viewer_position(sender);
        }
        self.shell.settings_dialog.sync(&self.settings);
        self.settings.save();
    }

    pub(super) fn set_slider_auto_hide(&mut self, auto_hide: bool) {
        if self.settings.slider_auto_hide == auto_hide {
            return;
        }
        let old_effective_auto_hide = self.effective_slider_auto_hide();
        self.settings.slider_auto_hide = auto_hide;
        let effective_auto_hide = self.effective_slider_auto_hide();
        if old_effective_auto_hide != effective_auto_hide {
            self.viewer.slider.set_auto_hide(effective_auto_hide);
        }
        self.viewer
            .slider
            .reevaluate_visibility(effective_auto_hide, self.slider_content_available());
        self.hide_preview_if_slider_hidden();
        self.shell.settings_dialog.sync(&self.settings);
        self.settings.save();
    }

    pub(super) fn set_header_auto_hide(&mut self, auto_hide: bool) {
        if self.settings.header_auto_hide == auto_hide {
            return;
        }
        self.settings.header_auto_hide = auto_hide;
        self.sync_header_mode();
        self.shell.settings_dialog.sync(&self.settings);
        self.settings.save();
    }

    pub(super) fn update_header_visibility(
        &mut self,
        distance_from_top: f64,
        pointer_over_header: bool,
    ) {
        self.viewer.header.update_pointer(
            self.effective_header_auto_hide(),
            distance_from_top,
            pointer_over_header,
        );
    }

    pub(super) fn update_slider_visibility(
        &mut self,
        distance_from_bottom: f64,
        pointer_over_bar: bool,
    ) {
        self.viewer.slider.update_visibility(
            self.effective_slider_auto_hide(),
            self.slider_content_available(),
            distance_from_bottom,
            pointer_over_bar,
        );
        self.hide_preview_if_slider_hidden();
    }

    pub(super) fn set_slider_dragging(&mut self, dragging: bool) {
        self.viewer.slider.set_dragging(
            self.effective_slider_auto_hide(),
            self.slider_content_available(),
            dragging,
        );
        self.hide_preview_if_slider_hidden();
    }

    pub(super) fn fullscreen_changed(&mut self, fullscreen: bool) {
        if self.shell.fullscreen == fullscreen {
            return;
        }
        let old_slider_auto_hide = self.effective_slider_auto_hide();
        self.shell.fullscreen = fullscreen;

        self.sync_header_mode();

        let slider_auto_hide = self.effective_slider_auto_hide();
        if old_slider_auto_hide != slider_auto_hide {
            self.viewer.slider.set_auto_hide(slider_auto_hide);
        }
        self.viewer
            .slider
            .reevaluate_visibility(slider_auto_hide, self.slider_content_available());
        self.hide_preview_if_slider_hidden();
    }

    pub(super) fn set_thumbnails_enabled(&mut self, thumbnails_enabled: bool, sender: &AppSender) {
        if self.settings.thumbnails_enabled == thumbnails_enabled {
            return;
        }
        self.settings.thumbnails_enabled = thumbnails_enabled;
        if thumbnails_enabled {
            self.schedule_thumbnail_generation(sender);
            self.schedule_preload(sender);
        } else {
            self.document.progressive_thumbnails.reset();
            self.viewer.preview.disable();
            self.viewer
                .background
                .cancel_thumbnails(self.viewer.session.document_generation());
        }
        self.shell.settings_dialog.sync(&self.settings);
        self.settings.save();
    }

    pub(super) fn set_thumbnail_generation_speed(&mut self, speed: ThumbnailGenerationSpeed) {
        if self.settings.thumbnail_generation_speed == speed {
            return;
        }
        self.settings.thumbnail_generation_speed = speed;
        self.viewer.background.set_thumbnail_generation_speed(speed);
        self.shell.settings_dialog.sync(&self.settings);
        self.settings.save();
    }
}
