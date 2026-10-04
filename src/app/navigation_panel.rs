use adw::prelude::*;

pub(super) fn navigation_empty_state(icon_name: &str, title: &str, description: &str) -> gtk::Box {
    let state = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .vexpand(true)
        .margin_start(24)
        .margin_end(24)
        .build();
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(48);
    icon.add_css_class("dim-label");
    state.append(&icon);
    state.append(
        &gtk::Label::builder()
            .label(title)
            .css_classes(["title-2"])
            .build(),
    );
    state.append(
        &gtk::Label::builder()
            .label(description)
            .wrap(true)
            .justify(gtk::Justification::Center)
            .css_classes(["dim-label"])
            .build(),
    );
    state
}

pub(super) fn detach_navigation_popover(popover: &gtk::Popover) {
    popover.popdown();
    if popover.parent().is_some() {
        popover.unparent();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NavigationPanel {
    Search,
    History,
    Favorites,
}

impl NavigationPanel {
    fn stack_child_name(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::History => "history",
            Self::Favorites => "favorites",
        }
    }
}

pub(super) struct NavigationPanelController {
    selected: Option<NavigationPanel>,
    stack: Option<gtk::Stack>,
    search_dirty: bool,
    history_dirty: bool,
    favorites_dirty: bool,
}

impl Default for NavigationPanelController {
    fn default() -> Self {
        Self {
            selected: None,
            stack: None,
            search_dirty: true,
            history_dirty: true,
            favorites_dirty: true,
        }
    }
}

impl NavigationPanelController {
    pub(super) fn selected(&self) -> Option<NavigationPanel> {
        self.selected
    }

    pub(super) fn is_selected(&self, panel: NavigationPanel) -> bool {
        self.selected == Some(panel)
    }

    pub(super) fn is_open(&self) -> bool {
        self.selected.is_some()
    }

    pub(super) fn toggled_selection(&self, panel: NavigationPanel) -> Option<NavigationPanel> {
        (self.selected != Some(panel)).then_some(panel)
    }

    pub(super) fn set_selected(&mut self, panel: Option<NavigationPanel>) -> bool {
        if self.selected == panel {
            return false;
        }
        self.selected = panel;
        self.sync_stack();
        true
    }

    pub(super) fn attach(&mut self, stack: gtk::Stack) {
        self.stack = Some(stack);
        self.sync_stack();
    }

    pub(super) fn is_dirty(&self, panel: NavigationPanel) -> bool {
        match panel {
            NavigationPanel::Search => self.search_dirty,
            NavigationPanel::History => self.history_dirty,
            NavigationPanel::Favorites => self.favorites_dirty,
        }
    }

    pub(super) fn mark_dirty(&mut self, panel: NavigationPanel) {
        match panel {
            NavigationPanel::Search => self.search_dirty = true,
            NavigationPanel::History => self.history_dirty = true,
            NavigationPanel::Favorites => self.favorites_dirty = true,
        }
    }

    pub(super) fn mark_cover_override_dirty(&mut self) {
        self.mark_dirty(NavigationPanel::Search);
        self.mark_dirty(NavigationPanel::History);
        self.mark_dirty(NavigationPanel::Favorites);
    }

    pub(super) fn mark_clean(&mut self, panel: NavigationPanel) {
        match panel {
            NavigationPanel::Search => self.search_dirty = false,
            NavigationPanel::History => self.history_dirty = false,
            NavigationPanel::Favorites => self.favorites_dirty = false,
        }
    }

    fn sync_stack(&self) {
        if let (Some(panel), Some(stack)) = (self.selected, &self.stack) {
            stack.set_visible_child_name(panel.stack_child_name());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_opens_closes_and_switches_panels() {
        let mut controller = NavigationPanelController::default();

        let selected = controller.toggled_selection(NavigationPanel::Search);
        assert!(controller.set_selected(selected));
        assert_eq!(controller.selected(), Some(NavigationPanel::Search));

        let selected = controller.toggled_selection(NavigationPanel::Search);
        assert!(controller.set_selected(selected));
        assert_eq!(controller.selected(), None);

        controller.set_selected(Some(NavigationPanel::Search));
        let selected = controller.toggled_selection(NavigationPanel::History);
        controller.set_selected(selected);
        assert_eq!(controller.selected(), Some(NavigationPanel::History));

        let selected = controller.toggled_selection(NavigationPanel::Search);
        controller.set_selected(selected);
        assert_eq!(controller.selected(), Some(NavigationPanel::Search));

        let selected = controller.toggled_selection(NavigationPanel::Favorites);
        controller.set_selected(selected);
        assert_eq!(controller.selected(), Some(NavigationPanel::Favorites));

        let selected = controller.toggled_selection(NavigationPanel::Favorites);
        controller.set_selected(selected);
        assert_eq!(controller.selected(), None);

        controller.set_selected(Some(NavigationPanel::Favorites));
        let selected = controller.toggled_selection(NavigationPanel::History);
        controller.set_selected(selected);
        assert_eq!(controller.selected(), Some(NavigationPanel::History));
    }

    #[test]
    fn dirty_state_is_independent_for_each_panel() {
        let mut controller = NavigationPanelController::default();
        assert!(controller.is_dirty(NavigationPanel::Search));
        assert!(controller.is_dirty(NavigationPanel::History));
        assert!(controller.is_dirty(NavigationPanel::Favorites));

        controller.mark_clean(NavigationPanel::Search);
        assert!(!controller.is_dirty(NavigationPanel::Search));
        assert!(controller.is_dirty(NavigationPanel::History));
        assert!(controller.is_dirty(NavigationPanel::Favorites));

        controller.mark_dirty(NavigationPanel::Search);
        controller.mark_clean(NavigationPanel::History);
        assert!(controller.is_dirty(NavigationPanel::Search));
        assert!(!controller.is_dirty(NavigationPanel::History));
        assert!(controller.is_dirty(NavigationPanel::Favorites));

        controller.mark_clean(NavigationPanel::Favorites);
        assert!(!controller.is_dirty(NavigationPanel::Favorites));
    }
}
