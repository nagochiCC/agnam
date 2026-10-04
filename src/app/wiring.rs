use super::message::Msg;
use super::sender::AppSender;
use super::slider::pointer_is_inside_slider;
use super::widgets::AppWidgets;
use adw::prelude::*;
use gtk::glib::clone;
use std::cell::Cell;
use std::rc::Rc;

pub(super) struct UiSignalHandlers {
    pub(super) spread_toggle: gtk::glib::SignalHandlerId,
    pub(super) smart_crop_toggle: gtk::glib::SignalHandlerId,
    pub(super) page_slider: gtk::glib::SignalHandlerId,
}

pub(super) struct UiWiring {
    pub(super) signal_handlers: UiSignalHandlers,
    pub(super) preview_pointer_controller: gtk::EventControllerMotion,
}

pub(super) fn connect(widgets: &AppWidgets, sender: &AppSender) -> UiWiring {
    // Capture-phase primary button events bracket the Scale's pointer-driven
    // change-value emissions; keyboard and other change-value paths do not.
    let slider_pointer_active = Rc::new(Cell::new(false));
    let signal_handlers = connect_widget_signals(widgets, sender, &slider_pointer_active);
    connect_library_search(widgets, sender);
    connect_library_viewport(widgets, sender);
    connect_document_boundary_size(widgets);
    connect_navigation_split_view(widgets, sender);
    connect_window_pointer(widgets, sender);
    connect_drop_target(widgets, sender);
    let preview_pointer_controller = connect_slider_pointer(widgets, sender, slider_pointer_active);
    connect_window_state(widgets, sender);

    UiWiring {
        signal_handlers,
        preview_pointer_controller,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DocumentBoundarySize {
    Small,
    Medium,
    Large,
}

impl DocumentBoundarySize {
    const ALL_CSS_CLASSES: [&'static str; 3] = [
        "document-boundary-size-small",
        "document-boundary-size-medium",
        "document-boundary-size-large",
    ];

    fn css_class(self) -> &'static str {
        match self {
            Self::Small => "document-boundary-size-small",
            Self::Medium => "document-boundary-size-medium",
            Self::Large => "document-boundary-size-large",
        }
    }
}

fn document_boundary_size(window_width: i32) -> DocumentBoundarySize {
    match window_width {
        ..=899 => DocumentBoundarySize::Small,
        900..=1399 => DocumentBoundarySize::Medium,
        1400.. => DocumentBoundarySize::Large,
    }
}

fn apply_document_boundary_size(document_boundary: &gtk::Box, window_width: i32) {
    if window_width <= 0 {
        return;
    }

    let size = document_boundary_size(window_width);
    for css_class in DocumentBoundarySize::ALL_CSS_CLASSES {
        document_boundary.remove_css_class(css_class);
    }
    document_boundary.add_css_class(size.css_class());
}

fn connect_document_boundary_size(widgets: &AppWidgets) {
    apply_document_boundary_size(&widgets.document_boundary, widgets.main_window.width());

    widgets.main_window.connect_realize(clone!(
        #[weak(rename_to = document_boundary)]
        widgets.document_boundary,
        move |window| {
            let Some(surface) = window.surface() else {
                return;
            };
            surface.connect_layout(clone!(
                #[weak]
                document_boundary,
                move |_surface, width, _height| {
                    apply_document_boundary_size(&document_boundary, width);
                }
            ));
        }
    ));
    widgets.main_window.connect_map(clone!(
        #[weak(rename_to = document_boundary)]
        widgets.document_boundary,
        move |window| {
            apply_document_boundary_size(&document_boundary, window.width());
        }
    ));
}

fn connect_widget_signals(
    widgets: &AppWidgets,
    sender: &AppSender,
    slider_pointer_active: &Rc<Cell<bool>>,
) -> UiSignalHandlers {
    {
        let sender = sender.clone();
        widgets
            .navigate_up_button
            .connect_clicked(move |_| sender.input(Msg::NavigateUp));
    }
    {
        let sender = sender.clone();
        widgets
            .history_toggle
            .connect_clicked(move |_| sender.input(Msg::ToggleHistoryPanel));
    }
    {
        let sender = sender.clone();
        widgets
            .favorites_toggle
            .connect_clicked(move |_| sender.input(Msg::ToggleFavoritesPanel));
    }
    {
        let sender = sender.clone();
        widgets
            .clear_history_button
            .connect_clicked(move |_| sender.input(Msg::ConfirmClearHistory));
    }
    {
        let sender = sender.clone();
        widgets
            .next_single_button
            .connect_clicked(move |_| sender.input(Msg::NextSinglePage));
    }
    {
        let sender = sender.clone();
        widgets
            .prev_single_button
            .connect_clicked(move |_| sender.input(Msg::PrevSinglePage));
    }
    {
        let sender = sender.clone();
        widgets
            .select_bookshelf_button
            .connect_clicked(move |_| sender.input(Msg::SelectBookshelfRoot));
    }

    let spread_toggle = widgets.spread_toggle.connect_toggled({
        let sender = sender.clone();
        move |_| sender.input(Msg::ToggleViewMode)
    });
    let smart_crop_toggle = widgets.smart_crop_toggle.connect_toggled({
        let sender = sender.clone();
        move |toggle| sender.input(Msg::SetSmartCrop(toggle.is_active()))
    });
    let page_slider = widgets.page_slider.connect_change_value({
        let sender = sender.clone();
        let slider_pointer_active = slider_pointer_active.clone();
        move |_scale, _scroll_type, value| {
            if slider_pointer_active.get() {
                sender.input(Msg::SetPageFromSliderPointer(value));
            } else {
                sender.input(Msg::SetPage(value));
            }
            gtk::glib::Propagation::Stop
        }
    });

    let image_click = gtk::GestureClick::new();
    image_click.set_button(0);
    image_click.connect_pressed({
        let sender = sender.clone();
        move |gesture, _, x, _y| {
            if let Some(widget) = gesture.widget() {
                let width = widget.width();
                let button = gesture.current_button();
                sender.input(Msg::ImageClick { button, x, width });
            }
        }
    });
    widgets.image_container.add_controller(image_click);

    let image_scroll = gtk::EventControllerScroll::new(
        gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
    );
    image_scroll.connect_scroll({
        let sender = sender.clone();
        move |_controller, _dx, dy| {
            sender.input(Msg::Scroll { dy });
            gtk::glib::Propagation::Stop
        }
    });
    widgets.image_container.add_controller(image_scroll);

    let loading_events = gtk::EventControllerLegacy::new();
    loading_events.set_propagation_phase(gtk::PropagationPhase::Capture);
    loading_events.connect_event(move |_, _| gtk::glib::Propagation::Stop);
    widgets.loading_layer.add_controller(loading_events);

    UiSignalHandlers {
        spread_toggle,
        smart_crop_toggle,
        page_slider,
    }
}

fn connect_library_search(widgets: &AppWidgets, sender: &AppSender) {
    widgets.library_search_entry.connect_search_changed({
        let sender = sender.clone();
        move |entry| {
            sender.input(Msg::LibrarySearchQueryChanged(entry.text().to_string()));
        }
    });
    widgets.library_search_entry.connect_stop_search({
        let sender = sender.clone();
        move |_| sender.input(Msg::CloseNavigationPanel)
    });
}

fn connect_library_viewport(widgets: &AppWidgets, sender: &AppSender) {
    widgets.library_scroller.connect_notify_local(
        Some("width"),
        clone!(
            #[strong]
            sender,
            move |_, _| {
                sender.input(Msg::LibraryThumbnailViewportChanged);
            }
        ),
    );
    widgets.library_scroller.connect_notify_local(
        Some("height"),
        clone!(
            #[strong]
            sender,
            move |_, _| {
                sender.input(Msg::LibraryThumbnailViewportChanged);
            }
        ),
    );
    let library_adjustment = widgets.library_scroller.vadjustment();
    library_adjustment.connect_value_changed(clone!(
        #[strong]
        sender,
        move |_| {
            sender.input(Msg::LibraryThumbnailViewportChanged);
        }
    ));
    library_adjustment.connect_changed(clone!(
        #[strong]
        sender,
        move |_| {
            sender.input(Msg::LibraryThumbnailViewportChanged);
        }
    ));
}

fn connect_navigation_split_view(widgets: &AppWidgets, sender: &AppSender) {
    widgets.navigation_split_view.connect_show_sidebar_notify({
        let sender = sender.clone();
        move |split_view| {
            if split_view.shows_sidebar() {
                let split_view = split_view.clone();
                gtk::glib::idle_add_local_once(move || {
                    if split_view.shows_sidebar() {
                        split_view.queue_allocate();
                    }
                });
            } else {
                sender.input(Msg::NavigationDrawerClosed);
            }
        }
    });
}

fn connect_window_pointer(widgets: &AppWidgets, sender: &AppSender) {
    let window_motion = gtk::EventControllerMotion::new();
    window_motion.set_propagation_phase(gtk::PropagationPhase::Capture);
    window_motion.connect_motion({
        let page_slider = widgets.page_slider.clone();
        let page_slider_bar = widgets.page_slider_bar.clone();
        let header_bar = widgets.header_bar.clone();
        let sender = sender.clone();
        move |controller, x, y| {
            let Some(window) = controller.widget() else {
                return;
            };
            let pointer_over_bar = window
                .compute_point(
                    &page_slider_bar,
                    &gtk::graphene::Point::new(x as f32, y as f32),
                )
                .is_some_and(|point| {
                    let x = f64::from(point.x());
                    let y = f64::from(point.y());
                    x >= 0.0
                        && x < f64::from(page_slider_bar.width())
                        && y >= 0.0
                        && y < f64::from(page_slider_bar.height())
                });
            sender.input(Msg::UpdateSliderVisibility {
                distance_from_bottom: (f64::from(window.height()) - y).max(0.0),
                pointer_over_bar,
            });
            let pointer_over_header = window
                .compute_point(&header_bar, &gtk::graphene::Point::new(x as f32, y as f32))
                .is_some_and(|point| {
                    let x = f64::from(point.x());
                    let y = f64::from(point.y());
                    x >= 0.0
                        && x < f64::from(header_bar.width())
                        && y >= 0.0
                        && y < f64::from(header_bar.height())
                });
            sender.input(Msg::UpdateHeaderVisibility {
                distance_from_top: y.max(0.0),
                pointer_over_header,
            });
            let Some(point) =
                window.compute_point(&page_slider, &gtk::graphene::Point::new(x as f32, y as f32))
            else {
                sender.input(Msg::HidePreview);
                return;
            };
            let x = f64::from(point.x());
            let y = f64::from(point.y());
            let width = page_slider.width();
            let height = page_slider.height();
            if !pointer_is_inside_slider(x, y, width, height) {
                sender.input(Msg::HidePreview);
                return;
            }
            let position = (1.0 - x / f64::from(width)).clamp(0.0, 1.0);
            sender.input(Msg::HoverPreview {
                value: position * page_slider.adjustment().upper(),
                pointer_x: x.round() as i32,
                slider_width: width,
                slider_height: height,
            });
        }
    });
    window_motion.connect_leave({
        let sender = sender.clone();
        move |_| {
            sender.input(Msg::HidePreview);
            sender.input(Msg::UpdateSliderVisibility {
                distance_from_bottom: f64::INFINITY,
                pointer_over_bar: false,
            });
            sender.input(Msg::UpdateHeaderVisibility {
                distance_from_top: f64::INFINITY,
                pointer_over_header: false,
            });
        }
    });
    widgets.main_window.add_controller(window_motion);
}

fn connect_drop_target(widgets: &AppWidgets, sender: &AppSender) {
    let file_drop_target = gtk::DropTarget::new(
        gtk::gdk::FileList::static_type(),
        gtk::gdk::DragAction::COPY,
    );
    file_drop_target.connect_drop({
        let sender = sender.clone();
        move |_, value, _, _| {
            let Ok(file_list) = value.get::<gtk::gdk::FileList>() else {
                return false;
            };
            let files = file_list.files();
            let [file] = files.as_slice() else {
                return false;
            };
            let Some(path) = file.path() else {
                return false;
            };
            sender.input(Msg::PathDropped(path));
            true
        }
    });
    widgets.main_window.add_controller(file_drop_target);
}

fn connect_slider_pointer(
    widgets: &AppWidgets,
    sender: &AppSender,
    slider_pointer_active: Rc<Cell<bool>>,
) -> gtk::EventControllerMotion {
    let popover_motion = gtk::EventControllerMotion::new();
    popover_motion.set_propagation_phase(gtk::PropagationPhase::Capture);
    let hide_if_outside_slider = Rc::new({
        let page_slider = widgets.page_slider.clone();
        let sender = sender.clone();
        move |controller: &gtk::EventControllerMotion, x: f64, y: f64| {
            let Some(popover) = controller.widget() else {
                return;
            };
            let Some(point) =
                popover.compute_point(&page_slider, &gtk::graphene::Point::new(x as f32, y as f32))
            else {
                sender.input(Msg::HidePreview);
                return;
            };
            if !pointer_is_inside_slider(
                f64::from(point.x()),
                f64::from(point.y()),
                page_slider.width(),
                page_slider.height(),
            ) {
                sender.input(Msg::HidePreview);
            }
        }
    });
    popover_motion.connect_enter({
        let hide_if_outside_slider = hide_if_outside_slider.clone();
        move |controller, x, y| hide_if_outside_slider(controller, x, y)
    });
    popover_motion.connect_motion(move |controller, x, y| hide_if_outside_slider(controller, x, y));

    let slider_events = gtk::EventControllerLegacy::new();
    slider_events.set_propagation_phase(gtk::PropagationPhase::Capture);
    slider_events.connect_event({
        let sender = sender.clone();
        let slider_pointer_active = slider_pointer_active.clone();
        move |_, event| {
            if let Some(button_event) = event.downcast_ref::<gtk::gdk::ButtonEvent>()
                && button_event.button() == gtk::gdk::BUTTON_PRIMARY
            {
                match event.event_type() {
                    gtk::gdk::EventType::ButtonPress => {
                        slider_pointer_active.set(true);
                        sender.input(Msg::SetSliderDragging(true));
                    }
                    gtk::gdk::EventType::ButtonRelease => {
                        slider_pointer_active.set(false);
                        sender.input(Msg::SetSliderDragging(false));
                    }
                    _ => {}
                }
            }
            gtk::glib::Propagation::Proceed
        }
    });
    widgets.page_slider.add_controller(slider_events);

    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_primary_button_warps_slider(true);
    }

    popover_motion
}

fn connect_window_state(widgets: &AppWidgets, sender: &AppSender) {
    widgets.main_window.connect_fullscreened_notify({
        let sender = sender.clone();
        move |window| sender.input(Msg::FullscreenChanged(window.is_fullscreen()))
    });

    for property in ["default-width", "default-height", "maximized"] {
        widgets.main_window.connect_notify_local(
            Some(property),
            clone!(
                #[strong]
                sender,
                move |window, _| {
                    let window = window.clone();
                    let sender = sender.clone();
                    gtk::glib::idle_add_local_once(move || {
                        let (width, height) = window.default_size();
                        sender.input(Msg::WindowStateChanged {
                            width,
                            height,
                            maximized: window.is_maximized(),
                            fullscreened: window.is_fullscreen(),
                        });
                    });
                }
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{DocumentBoundarySize, document_boundary_size};

    #[test]
    fn document_boundary_size_uses_exact_width_thresholds() {
        assert_eq!(document_boundary_size(899), DocumentBoundarySize::Small);
        assert_eq!(document_boundary_size(900), DocumentBoundarySize::Medium);
        assert_eq!(document_boundary_size(1399), DocumentBoundarySize::Medium);
        assert_eq!(document_boundary_size(1400), DocumentBoundarySize::Large);
    }
}
