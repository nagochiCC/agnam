use adw::prelude::*;

const HEADER_SHOW_DISTANCE: f64 = 50.0;
const HEADER_HIDE_DISTANCE: f64 = 100.0;
const AUTO_HIDE_CSS_CLASS: &str = "header-auto-hide";

fn header_should_be_visible(
    auto_hide: bool,
    currently_visible: bool,
    distance_from_top: f64,
    pointer_over_header: bool,
) -> bool {
    if !auto_hide || pointer_over_header {
        return true;
    }
    if currently_visible {
        distance_from_top < HEADER_HIDE_DISTANCE
    } else {
        distance_from_top <= HEADER_SHOW_DISTANCE
    }
}

pub(super) struct HeaderController {
    visible: bool,
    effective_auto_hide: bool,
    pointer_distance_from_top: f64,
    pointer_over_header: bool,
    toolbar_view: Option<adw::ToolbarView>,
}

impl HeaderController {
    pub(super) fn new() -> Self {
        Self {
            visible: true,
            effective_auto_hide: false,
            pointer_distance_from_top: f64::INFINITY,
            pointer_over_header: false,
            toolbar_view: None,
        }
    }

    pub(super) fn set_toolbar_view(&mut self, toolbar_view: adw::ToolbarView) {
        self.toolbar_view = Some(toolbar_view);
        self.apply();
    }

    pub(super) fn set_auto_hide(&mut self, auto_hide: bool) {
        if auto_hide && !self.effective_auto_hide {
            self.visible =
                self.pointer_over_header || self.pointer_distance_from_top <= HEADER_SHOW_DISTANCE;
        } else {
            self.visible = header_should_be_visible(
                auto_hide,
                self.visible,
                self.pointer_distance_from_top,
                self.pointer_over_header,
            );
        }
        self.effective_auto_hide = auto_hide;
        self.apply();
    }

    pub(super) fn update_pointer(
        &mut self,
        auto_hide: bool,
        distance_from_top: f64,
        pointer_over_header: bool,
    ) {
        self.pointer_distance_from_top = distance_from_top;
        self.pointer_over_header = pointer_over_header;
        self.visible = header_should_be_visible(
            auto_hide,
            self.visible,
            distance_from_top,
            pointer_over_header,
        );
        self.effective_auto_hide = auto_hide;
        self.apply();
    }

    fn apply(&self) {
        let Some(toolbar_view) = self.toolbar_view.as_ref() else {
            return;
        };
        if self.effective_auto_hide {
            toolbar_view.add_css_class(AUTO_HIDE_CSS_CLASS);
        } else {
            toolbar_view.remove_css_class(AUTO_HIDE_CSS_CLASS);
        }
        toolbar_view.set_extend_content_to_top_edge(self.effective_auto_hide);
        toolbar_view.set_reveal_top_bars(self.visible);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_auto_hide_always_shows_the_header() {
        assert!(header_should_be_visible(false, false, f64::INFINITY, false));
    }

    #[test]
    fn hidden_header_is_revealed_only_at_the_top_edge() {
        assert!(header_should_be_visible(true, false, 50.0, false));
        assert!(!header_should_be_visible(true, false, 50.1, false));
    }

    #[test]
    fn visible_header_uses_the_hide_threshold_and_stays_visible_on_hover() {
        assert!(header_should_be_visible(true, true, 99.9, false));
        assert!(!header_should_be_visible(true, true, 100.0, false));
        assert!(header_should_be_visible(true, true, 100.0, true));
    }

    #[test]
    fn leaving_the_window_hides_an_auto_hidden_viewer_header() {
        assert!(!header_should_be_visible(true, true, f64::INFINITY, false));
    }
}
