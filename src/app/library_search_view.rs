use super::library_search::{LibrarySearchState, location_text};
use super::library_search_thumbnail::SearchThumbnailDemandEvaluation;
use super::thumbnail_texture::texture_from_thumbnail;
use super::{AppSender, Msg};
use crate::bookshelf::search::{SearchArchive, SearchFolder, SearchItem};
use crate::bookshelf::thumbnail::ThumbnailData;
use adw::prelude::*;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

const SEARCH_SECTION_HORIZONTAL_MARGIN: i32 = 6;
pub(super) const SEARCH_ROW_THUMBNAIL_HEIGHT: i32 = 100;
pub(super) const SEARCH_ARCHIVE_COVER_WIDTH: i32 = 72;
const SEARCH_TEXTURE_CACHE_BUDGET: usize = 8 * 1024 * 1024;

struct TextureCacheEntry<T> {
    texture: T,
    bytes: usize,
    recency: u64,
}

struct SearchTextureLru<T> {
    entries: HashMap<PathBuf, TextureCacheEntry<T>>,
    total_bytes: usize,
    next_recency: u64,
    budget: usize,
}

impl<T> SearchTextureLru<T> {
    fn new(budget: usize) -> Self {
        Self {
            entries: HashMap::new(),
            total_bytes: 0,
            next_recency: 0,
            budget,
        }
    }

    fn get(&self, source: &Path) -> Option<&T> {
        self.entries.get(source).map(|entry| &entry.texture)
    }

    fn contains(&self, source: &Path) -> bool {
        self.entries.contains_key(source)
    }

    fn insert(&mut self, source: PathBuf, texture: T, bytes: usize) {
        if let Some(previous) = self.entries.remove(&source) {
            self.total_bytes = self.total_bytes.saturating_sub(previous.bytes);
        }
        let recency = self.bump_recency();
        self.total_bytes = self.total_bytes.saturating_add(bytes);
        self.entries.insert(
            source,
            TextureCacheEntry {
                texture,
                bytes,
                recency,
            },
        );
    }

    fn touch(&mut self, source: &Path) {
        let recency = self.bump_recency();
        if let Some(entry) = self.entries.get_mut(source) {
            entry.recency = recency;
        }
    }

    fn trim(&mut self, pinned: &HashSet<PathBuf>) -> Vec<PathBuf> {
        let mut removed = Vec::new();
        while self.total_bytes > self.budget {
            let candidate = self
                .entries
                .iter()
                .filter(|(source, _)| !pinned.contains(*source))
                .min_by_key(|(_, entry)| entry.recency)
                .map(|(source, _)| source.clone());
            let Some(source) = candidate else {
                break;
            };
            self.remove(&source);
            removed.push(source);
        }
        removed
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.total_bytes = 0;
        self.next_recency = 0;
    }

    fn remove(&mut self, source: &Path) {
        if let Some(entry) = self.entries.remove(source) {
            self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
        }
    }

    fn bump_recency(&mut self) -> u64 {
        self.next_recency = self.next_recency.saturating_add(1);
        self.next_recency
    }
}

impl<T> Default for SearchTextureLru<T> {
    fn default() -> Self {
        Self::new(SEARCH_TEXTURE_CACHE_BUDGET)
    }
}

struct CoverTarget {
    picture: gtk::Picture,
    placeholder: gtk::Box,
    failure_icon: Option<gtk::Image>,
}

struct ResultRowTarget {
    row: gtk::Button,
    sources: Vec<PathBuf>,
}

#[derive(Default)]
struct SearchScrollState {
    position: Cell<f64>,
    restore_pending: Cell<bool>,
}

#[derive(Default)]
pub(super) struct LibrarySearchView {
    items: Option<gtk::Box>,
    viewport: Option<gtk::ScrolledWindow>,
    generation: u64,
    targets: HashMap<PathBuf, Vec<CoverTarget>>,
    rows: Vec<ResultRowTarget>,
    texture_cache: SearchTextureLru<gtk::gdk::Texture>,
    failed_sources: HashSet<PathBuf>,
    scroll_state: Rc<SearchScrollState>,
}

impl LibrarySearchView {
    pub(super) fn invalidate_cover_textures(&mut self) {
        self.texture_cache.clear();
        self.failed_sources.clear();
        for targets in self.targets.values() {
            for target in targets {
                target.picture.set_paintable(None::<&gtk::gdk::Paintable>);
                if let Some(icon) = &target.failure_icon {
                    icon.set_visible(false);
                }
                target.placeholder.set_visible(true);
            }
        }
    }

    pub(super) fn attach(
        &mut self,
        items: gtk::Box,
        viewport: gtk::ScrolledWindow,
        sender: AppSender,
    ) {
        viewport.connect_notify_local(Some("width"), {
            let sender = sender.clone();
            move |_, _| sender.input(Msg::LibrarySearchThumbnailViewportChanged)
        });
        viewport.connect_notify_local(Some("height"), {
            let sender = sender.clone();
            move |_, _| sender.input(Msg::LibrarySearchThumbnailViewportChanged)
        });
        viewport.connect_map({
            let scroll_state = self.scroll_state.clone();
            move |viewport| {
                restore_pending_scroll_position(&viewport.vadjustment(), &scroll_state);
            }
        });
        let adjustment = viewport.vadjustment();
        adjustment.connect_value_changed({
            let sender = sender.clone();
            move |_| sender.input(Msg::LibrarySearchThumbnailViewportChanged)
        });
        adjustment.connect_changed({
            let scroll_state = self.scroll_state.clone();
            move |adjustment| {
                restore_pending_scroll_position(adjustment, &scroll_state);
                sender.input(Msg::LibrarySearchThumbnailViewportChanged);
            }
        });
        self.items = Some(items);
        self.viewport = Some(viewport);
    }

    pub(super) fn reset_session(&mut self, generation: u64) {
        self.generation = generation;
        self.clear_rows();
        self.texture_cache.clear();
        self.failed_sources.clear();
        self.reset_scroll_position();
    }

    pub(super) fn pause(&mut self) {
        if let Some(viewport) = &self.viewport {
            self.scroll_state
                .position
                .set(viewport.vadjustment().value());
        }
        self.scroll_state.restore_pending.set(false);
    }

    pub(super) fn resume(&self) {
        self.scroll_state.restore_pending.set(true);
    }

    pub(super) fn render(
        &mut self,
        state: &LibrarySearchState,
        generation: u64,
        sender: &AppSender,
    ) {
        self.generation = generation;
        self.reset_scroll_position();
        self.clear_rows();
        let Some(items) = self.items.clone() else {
            return;
        };
        let presentation = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .margin_top(12)
            .margin_bottom(24)
            .build();
        items.append(&presentation);
        if !state.has_meaningful_query() {
            self.request_thumbnail_evaluation(sender);
            return;
        }
        if state.is_building() {
            presentation.append(&status("本棚を検索しています…", true));
            self.request_thumbnail_evaluation(sender);
            return;
        }
        if state.error().is_some() {
            presentation.append(&status("本棚を検索できません", false));
            self.request_thumbnail_evaluation(sender);
            return;
        }
        let Some(results) = state.results() else {
            self.request_thumbnail_evaluation(sender);
            return;
        };
        if results.is_empty() {
            presentation.append(&status("該当する項目はありません", false));
            self.request_thumbnail_evaluation(sender);
            return;
        }

        let folders = results
            .iter()
            .filter_map(|item| match item {
                SearchItem::Folder(folder) => Some(folder),
                SearchItem::Archive(_) => None,
            })
            .collect::<Vec<_>>();
        if !folders.is_empty() {
            let section = search_section("フォルダ", false);
            let list = result_list();
            for folder in folders.iter().copied() {
                let sources = vec![folder.cover_source.clone()];
                let row = self.folder_row(folder, sender);
                self.rows.push(ResultRowTarget {
                    row: row.clone(),
                    sources,
                });
                list.append(&row);
            }
            section.append(&list);
            presentation.append(&section);
        }

        let archives = results
            .iter()
            .filter_map(|item| match item {
                SearchItem::Folder(_) => None,
                SearchItem::Archive(archive) => Some(archive),
            })
            .collect::<Vec<_>>();
        if !archives.is_empty() {
            let section = search_section("ファイル", true);
            let list = result_list();
            for archive in archives {
                let row = self.archive_row(archive, sender);
                self.rows.push(ResultRowTarget {
                    row: row.clone(),
                    sources: vec![archive.path.clone()],
                });
                list.append(&row);
            }
            section.append(&list);
            presentation.append(&section);
        }
        self.request_thumbnail_evaluation(sender);
    }

    pub(super) fn visible_thumbnail_sources(&mut self) -> SearchThumbnailDemandEvaluation {
        let Ok(visible_sources) = self.visible_row_sources() else {
            return SearchThumbnailDemandEvaluation::AllocationPending;
        };
        for source in &visible_sources {
            self.texture_cache.touch(source);
        }
        let evicted = self.texture_cache.trim(&visible_sources);
        self.release_evicted_targets(&evicted);
        SearchThumbnailDemandEvaluation::Ready(
            visible_sources
                .into_iter()
                .filter(|source| {
                    !self.texture_cache.contains(source) && !self.failed_sources.contains(source)
                })
                .collect(),
        )
    }

    fn visible_row_sources(&self) -> Result<HashSet<PathBuf>, ()> {
        let (Some(items), Some(viewport)) = (&self.items, &self.viewport) else {
            return Err(());
        };
        let adjustment = viewport.vadjustment();
        let visible_top = adjustment.value() as f32;
        let page_size = if adjustment.page_size() > 0.0 {
            adjustment.page_size()
        } else {
            f64::from(viewport.height().max(0))
        } as f32;
        if page_size <= 0.0 {
            return Err(());
        }
        let visible_bottom = visible_top + page_size;
        let mut sources = HashSet::new();
        for target in &self.rows {
            let Some(bounds) = target.row.compute_bounds(items) else {
                return Err(());
            };
            if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
                return Err(());
            }
            let top = bounds.y();
            let bottom = top + bounds.height();
            if bottom > visible_top && top < visible_bottom {
                sources.extend(target.sources.iter().cloned());
            }
        }
        Ok(sources)
    }

    pub(super) fn show_thumbnail(
        &mut self,
        generation: u64,
        source: &Path,
        thumbnail: ThumbnailData,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        let bytes = thumbnail.pixels.len();
        let Some(texture) = texture_from_thumbnail(thumbnail) else {
            return false;
        };
        self.failed_sources.remove(source);
        self.texture_cache
            .insert(source.to_path_buf(), texture.clone(), bytes);
        if let Some(targets) = self.targets.get(source) {
            for target in targets {
                target.picture.set_paintable(Some(&texture));
                if let Some(icon) = &target.failure_icon {
                    icon.set_visible(false);
                }
                target.placeholder.set_visible(false);
            }
        }
        let pinned = self
            .visible_row_sources()
            .unwrap_or_else(|_| HashSet::from([source.to_path_buf()]));
        for visible in &pinned {
            self.texture_cache.touch(visible);
        }
        let evicted = self.texture_cache.trim(&pinned);
        self.release_evicted_targets(&evicted);
        true
    }

    pub(super) fn show_failure(&mut self, generation: u64, source: &Path) -> bool {
        if generation != self.generation {
            return false;
        }
        self.failed_sources.insert(source.to_path_buf());
        if let Some(targets) = self.targets.get(source) {
            for target in targets {
                target.picture.set_paintable(None::<&gtk::gdk::Paintable>);
                if let Some(icon) = &target.failure_icon {
                    icon.set_visible(true);
                }
                target.placeholder.set_visible(true);
            }
        }
        true
    }

    fn clear_rows(&mut self) {
        self.targets.clear();
        self.rows.clear();
        let Some(items) = &self.items else {
            return;
        };
        while let Some(child) = items.first_child() {
            items.remove(&child);
        }
    }

    fn reset_scroll_position(&mut self) {
        self.scroll_state.position.set(0.0);
        self.scroll_state.restore_pending.set(false);
        if let Some(viewport) = &self.viewport {
            let adjustment = viewport.vadjustment();
            adjustment.set_value(adjustment.lower());
        }
    }

    fn request_thumbnail_evaluation(&self, sender: &AppSender) {
        let idle_sender = sender.clone();
        gtk::glib::idle_add_local_once(move || {
            idle_sender.input(Msg::LibrarySearchThumbnailViewportChanged);
        });
        if let Some(viewport) = &self.viewport {
            let sender = sender.clone();
            viewport.add_tick_callback(move |_, _| {
                sender.input(Msg::LibrarySearchThumbnailViewportChanged);
                gtk::glib::ControlFlow::Break
            });
            viewport.queue_draw();
        }
    }

    fn folder_row(&mut self, folder: &SearchFolder, sender: &AppSender) -> gtk::Button {
        let location = location_text(&folder.parent_relative);
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(10)
            .hexpand(true)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(4)
            .margin_end(4)
            .build();
        content.append(&self.archive_cover(&folder.cover_source));
        content.append(&result_metadata(&folder.display_name, &location));

        let row = gtk::Button::builder()
            .child(&content)
            .hexpand(true)
            .halign(gtk::Align::Fill)
            .css_classes(["flat", "search-result-row"])
            .build();
        row.set_tooltip_text(Some(&format!("{}\n{location}", folder.display_name)));
        let path = folder.path.clone();
        let image_document_entry = folder.image_document_entry.clone();
        let sender = sender.clone();
        row.connect_clicked(move |_| {
            sender.input(Msg::LibrarySearchFolderSelected {
                path: path.clone(),
                image_document_entry: image_document_entry.clone(),
            });
        });
        row
    }

    fn archive_row(&mut self, archive: &SearchArchive, sender: &AppSender) -> gtk::Button {
        let location = location_text(&archive.parent_relative);
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(10)
            .hexpand(true)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(4)
            .margin_end(4)
            .build();
        content.append(&self.archive_cover(&archive.path));
        content.append(&result_metadata(&archive.display_name, &location));
        let row = gtk::Button::builder()
            .child(&content)
            .hexpand(true)
            .halign(gtk::Align::Fill)
            .css_classes(["flat", "search-result-row"])
            .build();
        row.set_tooltip_text(Some(&format!("{}\n{location}", archive.display_name)));
        let path = archive.path.clone();
        let sender = sender.clone();
        row.connect_clicked(move |_| {
            sender.input(Msg::LibrarySearchArchiveSelected(path.clone()));
        });
        row
    }

    fn archive_cover(&mut self, source: &Path) -> gtk::Overlay {
        let (cover, target) = thumbnail_slot(
            SEARCH_ARCHIVE_COVER_WIDTH,
            SEARCH_ROW_THUMBNAIL_HEIGHT,
            Some("text-x-generic-symbolic"),
        );
        self.initialize_target(source, &target);
        self.targets
            .entry(source.to_path_buf())
            .or_default()
            .push(target);
        cover
    }

    fn initialize_target(&self, source: &Path, target: &CoverTarget) {
        if let Some(texture) = self.texture_cache.get(source) {
            target.picture.set_paintable(Some(texture));
            target.placeholder.set_visible(false);
        } else if self.failed_sources.contains(source) {
            if let Some(icon) = &target.failure_icon {
                icon.set_visible(true);
            }
        }
    }

    fn release_evicted_targets(&self, evicted: &[PathBuf]) {
        for source in evicted {
            if let Some(targets) = self.targets.get(source) {
                for target in targets {
                    target.picture.set_paintable(None::<&gtk::gdk::Paintable>);
                    if let Some(icon) = &target.failure_icon {
                        icon.set_visible(false);
                    }
                    target.placeholder.set_visible(true);
                }
            }
        }
    }
}

fn restore_pending_scroll_position(adjustment: &gtk::Adjustment, scroll_state: &SearchScrollState) {
    if !scroll_state.restore_pending.get()
        || adjustment.page_size() <= 0.0
        || adjustment.upper() <= adjustment.lower()
    {
        return;
    }
    adjustment.set_value(clamped_scroll_position(
        scroll_state.position.get(),
        adjustment.lower(),
        adjustment.upper(),
        adjustment.page_size(),
    ));
    scroll_state.restore_pending.set(false);
}

fn clamped_scroll_position(value: f64, lower: f64, upper: f64, page_size: f64) -> f64 {
    if !value.is_finite() {
        return lower;
    }
    value.clamp(lower, (upper - page_size).max(lower))
}

fn thumbnail_slot(
    width: i32,
    height: i32,
    failure_icon_name: Option<&str>,
) -> (gtk::Overlay, CoverTarget) {
    let placeholder = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .halign(gtk::Align::Fill)
        .valign(gtk::Align::Fill)
        .css_classes(["cover-placeholder"])
        .build();
    let failure_icon = failure_icon_name.map(|icon_name| {
        let icon = gtk::Image::from_icon_name(icon_name);
        icon.set_visible(false);
        icon.set_vexpand(true);
        icon.set_valign(gtk::Align::Center);
        placeholder.append(&icon);
        icon
    });
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
                .width_request(width)
                .height_request(height)
                .build(),
        )
        .width_request(width)
        .height_request(height)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    cover.add_overlay(&placeholder);
    cover.add_overlay(&picture);
    (
        cover,
        CoverTarget {
            picture,
            placeholder,
            failure_icon,
        },
    )
}

fn search_section(title: &str, margin_top: bool) -> gtk::Box {
    let section = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .margin_start(SEARCH_SECTION_HORIZONTAL_MARGIN)
        .margin_end(SEARCH_SECTION_HORIZONTAL_MARGIN)
        .build();
    if margin_top {
        section.set_margin_top(12);
    }
    section.append(
        &gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .margin_bottom(4)
            .css_classes(["heading"])
            .build(),
    );
    section
}

fn result_list() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .valign(gtk::Align::Start)
        .build()
}

fn result_metadata(title: &str, location: &str) -> gtk::Box {
    let metadata = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(3)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    let title_label = gtk::Label::builder()
        .label(title)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(1)
        .single_line_mode(true)
        .hexpand(true)
        .xalign(0.0)
        .build();
    title_label.set_tooltip_text(Some(title));
    metadata.append(&title_label);
    let location_label = gtk::Label::builder()
        .label(location)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(1)
        .single_line_mode(true)
        .hexpand(true)
        .xalign(0.0)
        .css_classes(["caption", "dim-label"])
        .build();
    location_label.set_tooltip_text(Some(location));
    metadata.append(&location_label);
    metadata
}

fn status(text: &str, spinning: bool) -> gtk::Box {
    let state = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .vexpand(true)
        .margin_start(24)
        .margin_end(24)
        .build();
    if spinning {
        state.append(&gtk::Spinner::builder().spinning(true).build());
    }
    state.append(
        &gtk::Label::builder()
            .label(text)
            .justify(gtk::Justification::Center)
            .wrap(true)
            .css_classes(["dim-label"])
            .build(),
    );
    state
}

#[cfg(test)]
mod tests {
    use super::{SearchTextureLru, clamped_scroll_position};
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    fn pins(paths: &[&str]) -> HashSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn restored_scroll_position_is_clamped_to_the_current_adjustment_range() {
        assert_eq!(clamped_scroll_position(240.0, 0.0, 1000.0, 300.0), 240.0);
        assert_eq!(clamped_scroll_position(900.0, 0.0, 1000.0, 300.0), 700.0);
        assert_eq!(clamped_scroll_position(50.0, 100.0, 200.0, 150.0), 100.0);
        assert_eq!(clamped_scroll_position(f64::NAN, 10.0, 100.0, 20.0), 10.0);
    }

    #[test]
    fn lru_stays_below_budget_and_evicts_oldest_unpinned() {
        let mut cache = SearchTextureLru::new(10);
        cache.insert(PathBuf::from("a"), (), 4);
        cache.insert(PathBuf::from("b"), (), 4);
        assert!(cache.trim(&HashSet::new()).is_empty());

        cache.insert(PathBuf::from("c"), (), 4);
        assert_eq!(cache.trim(&pins(&["b"])), [PathBuf::from("a")]);
        assert_eq!(cache.total_bytes, 8);
    }

    #[test]
    fn touch_preserves_the_recent_source() {
        let mut cache = SearchTextureLru::new(8);
        cache.insert(PathBuf::from("a"), (), 4);
        cache.insert(PathBuf::from("b"), (), 4);
        cache.touch(Path::new("a"));
        cache.insert(PathBuf::from("c"), (), 4);

        assert_eq!(cache.trim(&HashSet::new()), [PathBuf::from("b")]);
    }

    #[test]
    fn pinned_only_overage_is_a_soft_budget() {
        let mut cache = SearchTextureLru::new(8);
        cache.insert(PathBuf::from("a"), (), 6);
        cache.insert(PathBuf::from("b"), (), 6);
        assert!(cache.trim(&pins(&["a", "b"])).is_empty());
        assert_eq!(cache.total_bytes, 12);
        assert_eq!(cache.trim(&pins(&["a"])), [PathBuf::from("b")]);
    }

    #[test]
    fn replacement_is_accounted_once_and_clear_resets_accounting() {
        let mut cache = SearchTextureLru::new(10);
        cache.insert(PathBuf::from("a"), (), 6);
        cache.insert(PathBuf::from("a"), (), 4);
        assert_eq!(cache.total_bytes, 4);

        cache.clear();
        assert_eq!(cache.total_bytes, 0);
        assert_eq!(cache.next_recency, 0);
        assert!(!cache.contains(Path::new("a")));
    }
}
