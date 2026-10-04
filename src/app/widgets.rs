use adw::prelude::*;

pub struct AppWidgets {
    pub(super) main_window: adw::ApplicationWindow,
    pub(super) search_toggle: gtk::ToggleButton,
    pub(super) history_toggle: gtk::ToggleButton,
    pub(super) favorites_toggle: gtk::ToggleButton,
    pub(super) navigate_up_button: gtk::Button,
    pub(super) navigation_split_view: adw::OverlaySplitView,
    pub(super) sidebar_header: adw::HeaderBar,
    pub(super) navigation_panel_title: gtk::Label,
    pub(super) clear_history_button: gtk::Button,
    pub(super) navigation_panel_stack: gtk::Stack,
    pub(super) library_search_entry: gtk::SearchEntry,
    pub(super) library_search_scroller: gtk::ScrolledWindow,
    pub(super) library_search_items: gtk::Box,
    pub(super) history_scroller: gtk::ScrolledWindow,
    pub(super) history_items: gtk::Box,
    pub(super) favorites_scroller: gtk::ScrolledWindow,
    pub(super) favorites_items: gtk::Box,
    pub(super) toolbar_view: adw::ToolbarView,
    pub(super) header_bar: adw::HeaderBar,
    pub(super) header_title: adw::WindowTitle,
    pub(super) breadcrumb_container: gtk::Box,
    pub(super) library_sort_button: gtk::MenuButton,
    pub(super) smart_crop_toggle: gtk::ToggleButton,
    pub(super) viewer_controls: gtk::Box,
    pub(super) next_single_button: gtk::Button,
    pub(super) spread_toggle: gtk::ToggleButton,
    pub(super) prev_single_button: gtk::Button,
    pub(super) library_content: gtk::Box,
    pub(super) library_status: gtk::Box,
    pub(super) library_status_spinner: gtk::Spinner,
    pub(super) library_status_label: gtk::Label,
    pub(super) select_bookshelf_button: gtk::Button,
    pub(super) library_scroller: gtk::ScrolledWindow,
    pub(super) library_items: gtk::Box,
    pub(super) viewer_content: gtk::Overlay,
    pub(super) slider_overlay: gtk::Overlay,
    pub(super) document_toast_overlay: adw::ToastOverlay,
    pub(super) image_container: gtk::Box,
    pub(super) left_picture: gtk::Picture,
    pub(super) right_picture: gtk::Picture,
    pub(super) document_boundary: gtk::Box,
    pub(super) boundary_next_label: gtk::Label,
    pub(super) boundary_previous_label: gtk::Label,
    pub(super) slider_reserved_container: gtk::Box,
    pub(super) page_slider_bar: gtk::Box,
    pub(super) page_slider: gtk::Scale,
    pub(super) page_label: gtk::Label,
    pub(super) loading_layer: gtk::Overlay,
    pub(super) loading_spinner: gtk::Spinner,
}

impl AppWidgets {
    pub(super) fn build(
        root: &adw::ApplicationWindow,
        main_menu: &gtk::gio::Menu,
        library_sort_menu: &gtk::gio::Menu,
    ) -> Self {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 0);

        let navigation_rail = gtk::Box::new(gtk::Orientation::Vertical, 2);
        navigation_rail.set_width_request(48);
        navigation_rail.add_css_class("navigation-rail");

        let menu_button = gtk::MenuButton::new();
        menu_button.set_icon_name("open-menu-symbolic");
        menu_button.set_tooltip_text(Some("メインメニュー"));
        menu_button.set_width_request(40);
        menu_button.set_height_request(40);
        menu_button.set_margin_top(6);
        menu_button.set_margin_start(4);
        menu_button.set_margin_end(4);
        menu_button.set_margin_bottom(14);
        menu_button.add_css_class("flat");
        let menu_popover = gtk::PopoverMenu::from_model(Some(main_menu));
        menu_button.set_popover(Some(&menu_popover));
        navigation_rail.append(&menu_button);

        let navigate_up_button = gtk::Button::new();
        navigate_up_button.set_icon_name("go-up-symbolic");
        navigate_up_button.set_tooltip_text(Some("上の階層へ"));
        navigate_up_button.set_width_request(40);
        navigate_up_button.set_height_request(40);
        navigate_up_button.set_margin_top(3);
        navigate_up_button.set_margin_start(4);
        navigate_up_button.set_margin_end(4);
        navigate_up_button.set_margin_bottom(4);
        navigate_up_button.add_css_class("flat");
        navigation_rail.append(&navigate_up_button);

        let search_toggle = gtk::ToggleButton::new();
        search_toggle.set_icon_name("system-search-symbolic");
        search_toggle.set_tooltip_text(Some("本棚を検索"));
        search_toggle.set_action_name(Some("app.search-library"));
        search_toggle.set_width_request(40);
        search_toggle.set_height_request(40);
        search_toggle.set_margin_start(4);
        search_toggle.set_margin_end(4);
        search_toggle.add_css_class("flat");
        navigation_rail.append(&search_toggle);

        let history_toggle = gtk::ToggleButton::new();
        history_toggle.set_icon_name("document-open-recent-symbolic");
        history_toggle.set_tooltip_text(Some("閲覧履歴"));
        history_toggle.set_width_request(40);
        history_toggle.set_height_request(40);
        history_toggle.set_margin_start(4);
        history_toggle.set_margin_end(4);
        history_toggle.add_css_class("flat");
        navigation_rail.append(&history_toggle);

        let favorites_toggle = gtk::ToggleButton::new();
        favorites_toggle.set_icon_name("starred-symbolic");
        favorites_toggle.set_tooltip_text(Some("お気に入り"));
        favorites_toggle.set_width_request(40);
        favorites_toggle.set_height_request(40);
        favorites_toggle.set_margin_start(4);
        favorites_toggle.set_margin_end(4);
        favorites_toggle.set_margin_bottom(6);
        favorites_toggle.add_css_class("flat");
        navigation_rail.append(&favorites_toggle);

        let navigation_spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        navigation_spacer.set_vexpand(true);
        navigation_rail.append(&navigation_spacer);
        navigation_rail.reorder_child_after(&menu_button, Some(&navigation_spacer));
        menu_button.set_margin_top(0);
        menu_button.set_margin_bottom(6);

        content.append(&navigation_rail);
        let separator = gtk::Separator::new(gtk::Orientation::Vertical);
        content.append(&separator);

        let navigation_panel_title = gtk::Label::new(None);
        navigation_panel_title.add_css_class("heading");
        let clear_history_button = gtk::Button::new();
        clear_history_button.set_icon_name("user-trash-symbolic");
        clear_history_button.set_tooltip_text(Some("履歴をすべて削除"));
        clear_history_button.add_css_class("destructive-action");
        let sidebar_header = adw::HeaderBar::new();
        sidebar_header.set_title_widget(Some(&navigation_panel_title));
        sidebar_header.pack_end(&clear_history_button);

        let library_search_entry = gtk::SearchEntry::new();
        library_search_entry.set_placeholder_text(Some("本棚を検索…"));
        library_search_entry.set_margin_top(6);
        library_search_entry.set_margin_bottom(6);
        library_search_entry.set_margin_start(6);
        library_search_entry.set_margin_end(6);
        let library_search_items = gtk::Box::new(gtk::Orientation::Vertical, 0);
        library_search_items.set_vexpand(true);
        let library_search_scroller = gtk::ScrolledWindow::new();
        library_search_scroller.set_vexpand(true);
        library_search_scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        library_search_scroller.set_vscrollbar_policy(gtk::PolicyType::Automatic);
        library_search_scroller.set_child(Some(&library_search_items));
        let search_panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
        search_panel.append(&library_search_entry);
        search_panel.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        search_panel.append(&library_search_scroller);

        let history_items = gtk::Box::new(gtk::Orientation::Vertical, 0);
        history_items.set_vexpand(true);
        let history_scroller = gtk::ScrolledWindow::new();
        history_scroller.set_vexpand(true);
        history_scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        history_scroller.set_vscrollbar_policy(gtk::PolicyType::Automatic);
        history_scroller.set_child(Some(&history_items));
        let history_panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
        history_panel.append(&history_scroller);

        let favorites_items = gtk::Box::new(gtk::Orientation::Vertical, 0);
        favorites_items.set_vexpand(true);
        let favorites_scroller = gtk::ScrolledWindow::new();
        favorites_scroller.set_vexpand(true);
        favorites_scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        favorites_scroller.set_vscrollbar_policy(gtk::PolicyType::Automatic);
        favorites_scroller.set_child(Some(&favorites_items));
        let favorites_panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
        favorites_panel.append(&favorites_scroller);

        let navigation_panel_stack = gtk::Stack::new();
        navigation_panel_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        navigation_panel_stack.add_named(&search_panel, Some("search"));
        navigation_panel_stack.add_named(&history_panel, Some("history"));
        navigation_panel_stack.add_named(&favorites_panel, Some("favorites"));
        let sidebar = adw::ToolbarView::new();
        sidebar.add_top_bar(&sidebar_header);
        sidebar.set_content(Some(&navigation_panel_stack));

        let breadcrumb_container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        breadcrumb_container.set_valign(gtk::Align::Fill);
        breadcrumb_container.add_css_class("breadcrumb-container");
        let library_sort_button = gtk::MenuButton::new();
        library_sort_button.set_icon_name("view-list-ordered-symbolic");
        library_sort_button.set_tooltip_text(Some("並び替え"));
        let sort_popover = gtk::PopoverMenu::from_model(Some(library_sort_menu));
        library_sort_button.set_popover(Some(&sort_popover));
        let smart_crop_toggle = gtk::ToggleButton::new();
        smart_crop_toggle.set_icon_name("image-crop-symbolic");
        smart_crop_toggle.set_tooltip_text(Some("スマートcrop"));
        let viewer_controls = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        viewer_controls.add_css_class("linked");
        let next_single_button = gtk::Button::new();
        next_single_button.set_icon_name("go-previous-symbolic");
        next_single_button.set_tooltip_text(Some("1ページ進む"));
        let spread_toggle = gtk::ToggleButton::new();
        spread_toggle.set_icon_name("view-dual-symbolic");
        spread_toggle.set_tooltip_text(Some("見開き"));
        let prev_single_button = gtk::Button::new();
        prev_single_button.set_icon_name("go-next-symbolic");
        prev_single_button.set_tooltip_text(Some("1ページ戻す"));
        viewer_controls.append(&next_single_button);
        viewer_controls.append(&spread_toggle);
        viewer_controls.append(&prev_single_button);
        let header_title = adw::WindowTitle::new("Agnam", "");
        let header_bar = adw::HeaderBar::new();
        header_bar.add_css_class("breadcrumb-header-bar");
        header_bar.set_show_title(true);
        header_bar.set_title_widget(Some(&header_title));
        header_bar.pack_start(&breadcrumb_container);
        header_bar.pack_end(&library_sort_button);
        header_bar.pack_end(&smart_crop_toggle);
        header_bar.pack_end(&viewer_controls);

        let library_status = gtk::Box::new(gtk::Orientation::Vertical, 12);
        library_status.set_halign(gtk::Align::Center);
        library_status.set_valign(gtk::Align::Center);
        library_status.set_vexpand(true);
        library_status.set_margin_start(24);
        library_status.set_margin_end(24);
        let library_status_spinner = gtk::Spinner::new();
        let library_status_label = gtk::Label::new(None);
        library_status_label.set_justify(gtk::Justification::Center);
        library_status_label.set_wrap(true);
        let select_bookshelf_button = gtk::Button::with_label("本棚フォルダを選択…");
        library_status.append(&library_status_spinner);
        library_status.append(&library_status_label);
        library_status.append(&select_bookshelf_button);
        let library_items = gtk::Box::new(gtk::Orientation::Vertical, 2);
        library_items.set_margin_top(12);
        library_items.set_margin_bottom(24);
        library_items.set_valign(gtk::Align::Start);
        let library_scroller = gtk::ScrolledWindow::new();
        library_scroller.set_vexpand(true);
        library_scroller.set_hscrollbar_policy(gtk::PolicyType::External);
        library_scroller.set_vscrollbar_policy(gtk::PolicyType::Automatic);
        library_scroller.set_child(Some(&library_items));
        let library_content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        library_content.set_vexpand(true);
        library_content.set_hexpand(true);
        library_content.append(&library_status);
        library_content.append(&library_scroller);

        let left_picture = gtk::Picture::new();
        left_picture.set_can_shrink(true);
        left_picture.set_hexpand(true);
        left_picture.set_halign(gtk::Align::End);
        let right_picture = gtk::Picture::new();
        right_picture.set_can_shrink(true);
        right_picture.set_hexpand(true);

        let boundary_next_label = gtk::Label::new(None);
        boundary_next_label.add_css_class("document-boundary-name");
        boundary_next_label.set_hexpand(true);
        boundary_next_label.set_xalign(0.5);
        boundary_next_label.set_justify(gtk::Justification::Center);
        boundary_next_label.set_max_width_chars(30);
        boundary_next_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        let boundary_previous_label = gtk::Label::new(None);
        boundary_previous_label.add_css_class("document-boundary-name");
        boundary_previous_label.set_hexpand(true);
        boundary_previous_label.set_xalign(0.5);
        boundary_previous_label.set_justify(gtk::Justification::Center);
        boundary_previous_label.set_max_width_chars(30);
        boundary_previous_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        let next_side = gtk::Box::new(gtk::Orientation::Vertical, 0);
        next_side.add_css_class("document-boundary-side");
        next_side.set_hexpand(true);
        let next_heading = gtk::Label::new(Some("次のファイル"));
        next_heading.add_css_class("dim-label");
        next_heading.add_css_class("document-boundary-heading");
        next_heading.set_hexpand(true);
        next_heading.set_xalign(0.5);
        next_heading.set_justify(gtk::Justification::Center);
        next_side.append(&next_heading);
        next_side.append(&boundary_next_label);
        let previous_side = gtk::Box::new(gtk::Orientation::Vertical, 0);
        previous_side.add_css_class("document-boundary-side");
        previous_side.set_hexpand(true);
        let previous_heading = gtk::Label::new(Some("前のファイル"));
        previous_heading.add_css_class("dim-label");
        previous_heading.add_css_class("document-boundary-heading");
        previous_heading.set_hexpand(true);
        previous_heading.set_xalign(0.5);
        previous_heading.set_justify(gtk::Justification::Center);
        previous_side.append(&previous_heading);
        previous_side.append(&boundary_previous_label);
        let boundary_spread = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        boundary_spread.add_css_class("document-boundary-spread");
        boundary_spread.set_hexpand(true);
        boundary_spread.set_homogeneous(true);
        boundary_spread.append(&next_side);
        boundary_spread.append(&previous_side);
        let boundary_separator = gtk::Separator::new(gtk::Orientation::Vertical);
        boundary_separator.add_css_class("document-boundary-divider");
        boundary_separator.set_halign(gtk::Align::Center);
        boundary_separator.set_valign(gtk::Align::Center);
        let boundary_overlay = gtk::Overlay::new();
        boundary_overlay.set_child(Some(&boundary_spread));
        boundary_overlay.add_overlay(&boundary_separator);
        let boundary_clamp = adw::Clamp::new();
        boundary_clamp.set_maximum_size(680);
        boundary_clamp.set_tightening_threshold(500);
        boundary_clamp.set_hexpand(true);
        boundary_clamp.set_vexpand(true);
        boundary_clamp.set_valign(gtk::Align::Center);
        boundary_clamp.set_child(Some(&boundary_overlay));
        let document_boundary = gtk::Box::new(gtk::Orientation::Vertical, 0);
        document_boundary.add_css_class("document-boundary-size-medium");
        document_boundary.set_hexpand(true);
        document_boundary.set_vexpand(true);
        document_boundary.set_halign(gtk::Align::Fill);
        document_boundary.set_valign(gtk::Align::Fill);
        document_boundary.set_margin_start(24);
        document_boundary.set_margin_end(24);
        document_boundary.append(&boundary_clamp);

        let image_container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        image_container.set_vexpand(true);
        image_container.set_hexpand(true);
        image_container.set_halign(gtk::Align::Fill);
        image_container.set_valign(gtk::Align::Fill);
        image_container.append(&left_picture);
        image_container.append(&right_picture);
        image_container.append(&document_boundary);
        let document_toast_overlay = adw::ToastOverlay::new();
        document_toast_overlay.set_vexpand(true);
        document_toast_overlay.set_hexpand(true);
        document_toast_overlay.set_child(Some(&image_container));
        let slider_reserved_container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let viewer_stack = gtk::Box::new(gtk::Orientation::Vertical, 0);
        viewer_stack.append(&document_toast_overlay);
        viewer_stack.append(&slider_reserved_container);
        let page_slider = gtk::Scale::new(gtk::Orientation::Horizontal, None::<&gtk::Adjustment>);
        page_slider.set_hexpand(true);
        page_slider.set_focusable(false);
        page_slider.set_draw_value(false);
        page_slider.set_inverted(true);
        page_slider.set_round_digits(0);
        let page_label = gtk::Label::new(None);
        page_label.add_css_class("viewer-page-label");
        page_label.set_use_markup(true);
        page_label.set_xalign(0.5);
        page_label.set_valign(gtk::Align::Center);
        let page_slider_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        page_slider_bar.set_halign(gtk::Align::Fill);
        page_slider_bar.set_valign(gtk::Align::End);
        page_slider_bar.set_margin_top(6);
        page_slider_bar.set_margin_bottom(6);
        page_slider_bar.set_margin_start(12);
        page_slider_bar.set_margin_end(12);
        page_slider_bar.append(&page_slider);
        page_slider_bar.append(&page_label);
        let slider_overlay = gtk::Overlay::new();
        slider_overlay.set_child(Some(&viewer_stack));
        slider_overlay.add_overlay(&page_slider_bar);
        let viewer_content = gtk::Overlay::new();
        viewer_content.set_focusable(true);
        viewer_content.set_vexpand(true);
        viewer_content.set_child(Some(&slider_overlay));

        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.append(&library_content);
        body.append(&viewer_content);
        let loading_layer = gtk::Overlay::new();
        loading_layer.add_css_class("loading-overlay");
        loading_layer.set_can_target(true);
        loading_layer.set_halign(gtk::Align::Fill);
        loading_layer.set_valign(gtk::Align::Fill);
        let loading_spinner = gtk::Spinner::new();
        let loading_indicator = gtk::Box::new(gtk::Orientation::Vertical, 8);
        loading_indicator.add_css_class("loading-indicator");
        loading_indicator.set_halign(gtk::Align::Center);
        loading_indicator.set_valign(gtk::Align::Center);
        loading_indicator.append(&loading_spinner);
        loading_indicator.append(&gtk::Label::new(Some("読み込み中…")));
        loading_layer.set_child(Some(&loading_indicator));
        let main_overlay = gtk::Overlay::new();
        main_overlay.set_child(Some(&body));
        main_overlay.add_overlay(&loading_layer);
        let drop_highlight = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        drop_highlight.add_css_class("drop-highlight");
        drop_highlight.set_can_target(false);
        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&header_bar);
        toolbar_view.set_content(Some(&main_overlay));
        let main_content = gtk::Overlay::new();
        main_content.set_child(Some(&toolbar_view));
        main_content.add_overlay(&drop_highlight);

        let navigation_split_view = adw::OverlaySplitView::new();
        navigation_split_view.set_hexpand(true);
        navigation_split_view.set_collapsed(true);
        navigation_split_view.set_enable_show_gesture(false);
        navigation_split_view.set_enable_hide_gesture(true);
        navigation_split_view.set_min_sidebar_width(super::NAVIGATION_DRAWER_MIN_WIDTH);
        navigation_split_view.set_max_sidebar_width(super::NAVIGATION_DRAWER_MAX_WIDTH);
        navigation_split_view.set_sidebar_width_fraction(super::NAVIGATION_DRAWER_WIDTH_FRACTION);
        navigation_split_view.set_sidebar_position(gtk::PackType::Start);
        navigation_split_view.set_sidebar(Some(&sidebar));
        navigation_split_view.set_content(Some(&main_content));
        content.append(&navigation_split_view);
        root.set_content(Some(&content));

        Self {
            main_window: root.clone(),
            search_toggle,
            history_toggle,
            favorites_toggle,
            navigate_up_button,
            navigation_split_view,
            sidebar_header,
            navigation_panel_title,
            clear_history_button,
            navigation_panel_stack,
            library_search_entry,
            library_search_scroller,
            library_search_items,
            history_scroller,
            history_items,
            favorites_scroller,
            favorites_items,
            toolbar_view,
            header_bar,
            header_title,
            breadcrumb_container,
            library_sort_button,
            smart_crop_toggle,
            viewer_controls,
            next_single_button,
            spread_toggle,
            prev_single_button,
            library_content,
            library_status,
            library_status_spinner,
            library_status_label,
            select_bookshelf_button,
            library_scroller,
            library_items,
            viewer_content,
            slider_overlay,
            document_toast_overlay,
            image_container,
            left_picture,
            right_picture,
            document_boundary,
            boundary_next_label,
            boundary_previous_label,
            slider_reserved_container,
            page_slider_bar,
            page_slider,
            page_label,
            loading_layer,
            loading_spinner,
        }
    }
}
