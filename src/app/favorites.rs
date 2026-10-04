use super::cover_cancel::CoverCancel;
use super::cover_scheduler::{CoverLoadScheduler, CoverSchedulerJob};
use super::list_model_sync::{
    apply_boxed_list_model_changes, boxed_list_model_entries, plan_list_model_changes,
};
use super::navigation_panel::{detach_navigation_popover, navigation_empty_state};
use super::thumbnail_texture::texture_from_thumbnail;
use super::{AppSender, Msg};
use crate::bookshelf::thumbnail::ThumbnailData;
use crate::favorites::{FavoriteEntry, FavoriteIdentity};
use crate::history::is_volume_directory_name;
use adw::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FAVORITE_ROW_ID: AtomicU64 = AtomicU64::new(0);

pub(super) const COVER_HEIGHT: i32 = 100;
pub(super) const COVER_WIDTH: i32 = 72;
const ROW_MARGIN: i32 = 12;

struct FavoriteRowWidgets {
    identity: RefCell<Option<FavoriteIdentity>>,
    title: gtk::Label,
    document: gtk::Label,
    picture: gtk::Picture,
    placeholder: gtk::Box,
    failure_icon: gtk::Image,
    context_popover: gtk::Popover,
}

#[derive(Debug)]
pub(super) enum FavoritesCoverJob {
    LoadCache {
        generation: u64,
        identity: FavoriteIdentity,
    },
    Generate {
        generation: u64,
        identity: FavoriteIdentity,
        source: PathBuf,
        cancel: CoverCancel,
    },
}

impl From<CoverSchedulerJob<FavoriteIdentity, PathBuf>> for FavoritesCoverJob {
    fn from(job: CoverSchedulerJob<FavoriteIdentity, PathBuf>) -> Self {
        match job {
            CoverSchedulerJob::LoadCache { generation, key } => Self::LoadCache {
                generation,
                identity: key,
            },
            CoverSchedulerJob::Generate {
                generation,
                key,
                payload,
                cancel,
            } => Self::Generate {
                generation,
                identity: key,
                source: payload,
                cancel,
            },
        }
    }
}

#[derive(Default)]
pub(super) struct FavoritesView {
    items: Option<gtk::Box>,
    viewport: Option<gtk::ScrolledWindow>,
    model: Option<gtk::gio::ListStore>,
    list: Option<gtk::ListView>,
    scheduler: CoverLoadScheduler<FavoriteIdentity, PathBuf>,
    rows: Rc<RefCell<HashMap<u64, Rc<FavoriteRowWidgets>>>>,
}

impl FavoritesView {
    pub(super) fn attach(&mut self, items: gtk::Box, viewport: gtk::ScrolledWindow) {
        self.items = Some(items);
        self.viewport = Some(viewport);
    }

    pub(super) fn invalidate(&mut self) {
        self.scheduler.invalidate();
        self.cleanup_realized_rows();
        self.model = None;
        self.list = None;
    }

    pub(super) fn invalidate_cover_textures(&mut self) -> Vec<FavoritesCoverJob> {
        self.scheduler.invalidate();
        for row in self.rows.borrow().values() {
            row.picture.set_paintable(None::<&gtk::gdk::Paintable>);
            row.failure_icon.set_visible(false);
            row.placeholder.set_visible(true);
        }
        self.demand_changed()
    }

    pub(super) fn shutdown(&mut self) {
        self.pause();
        self.invalidate();
    }

    pub(super) fn pause(&mut self) {
        self.scheduler.pause();
    }

    pub(super) fn resume(&mut self) -> Vec<FavoritesCoverJob> {
        self.scheduler
            .resume()
            .into_iter()
            .map(Into::into)
            .collect()
    }

    pub(super) fn render(
        &mut self,
        entries: &[FavoriteEntry],
        bookshelf_root: Option<&std::path::Path>,
        on_open: Rc<dyn Fn(FavoriteIdentity)>,
        on_remove: Rc<dyn Fn(FavoriteIdentity)>,
        sender: AppSender,
    ) -> Vec<FavoritesCoverJob> {
        let (Some(items), Some(viewport)) = (self.items.clone(), self.viewport.clone()) else {
            return Vec::new();
        };

        if entries.is_empty() {
            self.invalidate();
            while let Some(child) = items.first_child() {
                items.remove(&child);
            }
            viewport.set_child(Some(&items));
            items.append(&navigation_empty_state(
                "non-starred-symbolic",
                "お気に入りはありません",
                "本棚の項目を右クリックして\nお気に入りに追加できます",
            ));
            return Vec::new();
        }

        if let Some(model) = self.model.clone() {
            let current = boxed_list_model_entries::<FavoriteEntry>(&model);
            let changes =
                plan_list_model_changes(&current, entries, |entry| entry.identity.clone());
            if changes.is_empty() {
                let _ = self.scheduler.resume();
                return self.demand_changed();
            }
            self.scheduler.invalidate();
            let _ = self.scheduler.resume();
            apply_boxed_list_model_changes(&model, changes);
            return self.demand_changed();
        }

        self.invalidate();
        let _ = self.scheduler.resume();
        while let Some(child) = items.first_child() {
            items.remove(&child);
        }
        let model = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        for entry in entries {
            model.append(&glib::BoxedAnyObject::new(entry.clone()));
        }
        let factory = favorite_row_factory(
            self.rows.clone(),
            bookshelf_root.map(ToOwned::to_owned),
            on_open,
            on_remove,
            sender,
        );
        let selection = gtk::NoSelection::new(Some(model.clone()));
        let list = gtk::ListView::new(Some(selection), Some(factory));
        list.set_margin_start(ROW_MARGIN);
        list.set_margin_end(ROW_MARGIN);
        list.set_margin_top(ROW_MARGIN - 3);
        list.set_margin_bottom(ROW_MARGIN - 3);
        list.add_css_class("navigation-cover-list");
        viewport.set_child(Some(&list));
        self.model = Some(model);
        self.list = Some(list);
        Vec::new()
    }

    pub(super) fn demand_changed(&mut self) -> Vec<FavoritesCoverJob> {
        let demand = self
            .rows
            .borrow()
            .values()
            .filter_map(|row| row.identity.borrow().clone())
            .collect::<Vec<_>>();
        self.scheduler.replace_demand(demand);
        self.schedule_jobs()
    }

    pub(super) fn cache_finished(
        &mut self,
        generation: u64,
        identity: FavoriteIdentity,
        source: Option<PathBuf>,
        thumbnail: Option<ThumbnailData>,
    ) -> Vec<FavoritesCoverJob> {
        if !self.scheduler.cache_finished(generation, &identity) {
            return self.schedule_jobs();
        }
        match (source, thumbnail.and_then(texture_from_thumbnail)) {
            (_, Some(texture)) => {
                self.show_texture(&identity, &texture);
                self.scheduler.mark_resolved(&identity);
            }
            (Some(source), None) => self.scheduler.enqueue_generation(identity, source),
            (None, None) => {
                self.show_failure(&identity);
                self.scheduler.mark_resolved(&identity);
            }
        }
        self.schedule_jobs()
    }

    pub(super) fn generation_finished(
        &mut self,
        generation: u64,
        identity: FavoriteIdentity,
        _source: PathBuf,
        result: Result<Option<ThumbnailData>, String>,
    ) -> Vec<FavoritesCoverJob> {
        if !self.scheduler.generation_finished(generation, &identity) {
            return self.schedule_jobs();
        }
        if matches!(result, Ok(None)) {
            return self.schedule_jobs();
        }
        match result.ok().flatten().and_then(texture_from_thumbnail) {
            Some(texture) => self.show_texture(&identity, &texture),
            None => self.show_failure(&identity),
        }
        self.scheduler.mark_resolved(&identity);
        self.schedule_jobs()
    }

    fn schedule_jobs(&mut self) -> Vec<FavoritesCoverJob> {
        self.scheduler
            .schedule_jobs()
            .into_iter()
            .map(Into::into)
            .collect()
    }

    fn show_texture(&self, identity: &FavoriteIdentity, texture: &gtk::gdk::Texture) {
        for row in self.rows.borrow().values() {
            if binding_matches(row.identity.borrow().as_ref(), identity) {
                row.picture.set_paintable(Some(texture));
                row.failure_icon.set_visible(false);
                row.placeholder.set_visible(false);
            }
        }
    }

    fn show_failure(&self, identity: &FavoriteIdentity) {
        for row in self.rows.borrow().values() {
            if binding_matches(row.identity.borrow().as_ref(), identity) {
                row.picture.set_paintable(None::<&gtk::gdk::Paintable>);
                row.failure_icon.set_visible(true);
                row.placeholder.set_visible(true);
            }
        }
    }

    fn cleanup_realized_rows(&mut self) {
        let realized_rows = self.rows.borrow().values().cloned().collect::<Vec<_>>();
        for row in &realized_rows {
            cleanup_favorite_row(row);
        }
        self.rows.borrow_mut().clear();
    }
}

fn binding_matches(bound: Option<&FavoriteIdentity>, completed: &FavoriteIdentity) -> bool {
    bound == Some(completed)
}

fn favorite_row_factory(
    rows: Rc<RefCell<HashMap<u64, Rc<FavoriteRowWidgets>>>>,
    bookshelf_root: Option<PathBuf>,
    on_open: Rc<dyn Fn(FavoriteIdentity)>,
    on_remove: Rc<dyn Fn(FavoriteIdentity)>,
    sender: AppSender,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup({
        let rows = rows.clone();
        move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let id = NEXT_FAVORITE_ROW_ID.fetch_add(1, Ordering::Relaxed);
            let (card, row) = favorite_row_widgets(on_open.clone(), on_remove.clone());
            card.set_widget_name(&id.to_string());
            rows.borrow_mut().insert(id, row);
            item.set_child(Some(&card));
        }
    });
    factory.connect_bind({
        let rows = rows.clone();
        let sender = sender.clone();
        move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some((_, row)) = favorite_factory_row(item, &rows) else {
                return;
            };
            let Some(object) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let entry = object.borrow::<FavoriteEntry>();
            let display = favorite_display(&entry.identity, bookshelf_root.as_deref());
            row.title.set_label(&display.work_title);
            row.document
                .set_label(display.document_title.as_deref().unwrap_or(""));
            row.document.set_visible(display.document_title.is_some());
            row.picture.set_paintable(None::<&gtk::gdk::Paintable>);
            row.failure_icon.set_visible(false);
            row.placeholder.set_visible(true);
            *row.identity.borrow_mut() = Some(entry.identity.clone());
            sender.input(Msg::FavoritesCoverDemandChanged);
        }
    });
    factory.connect_unbind({
        let rows = rows.clone();
        move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some((_, row)) = favorite_factory_row(item, &rows) else {
                return;
            };
            cleanup_favorite_row(&row);
            sender.input(Msg::FavoritesCoverDemandChanged);
        }
    });
    factory.connect_teardown({
        move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            if let Some((id, row)) = favorite_factory_row(item, &rows) {
                cleanup_favorite_row(&row);
                rows.borrow_mut().remove(&id);
            }
            item.set_child(None::<&gtk::Widget>);
        }
    });
    factory
}

fn cleanup_favorite_row(row: &FavoriteRowWidgets) {
    detach_navigation_popover(&row.context_popover);
    row.picture.set_paintable(None::<&gtk::gdk::Paintable>);
    row.failure_icon.set_visible(false);
    row.placeholder.set_visible(true);
    *row.identity.borrow_mut() = None;
}

fn favorite_factory_row(
    item: &gtk::ListItem,
    rows: &RefCell<HashMap<u64, Rc<FavoriteRowWidgets>>>,
) -> Option<(u64, Rc<FavoriteRowWidgets>)> {
    let id = item.child()?.widget_name().parse().ok()?;
    let row = rows.borrow().get(&id)?.clone();
    Some((id, row))
}

fn favorite_row_widgets(
    on_open: Rc<dyn Fn(FavoriteIdentity)>,
    on_remove: Rc<dyn Fn(FavoriteIdentity)>,
) -> (gtk::Button, Rc<FavoriteRowWidgets>) {
    let placeholder = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["cover-placeholder"])
        .build();
    let failure_icon = gtk::Image::from_icon_name("text-x-generic-symbolic");
    failure_icon.set_visible(false);
    failure_icon.set_vexpand(true);
    failure_icon.set_valign(gtk::Align::Center);
    placeholder.append(&failure_icon);
    let picture = gtk::Picture::builder()
        .can_shrink(true)
        .content_fit(gtk::ContentFit::Contain)
        .halign(gtk::Align::Fill)
        .valign(gtk::Align::Fill)
        .build();
    let cover = gtk::Overlay::builder()
        .child(
            &gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .width_request(COVER_WIDTH)
                .height_request(COVER_HEIGHT)
                .build(),
        )
        .width_request(COVER_WIDTH)
        .height_request(COVER_HEIGHT)
        .halign(gtk::Align::Start)
        .build();
    cover.add_overlay(&placeholder);
    cover.add_overlay(&picture);

    let title = gtk::Label::builder()
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(28)
        .xalign(0.0)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .css_classes(["heading"])
        .build();
    let document = gtk::Label::builder()
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(28)
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["caption", "dim-label"])
        .build();
    let labels = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(3)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    labels.append(&title);
    labels.append(&document);
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .hexpand(true)
        .build();
    content.append(&cover);
    content.append(&labels);
    let card = gtk::Button::builder()
        .child(&content)
        .hexpand(true)
        .margin_top(3)
        .margin_bottom(3)
        .css_classes(["flat", "history-row"])
        .build();

    let popover = gtk::Popover::builder().has_arrow(false).build();
    popover.connect_closed(|popover| {
        if popover.parent().is_some() {
            popover.unparent();
        }
    });
    let remove_button = gtk::Button::builder()
        .label("お気に入りから削除")
        .css_classes(["flat"])
        .margin_top(4)
        .margin_bottom(4)
        .margin_start(4)
        .margin_end(4)
        .build();
    popover.set_child(Some(&remove_button));
    let row = Rc::new(FavoriteRowWidgets {
        identity: RefCell::new(None),
        title,
        document,
        picture,
        placeholder,
        failure_icon,
        context_popover: popover.clone(),
    });
    card.connect_clicked({
        let row = Rc::downgrade(&row);
        move |_| {
            let Some(row) = row.upgrade() else {
                return;
            };
            if let Some(identity) = row.identity.borrow().as_ref() {
                on_open(identity.clone());
            }
        }
    });
    remove_button.connect_clicked({
        let row = Rc::downgrade(&row);
        move |_| {
            let Some(row) = row.upgrade() else {
                return;
            };
            let identity = row.identity.borrow().clone();
            detach_navigation_popover(&row.context_popover);
            if let Some(identity) = identity {
                on_remove(identity);
            }
        }
    });
    let secondary_click = gtk::GestureClick::new();
    secondary_click.set_button(gtk::gdk::BUTTON_SECONDARY);
    secondary_click.connect_pressed({
        move |gesture, _, x, y| {
            if popover.parent().is_none() {
                let Some(card) = gesture.widget() else {
                    return;
                };
                popover.set_parent(&card);
            }
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

    (card, row)
}

#[derive(Debug, PartialEq, Eq)]
struct FavoriteDisplay {
    work_title: String,
    document_title: Option<String>,
}

fn favorite_display(
    identity: &FavoriteIdentity,
    bookshelf_root: Option<&std::path::Path>,
) -> FavoriteDisplay {
    let path = identity.path();
    let document_name = display_name(path);
    if bookshelf_root.is_some_and(|root| path.parent() == Some(root)) {
        return FavoriteDisplay {
            work_title: document_name,
            document_title: None,
        };
    }

    let work_path = match identity {
        FavoriteIdentity::FileDocument(_) => path.parent(),
        FavoriteIdentity::ImageFolderDocument(_) if is_volume_directory_name(&document_name) => {
            path.parent()
        }
        FavoriteIdentity::ImageFolderDocument(_) => None,
    };
    let Some(work_path) = work_path else {
        return FavoriteDisplay {
            work_title: document_name,
            document_title: None,
        };
    };
    let work_title = display_name(work_path);
    if work_title == document_name {
        FavoriteDisplay {
            work_title,
            document_title: None,
        }
    } else {
        FavoriteDisplay {
            work_title,
            document_title: Some(document_name),
        }
    }
}

fn display_name(path: &std::path::Path) -> String {
    path.file_name()
        .filter(|name| !name.is_empty())
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_keeps_pending_cover_state_until_resume() {
        let mut view = FavoritesView::default();
        let identity = FavoriteIdentity::FileDocument(PathBuf::from("/book.cbz"));
        view.scheduler.replace_demand([identity.clone()]);
        let generation = view.scheduler.generation();

        view.pause();
        assert!(view.schedule_jobs().is_empty());
        assert_eq!(view.scheduler.generation(), generation);
        assert_eq!(view.scheduler.pending_cache_len(), 1);
        let jobs = view.resume();
        assert!(matches!(
            &jobs[0],
            FavoritesCoverJob::LoadCache {
                generation: job_generation,
                identity: job_identity,
            } if *job_generation == generation && job_identity == &identity
        ));
    }

    #[test]
    fn cover_override_invalidates_visible_cover_jobs() {
        let mut view = FavoritesView::default();
        let identity = FavoriteIdentity::FileDocument(PathBuf::from("/book.cbz"));
        view.scheduler.replace_demand([identity.clone()]);
        let old_generation = view.scheduler.generation();
        assert_eq!(view.schedule_jobs().len(), 1);

        assert!(view.invalidate_cover_textures().is_empty());
        assert_eq!(view.scheduler.generation(), old_generation.wrapping_add(1));
        view.scheduler.replace_demand([identity.clone()]);
        assert!(view.schedule_jobs().is_empty());
        assert!(!view.scheduler.cache_finished(old_generation, &identity));
        assert_eq!(view.schedule_jobs().len(), 1);
    }

    #[test]
    fn recycled_binding_accepts_only_its_current_identity() {
        let old = FavoriteIdentity::FileDocument(PathBuf::from("/old.cbz"));
        let current = FavoriteIdentity::FileDocument(PathBuf::from("/current.cbz"));
        assert!(binding_matches(Some(&current), &current));
        assert!(!binding_matches(Some(&current), &old));
        assert!(!binding_matches(None, &old));
    }

    #[test]
    fn favorite_display_separates_work_and_document_without_changing_identity() {
        let root = PathBuf::from("/本棚");
        assert_eq!(
            favorite_display(
                &FavoriteIdentity::FileDocument(PathBuf::from("/本棚/作品/03巻.rar")),
                Some(&root),
            ),
            FavoriteDisplay {
                work_title: "作品".into(),
                document_title: Some("03巻.rar".into()),
            }
        );
        assert_eq!(
            favorite_display(
                &FavoriteIdentity::ImageFolderDocument(PathBuf::from("/本棚/作品/vol.03")),
                Some(&root),
            ),
            FavoriteDisplay {
                work_title: "作品".into(),
                document_title: Some("vol.03".into()),
            }
        );
    }

    #[test]
    fn favorite_display_avoids_root_and_image_work_duplication() {
        let root = PathBuf::from("/本棚");
        for identity in [
            FavoriteIdentity::FileDocument(PathBuf::from("/本棚/単独作品.zip")),
            FavoriteIdentity::ImageFolderDocument(PathBuf::from("/本棚/作品")),
        ] {
            let display = favorite_display(&identity, Some(&root));
            assert!(display.document_title.is_none());
        }
    }
}
