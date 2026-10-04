use adw::prelude::*;
use gtk::subclass::prelude::ObjectSubclassIsExt;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

const BOOKSHELF_TOP_PLANE_REAR_Y: f64 = -10.0;
const BOOKSHELF_TOP_PLANE_FRONT_Y: f64 = 0.0;
const BOOKSHELF_CONTACT_DEPTH_FROM_FRONT_RATIO: f64 = 0.25;
const BOOKSHELF_REAR_INSET: f64 = 52.0;
const BOOKSHELF_FRONT_INSET: f64 = 4.0;
const BOOKSHELF_REAR_SHADOW_SIDE_EXPANSION: f64 = 1.75;
const BOOKSHELF_FRONT_SHADOW_LENGTH: f64 = 0.75;
const BOOKSHELF_FRONT_SHADOW_SIDE_EXPANSION: f64 = 0.75;
const BOOKSHELF_CONTENT_SIDE_MARGIN: i32 = 12;
const BOOKSHELF_BOTTOM_SHADOW_Y: f64 = 7.0;
const BOOKSHELF_BOTTOM_SHADOW_HEIGHT: f64 = 1.5;
const BOOKSHELF_BOTTOM_SHADOW_END_Y: f64 =
    BOOKSHELF_BOTTOM_SHADOW_Y + BOOKSHELF_BOTTOM_SHADOW_HEIGHT;
const BOOKSHELF_TITLE_GAP: f64 = 2.0;
pub(super) const SHELF_CONTACT_TARGET_CSS_CLASS: &str = "shelf-contact-target";
const FOLDER_BOOK_BUNDLE_CSS_CLASS: &str = "folder-book-bundle";
const FOLDER_BOOK_FRONT_CSS_CLASS: &str = "folder-book-front";
const FOLDER_BOOK_SPINE_CSS_CLASS: &str = "folder-book-spine";

#[derive(Clone)]
enum ShelfShadowTarget {
    Individual {
        contact_target: gtk::Widget,
        visual_target: gtk::Widget,
    },
    SpineGroup {
        contact_target: gtk::Widget,
        spine_targets: Vec<gtk::Widget>,
    },
}

struct ShelfShadowGeometry {
    left: f64,
    right: f64,
    book_height: f64,
}

struct ShelfShadowProjection {
    contact_left: f64,
    contact_right: f64,
    rear_left: f64,
    rear_right: f64,
    rear_depth: f64,
    front_contact_left: f64,
    front_contact_right: f64,
    front_left: f64,
    front_right: f64,
    front_depth: f64,
}

fn bookshelf_contact_y_from_shelf_y(shelf_y: f64) -> f64 {
    shelf_y
        + BOOKSHELF_TOP_PLANE_FRONT_Y
        + (BOOKSHELF_TOP_PLANE_REAR_Y - BOOKSHELF_TOP_PLANE_FRONT_Y)
            * BOOKSHELF_CONTACT_DEPTH_FROM_FRONT_RATIO
}

fn bookshelf_shelf_y_from_contact_y(contact_y: f64) -> f64 {
    contact_y - bookshelf_contact_y_from_shelf_y(0.0)
}

pub(super) fn bookshelf_title_spacing() -> i32 {
    (bookshelf_shelf_y_from_contact_y(0.0) + BOOKSHELF_BOTTOM_SHADOW_END_Y + BOOKSHELF_TITLE_GAP)
        .ceil() as i32
}

fn bookshelf_content_side_inset() -> i32 {
    (BOOKSHELF_FRONT_INSET
        + (BOOKSHELF_REAR_INSET - BOOKSHELF_FRONT_INSET) * BOOKSHELF_CONTACT_DEPTH_FROM_FRONT_RATIO)
        .round() as i32
        + BOOKSHELF_CONTENT_SIDE_MARGIN
}

fn shelf_top_edges_at_depth(width: f64, depth: f64) -> (f64, f64) {
    let left = BOOKSHELF_REAR_INSET + (BOOKSHELF_FRONT_INSET - BOOKSHELF_REAR_INSET) * depth;
    let right = width - left;
    (left, right)
}

fn project_shelf_x_between_depths(width: f64, x: f64, from_depth: f64, to_depth: f64) -> f64 {
    let (from_left, from_right) = shelf_top_edges_at_depth(width, from_depth);
    let x_ratio = (x - from_left) / (from_right - from_left);
    let (to_left, to_right) = shelf_top_edges_at_depth(width, to_depth);
    to_left + (to_right - to_left) * x_ratio
}

fn shelf_shadow_length(book_height: f64) -> f64 {
    (book_height * 0.025).clamp(2.0, 5.0)
}

fn expanded_projected_shelf_span(
    width: f64,
    left: f64,
    right: f64,
    from_depth: f64,
    to_depth: f64,
    expansion: f64,
) -> (f64, f64) {
    let projected_left = project_shelf_x_between_depths(width, left, from_depth, to_depth);
    let projected_right = project_shelf_x_between_depths(width, right, from_depth, to_depth);
    let (shelf_left, shelf_right) = shelf_top_edges_at_depth(width, to_depth);
    (
        (projected_left - expansion).max(shelf_left),
        (projected_right + expansion).min(shelf_right),
    )
}

fn shelf_shadow_projection(
    width: f64,
    left: f64,
    right: f64,
    book_height: f64,
) -> ShelfShadowProjection {
    let contact_depth = 1.0 - BOOKSHELF_CONTACT_DEPTH_FROM_FRONT_RATIO;
    let plane_pixel_depth = BOOKSHELF_TOP_PLANE_FRONT_Y - BOOKSHELF_TOP_PLANE_REAR_Y;
    let rear_depth =
        (contact_depth - shelf_shadow_length(book_height) / plane_pixel_depth).max(0.0);
    let front_depth = (contact_depth + BOOKSHELF_FRONT_SHADOW_LENGTH / plane_pixel_depth).min(1.0);
    let (rear_left, rear_right) = expanded_projected_shelf_span(
        width,
        left,
        right,
        contact_depth,
        rear_depth,
        BOOKSHELF_REAR_SHADOW_SIDE_EXPANSION,
    );
    let (front_contact_left, front_contact_right) = expanded_projected_shelf_span(
        width,
        left,
        right,
        contact_depth,
        contact_depth,
        BOOKSHELF_FRONT_SHADOW_SIDE_EXPANSION,
    );
    let (front_left, front_right) = expanded_projected_shelf_span(
        width,
        left,
        right,
        contact_depth,
        front_depth,
        BOOKSHELF_FRONT_SHADOW_SIDE_EXPANSION,
    );

    ShelfShadowProjection {
        contact_left: left,
        contact_right: right,
        rear_left,
        rear_right,
        rear_depth,
        front_contact_left,
        front_contact_right,
        front_left,
        front_right,
        front_depth,
    }
}

fn widgets_with_css_class(root: &gtk::Widget, css_class: &str) -> Vec<gtk::Widget> {
    let mut targets = Vec::new();
    let mut pending = vec![root.clone()];
    while let Some(widget) = pending.pop() {
        if widget.has_css_class(css_class) {
            targets.push(widget.clone());
        }
        let mut child = widget.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            pending.push(widget);
        }
    }
    targets
}

fn shelf_contact_targets(root: &gtk::Widget) -> Vec<gtk::Widget> {
    widgets_with_css_class(root, SHELF_CONTACT_TARGET_CSS_CLASS)
}

fn shelf_shadow_targets(contact_targets: &[gtk::Widget]) -> Vec<ShelfShadowTarget> {
    let mut shadow_targets = Vec::new();
    for contact_target in contact_targets {
        if contact_target.has_css_class(FOLDER_BOOK_BUNDLE_CSS_CLASS) {
            for visual_target in widgets_with_css_class(contact_target, FOLDER_BOOK_FRONT_CSS_CLASS)
            {
                shadow_targets.push(ShelfShadowTarget::Individual {
                    contact_target: contact_target.clone(),
                    visual_target,
                });
            }
            let spine_targets = widgets_with_css_class(contact_target, FOLDER_BOOK_SPINE_CSS_CLASS);
            if !spine_targets.is_empty() {
                shadow_targets.push(ShelfShadowTarget::SpineGroup {
                    contact_target: contact_target.clone(),
                    spine_targets,
                });
            }
        } else {
            shadow_targets.push(ShelfShadowTarget::Individual {
                contact_target: contact_target.clone(),
                visual_target: contact_target.clone(),
            });
        }
    }
    shadow_targets
}

fn shelf_shadow_geometries(
    targets: &[ShelfShadowTarget],
    container: &impl IsA<gtk::Widget>,
) -> BTreeMap<i32, Vec<ShelfShadowGeometry>> {
    let mut geometries = BTreeMap::<i32, Vec<ShelfShadowGeometry>>::new();
    for target in targets {
        let (contact_target, left, right, book_height) = match target {
            ShelfShadowTarget::Individual {
                contact_target,
                visual_target,
            } => {
                let Some(bounds) = visual_target.compute_bounds(container) else {
                    continue;
                };
                (
                    contact_target,
                    f64::from(bounds.x()),
                    f64::from(bounds.x() + bounds.width()),
                    f64::from(bounds.height()),
                )
            }
            ShelfShadowTarget::SpineGroup {
                contact_target,
                spine_targets,
            } => {
                let Some(contact_bounds) = contact_target.compute_bounds(container) else {
                    continue;
                };
                let mut left = f64::INFINITY;
                let mut right = f64::NEG_INFINITY;
                for spine_target in spine_targets {
                    let Some(bounds) = spine_target.compute_bounds(container) else {
                        continue;
                    };
                    left = left.min(f64::from(bounds.x()));
                    right = right.max(f64::from(bounds.x() + bounds.width()));
                }
                (
                    contact_target,
                    left,
                    right,
                    f64::from(contact_bounds.height()),
                )
            }
        };
        if !left.is_finite() || !right.is_finite() || right <= left || book_height <= 0.0 {
            continue;
        }
        let Some(contact_bounds) = contact_target.compute_bounds(container) else {
            continue;
        };
        let contact_bottom = (contact_bounds.y() + contact_bounds.height()).round() as i32;
        geometries
            .entry(contact_bottom)
            .or_default()
            .push(ShelfShadowGeometry {
                left,
                right,
                book_height,
            });
    }
    geometries
}

mod imp {
    use super::*;
    use gtk::subclass::prelude::*;

    #[derive(Default)]
    pub struct ShelfContainer {
        pub content: RefCell<Option<gtk::Widget>>,
        pub contact_targets: RefCell<Vec<gtk::Widget>>,
        pub(super) shadow_targets: RefCell<Vec<ShelfShadowTarget>>,
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for ShelfContainer {
        const NAME: &'static str = "AgnamShelfContainer";
        type Type = super::ShelfContainer;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for ShelfContainer {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().set_layout_manager(Some(gtk::BinLayout::new()));
        }

        fn dispose(&self) {
            self.contact_targets.borrow_mut().clear();
            self.shadow_targets.borrow_mut().clear();
            if let Some(content) = self.content.take() {
                content.unparent();
            }
        }
    }

    impl WidgetImpl for ShelfContainer {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(content) = self.content.borrow().as_ref().cloned() else {
                return;
            };
            let container = self.obj();
            let contact_bottoms = self
                .contact_targets
                .borrow()
                .iter()
                .filter_map(|target| target.compute_bounds(container.as_ref()))
                .map(|bounds| (bounds.y() + bounds.height()).round() as i32)
                .collect::<BTreeSet<_>>();
            let shadow_geometries =
                shelf_shadow_geometries(&self.shadow_targets.borrow(), container.as_ref());

            let width = container.width();
            let height = container.height();
            if width > 0 && height > 0 {
                let bounds = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
                let context = snapshot.append_cairo(&bounds);
                draw_shelves(
                    &context,
                    width,
                    contact_bottoms.iter().copied(),
                    &shadow_geometries,
                );
            }

            container.snapshot_child(&content, snapshot);
        }
    }
}

gtk::glib::wrapper! {
    pub struct ShelfContainer(ObjectSubclass<imp::ShelfContainer>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ShelfContainer {
    pub(super) fn new(content: &(impl IsA<gtk::Widget> + Clone)) -> Self {
        let container: Self = gtk::glib::Object::new();
        let content = content.clone().upcast::<gtk::Widget>();
        let contact_targets = shelf_contact_targets(&content);
        let shadow_targets = shelf_shadow_targets(&contact_targets);
        let content_side_inset = bookshelf_content_side_inset();
        content.set_margin_start(content_side_inset);
        content.set_margin_end(content_side_inset);
        content.set_parent(&container);
        container.imp().content.replace(Some(content));
        container.imp().contact_targets.replace(contact_targets);
        container.imp().shadow_targets.replace(shadow_targets);
        container
    }

    pub(super) fn refresh_targets(&self) {
        let content = self.imp().content.borrow().as_ref().cloned();
        let contact_targets = content
            .as_ref()
            .map(shelf_contact_targets)
            .unwrap_or_default();
        let shadow_targets = shelf_shadow_targets(&contact_targets);
        self.imp().contact_targets.replace(contact_targets);
        self.imp().shadow_targets.replace(shadow_targets);
        self.queue_draw();
    }
}

fn draw_shelves(
    context: &gtk::cairo::Context,
    width: i32,
    contact_bottoms: impl IntoIterator<Item = i32>,
    shadows_by_contact: &BTreeMap<i32, Vec<ShelfShadowGeometry>>,
) {
    let width = f64::from(width);
    let back_inset = BOOKSHELF_REAR_INSET;
    let front_inset = BOOKSHELF_FRONT_INSET;
    if width <= back_inset * 2.0 {
        return;
    }

    for contact_bottom in contact_bottoms {
        let y = bookshelf_shelf_y_from_contact_y(f64::from(contact_bottom));
        let grain_phase = f64::from(contact_bottom) * 0.071;

        draw_shelf_top(context, width, y, grain_phase);

        // Keep a very light rear highlight so the shelf remains legible
        // without competing with colorful cover artwork.
        context.set_source_rgba(0.98, 0.91, 0.81, 0.45);
        context.rectangle(
            back_inset,
            y + BOOKSHELF_TOP_PLANE_REAR_Y,
            width - back_inset * 2.0,
            1.0,
        );
        let _ = context.fill();

        if let Some(shadows) = shadows_by_contact.get(&contact_bottom) {
            draw_shelf_book_shadows(context, width, y, shadows);
        }

        draw_shelf_front(context, width, y, grain_phase);

        // The thin front edge and faint page shadow keep the shelf grounded
        // while letting it visually recede behind the covers.
        context.set_source_rgba(0.52, 0.38, 0.24, 0.20);
        context.rectangle(front_inset, y + 6.0, width - front_inset * 2.0, 1.0);
        let _ = context.fill();
        context.set_source_rgba(0.0, 0.0, 0.0, 0.06);
        context.rectangle(
            8.0,
            y + BOOKSHELF_BOTTOM_SHADOW_Y,
            (width - 16.0).max(0.0),
            BOOKSHELF_BOTTOM_SHADOW_HEIGHT,
        );
        let _ = context.fill();
    }
}

fn shelf_top_y_at_depth(y: f64, depth: f64) -> f64 {
    y + BOOKSHELF_TOP_PLANE_REAR_Y
        + (BOOKSHELF_TOP_PLANE_FRONT_Y - BOOKSHELF_TOP_PLANE_REAR_Y) * depth
}

fn append_shelf_shadow_quad(
    context: &gtk::cairo::Context,
    y: f64,
    contact_left: f64,
    contact_right: f64,
    contact_depth: f64,
    outer_left: f64,
    outer_right: f64,
    outer_depth: f64,
) {
    let contact_y = shelf_top_y_at_depth(y, contact_depth);
    let outer_y = shelf_top_y_at_depth(y, outer_depth);
    context.move_to(contact_left, contact_y);
    context.line_to(contact_right, contact_y);
    context.line_to(outer_right, outer_y);
    context.line_to(outer_left, outer_y);
    context.close_path();
}

fn draw_shelf_book_shadows(
    context: &gtk::cairo::Context,
    width: f64,
    y: f64,
    shadows: &[ShelfShadowGeometry],
) {
    let contact_depth = 1.0 - BOOKSHELF_CONTACT_DEPTH_FROM_FRONT_RATIO;
    let contact_y = shelf_top_y_at_depth(y, contact_depth);

    let _ = context.save();
    append_shelf_top_path(context, width, y);
    context.clip();

    for shadow in shadows {
        let projection =
            shelf_shadow_projection(width, shadow.left, shadow.right, shadow.book_height);
        let rear_y = shelf_top_y_at_depth(y, projection.rear_depth);
        let gradient = gtk::cairo::LinearGradient::new(0.0, contact_y, 0.0, rear_y);
        gradient.add_color_stop_rgba(0.0, 0.12, 0.075, 0.04, 0.07);
        gradient.add_color_stop_rgba(1.0, 0.12, 0.075, 0.04, 0.0);
        append_shelf_shadow_quad(
            context,
            y,
            projection.contact_left,
            projection.contact_right,
            contact_depth,
            projection.rear_left,
            projection.rear_right,
            projection.rear_depth,
        );
        let _ = context.set_source(&gradient);
        let _ = context.fill();

        append_shelf_shadow_quad(
            context,
            y,
            projection.front_contact_left,
            projection.front_contact_right,
            contact_depth,
            projection.front_left,
            projection.front_right,
            projection.front_depth,
        );
        context.set_source_rgba(0.10, 0.06, 0.03, 0.09);
        let _ = context.fill();
    }
    let _ = context.restore();
}

fn append_shelf_top_path(context: &gtk::cairo::Context, width: f64, y: f64) {
    context.move_to(BOOKSHELF_REAR_INSET, y + BOOKSHELF_TOP_PLANE_REAR_Y);
    context.line_to(width - BOOKSHELF_REAR_INSET, y + BOOKSHELF_TOP_PLANE_REAR_Y);
    context.line_to(
        width - BOOKSHELF_FRONT_INSET,
        y + BOOKSHELF_TOP_PLANE_FRONT_Y,
    );
    context.line_to(BOOKSHELF_FRONT_INSET, y + BOOKSHELF_TOP_PLANE_FRONT_Y);
    context.close_path();
}

fn shelf_top_grain_point(width: f64, y: f64, x_ratio: f64, depth: f64) -> (f64, f64) {
    let depth = depth.clamp(0.0, 1.0);
    let (left, right) = shelf_top_edges_at_depth(width, depth);
    let x = left + (right - left) * x_ratio;
    let y = y
        + BOOKSHELF_TOP_PLANE_REAR_Y
        + (BOOKSHELF_TOP_PLANE_FRONT_Y - BOOKSHELF_TOP_PLANE_REAR_Y) * depth;
    (x, y)
}

fn append_shelf_top_grain_path(
    context: &gtk::cairo::Context,
    width: f64,
    y: f64,
    depth: f64,
    phase: f64,
) {
    let plane_depth = BOOKSHELF_TOP_PLANE_FRONT_Y - BOOKSHELF_TOP_PLANE_REAR_Y;
    let amplitude = (0.08 + depth * 0.22) / plane_depth;
    let grain_depth = |x_ratio: f64| depth + (phase + x_ratio * 7.4).sin() * amplitude;
    let (start_x, start_y) = shelf_top_grain_point(width, y, 0.0, grain_depth(0.0));
    let (control_1_x, control_1_y) = shelf_top_grain_point(width, y, 0.17, grain_depth(0.17));
    let (control_2_x, control_2_y) = shelf_top_grain_point(width, y, 0.34, grain_depth(0.34));
    let (middle_x, middle_y) = shelf_top_grain_point(width, y, 0.5, grain_depth(0.5));
    let (control_3_x, control_3_y) = shelf_top_grain_point(width, y, 0.66, grain_depth(0.66));
    let (control_4_x, control_4_y) = shelf_top_grain_point(width, y, 0.83, grain_depth(0.83));
    let (end_x, end_y) = shelf_top_grain_point(width, y, 1.0, grain_depth(1.0));

    context.move_to(start_x, start_y);
    context.curve_to(
        control_1_x,
        control_1_y,
        control_2_x,
        control_2_y,
        middle_x,
        middle_y,
    );
    context.curve_to(
        control_3_x,
        control_3_y,
        control_4_x,
        control_4_y,
        end_x,
        end_y,
    );
}

fn append_shelf_top_contour_path(
    context: &gtk::cairo::Context,
    width: f64,
    y: f64,
    depth: f64,
    center: f64,
    half_width: f64,
    depth_rise: f64,
    phase: f64,
    core_only: bool,
) {
    let plane_depth = BOOKSHELF_TOP_PLANE_FRONT_Y - BOOKSHELF_TOP_PLANE_REAR_Y;
    let tail_amplitude = (0.05 + depth * 0.07) / plane_depth;
    let tail_depth = |x_ratio: f64| depth + (phase + x_ratio * 5.6).sin() * tail_amplitude;
    let entry = (center - half_width).clamp(0.06, 0.82);
    let exit = (center + half_width).clamp(0.18, 0.94);
    let contour_width = exit - entry;
    let entry_depth = tail_depth(entry);
    let exit_depth = tail_depth(exit);

    if core_only {
        let (entry_x, entry_y) = shelf_top_grain_point(width, y, entry, entry_depth);
        context.move_to(entry_x, entry_y);
    } else {
        let (start_x, start_y) = shelf_top_grain_point(width, y, 0.0, tail_depth(0.0));
        let (control_1_x, control_1_y) =
            shelf_top_grain_point(width, y, entry * 0.35, tail_depth(entry * 0.35));
        let (control_2_x, control_2_y) =
            shelf_top_grain_point(width, y, entry * 0.75, tail_depth(entry * 0.75));
        let (entry_x, entry_y) = shelf_top_grain_point(width, y, entry, entry_depth);
        context.move_to(start_x, start_y);
        context.curve_to(
            control_1_x,
            control_1_y,
            control_2_x,
            control_2_y,
            entry_x,
            entry_y,
        );
    }

    let peak_x_ratio = (center + phase.sin() * contour_width * 0.025).clamp(entry, exit);
    let (left_control_1_x, left_control_1_y) = shelf_top_grain_point(
        width,
        y,
        entry + contour_width * 0.18,
        entry_depth + depth_rise * 0.24,
    );
    let (left_control_2_x, left_control_2_y) = shelf_top_grain_point(
        width,
        y,
        peak_x_ratio - contour_width * 0.15,
        depth + depth_rise * 0.90,
    );
    let (peak_x, peak_y) = shelf_top_grain_point(width, y, peak_x_ratio, depth + depth_rise);
    context.curve_to(
        left_control_1_x,
        left_control_1_y,
        left_control_2_x,
        left_control_2_y,
        peak_x,
        peak_y,
    );

    let (right_control_1_x, right_control_1_y) = shelf_top_grain_point(
        width,
        y,
        peak_x_ratio + contour_width * 0.12,
        depth + depth_rise * 0.92,
    );
    let (right_control_2_x, right_control_2_y) = shelf_top_grain_point(
        width,
        y,
        exit - contour_width * 0.22,
        exit_depth + depth_rise * 0.20,
    );
    let (exit_x, exit_y) = shelf_top_grain_point(width, y, exit, exit_depth);
    context.curve_to(
        right_control_1_x,
        right_control_1_y,
        right_control_2_x,
        right_control_2_y,
        exit_x,
        exit_y,
    );

    if !core_only {
        let remaining_width = 1.0 - exit;
        let (control_3_x, control_3_y) = shelf_top_grain_point(
            width,
            y,
            exit + remaining_width * 0.25,
            tail_depth(exit + remaining_width * 0.25),
        );
        let (control_4_x, control_4_y) = shelf_top_grain_point(
            width,
            y,
            exit + remaining_width * 0.65,
            tail_depth(exit + remaining_width * 0.65),
        );
        let (end_x, end_y) = shelf_top_grain_point(width, y, 1.0, tail_depth(1.0));
        context.curve_to(
            control_3_x,
            control_3_y,
            control_4_x,
            control_4_y,
            end_x,
            end_y,
        );
    }
}

fn draw_shelf_top(context: &gtk::cairo::Context, width: f64, y: f64, phase: f64) {
    let gradient = gtk::cairo::LinearGradient::new(
        0.0,
        y + BOOKSHELF_TOP_PLANE_REAR_Y,
        0.0,
        y + BOOKSHELF_TOP_PLANE_FRONT_Y,
    );
    gradient.add_color_stop_rgba(0.0, 0.88, 0.78, 0.64, 1.0);
    gradient.add_color_stop_rgba(1.0, 0.94, 0.86, 0.72, 1.0);
    append_shelf_top_path(context, width, y);
    let _ = context.set_source(&gradient);
    let _ = context.fill();

    let _ = context.save();
    append_shelf_top_path(context, width, y);
    context.clip();
    context.set_line_cap(gtk::cairo::LineCap::Round);

    // Broad contours remain only as a hint of pale wood rather than a
    // decorative feature competing with the manga covers.
    let contour_presets = [(0.23, 0.37, 0.22), (0.47, 0.61, 0.27), (0.69, 0.43, 0.19)];
    for (contour_index, (base_depth, base_center, base_half_width)) in
        contour_presets.into_iter().enumerate()
    {
        let contour_phase = phase + contour_index as f64 * 2.17;
        let depth = (base_depth + contour_phase.sin() * 0.025).clamp(0.12, 0.78);
        let center = (base_center + (phase * 0.83 + contour_index as f64 * 1.71).cos() * 0.055)
            .clamp(0.20, 0.80);
        let half_width = (base_half_width
            + (phase * 0.61 + contour_index as f64 * 2.43).sin() * 0.025)
            .clamp(0.14, 0.32);
        let depth_rise = (0.42 + depth * 0.72 + (phase * 0.47 + contour_index as f64).cos() * 0.08)
            / (BOOKSHELF_TOP_PLANE_FRONT_Y - BOOKSHELF_TOP_PLANE_REAR_Y);

        append_shelf_top_contour_path(
            context,
            width,
            y,
            depth,
            center,
            half_width,
            depth_rise,
            contour_phase,
            false,
        );
        if contour_index == 1 {
            context.set_source_rgba(1.0, 0.94, 0.84, 0.025);
        } else {
            context.set_source_rgba(0.43, 0.30, 0.19, 0.02);
        }
        context.set_line_width(1.25 + depth * 0.45);
        let _ = context.stroke();

        append_shelf_top_contour_path(
            context,
            width,
            y,
            depth,
            center,
            half_width,
            depth_rise,
            contour_phase,
            true,
        );
        if contour_index == 1 {
            context.set_source_rgba(1.0, 0.95, 0.86, 0.045);
        } else {
            context.set_source_rgba(0.45, 0.31, 0.19, 0.04);
        }
        context.set_line_width(0.45 + depth * 0.10);
        let _ = context.stroke();
    }

    // Fine grain is intentionally barely visible. It provides enough material
    // cue to read as light wood without becoming a second visual layer.
    for grain_index in 0..5 {
        let linear_depth = f64::from(grain_index + 1) / 6.0;
        let depth = linear_depth.powf(1.35);
        let grain_phase = phase + f64::from(grain_index) * 1.37;

        append_shelf_top_grain_path(context, width, y, depth, grain_phase);
        if grain_index % 2 == 0 {
            context.set_source_rgba(0.46, 0.33, 0.21, 0.035);
        } else {
            context.set_source_rgba(1.0, 0.95, 0.86, 0.035);
        }
        context.set_line_width(0.45);
        let _ = context.stroke();
    }
    let _ = context.restore();
}

fn append_shelf_front_grain_path(
    context: &gtk::cairo::Context,
    width: f64,
    y: f64,
    offset: f64,
    amplitude: f64,
    phase: f64,
) {
    let left = BOOKSHELF_FRONT_INSET;
    let grain_width = width - BOOKSHELF_FRONT_INSET * 2.0;
    let grain_y = |x_ratio: f64| {
        y + BOOKSHELF_TOP_PLANE_FRONT_Y + offset + (phase + x_ratio * 8.2).sin() * amplitude
    };

    context.move_to(left, grain_y(0.0));
    context.curve_to(
        left + grain_width * 0.17,
        grain_y(0.17),
        left + grain_width * 0.34,
        grain_y(0.34),
        left + grain_width * 0.5,
        grain_y(0.5),
    );
    context.curve_to(
        left + grain_width * 0.66,
        grain_y(0.66),
        left + grain_width * 0.83,
        grain_y(0.83),
        width - BOOKSHELF_FRONT_INSET,
        grain_y(1.0),
    );
}

fn draw_shelf_front(context: &gtk::cairo::Context, width: f64, y: f64, phase: f64) {
    let front_y = y + BOOKSHELF_TOP_PLANE_FRONT_Y;
    let gradient = gtk::cairo::LinearGradient::new(0.0, front_y, 0.0, front_y + 6.0);
    gradient.add_color_stop_rgba(0.0, 0.84, 0.74, 0.59, 1.0);
    gradient.add_color_stop_rgba(1.0, 0.79, 0.68, 0.52, 1.0);
    context.rectangle(
        BOOKSHELF_FRONT_INSET,
        front_y,
        width - BOOKSHELF_FRONT_INSET * 2.0,
        6.0,
    );
    let _ = context.set_source(&gradient);
    let _ = context.fill();

    let _ = context.save();
    context.rectangle(
        BOOKSHELF_FRONT_INSET,
        front_y,
        width - BOOKSHELF_FRONT_INSET * 2.0,
        6.0,
    );
    context.clip();
    context.set_line_cap(gtk::cairo::LineCap::Round);

    let grains = [
        (1.1, 0.10, 0.0, 0.45, (0.42, 0.29, 0.18, 0.05)),
        (2.7, 0.13, 2.1, 0.45, (1.0, 0.94, 0.84, 0.055)),
        (4.1, 0.09, 4.5, 0.40, (0.42, 0.29, 0.18, 0.04)),
    ];
    for (offset, amplitude, phase_offset, line_width, color) in grains {
        append_shelf_front_grain_path(context, width, y, offset, amplitude, phase + phase_offset);
        context.set_source_rgba(color.0, color.1, color.2, color.3);
        context.set_line_width(line_width);
        let _ = context.stroke();
    }
    let _ = context.restore();
}

#[cfg(test)]
mod tests {
    use super::{
        BOOKSHELF_CONTACT_DEPTH_FROM_FRONT_RATIO, BOOKSHELF_TOP_PLANE_FRONT_Y,
        BOOKSHELF_TOP_PLANE_REAR_Y, bookshelf_contact_y_from_shelf_y, bookshelf_content_side_inset,
        bookshelf_shelf_y_from_contact_y, bookshelf_title_spacing, project_shelf_x_between_depths,
        shelf_shadow_length, shelf_shadow_projection, shelf_top_edges_at_depth,
        shelf_top_y_at_depth,
    };

    #[test]
    fn shelf_contact_is_one_quarter_of_the_depth_from_the_front() {
        let shelf_y = bookshelf_shelf_y_from_contact_y(100.0);
        assert_eq!(shelf_y, 102.5);
        assert_eq!(bookshelf_contact_y_from_shelf_y(shelf_y), 100.0);
        assert_eq!(
            shelf_y + BOOKSHELF_TOP_PLANE_FRONT_Y - 100.0,
            (BOOKSHELF_TOP_PLANE_FRONT_Y - BOOKSHELF_TOP_PLANE_REAR_Y)
                * BOOKSHELF_CONTACT_DEPTH_FROM_FRONT_RATIO
        );
    }

    #[test]
    fn content_inset_uses_the_same_depth_ratio_with_safety_margin() {
        assert_eq!(bookshelf_content_side_inset(), 28);
    }

    #[test]
    fn title_starts_below_the_shelf_shadow_with_a_natural_gap() {
        assert_eq!(bookshelf_title_spacing(), 13);
    }

    #[test]
    fn shelf_top_edges_follow_the_trapezoid_depth() {
        assert_eq!(shelf_top_edges_at_depth(400.0, 0.0), (52.0, 348.0));
        assert_eq!(shelf_top_edges_at_depth(400.0, 0.5), (28.0, 372.0));
        assert_eq!(shelf_top_edges_at_depth(400.0, 1.0), (4.0, 396.0));
    }

    #[test]
    fn shelf_shadow_x_projection_follows_the_top_perspective() {
        let width = 400.0;
        let contact_depth = 1.0 - BOOKSHELF_CONTACT_DEPTH_FROM_FRONT_RATIO;
        let rear_depth = 0.4;
        let original_x = 137.0;
        assert!(
            (project_shelf_x_between_depths(width, original_x, contact_depth, contact_depth,)
                - original_x)
                .abs()
                < f64::EPSILON
        );
        assert_eq!(
            project_shelf_x_between_depths(width, width / 2.0, contact_depth, rear_depth),
            width / 2.0
        );

        let (contact_left, contact_right) = shelf_top_edges_at_depth(width, contact_depth);
        let (rear_left, rear_right) = shelf_top_edges_at_depth(width, rear_depth);
        assert_eq!(
            project_shelf_x_between_depths(width, contact_left, contact_depth, rear_depth),
            rear_left
        );
        assert_eq!(
            project_shelf_x_between_depths(width, contact_right, contact_depth, rear_depth),
            rear_right
        );
        assert!(rear_left > contact_left);
        assert!(rear_right < contact_right);
    }

    #[test]
    fn shelf_shadow_length_scales_with_book_height_within_limits() {
        assert_eq!(shelf_shadow_length(20.0), 2.0);
        assert!((shelf_shadow_length(100.0) - 2.5).abs() < f64::EPSILON * 4.0);
        assert_eq!(shelf_shadow_length(400.0), 5.0);
    }

    #[test]
    fn rear_shadow_expands_after_projection_without_leaving_the_shelf_top() {
        let width = 400.0;
        let projection = shelf_shadow_projection(width, 150.0, 250.0, 400.0);
        let (rear_shelf_left, rear_shelf_right) =
            shelf_top_edges_at_depth(width, projection.rear_depth);

        assert!(projection.rear_left > projection.contact_left);
        assert!(projection.rear_right < projection.contact_right);
        assert!(projection.rear_left >= rear_shelf_left);
        assert!(projection.rear_right <= rear_shelf_right);
    }

    #[test]
    fn front_contact_shadow_is_only_a_short_step_toward_the_front() {
        let width = 400.0;
        let contact_depth = 1.0 - BOOKSHELF_CONTACT_DEPTH_FROM_FRONT_RATIO;
        let projection = shelf_shadow_projection(width, 150.0, 250.0, 400.0);
        let contact_y = shelf_top_y_at_depth(0.0, contact_depth);
        let front_y = shelf_top_y_at_depth(0.0, projection.front_depth);

        assert!(projection.front_depth > contact_depth);
        assert!(projection.front_depth <= 1.0);
        assert!((0.5..=1.0).contains(&(front_y - contact_y)));
    }

    #[test]
    fn centered_shadow_stays_centered_at_rear_and_front_depths() {
        let width = 400.0;
        let projection = shelf_shadow_projection(width, 150.0, 250.0, 100.0);
        let rear_center = (projection.rear_left + projection.rear_right) / 2.0;
        let front_center = (projection.front_left + projection.front_right) / 2.0;

        assert!((rear_center - width / 2.0).abs() < 1e-9);
        assert!((front_center - width / 2.0).abs() < 1e-9);
    }
}
