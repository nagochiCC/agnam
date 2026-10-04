mod cache;
mod state;

use crate::document::ImageLayout;
use crate::settings::PreviewPositionMode;
use crate::thumbnail::{THUMBNAIL_BYTES_PER_PIXEL, THUMBNAIL_HEIGHT, Thumbnail};
use cache::ThumbnailCache;
use gtk::prelude::*;
use state::{PreviewState, preview_pages_ready};
pub(super) use state::{PreviewTarget, nearest_ready_view};
use std::time::Duration;

pub(super) const PREVIOUS_PREVIEW_HOLD_DURATION: Duration = Duration::from_millis(100);
const THUMBNAIL_MEMORY_FORMAT: gtk::gdk::MemoryFormat = gtk::gdk::MemoryFormat::R8g8b8;
const THUMBNAIL_CACHE_BUDGET: usize = 32 * 1024 * 1024;

fn anchor_geometry(
    mode: PreviewPositionMode,
    pointer_x: i32,
    slider_width: i32,
    slider_height: i32,
) -> Option<(i32, i32, i32, i32)> {
    match mode {
        PreviewPositionMode::Centered => None,
        PreviewPositionMode::FollowPointer => {
            let max_x = slider_width.saturating_sub(1).max(0);
            let slider_height = slider_height.max(1);
            Some((pointer_x.clamp(0, max_x), -slider_height, 1, slider_height))
        }
    }
}

fn thumbnail_buffer_is_valid(thumbnail: &Thumbnail) -> bool {
    let (Ok(width), Ok(height)) = (
        usize::try_from(thumbnail.width),
        usize::try_from(thumbnail.height),
    ) else {
        return false;
    };
    let Some(stride) = width.checked_mul(THUMBNAIL_BYTES_PER_PIXEL) else {
        return false;
    };
    let Some(buffer_len) = stride.checked_mul(height) else {
        return false;
    };
    thumbnail.stride == stride && thumbnail.pixels.len() == buffer_len
}

pub(super) struct PreviewController {
    cache: ThumbnailCache<gtk::gdk::Texture>,
    state: PreviewState,
    popover: gtk::Popover,
    left_slot: gtk::Overlay,
    left: gtk::Picture,
    right: gtk::Picture,
}

impl PreviewController {
    pub(super) fn new() -> Self {
        let left = gtk::Picture::new();
        left.set_can_shrink(true);
        left.set_halign(gtk::Align::End);
        left.set_valign(gtk::Align::Fill);
        left.set_content_fit(gtk::ContentFit::Contain);
        let left_slot = gtk::Overlay::new();
        let left_placeholder = gtk::DrawingArea::new();
        left_placeholder.set_size_request(160, THUMBNAIL_HEIGHT as i32);
        left_slot.set_child(Some(&left_placeholder));
        left_slot.add_overlay(&left);

        let right = gtk::Picture::new();
        right.set_can_shrink(true);
        right.set_halign(gtk::Align::Fill);
        right.set_valign(gtk::Align::Fill);
        right.set_content_fit(gtk::ContentFit::Contain);
        let right_slot = gtk::Overlay::new();
        let right_placeholder = gtk::DrawingArea::new();
        right_placeholder.set_size_request(160, THUMBNAIL_HEIGHT as i32);
        right_slot.set_child(Some(&right_placeholder));
        right_slot.add_overlay(&right);

        let preview_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        preview_box.append(&left_slot);
        preview_box.append(&right_slot);
        let popover = gtk::Popover::new();
        popover.set_position(gtk::PositionType::Top);
        popover.set_autohide(false);
        popover.connect_realize(|popover| {
            if let Some(surface) = popover.surface() {
                // マウス追従時も背後のスライダーへmotion/leaveを届ける。
                surface.set_input_region(&gtk::cairo::Region::create());
            }
        });
        popover.set_child(Some(&preview_box));

        Self {
            cache: ThumbnailCache::new(THUMBNAIL_CACHE_BUDGET),
            state: PreviewState::new(),
            popover,
            left_slot,
            left,
            right,
        }
    }

    pub(super) fn set_parent(&self, parent: &impl IsA<gtk::Widget>) {
        self.popover.set_parent(parent);
    }

    pub(super) fn add_pointer_controller(&self, controller: gtk::EventControllerMotion) {
        self.popover.add_controller(controller);
    }

    pub(super) fn reset_for_document(&mut self) {
        self.cache.clear();
        self.state.reset_for_document();
        self.clear_preview();
        self.popover.popdown();
    }

    pub(super) fn generation_context(&self) -> (u64, std::sync::Arc<std::sync::atomic::AtomicU64>) {
        self.state.generation_context()
    }

    pub(super) fn disable(&mut self) {
        self.reset_for_document();
    }

    pub(super) fn raw_hover_target(&self) -> Option<PreviewTarget> {
        self.state.raw_hover_target()
    }

    pub(super) fn displayed_navigation_index(&self) -> Option<usize> {
        self.state
            .displayed_preview_target()
            .map(PreviewTarget::navigation_index)
    }

    pub(super) fn pointer(&self) -> Option<(i32, i32, i32)> {
        self.state.pointer()
    }

    pub(super) fn reset_hover_selection_for_view_change(&mut self) {
        self.state.reset_hover_selection_for_view_change();
        self.clear_preview();
    }

    pub(super) fn has_cached_preview(&self, target: PreviewTarget) -> bool {
        preview_pages_ready(target, |index| self.cache.contains(index))
    }

    pub(super) fn has_cached_asset(&self, first_page: usize, layout: ImageLayout) -> bool {
        self.cache.contains(first_page)
            && (layout == ImageLayout::Single || self.cache.contains(first_page + 1))
    }

    pub(super) fn distributed_warmup_needed(&self) -> bool {
        !self.cache.warmup_saturated()
    }

    pub(super) fn thumbnail_ready(
        &mut self,
        generation: u64,
        index: usize,
        thumbnail: Thumbnail,
    ) -> ThumbnailReady {
        if !self.state.accepts_generation(generation) {
            return ThumbnailReady::rejected();
        }
        if !thumbnail_buffer_is_valid(&thumbnail) {
            return ThumbnailReady::rejected();
        }
        use gtk::prelude::Cast;
        let accounted_bytes = thumbnail.pixels.len();
        let stride = thumbnail.stride;
        let pixels = gtk::glib::Bytes::from_owned(thumbnail.pixels);
        let texture = gtk::gdk::MemoryTexture::new(
            thumbnail.width,
            thumbnail.height,
            THUMBNAIL_MEMORY_FORMAT,
            &pixels,
            stride,
        );
        let raw_target = self.state.raw_hover_target();
        let displayed_target = self.state.displayed_preview_target();
        let warmup_saturated =
            self.cache
                .insert(index, texture.upcast(), accounted_bytes, |candidate| {
                    raw_target.is_some_and(|target| target.contains(candidate))
                        || displayed_target.is_some_and(|target| target.contains(candidate))
                });

        if let Some(target) = self.state.displayed_preview_target()
            && target.contains(index)
        {
            self.display_cached_preview(target);
        }
        ThumbnailReady {
            accepted: true,
            warmup_saturated,
        }
    }

    pub(super) fn display_current_cached_if_placeholder(&mut self) -> bool {
        let Some(target) = self.state.placeholder_target() else {
            return false;
        };
        if !self.display_cached_preview(target) {
            return false;
        }
        self.state.mark_preview_displayed(target);
        true
    }

    pub(super) fn show_hover_preview(
        &mut self,
        raw_target: PreviewTarget,
        nearest_ready: Option<PreviewTarget>,
        pointer: (i32, i32, i32),
        position_mode: PreviewPositionMode,
        image_textures: (Option<gtk::gdk::Paintable>, Option<gtk::gdk::Paintable>),
    ) -> Option<u64> {
        let update = self.state.update_hover(raw_target, nearest_ready, pointer);
        self.update_position(position_mode);

        let update = update?;
        let (image_right, image_left) = image_textures;

        let hold_revision = if let Some(target) = update.selected_target {
            if self.display_cached_preview(target)
                || self.display_image_preview(target, image_right.as_ref(), image_left.as_ref())
            {
                self.state.mark_preview_displayed(target);
            } else {
                self.state.mark_preview_unavailable();
                self.show_placeholder(raw_target.second_page.is_some());
            }
            None
        } else if self.right.paintable().is_some()
            && self.state.displayed_preview_target().is_some()
        {
            Some(update.hover_revision)
        } else {
            self.state.mark_preview_unavailable();
            self.show_placeholder(raw_target.second_page.is_some());
            None
        };

        if !self.popover.is_visible() {
            self.popover.popup();
        }
        hold_revision
    }

    fn display_cached_preview(&mut self, target: PreviewTarget) -> bool {
        if !self.has_cached_preview(target) {
            return false;
        }
        self.cache.touch(target.first_page);
        if let Some(second_page) = target.second_page {
            self.cache.touch(second_page);
        }
        if let Some(second_page) = target.second_page {
            let (Some(right), Some(left)) = (
                self.cache.get(target.first_page).cloned(),
                self.cache.get(second_page).cloned(),
            ) else {
                return false;
            };
            self.display_preview(right.upcast_ref(), Some(left.upcast_ref()));
        } else {
            let Some(right) = self.cache.get(target.first_page).cloned() else {
                return false;
            };
            self.display_preview(right.upcast_ref(), None);
        }
        true
    }

    fn display_image_preview(
        &self,
        target: PreviewTarget,
        right: Option<&gtk::gdk::Paintable>,
        left: Option<&gtk::gdk::Paintable>,
    ) -> bool {
        let Some(right) = right else {
            return false;
        };
        if target.second_page.is_some() {
            let Some(left) = left else {
                return false;
            };
            self.display_preview(right, Some(left));
        } else {
            self.display_preview(right, None);
        }
        true
    }

    fn display_preview(&self, right: &gtk::gdk::Paintable, left: Option<&gtk::gdk::Paintable>) {
        self.right.set_halign(if left.is_some() {
            gtk::Align::Start
        } else {
            gtk::Align::Fill
        });
        self.right.set_paintable(Some(right));
        self.left.set_paintable(left);
        self.left_slot.set_visible(left.is_some());
    }

    fn show_placeholder(&self, is_spread: bool) {
        self.clear_preview();
        self.left_slot.set_visible(is_spread);
    }

    fn clear_preview(&self) {
        self.right.set_paintable(None::<&gtk::gdk::Texture>);
        self.left.set_paintable(None::<&gtk::gdk::Texture>);
        self.left_slot.set_visible(false);
    }

    pub(super) fn previous_preview_hold_expired(&mut self, hover_revision: u64) {
        let Some(is_spread) = self.state.previous_preview_hold_expired(hover_revision) else {
            return;
        };
        self.show_placeholder(is_spread);
    }

    pub(super) fn hide(&mut self) {
        if self.state.raw_hover_target().is_none() && !self.popover.is_visible() {
            return;
        }
        self.state.hide();
        self.clear_preview();
        self.popover.popdown();
    }

    fn update_position(&self, position_mode: PreviewPositionMode) {
        let pointing_to = self
            .state
            .pointer()
            .and_then(|(x, width, height)| anchor_geometry(position_mode, x, width, height))
            .map(|(x, y, width, height)| gtk::gdk::Rectangle::new(x, y, width, height));

        self.popover.set_pointing_to(pointing_to.as_ref());
        if self.popover.is_visible() {
            self.popover.present();
        }
    }

    pub(super) fn refresh_position(&self, mode: PreviewPositionMode) {
        self.update_position(mode);
    }

    pub(super) fn shutdown(&mut self) {
        self.state.shutdown();
        self.clear_preview();
        self.popover.popdown();
        self.popover.unparent();
    }
}

pub(super) struct ThumbnailReady {
    accepted: bool,
    warmup_saturated: bool,
}

impl ThumbnailReady {
    fn rejected() -> Self {
        Self {
            accepted: false,
            warmup_saturated: false,
        }
    }

    pub(super) fn accepted(&self) -> bool {
        self.accepted
    }

    pub(super) fn warmup_saturated(&self) -> bool {
        self.warmup_saturated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_preview_uses_the_whole_slider_as_its_anchor() {
        assert_eq!(
            anchor_geometry(PreviewPositionMode::Centered, 25, 200, 20),
            None
        );
    }

    #[test]
    fn following_preview_tracks_pointer_x_and_offsets_up_from_the_slider() {
        assert_eq!(
            anchor_geometry(PreviewPositionMode::FollowPointer, 25, 200, 20),
            Some((25, -20, 1, 20))
        );
        assert_eq!(
            anchor_geometry(PreviewPositionMode::FollowPointer, -20, 200, 20),
            Some((0, -20, 1, 20))
        );
        assert_eq!(
            anchor_geometry(PreviewPositionMode::FollowPointer, 220, 200, 20),
            Some((199, -20, 1, 20))
        );
    }

    #[test]
    fn rgb_thumbnail_texture_layout_is_consistent() {
        let thumbnail = Thumbnail {
            pixels: vec![0; 30 * 20 * THUMBNAIL_BYTES_PER_PIXEL],
            width: 30,
            height: 20,
            stride: 30 * THUMBNAIL_BYTES_PER_PIXEL,
        };

        assert_eq!(THUMBNAIL_MEMORY_FORMAT, gtk::gdk::MemoryFormat::R8g8b8);
        assert!(thumbnail_buffer_is_valid(&thumbnail));
    }
}
