use super::super::{AppSender, Msg};
use super::progress::{BookshelfDisplaySizes, LibraryProgress, library_badge_progress};
use super::shelf_container::{SHELF_CONTACT_TARGET_CSS_CLASS, bookshelf_title_spacing};
use super::view::LibraryView;
use crate::app::library_thumbnail::ThumbnailSourceKind;
use crate::covers::CoverBookIdentity;
use crate::favorites::{FavoriteEntry, FavoriteIdentity};
use crate::history::HistoryEntry;
use adw::prelude::*;
use gtk::subclass::prelude::ObjectSubclassIsExt;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub(super) const LIBRARY_HORIZONTAL_MARGIN: i32 = 18;
pub(super) const COVER_COLUMN_SPACING: u32 = 12;
pub(super) const COVER_ROW_SPACING: u32 = 16;
pub(super) const APPROXIMATE_LABEL_CHAR_WIDTH: i32 = 8;
const PROGRESS_BAR_SIZE: i32 = 5;
const FOLDER_SPINE_WIDTH_REFERENCE_INSET: i32 = 14;
const FOLDER_SPINE_BOTTOM_CORNER_OPACITY: f64 = 0.4;
const BOOKSHELF_CARD_MIN_WIDTH: i32 = 100;

pub(super) struct ThumbnailTarget {
    pub(super) source: PathBuf,
    pub(super) kind: ThumbnailSourceKind,
    pub(super) slot: gtk::Overlay,
    pub(super) placeholder: gtk::Box,
    pub(super) placeholder_icon: gtk::Image,
    pub(super) picture: gtk::Picture,
    pub(super) aspect_frame: Option<gtk::AspectFrame>,
    pub(super) favorite_indicator: Option<gtk::Overlay>,
    pub(super) favorite_state: Option<Rc<Cell<bool>>>,
    pub(super) favorite_ready: Option<Rc<Cell<bool>>>,
    pub(super) favorite_indicator_enabled: Option<bool>,
    pub(super) bookshelf_book_height: Option<i32>,
}

pub(super) struct ProgressTarget {
    pub(super) identity: FavoriteIdentity,
    pub(super) title: gtk::Label,
    pub(super) track: gtk::Box,
    pub(super) bar: gtk::Box,
    pub(super) fraction: Rc<Cell<f64>>,
    pub(super) cover: gtk::Overlay,
}

pub(super) struct FavoriteTarget {
    pub(super) identity: FavoriteIdentity,
    pub(super) button: gtk::Button,
    pub(super) indicator: gtk::Overlay,
    pub(super) indicator_enabled: bool,
    pub(super) favorite_state: Rc<Cell<bool>>,
    pub(super) favorite_ready: Rc<Cell<bool>>,
}

fn folder_spine_bottom_clip_width(width: i32) -> Option<i32> {
    (width >= 4).then_some(width - 2)
}

mod folder_spine_clip {
    use super::*;
    use gtk::subclass::prelude::*;

    #[derive(Default)]
    pub struct FolderSpineClip {
        pub child: RefCell<Option<gtk::Widget>>,
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for FolderSpineClip {
        const NAME: &'static str = "AgnamFolderSpineClip";
        type Type = super::FolderSpineClip;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for FolderSpineClip {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().set_layout_manager(Some(gtk::BinLayout::new()));
        }

        fn dispose(&self) {
            if let Some(child) = self.child.take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for FolderSpineClip {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(child) = self.child.borrow().as_ref().cloned() else {
                return;
            };
            let widget = self.obj();
            let width = widget.width();
            let height = widget.height();
            let Some(bottom_width) = folder_spine_bottom_clip_width(width) else {
                widget.snapshot_child(&child, snapshot);
                return;
            };
            if height <= 0 {
                return;
            }

            if height > 1 {
                snapshot.push_clip(&gtk::graphene::Rect::new(
                    0.0,
                    0.0,
                    width as f32,
                    (height - 1) as f32,
                ));
                widget.snapshot_child(&child, snapshot);
                snapshot.pop();
            }
            snapshot.push_clip(&gtk::graphene::Rect::new(
                1.0,
                (height - 1) as f32,
                bottom_width as f32,
                1.0,
            ));
            widget.snapshot_child(&child, snapshot);
            snapshot.pop();

            for x in [0.0, (width - 1) as f32] {
                snapshot.push_clip(&gtk::graphene::Rect::new(x, (height - 1) as f32, 1.0, 1.0));
                snapshot.push_opacity(FOLDER_SPINE_BOTTOM_CORNER_OPACITY);
                widget.snapshot_child(&child, snapshot);
                snapshot.pop();
                snapshot.pop();
            }
        }
    }
}

gtk::glib::wrapper! {
    pub struct FolderSpineClip(ObjectSubclass<folder_spine_clip::FolderSpineClip>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl FolderSpineClip {
    fn new(child: &(impl IsA<gtk::Widget> + Clone)) -> Self {
        let clip: Self = gtk::glib::Object::new();
        let child = child.clone().upcast::<gtk::Widget>();
        child.set_parent(&clip);
        clip.imp().child.replace(Some(child));
        clip
    }
}

fn bookshelf_card_content() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(bookshelf_title_spacing())
        .width_request(BOOKSHELF_CARD_MIN_WIDTH)
        .halign(gtk::Align::Fill)
        .build()
}

fn bookshelf_card_title(path: &Path) -> gtk::Label {
    gtk::Label::builder()
        .label(display_name(path))
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars((BOOKSHELF_CARD_MIN_WIDTH / APPROXIMATE_LABEL_CHAR_WIDTH).max(1))
        .single_line_mode(true)
        .hexpand(true)
        .halign(gtk::Align::Fill)
        .xalign(0.5)
        .build()
}

impl LibraryView {
    pub(super) fn archive_container_button(
        &self,
        path: &Path,
        icon: &str,
        sizes: BookshelfDisplaySizes,
    ) -> gtk::Button {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .width_request(book_placeholder_width(sizes.book_height))
            .build();
        let icon = gtk::Image::from_icon_name(icon);
        icon.set_pixel_size(48);
        icon.set_hexpand(true);
        icon.set_vexpand(true);
        icon.set_halign(gtk::Align::Center);
        icon.set_valign(gtk::Align::Center);
        let slot = gtk::Box::builder()
            .width_request(book_placeholder_width(sizes.book_height))
            .height_request(sizes.book_height)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .css_classes(["cover-placeholder"])
            .build();
        slot.append(&icon);
        let title = gtk::Label::builder()
            .label(display_name(path))
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .width_request(book_placeholder_width(sizes.book_height))
            .max_width_chars(
                (book_placeholder_width(sizes.book_height) / APPROXIMATE_LABEL_CHAR_WIDTH).max(1),
            )
            .xalign(0.5)
            .build();
        content.append(&slot);
        content.append(&title);
        let button = gtk::Button::builder()
            .child(&content)
            .css_classes(["flat", "cover-item"])
            .build();
        button.set_tooltip_text(Some(&display_name(path)));
        button
    }

    pub(super) fn content_image_button(
        &mut self,
        title_path: &Path,
        thumbnail_key: &Path,
        kind: ThumbnailSourceKind,
        cover_only: bool,
        sizes: BookshelfDisplaySizes,
        bookshelf: bool,
    ) -> gtk::Button {
        let content = if bookshelf {
            bookshelf_card_content()
        } else {
            gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(6)
                .halign(gtk::Align::Center)
                .build()
        };
        let (cover, indicator, _, _) =
            self.book_cover_slot(thumbnail_key, kind, sizes, false, false);
        indicator.set_visible(false);
        if cover_only {
            let icon = gtk::Image::from_icon_name("action-unavailable-symbolic");
            icon.set_pixel_size(16);
            icon.set_halign(gtk::Align::End);
            icon.set_valign(gtk::Align::Start);
            icon.set_margin_top(6);
            icon.set_margin_end(6);
            icon.set_can_target(false);
            cover.add_overlay(&icon);
            cover.set_measure_overlay(&icon, false);
        }
        let title = if bookshelf {
            bookshelf_card_title(title_path)
        } else {
            gtk::Label::builder()
                .label(display_name(title_path))
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .max_width_chars(1)
                .xalign(0.5)
                .build()
        };
        content.append(&cover);
        content.append(&title);
        let button = gtk::Button::builder()
            .child(&content)
            .css_classes(["flat", "cover-item"])
            .build();
        if bookshelf {
            button.add_css_class("bookshelf-card");
        }
        let tooltip = if cover_only {
            "表紙専用画像（閲覧不可）".to_string()
        } else {
            display_name(title_path)
        };
        button.set_tooltip_text(Some(&tooltip));
        button
    }

    pub(super) fn folder_button(
        &mut self,
        path: &Path,
        preview_candidates: &[PathBuf],
        preview_item_count: usize,
        sizes: BookshelfDisplaySizes,
        sender: &AppSender,
    ) -> gtk::Button {
        let content = bookshelf_card_content();
        let preview = self.folder_preview(preview_candidates, preview_item_count, sizes);
        let title = bookshelf_card_title(path);
        content.append(&preview);
        content.append(&title);
        let button = gtk::Button::builder()
            .child(&content)
            .css_classes(["flat", "folder-card", "bookshelf-card"])
            .build();
        button.set_tooltip_text(Some(&display_name(path)));
        let path = path.to_path_buf();
        let sender = sender.clone();
        button.connect_clicked(move |_| {
            sender.input(folder_message(path.clone(), None));
        });
        button
    }

    fn folder_preview(
        &mut self,
        preview_candidates: &[PathBuf],
        _preview_item_count: usize,
        sizes: BookshelfDisplaySizes,
    ) -> gtk::CenterBox {
        let height = sizes.book_height;
        let spine_width_reference_height = height
            .saturating_sub(FOLDER_SPINE_WIDTH_REFERENCE_INSET)
            .max(1);
        let books = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(0)
            .height_request(height)
            .valign(gtk::Align::End)
            .build();
        let front_width = (height * 2 / 3).max(1);
        let spine_width = (spine_width_reference_height / 9).max(1);
        for (index, candidate) in preview_candidates.iter().enumerate() {
            let spine = index != 0;
            let book_width = if spine { spine_width } else { front_width };
            let slot = self.thumbnail_slot(candidate, book_width, height, spine);
            if spine {
                slot.add_css_class("folder-book-spine");
                let clip = FolderSpineClip::new(&slot);
                clip.set_valign(gtk::Align::Fill);
                books.append(&clip);
            } else {
                slot.add_css_class("folder-book-front");
                books.append(&slot);
            }
        }
        let preview = gtk::CenterBox::builder()
            .center_widget(&books)
            .height_request(height)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::End)
            .css_classes(["folder-book-bundle", SHELF_CONTACT_TARGET_CSS_CLASS])
            .build();
        preview
    }

    pub(super) fn cover_button(
        &mut self,
        title_path: &Path,
        cover_source: &Path,
        thumbnail_kind: ThumbnailSourceKind,
        identity: FavoriteIdentity,
        history_entries: &[HistoryEntry],
        show_progress: bool,
        favorite: bool,
        indicator_enabled: bool,
        sizes: BookshelfDisplaySizes,
    ) -> (gtk::Button, Rc<Cell<bool>>, Rc<Cell<bool>>) {
        let content = bookshelf_card_content();
        let (cover, indicator, favorite_state, favorite_ready) = self.book_cover_slot(
            cover_source,
            thumbnail_kind,
            sizes,
            favorite,
            indicator_enabled,
        );
        let title = bookshelf_card_title(title_path);
        if show_progress {
            self.add_progress_indicators(&cover, identity, history_entries, &title);
        }
        content.append(&cover);
        content.append(&title);
        let button = gtk::Button::builder()
            .child(&content)
            .css_classes(["flat", "cover-item", "bookshelf-card"])
            .build();
        button.set_tooltip_text(Some(&display_name(title_path)));
        indicator.set_visible(indicator_enabled && favorite && favorite_ready.get());
        (button, favorite_state, favorite_ready)
    }

    fn book_cover_slot(
        &mut self,
        path: &Path,
        kind: ThumbnailSourceKind,
        sizes: BookshelfDisplaySizes,
        favorite: bool,
        indicator_enabled: bool,
    ) -> (gtk::Overlay, gtk::Overlay, Rc<Cell<bool>>, Rc<Cell<bool>>) {
        let slot_size = gtk::Box::builder()
            .width_request(book_placeholder_width(sizes.book_height))
            .height_request(sizes.book_height)
            .build();
        let placeholder = gtk::Box::builder()
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Fill)
            .css_classes(["cover-placeholder"])
            .build();
        let placeholder_icon = gtk::Image::from_icon_name("text-x-generic-symbolic");
        placeholder_icon.set_visible(false);
        placeholder_icon.set_vexpand(true);
        placeholder_icon.set_valign(gtk::Align::Center);
        placeholder.append(&placeholder_icon);

        let picture = gtk::Picture::builder()
            .can_shrink(true)
            .content_fit(gtk::ContentFit::Cover)
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Fill)
            .build();
        let image_overlay = gtk::Overlay::builder().child(&picture).build();
        let slot = gtk::Overlay::builder()
            .child(&slot_size)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .css_classes([SHELF_CONTACT_TARGET_CSS_CLASS])
            .build();
        slot.add_overlay(&placeholder);
        slot.add_overlay(&image_overlay);

        let indicator = self.add_favorite_indicator(&image_overlay);
        let favorite_state = Rc::new(Cell::new(favorite));
        let favorite_ready = Rc::new(Cell::new(false));
        if let Some(texture) = self.cached_texture(path, kind) {
            picture.set_paintable(Some(texture));
            placeholder.set_visible(false);
            set_bookshelf_book_width(&slot, sizes.book_height, texture.width(), texture.height());
            favorite_ready.set(true);
            indicator.set_visible(indicator_enabled && favorite);
        } else if self.source_failed(path, kind) {
            placeholder_icon.set_visible(true);
        }
        self.register_thumbnail_target(ThumbnailTarget {
            source: path.to_path_buf(),
            kind,
            slot: slot.clone(),
            placeholder,
            placeholder_icon,
            picture,
            aspect_frame: None,
            favorite_indicator: Some(indicator.clone()),
            favorite_state: Some(favorite_state.clone()),
            favorite_ready: Some(favorite_ready.clone()),
            favorite_indicator_enabled: Some(indicator_enabled),
            bookshelf_book_height: Some(sizes.book_height),
        });
        (slot, indicator, favorite_state, favorite_ready)
    }

    fn add_favorite_indicator(&self, cover: &gtk::Overlay) -> gtk::Overlay {
        let background = gtk::Box::builder().can_target(false).build();
        let indicator = gtk::Overlay::builder()
            .child(&background)
            .width_request(24)
            .height_request(24)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(4)
            .margin_end(4)
            .can_target(false)
            .css_classes(["library-favorite-indicator"])
            .build();
        let icon = gtk::Image::from_icon_name("starred-symbolic");
        icon.set_pixel_size(12);
        icon.set_halign(gtk::Align::Center);
        icon.set_valign(gtk::Align::Center);
        icon.set_can_target(false);
        indicator.add_overlay(&icon);
        indicator.set_measure_overlay(&icon, false);
        cover.add_overlay(&indicator);
        cover.set_measure_overlay(&indicator, false);
        indicator
    }

    pub(super) fn favorite_indicator_for(&self, button: &gtk::Button) -> Option<gtk::Overlay> {
        let content = button.child()?.downcast::<gtk::Box>().ok()?;
        let cover = content.first_child()?.downcast::<gtk::Overlay>().ok()?;
        find_favorite_indicator(cover.upcast_ref())
    }

    fn add_progress_indicators(
        &mut self,
        cover: &gtk::Overlay,
        identity: FavoriteIdentity,
        history_entries: &[HistoryEntry],
        title: &gtk::Label,
    ) {
        let progress_track = gtk::Box::builder()
            .can_target(false)
            .height_request(PROGRESS_BAR_SIZE)
            .css_classes(["library-progress-track"])
            .build();
        let progress_bar = gtk::Box::builder()
            .can_target(false)
            .height_request(PROGRESS_BAR_SIZE)
            .css_classes(["library-progress-bar"])
            .build();
        let fraction = Rc::new(Cell::new(0.0));
        set_progress_indicators(
            title,
            &progress_track,
            &progress_bar,
            &fraction,
            library_badge_progress(history_entries, &identity),
        );
        cover.add_overlay(&progress_track);
        cover.set_measure_overlay(&progress_track, false);
        cover.set_clip_overlay(&progress_track, true);
        cover.add_overlay(&progress_bar);
        cover.set_measure_overlay(&progress_bar, false);
        cover.set_clip_overlay(&progress_bar, true);
        let progress_track_weak = progress_track.downgrade();
        let progress_bar_weak = progress_bar.downgrade();
        let fraction_for_position = fraction.clone();
        cover.connect_get_child_position(move |cover, child| {
            let progress_track = progress_track_weak.upgrade()?;
            let progress_bar = progress_bar_weak.upgrade()?;
            if child != progress_track.upcast_ref::<gtk::Widget>()
                && child != progress_bar.upcast_ref::<gtk::Widget>()
            {
                return None;
            }
            let height = cover.height().clamp(0, PROGRESS_BAR_SIZE);
            let (x, width) = if child == progress_track.upcast_ref::<gtk::Widget>() {
                (0, cover.width())
            } else {
                let width = progress_bar_width(cover.width(), fraction_for_position.get());
                (progress_bar_x(cover.width(), width), width)
            };
            Some(gtk::gdk::Rectangle::new(
                x,
                cover.height() - height,
                width,
                height,
            ))
        });
        self.register_progress_target(ProgressTarget {
            identity,
            title: title.clone(),
            track: progress_track,
            bar: progress_bar,
            fraction,
            cover: cover.clone(),
        });
    }

    fn thumbnail_slot(
        &mut self,
        path: &Path,
        width: i32,
        height: i32,
        spine: bool,
    ) -> gtk::Overlay {
        let slot_size = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .width_request(width)
            .height_request(height)
            .build();
        let placeholder = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Fill)
            .css_classes(["cover-placeholder"])
            .build();
        let icon = gtk::Image::from_icon_name("text-x-generic-symbolic");
        icon.set_visible(false);
        icon.set_vexpand(true);
        icon.set_valign(gtk::Align::Center);
        placeholder.append(&icon);

        let picture = gtk::Picture::builder()
            .can_shrink(true)
            .content_fit(gtk::ContentFit::Cover)
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Fill)
            .build();
        let slot = gtk::Overlay::builder()
            .child(&slot_size)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        slot.add_overlay(&placeholder);
        slot.add_overlay(&picture);

        let edge = |class: &'static str, width: i32, halign: gtk::Align| {
            let edge = gtk::Box::builder()
                .width_request(width)
                .halign(halign)
                .valign(gtk::Align::Fill)
                .can_target(false)
                .css_classes([class])
                .build();
            slot.add_overlay(&edge);
            slot.set_measure_overlay(&edge, false);
        };
        if spine {
            edge("folder-book-spine-highlight-edge", 2, gtk::Align::Start);
            edge("folder-book-spine-shadow-edge", 2, gtk::Align::End);
        } else {
            edge("folder-book-front-highlight-edge", 1, gtk::Align::Start);
            edge("folder-book-front-shadow-edge", 1, gtk::Align::End);
        }

        if let Some(texture) = self.cached_texture(path, ThumbnailSourceKind::BookCover) {
            picture.set_paintable(Some(texture));
            placeholder.set_visible(false);
            if !spine {
                set_bookshelf_book_width(&slot, height, texture.width(), texture.height());
            }
        } else if self.source_failed(path, ThumbnailSourceKind::BookCover) {
            icon.set_visible(true);
        }
        self.register_thumbnail_target(ThumbnailTarget {
            source: path.to_path_buf(),
            kind: ThumbnailSourceKind::BookCover,
            slot: slot.clone(),
            placeholder,
            placeholder_icon: icon,
            picture,
            aspect_frame: None,
            favorite_indicator: None,
            favorite_state: None,
            favorite_ready: None,
            favorite_indicator_enabled: None,
            bookshelf_book_height: (!spine).then_some(height),
        });
        slot
    }
}

#[cfg(test)]
mod tests {
    use super::{bookshelf_book_width, folder_spine_bottom_clip_width};

    #[test]
    fn bookshelf_book_width_preserves_ratio_up_to_the_crop_limit() {
        assert_eq!(bookshelf_book_width(120, 2, 3), 80);
        assert_eq!(bookshelf_book_width(120, 1, 1), 120);
        assert_eq!(bookshelf_book_width(120, 3, 2), 180);
        assert_eq!(bookshelf_book_width(120, 301, 200), 180);
    }

    #[test]
    fn spine_bottom_clip_removes_one_pixel_from_each_side_when_safe() {
        assert_eq!(folder_spine_bottom_clip_width(4), Some(2));
        assert_eq!(folder_spine_bottom_clip_width(12), Some(10));
        assert_eq!(folder_spine_bottom_clip_width(3), None);
    }
}

pub(super) fn show_target_texture(target: &ThumbnailTarget, texture: &gtk::gdk::Texture) {
    target.picture.set_paintable(Some(texture));
    target.placeholder_icon.set_visible(false);
    target.placeholder.set_visible(false);
    if let Some(height) = target.bookshelf_book_height {
        set_bookshelf_book_width(&target.slot, height, texture.width(), texture.height());
    }
    if let Some(aspect_frame) = &target.aspect_frame {
        let ratio = texture.width() as f32 / texture.height() as f32;
        aspect_frame.set_ratio(ratio);
    }
    if let (Some(ready), Some(indicator), Some(favorite), Some(enabled)) = (
        &target.favorite_ready,
        &target.favorite_indicator,
        &target.favorite_state,
        target.favorite_indicator_enabled,
    ) {
        ready.set(true);
        indicator.set_visible(enabled && favorite.get());
    }
}

fn set_bookshelf_book_width(slot: &gtk::Overlay, height: i32, image_width: i32, image_height: i32) {
    let width = bookshelf_book_width(height, image_width, image_height);
    slot.set_size_request(width, height);
    if let Some(slot_size) = slot
        .child()
        .and_then(|child| child.downcast::<gtk::Box>().ok())
    {
        slot_size.set_size_request(width, height);
    }
}

fn bookshelf_book_width(height: i32, image_width: i32, image_height: i32) -> i32 {
    if image_width <= 0 || image_height <= 0 {
        return height.max(1);
    }
    let proportional_width =
        ((i64::from(height) * i64::from(image_width)) / i64::from(image_height)) as i32;
    proportional_width.clamp(1, height.saturating_mul(3) / 2)
}

fn book_placeholder_width(height: i32) -> i32 {
    (height * 2 / 3).max(1)
}

pub(super) fn show_target_failure(target: &ThumbnailTarget) {
    target.picture.set_paintable(None::<&gtk::gdk::Paintable>);
    target.placeholder_icon.set_visible(true);
    target.placeholder.set_visible(true);
    if let (Some(ready), Some(indicator)) = (&target.favorite_ready, &target.favorite_indicator) {
        ready.set(false);
        indicator.set_visible(false);
    }
}

pub(super) fn show_target_unloaded(target: &ThumbnailTarget) {
    target.picture.set_paintable(None::<&gtk::gdk::Paintable>);
    target.placeholder_icon.set_visible(false);
    target.placeholder.set_visible(true);
    if let (Some(ready), Some(indicator)) = (&target.favorite_ready, &target.favorite_indicator) {
        ready.set(false);
        indicator.set_visible(false);
    }
}

pub(super) fn direct_file_message(path: PathBuf) -> Msg {
    Msg::LibraryFileSelected(path)
}

pub(super) fn direct_file_read_from_start_message(path: PathBuf) -> Msg {
    Msg::LibraryFileReadFromStart(path)
}

pub(super) fn folder_message(path: PathBuf, image_document_entry: Option<PathBuf>) -> Msg {
    match image_document_entry {
        Some(entry) => Msg::LibraryImageFolderSelected {
            folder: path,
            entry,
        },
        None => Msg::NavigateLibrary(path),
    }
}

pub(super) fn direct_file_context_popover(
    card: &gtk::Button,
    path: PathBuf,
    identity: FavoriteIdentity,
    favorite: bool,
    sender: &AppSender,
) -> (gtk::Popover, gtk::Button) {
    let cover_identity = CoverBookIdentity::archive(&path);
    let show_contents = cover_identity
        .is_some()
        .then(|| BookContents::Archive(path.clone()));
    book_context_popover(
        card,
        identity,
        favorite,
        sender,
        move || direct_file_read_from_start_message(path.clone()),
        show_contents,
        cover_identity,
    )
}

pub(super) fn image_folder_context_popover(
    card: &gtk::Button,
    folder: PathBuf,
    entry: PathBuf,
    identity: FavoriteIdentity,
    favorite: bool,
    sender: &AppSender,
) -> (gtk::Popover, gtk::Button) {
    book_context_popover(
        card,
        identity,
        favorite,
        sender,
        move || Msg::LibraryImageFolderReadFromStart(entry.clone()),
        Some(BookContents::Folder(folder.clone())),
        Some(CoverBookIdentity::image_folder(&folder)),
    )
}

enum BookContents {
    Folder(PathBuf),
    Archive(PathBuf),
}

pub(super) fn folder_image_cover_popover(
    card: &gtk::Button,
    folder: PathBuf,
    image: PathBuf,
    sender: &AppSender,
) -> gtk::Popover {
    image_cover_popover(card, sender, move || Msg::SetFolderImageCover {
        folder: folder.clone(),
        image: image.clone(),
    })
}

pub(super) fn archive_image_cover_popover(
    card: &gtk::Button,
    archive: PathBuf,
    id: crate::archive::ArchiveImageId,
    sender: &AppSender,
) -> gtk::Popover {
    image_cover_popover(card, sender, move || Msg::SetArchiveImageCover {
        archive: archive.clone(),
        id: id.clone(),
    })
}

fn image_cover_popover(
    card: &gtk::Button,
    sender: &AppSender,
    message: impl Fn() -> Msg + 'static,
) -> gtk::Popover {
    let popover = gtk::Popover::builder().has_arrow(false).build();
    popover.set_parent(card);
    let menu = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_top(4)
        .margin_bottom(4)
        .margin_start(4)
        .margin_end(4)
        .build();
    let set_cover = context_menu_button("表紙にする");
    menu.append(&set_cover);
    popover.set_child(Some(&menu));
    set_cover.connect_clicked({
        let popover = popover.clone();
        let sender = sender.clone();
        move |_| {
            popover.popdown();
            sender.input(message());
        }
    });
    attach_secondary_click(card, &popover);
    popover
}

fn book_context_popover(
    card: &gtk::Button,
    identity: FavoriteIdentity,
    favorite: bool,
    sender: &AppSender,
    read_from_start_message: impl Fn() -> Msg + 'static,
    show_contents: Option<BookContents>,
    cover_identity: Option<CoverBookIdentity>,
) -> (gtk::Popover, gtk::Button) {
    let popover = gtk::Popover::builder().has_arrow(false).build();
    popover.set_parent(card);

    let menu = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_top(4)
        .margin_bottom(4)
        .margin_start(4)
        .margin_end(4)
        .build();
    let read_from_start = context_menu_button("最初から読む");
    menu.append(&read_from_start);
    popover.set_child(Some(&menu));

    read_from_start.connect_clicked({
        let popover = popover.clone();
        let sender = sender.clone();
        move |_| {
            popover.popdown();
            sender.input(read_from_start_message());
        }
    });
    let favorite_button = append_favorite_button(&menu, &popover, identity, favorite, sender);

    if let Some(contents) = show_contents {
        let label = match &contents {
            BookContents::Folder(_) => "フォルダの内容を表示",
            BookContents::Archive(_) => "アーカイブの内容を表示",
        };
        let show_contents = context_menu_button(label);
        menu.append(&show_contents);
        show_contents.connect_clicked({
            let popover = popover.clone();
            let sender = sender.clone();
            move |_| {
                popover.popdown();
                sender.input(match &contents {
                    BookContents::Folder(folder) => Msg::NavigateLibrary(folder.clone()),
                    BookContents::Archive(archive) => Msg::OpenArchiveContents(archive.clone()),
                });
            }
        });
    }

    if let Some(cover_identity) = cover_identity {
        let change_cover_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .build();
        let change_cover_label = gtk::Label::builder()
            .label("表紙を変更")
            .xalign(0.0)
            .hexpand(true)
            .build();
        change_cover_row.append(&change_cover_label);
        change_cover_row.append(&gtk::Image::from_icon_name("go-next-symbolic"));
        let change_cover = gtk::MenuButton::builder()
            .child(&change_cover_row)
            .css_classes(["flat"])
            .hexpand(true)
            .build();
        change_cover.set_always_show_arrow(false);
        menu.append(&change_cover);
        let cover_menu = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(4)
            .margin_end(4)
            .build();
        let cover_popover = gtk::Popover::builder()
            .has_arrow(false)
            .child(&cover_menu)
            .build();
        change_cover.set_popover(Some(&cover_popover));

        let automatic = context_menu_button("自動");
        cover_menu.append(&automatic);
        automatic.connect_clicked({
            let popover = popover.clone();
            let cover_popover = cover_popover.clone();
            let sender = sender.clone();
            let identity = cover_identity.clone();
            move |_| {
                cover_popover.popdown();
                popover.popdown();
                sender.input(Msg::ClearCoverOverride(identity.clone()));
            }
        });
        let external = context_menu_button("外部画像から選択…");
        cover_menu.append(&external);
        external.connect_clicked({
            let popover = popover.clone();
            let sender = sender.clone();
            move |_| {
                cover_popover.popdown();
                popover.popdown();
                sender.input(Msg::SelectExternalCover(cover_identity.clone()));
            }
        });
    }

    attach_secondary_click(card, &popover);

    (popover, favorite_button)
}

fn append_favorite_button(
    menu: &gtk::Box,
    popover: &gtk::Popover,
    identity: FavoriteIdentity,
    favorite: bool,
    sender: &AppSender,
) -> gtk::Button {
    let button = context_menu_button("");
    set_favorite_button_label(&button, favorite);
    menu.append(&button);
    button.connect_clicked({
        let popover = popover.clone();
        let sender = sender.clone();
        move |_| {
            popover.popdown();
            sender.input(Msg::ToggleFavorite(identity.clone()));
        }
    });
    button
}

fn context_menu_button(label: &str) -> gtk::Button {
    let button = gtk::Button::builder()
        .label(label)
        .css_classes(["flat"])
        .hexpand(true)
        .build();
    if let Some(label) = button.child().and_downcast::<gtk::Label>() {
        label.set_xalign(0.0);
        label.set_hexpand(true);
    }
    button
}

fn attach_secondary_click(card: &gtk::Button, popover: &gtk::Popover) {
    let secondary_click = gtk::GestureClick::new();
    secondary_click.set_button(gtk::gdk::BUTTON_SECONDARY);
    secondary_click.connect_pressed({
        let popover = popover.clone();
        move |gesture, _, x, y| {
            popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
                x.round() as i32,
                y.round() as i32,
                1,
                1,
            )));
            popover.popup();
            gesture.set_state(gtk::EventSequenceState::Claimed);
        }
    });
    card.add_controller(secondary_click);
}

pub(super) fn is_favorite(entries: &[FavoriteEntry], identity: &FavoriteIdentity) -> bool {
    entries.iter().any(|entry| &entry.identity == identity)
}

fn set_favorite_button_label(button: &gtk::Button, favorite: bool) {
    button.set_label(if favorite {
        "お気に入りから削除"
    } else {
        "お気に入りに追加"
    });
}

fn find_favorite_indicator(widget: &gtk::Widget) -> Option<gtk::Overlay> {
    if widget.has_css_class("library-favorite-indicator") {
        return widget.clone().downcast::<gtk::Overlay>().ok();
    }
    let mut child = widget.first_child();
    while let Some(child_widget) = child {
        if let Some(indicator) = find_favorite_indicator(&child_widget) {
            return Some(indicator);
        }
        child = child_widget.next_sibling();
    }
    None
}

pub(super) fn section_label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .margin_bottom(4)
        .css_classes(["heading"])
        .build()
}

pub(super) fn display_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn set_progress_indicators(
    title: &gtk::Label,
    track: &gtk::Box,
    bar: &gtk::Box,
    fraction: &Cell<f64>,
    progress: Option<LibraryProgress>,
) {
    if let Some(progress) = progress {
        fraction.set(progress.fraction());
        title.add_css_class("library-progress-title");
        track.set_visible(true);
        bar.set_visible(true);
    } else {
        fraction.set(0.0);
        title.remove_css_class("library-progress-title");
        track.set_visible(false);
        bar.set_visible(false);
    }
}

pub(super) fn progress_bar_width(cover_width: i32, fraction: f64) -> i32 {
    if cover_width <= 0 {
        return 0;
    }

    ((cover_width as f64 * fraction.clamp(0.0, 1.0)).round() as i32)
        .clamp(PROGRESS_BAR_SIZE, cover_width)
}

pub(super) fn progress_bar_x(cover_width: i32, width: i32) -> i32 {
    cover_width.saturating_sub(width)
}

impl LibraryView {
    pub(in crate::app) fn sync_progress(&self, history_entries: &[HistoryEntry]) {
        for target in self.progress_targets() {
            let progress = library_badge_progress(history_entries, &target.identity);
            set_progress_indicators(
                &target.title,
                &target.track,
                &target.bar,
                &target.fraction,
                progress,
            );
            target.cover.queue_allocate();
        }
    }

    pub(in crate::app) fn sync_favorites(&self, favorite_entries: &[FavoriteEntry]) {
        for target in self.favorite_targets() {
            let favorite = is_favorite(favorite_entries, &target.identity);
            target.favorite_state.set(favorite);
            set_favorite_button_label(&target.button, favorite);
            target
                .indicator
                .set_visible(target.indicator_enabled && favorite && target.favorite_ready.get());
        }
    }
}
