use crate::viewer::snap_to_view;
use gtk::prelude::*;

const SLIDER_HOVER_VERTICAL_PADDING: f64 = 10.0;
const SLIDER_SHOW_DISTANCE: f64 = 50.0;
const SLIDER_HIDE_DISTANCE: f64 = 100.0;
const PAGE_LABEL_HORIZONTAL_PADDING: i32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BottomAreaMode {
    ReservedSlider,
    OverlaySlider,
}

pub(super) fn bottom_area_mode(auto_hide: bool) -> BottomAreaMode {
    if auto_hide {
        BottomAreaMode::OverlaySlider
    } else {
        BottomAreaMode::ReservedSlider
    }
}

pub(super) fn page_label_text(current_index: usize, page_count: usize) -> String {
    if page_count == 0 {
        return "0 / 0".to_string();
    }
    let current = current_index + 1;
    format!("<span font_features='tnum'>{current} / {page_count}</span>")
}

pub(super) fn slider_page_target(requested: f64, view_starts: &[usize]) -> Option<usize> {
    snap_to_view(requested.round() as usize, view_starts)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SliderNavigationInput {
    Pointer,
    Other,
}

pub(super) fn slider_navigation_target(
    raw_target: usize,
    displayed_preview_target: Option<usize>,
    input: SliderNavigationInput,
) -> usize {
    match input {
        SliderNavigationInput::Pointer => displayed_preview_target.unwrap_or(raw_target),
        SliderNavigationInput::Other => raw_target,
    }
}

pub(super) fn pointer_is_inside_slider(x: f64, y: f64, width: i32, height: i32) -> bool {
    x >= 0.0
        && x < f64::from(width)
        && y >= -SLIDER_HOVER_VERTICAL_PADDING
        && y < f64::from(height) + SLIDER_HOVER_VERTICAL_PADDING
}

fn slider_should_be_visible(
    auto_hide: bool,
    has_pages: bool,
    currently_visible: bool,
    distance_from_bottom: f64,
    pointer_over_bar: bool,
    dragging: bool,
) -> bool {
    if !has_pages {
        return false;
    }
    if !auto_hide || pointer_over_bar || dragging {
        return true;
    }
    if currently_visible {
        distance_from_bottom < SLIDER_HIDE_DISTANCE
    } else {
        distance_from_bottom <= SLIDER_SHOW_DISTANCE
    }
}

pub(super) struct SliderController {
    visible: bool,
    dragging: bool,
    pointer_distance_from_bottom: f64,
    pointer_over_bar: bool,
    overlay: Option<gtk::Overlay>,
    reserved_container: Option<gtk::Box>,
    bar: Option<gtk::Box>,
    page_label: Option<gtk::Label>,
}

impl SliderController {
    pub(super) fn new(auto_hide: bool) -> Self {
        Self {
            visible: !auto_hide,
            dragging: false,
            pointer_distance_from_bottom: f64::INFINITY,
            pointer_over_bar: false,
            overlay: None,
            reserved_container: None,
            bar: None,
            page_label: None,
        }
    }

    pub(super) fn is_visible(&self) -> bool {
        self.visible
    }

    pub(super) fn set_widgets(
        &mut self,
        overlay: gtk::Overlay,
        reserved_container: gtk::Box,
        bar: gtk::Box,
        page_label: gtk::Label,
    ) {
        self.overlay = Some(overlay);
        self.reserved_container = Some(reserved_container);
        self.bar = Some(bar);
        self.page_label = Some(page_label);
    }

    pub(super) fn update_page_label_width(&self, page_count: usize) {
        let Some(page_label) = self.page_label.as_ref() else {
            return;
        };
        if page_count == 0 {
            page_label.set_width_request(-1);
            return;
        }

        let layout = page_label.create_pango_layout(None);
        layout.set_markup(&page_label_text(page_count - 1, page_count));
        let (text_width, _) = layout.pixel_size();
        page_label.set_width_request(text_width + PAGE_LABEL_HORIZONTAL_PADDING * 2);
    }

    pub(super) fn set_auto_hide(&mut self, auto_hide: bool) {
        self.visible = !auto_hide;
        self.place_bar(auto_hide);
    }

    pub(super) fn place_bar(&self, auto_hide: bool) {
        let (Some(overlay), Some(reserved_container), Some(bar)) = (
            self.overlay.as_ref(),
            self.reserved_container.as_ref(),
            self.bar.as_ref(),
        ) else {
            return;
        };

        let parent = bar.parent();
        match bottom_area_mode(auto_hide) {
            BottomAreaMode::ReservedSlider => {
                if parent.as_ref() == Some(overlay.upcast_ref()) {
                    overlay.remove_overlay(bar);
                }
                if bar.parent().is_none() {
                    reserved_container.append(bar);
                }
            }
            BottomAreaMode::OverlaySlider => {
                if parent.as_ref() == Some(reserved_container.upcast_ref()) {
                    reserved_container.remove(bar);
                }
                if bar.parent().is_none() {
                    overlay.add_overlay(bar);
                }
            }
        }
    }

    pub(super) fn reevaluate_visibility(&mut self, auto_hide: bool, has_pages: bool) {
        self.visible = slider_should_be_visible(
            auto_hide,
            has_pages,
            self.visible,
            self.pointer_distance_from_bottom,
            self.pointer_over_bar,
            self.dragging,
        );
    }

    pub(super) fn update_visibility(
        &mut self,
        auto_hide: bool,
        has_pages: bool,
        distance_from_bottom: f64,
        pointer_over_bar: bool,
    ) {
        self.pointer_distance_from_bottom = distance_from_bottom;
        self.pointer_over_bar = pointer_over_bar;
        self.reevaluate_visibility(auto_hide, has_pages);
    }

    pub(super) fn set_dragging(&mut self, auto_hide: bool, has_pages: bool, dragging: bool) {
        self.dragging = dragging;
        self.reevaluate_visibility(auto_hide, has_pages);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_page_labels_for_empty_and_loaded_documents() {
        assert_eq!(page_label_text(0, 0), "0 / 0");
        assert_eq!(
            page_label_text(4, 123),
            "<span font_features='tnum'>5 / 123</span>"
        );
        assert_eq!(
            page_label_text(25, 123),
            "<span font_features='tnum'>26 / 123</span>"
        );
        assert_eq!(
            page_label_text(122, 123),
            "<span font_features='tnum'>123 / 123</span>"
        );
    }

    #[test]
    fn slider_uses_the_requested_position() {
        let view_starts = [0, 2, 4];

        assert_eq!(slider_page_target(4.0, &view_starts), Some(4));
        assert_eq!(slider_page_target(3.0, &view_starts), Some(4));
        assert_eq!(slider_page_target(1.0, &view_starts), Some(2));
    }

    #[test]
    fn pointer_slider_navigation_uses_the_displayed_preview_target() {
        assert_eq!(
            slider_navigation_target(200, Some(198), SliderNavigationInput::Pointer),
            198
        );
    }

    #[test]
    fn pointer_slider_navigation_falls_back_to_the_raw_target_without_a_preview() {
        assert_eq!(
            slider_navigation_target(200, None, SliderNavigationInput::Pointer),
            200
        );
    }

    #[test]
    fn non_pointer_slider_navigation_ignores_the_displayed_preview_target() {
        assert_eq!(
            slider_navigation_target(200, Some(198), SliderNavigationInput::Other),
            200
        );
    }

    #[test]
    fn pointer_navigation_preserves_displayed_spread_view_starts() {
        let view_starts = [0, 2, 4, 6];
        let raw_target = slider_page_target(3.0, &view_starts).unwrap();

        assert_eq!(raw_target, 4);
        assert_eq!(
            slider_navigation_target(raw_target, Some(2), SliderNavigationInput::Pointer),
            2
        );
    }

    #[test]
    fn slider_pointer_bounds_include_the_extended_vertical_hit_area() {
        assert!(pointer_is_inside_slider(25.0, 10.0, 200, 20));
        assert!(pointer_is_inside_slider(25.0, -10.0, 200, 20));
        assert!(pointer_is_inside_slider(25.0, 29.9, 200, 20));
        assert!(!pointer_is_inside_slider(25.0, -10.1, 200, 20));
        assert!(!pointer_is_inside_slider(25.0, 30.0, 200, 20));
        assert!(!pointer_is_inside_slider(-0.1, 10.0, 200, 20));
        assert!(!pointer_is_inside_slider(200.0, 10.0, 200, 20));
    }

    #[test]
    fn slider_visibility_uses_hysteresis() {
        assert!(!slider_should_be_visible(
            true, true, false, 50.000_001, false, false
        ));
        assert!(slider_should_be_visible(
            true, true, false, 50.0, false, false
        ));
        assert!(slider_should_be_visible(
            true, true, true, 99.999_999, false, false
        ));
        assert!(!slider_should_be_visible(
            true, true, true, 100.0, false, false
        ));
    }

    #[test]
    fn slider_visibility_preserves_loaded_and_interaction_rules() {
        assert!(slider_should_be_visible(
            false, true, false, 500.0, false, false
        ));
        assert!(!slider_should_be_visible(
            false, false, true, 0.0, true, true
        ));
        assert!(slider_should_be_visible(
            true, true, false, 500.0, true, false
        ));
        assert!(slider_should_be_visible(
            true, true, false, 500.0, false, true
        ));
    }

    #[test]
    fn bottom_area_uses_the_configured_slider_placement() {
        assert_eq!(bottom_area_mode(false), BottomAreaMode::ReservedSlider);
        assert_eq!(bottom_area_mode(true), BottomAreaMode::OverlaySlider);
    }
}
