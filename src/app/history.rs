use super::cover_cancel::CoverCancel;
use super::cover_scheduler::{CoverLoadScheduler, CoverSchedulerJob};
use super::list_model_sync::{
    apply_boxed_list_model_changes, boxed_list_model_entries, plan_list_model_changes,
};
use super::navigation_panel::{detach_navigation_popover, navigation_empty_state};
use super::thumbnail_texture::texture_from_thumbnail;
use super::{AppSender, Msg};
use crate::bookshelf::thumbnail::ThumbnailData;
use crate::history::{HistoryEntry, HistoryIdentity};
use adw::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_HISTORY_ROW_ID: AtomicU64 = AtomicU64::new(0);

pub(super) const HISTORY_COVER_HEIGHT: i32 = 100;
pub(super) const HISTORY_COVER_WIDTH: i32 = 72;
const ROW_MARGIN: i32 = 12;

struct HistoryRowWidgets {
    entry: RefCell<Option<HistoryEntry>>,
    source: RefCell<Option<PathBuf>>,
    title: gtk::Label,
    document: gtk::Label,
    status: gtk::Label,
    picture: gtk::Picture,
    placeholder: gtk::Box,
    failure_icon: gtk::Image,
    context_popover: gtk::Popover,
}

#[derive(Debug)]
pub(super) enum HistoryCoverJob {
    LoadCache {
        generation: u64,
        source: PathBuf,
    },
    Generate {
        generation: u64,
        source: PathBuf,
        cancel: CoverCancel,
    },
}

impl From<CoverSchedulerJob<PathBuf, PathBuf>> for HistoryCoverJob {
    fn from(job: CoverSchedulerJob<PathBuf, PathBuf>) -> Self {
        match job {
            CoverSchedulerJob::LoadCache { generation, key } => Self::LoadCache {
                generation,
                source: key,
            },
            CoverSchedulerJob::Generate {
                generation,
                key: _,
                payload,
                cancel,
            } => Self::Generate {
                generation,
                source: payload,
                cancel,
            },
        }
    }
}

#[derive(Default)]
pub(super) struct HistoryView {
    items: Option<gtk::Box>,
    viewport: Option<gtk::ScrolledWindow>,
    model: Option<gtk::gio::ListStore>,
    list: Option<gtk::ListView>,
    scheduler: CoverLoadScheduler<PathBuf, PathBuf>,
    rows: Rc<RefCell<HashMap<u64, Rc<HistoryRowWidgets>>>>,
}

impl HistoryView {
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

    pub(super) fn invalidate_cover_textures(&mut self) -> Vec<HistoryCoverJob> {
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

    pub(super) fn resume(&mut self) -> Vec<HistoryCoverJob> {
        self.scheduler
            .resume()
            .into_iter()
            .map(Into::into)
            .collect()
    }

    pub(super) fn render(
        &mut self,
        entries: &[HistoryEntry],
        on_resume: Rc<dyn Fn(HistoryIdentity, PathBuf, usize, bool)>,
        on_remove: Rc<dyn Fn(HistoryIdentity)>,
        sender: AppSender,
    ) -> Vec<HistoryCoverJob> {
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
                "document-open-recent-symbolic",
                "閲覧履歴はありません",
                "漫画を開くと、\nここに最近読んだ作品が表示されます",
            ));
            return Vec::new();
        }

        if let Some(model) = self.model.clone() {
            let current = boxed_list_model_entries::<HistoryEntry>(&model);
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
        let factory = history_row_factory(self.rows.clone(), on_resume, on_remove, sender);
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

    pub(super) fn demand_changed(&mut self) -> Vec<HistoryCoverJob> {
        let demand = self
            .rows
            .borrow()
            .values()
            .filter_map(|row| row.source.borrow().clone())
            .collect::<Vec<_>>();
        self.scheduler.replace_demand(demand);
        self.schedule_jobs()
    }

    pub(super) fn cache_finished(
        &mut self,
        generation: u64,
        source: PathBuf,
        thumbnail: Option<ThumbnailData>,
    ) -> Vec<HistoryCoverJob> {
        if !self.scheduler.cache_finished(generation, &source) {
            return self.schedule_jobs();
        }
        match thumbnail.and_then(texture_from_thumbnail) {
            Some(texture) => {
                self.show_texture(&source, &texture);
                self.scheduler.mark_resolved(&source);
            }
            None => self.scheduler.enqueue_generation(source.clone(), source),
        }
        self.schedule_jobs()
    }

    pub(super) fn generation_finished(
        &mut self,
        generation: u64,
        source: PathBuf,
        result: Result<Option<ThumbnailData>, String>,
    ) -> Vec<HistoryCoverJob> {
        if !self.scheduler.generation_finished(generation, &source) {
            return self.schedule_jobs();
        }
        if matches!(result, Ok(None)) {
            return self.schedule_jobs();
        }
        match result.ok().flatten().and_then(texture_from_thumbnail) {
            Some(texture) => self.show_texture(&source, &texture),
            None => self.show_failure(&source),
        }
        self.scheduler.mark_resolved(&source);
        self.schedule_jobs()
    }

    fn schedule_jobs(&mut self) -> Vec<HistoryCoverJob> {
        self.scheduler
            .schedule_jobs()
            .into_iter()
            .map(Into::into)
            .collect()
    }

    fn show_texture(&self, source: &Path, texture: &gtk::gdk::Texture) {
        for row in self.rows.borrow().values() {
            if binding_matches(row.source.borrow().as_deref(), source) {
                row.picture.set_paintable(Some(texture));
                row.failure_icon.set_visible(false);
                row.placeholder.set_visible(false);
            }
        }
    }

    fn show_failure(&self, source: &Path) {
        for row in self.rows.borrow().values() {
            if binding_matches(row.source.borrow().as_deref(), source) {
                row.picture.set_paintable(None::<&gtk::gdk::Paintable>);
                row.failure_icon.set_visible(true);
                row.placeholder.set_visible(true);
            }
        }
    }

    fn cleanup_realized_rows(&mut self) {
        let realized_rows = self.rows.borrow().values().cloned().collect::<Vec<_>>();
        for row in &realized_rows {
            cleanup_history_row(row);
        }
        self.rows.borrow_mut().clear();
    }
}

fn binding_matches(bound: Option<&Path>, completed: &Path) -> bool {
    bound == Some(completed)
}

fn history_cover_source(entry: &HistoryEntry) -> PathBuf {
    if crate::archive::is_image_ext(&entry.last_document_path)
        && !matches!(
            &entry.identity,
            HistoryIdentity::Document(path) if path == &entry.last_document_path
        )
        && let Some(folder) = entry.last_document_path.parent()
        && let Some(cover) = crate::archive::cover_image_in_folder(folder)
    {
        return cover;
    }
    entry.last_document_path.clone()
}

fn history_row_factory(
    rows: Rc<RefCell<HashMap<u64, Rc<HistoryRowWidgets>>>>,
    on_resume: Rc<dyn Fn(HistoryIdentity, PathBuf, usize, bool)>,
    on_remove: Rc<dyn Fn(HistoryIdentity)>,
    sender: AppSender,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup({
        let rows = rows.clone();
        move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let id = NEXT_HISTORY_ROW_ID.fetch_add(1, Ordering::Relaxed);
            let (card, row) = history_row_widgets(on_resume.clone(), on_remove.clone());
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
            let Some((_, row)) = factory_row(item, &rows) else {
                return;
            };
            let Some(object) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let entry = object.borrow::<HistoryEntry>().clone();
            row.title.set_label(&entry.title);
            let document = history_document_name(&entry);
            row.document.set_label(document.as_deref().unwrap_or(""));
            row.document.set_visible(document.is_some());
            row.status.set_label(&history_status_text(&entry));
            row.picture.set_paintable(None::<&gtk::gdk::Paintable>);
            row.failure_icon.set_visible(false);
            row.placeholder.set_visible(true);
            *row.source.borrow_mut() = Some(history_cover_source(&entry));
            *row.entry.borrow_mut() = Some(entry);
            sender.input(Msg::HistoryCoverDemandChanged);
        }
    });
    factory.connect_unbind({
        let rows = rows.clone();
        move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some((_, row)) = factory_row(item, &rows) else {
                return;
            };
            cleanup_history_row(&row);
            sender.input(Msg::HistoryCoverDemandChanged);
        }
    });
    factory.connect_teardown({
        move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            if let Some((id, row)) = factory_row(item, &rows) {
                cleanup_history_row(&row);
                rows.borrow_mut().remove(&id);
            }
            item.set_child(None::<&gtk::Widget>);
        }
    });
    factory
}

fn cleanup_history_row(row: &HistoryRowWidgets) {
    detach_navigation_popover(&row.context_popover);
    row.picture.set_paintable(None::<&gtk::gdk::Paintable>);
    row.failure_icon.set_visible(false);
    row.placeholder.set_visible(true);
    *row.entry.borrow_mut() = None;
    *row.source.borrow_mut() = None;
}

fn factory_row(
    item: &gtk::ListItem,
    rows: &RefCell<HashMap<u64, Rc<HistoryRowWidgets>>>,
) -> Option<(u64, Rc<HistoryRowWidgets>)> {
    let id = item.child()?.widget_name().parse().ok()?;
    let row = rows.borrow().get(&id)?.clone();
    Some((id, row))
}

fn history_row_widgets(
    on_resume: Rc<dyn Fn(HistoryIdentity, PathBuf, usize, bool)>,
    on_remove: Rc<dyn Fn(HistoryIdentity)>,
) -> (gtk::Button, Rc<HistoryRowWidgets>) {
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
                .width_request(HISTORY_COVER_WIDTH)
                .height_request(HISTORY_COVER_HEIGHT)
                .build(),
        )
        .width_request(HISTORY_COVER_WIDTH)
        .height_request(HISTORY_COVER_HEIGHT)
        .halign(gtk::Align::Start)
        .build();
    cover.add_overlay(&placeholder);
    cover.add_overlay(&picture);

    let labels = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(3)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    let title = gtk::Label::builder()
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(28)
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["heading"])
        .build();
    let document = gtk::Label::builder()
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .max_width_chars(28)
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["caption", "dim-label"])
        .build();
    let status = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["caption", "dim-label"])
        .build();
    labels.append(&title);
    labels.append(&document);
    labels.append(&status);
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
        .label("履歴から削除")
        .css_classes(["flat"])
        .margin_top(4)
        .margin_bottom(4)
        .margin_start(4)
        .margin_end(4)
        .build();
    popover.set_child(Some(&remove_button));
    let row = Rc::new(HistoryRowWidgets {
        entry: RefCell::new(None),
        source: RefCell::new(None),
        title,
        document,
        status,
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
            let entry = row.entry.borrow();
            let Some(entry) = entry.as_ref() else {
                return;
            };
            on_resume(
                entry.identity.clone(),
                entry.last_document_path.clone(),
                entry.page_index,
                entry.at_document_end,
            );
        }
    });
    remove_button.connect_clicked({
        let row = Rc::downgrade(&row);
        move |_| {
            let Some(row) = row.upgrade() else {
                return;
            };
            let identity = row
                .entry
                .borrow()
                .as_ref()
                .map(|entry| entry.identity.clone());
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

pub(super) fn progress_text(page_index: usize, page_count: usize) -> String {
    let page = if page_count == 0 {
        0
    } else {
        page_index.saturating_add(1).min(page_count)
    };
    format!("{page} / {page_count}ページ")
}

fn history_status_text(entry: &HistoryEntry) -> String {
    format!(
        "{} · {}",
        progress_text(entry.page_index, entry.page_count),
        format_history_time(entry.last_viewed_unix_ms)
    )
}

#[cfg(test)]
fn history_status_text_at(entry: &HistoryEntry, now_ms: u64) -> String {
    format!(
        "{} · {}",
        progress_text(entry.page_index, entry.page_count),
        format_history_time_at(entry.last_viewed_unix_ms, now_ms)
    )
}

fn history_document_name(entry: &HistoryEntry) -> Option<String> {
    if matches!(entry.identity, HistoryIdentity::Document(_)) {
        return None;
    }
    let document_path = if crate::archive::is_image_ext(&entry.last_document_path) {
        entry.last_document_path.parent()?
    } else {
        &entry.last_document_path
    };
    let name = document_path
        .file_name()
        .filter(|name| !name.is_empty())?
        .to_string_lossy()
        .into_owned();
    if name == entry.title
        || entry.work_path.as_deref() == Some(document_path)
        || matches!(&entry.identity, HistoryIdentity::BookshelfWork(path) if path == document_path)
    {
        None
    } else {
        Some(name)
    }
}

pub(super) fn format_history_time(timestamp_ms: u64) -> String {
    let now_ms = glib::DateTime::now_local()
        .map(|now| {
            u64::try_from(now.to_unix())
                .unwrap_or(0)
                .saturating_mul(1000)
        })
        .unwrap_or(0);
    format_history_time_at(timestamp_ms, now_ms)
}

fn format_history_time_at(timestamp_ms: u64, now_ms: u64) -> String {
    let timestamp_seconds = i64::try_from(timestamp_ms / 1000).unwrap_or(i64::MAX);
    let now_seconds = i64::try_from(now_ms / 1000).unwrap_or(i64::MAX);
    let (Ok(timestamp), Ok(now)) = (
        glib::DateTime::from_unix_local(timestamp_seconds),
        glib::DateTime::from_unix_local(now_seconds),
    ) else {
        return "日時不明".into();
    };
    let is_today = timestamp.year() == now.year()
        && timestamp.month() == now.month()
        && timestamp.day_of_month() == now.day_of_month();
    let format = if is_today {
        "今日 %H:%M"
    } else {
        "%Y/%m/%d %H:%M"
    };
    timestamp
        .format(format)
        .map(|value| value.to_string())
        .unwrap_or_else(|_| "日時不明".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_keeps_pending_cover_state_until_resume() {
        let mut view = HistoryView::default();
        view.scheduler.replace_demand([PathBuf::from("/book.cbz")]);
        let generation = view.scheduler.generation();

        view.pause();
        assert!(view.schedule_jobs().is_empty());
        assert_eq!(view.scheduler.generation(), generation);
        assert_eq!(view.scheduler.pending_cache_len(), 1);

        let jobs = view.resume();
        assert_eq!(jobs.len(), 1);
        assert!(matches!(
            &jobs[0],
            HistoryCoverJob::LoadCache {
                generation: job_generation,
                source,
            } if *job_generation == generation && source == Path::new("/book.cbz")
        ));
    }

    #[test]
    fn cover_override_invalidates_visible_cover_jobs() {
        let mut view = HistoryView::default();
        let source = PathBuf::from("/book.cbz");
        view.scheduler.replace_demand([source.clone()]);
        let old_generation = view.scheduler.generation();
        assert_eq!(view.schedule_jobs().len(), 1);

        assert!(view.invalidate_cover_textures().is_empty());
        assert_eq!(view.scheduler.generation(), old_generation.wrapping_add(1));
        view.scheduler.replace_demand([source.clone()]);
        assert!(view.schedule_jobs().is_empty());
        assert!(!view.scheduler.cache_finished(old_generation, &source));
        assert_eq!(view.schedule_jobs().len(), 1);
    }

    #[test]
    fn recycled_binding_accepts_only_its_current_source() {
        assert!(binding_matches(
            Some(Path::new("/current.cbz")),
            Path::new("/current.cbz")
        ));
        assert!(!binding_matches(
            Some(Path::new("/replacement.cbz")),
            Path::new("/old.cbz")
        ));
        assert!(!binding_matches(None, Path::new("/old.cbz")));
    }

    #[test]
    fn image_book_history_uses_folder_cover_but_direct_image_does_not() {
        let directory = tempfile::tempdir().unwrap();
        let image = directory.path().join("001.png");
        let cover = directory.path().join("Cover.PNG");
        std::fs::write(&image, b"image").unwrap();
        std::fs::write(&cover, b"cover").unwrap();
        let image_book = HistoryEntry {
            identity: HistoryIdentity::BookshelfWork(directory.path().to_path_buf()),
            work_path: Some(directory.path().to_path_buf()),
            title: "book".into(),
            last_document_path: image.clone(),
            page_index: 0,
            page_count: 1,
            at_document_end: false,
            last_viewed_unix_ms: 0,
        };
        let mut direct = image_book.clone();
        direct.identity = HistoryIdentity::Document(image.clone());

        assert_eq!(history_cover_source(&image_book), cover);
        assert_eq!(history_cover_source(&direct), image);
    }

    fn local_timestamp(year: i32, month: i32, day: i32, hour: i32, minute: i32) -> u64 {
        let date = glib::DateTime::new(
            &glib::TimeZone::local(),
            year,
            month,
            day,
            hour,
            minute,
            0.0,
        )
        .unwrap();
        u64::try_from(date.to_unix()).unwrap() * 1000
    }

    #[test]
    fn page_progress_is_one_based_and_bounds_safe() {
        assert_eq!(progress_text(0, 242), "1 / 242ページ");
        assert_eq!(progress_text(127, 242), "128 / 242ページ");
        assert_eq!(progress_text(999, 242), "242 / 242ページ");
        assert_eq!(progress_text(usize::MAX, 0), "0 / 0ページ");
    }

    fn entry(
        identity: HistoryIdentity,
        work_path: Option<&str>,
        title: &str,
        document: &str,
    ) -> HistoryEntry {
        HistoryEntry {
            identity,
            work_path: work_path.map(PathBuf::from),
            title: title.into(),
            last_document_path: PathBuf::from(document),
            page_index: 23,
            page_count: 203,
            at_document_end: false,
            last_viewed_unix_ms: 0,
        }
    }

    #[test]
    fn history_display_separates_the_last_archive_document_and_status() {
        let entry = entry(
            HistoryIdentity::BookshelfWork(PathBuf::from("/本棚/ブラック・ジャック")),
            Some("/本棚/ブラック・ジャック"),
            "ブラック・ジャック",
            "/本棚/ブラック・ジャック/03巻.rar",
        );
        let now = local_timestamp(2026, 8, 13, 23, 59);
        let mut entry = entry;
        entry.last_viewed_unix_ms = local_timestamp(2026, 8, 13, 13, 50);
        assert_eq!(history_document_name(&entry), Some("03巻.rar".into()));
        assert_eq!(
            history_status_text_at(&entry, now),
            "24 / 203ページ · 今日 13:50"
        );
    }

    #[test]
    fn history_display_uses_image_book_not_image_file() {
        let entry = entry(
            HistoryIdentity::BookshelfWork(PathBuf::from("/本棚/ブラック・ジャック")),
            Some("/本棚/ブラック・ジャック"),
            "ブラック・ジャック",
            "/本棚/ブラック・ジャック/03巻/001.jpg",
        );
        assert_eq!(history_document_name(&entry), Some("03巻".into()));
    }

    #[test]
    fn history_display_avoids_repeating_single_documents() {
        let image_work = entry(
            HistoryIdentity::BookshelfWork(PathBuf::from("/本棚/作品")),
            Some("/本棚/作品"),
            "作品",
            "/本棚/作品/001.jpg",
        );
        let standalone = entry(
            HistoryIdentity::Document(PathBuf::from("/本棚/単独作品.zip")),
            None,
            "単独作品",
            "/本棚/単独作品.zip",
        );
        assert_eq!(history_document_name(&image_work), None);
        assert_eq!(history_document_name(&standalone), None);
    }

    #[test]
    fn same_local_date_uses_today_format() {
        let timestamp = local_timestamp(2026, 8, 13, 20, 32);
        let now = local_timestamp(2026, 8, 13, 23, 59);
        assert_eq!(format_history_time_at(timestamp, now), "今日 20:32");
    }

    #[test]
    fn older_local_date_includes_date() {
        let timestamp = local_timestamp(2026, 8, 10, 20, 32);
        let now = local_timestamp(2026, 8, 13, 1, 0);
        assert_eq!(format_history_time_at(timestamp, now), "2026/08/10 20:32");
    }
}
