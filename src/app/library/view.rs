use super::super::library_thumbnail::{
    ThumbnailDemand, ThumbnailDemandEvaluation, ThumbnailSourceKind, ThumbnailTextureKey,
};
use super::super::thumbnail_texture::texture_from_thumbnail;
use super::super::{AppSender, Msg};
use super::card::{
    COVER_COLUMN_SPACING, COVER_ROW_SPACING, FavoriteTarget, LIBRARY_HORIZONTAL_MARGIN,
    ProgressTarget, ThumbnailTarget, archive_image_cover_popover, direct_file_context_popover,
    direct_file_message, folder_image_cover_popover, folder_message, image_folder_context_popover,
    is_favorite, section_label, show_target_failure, show_target_texture, show_target_unloaded,
};
use super::progress::BookshelfDisplaySizes;
use super::shelf_container::ShelfContainer;
use crate::archive::{ArchiveContentItemKind, ArchiveContentLevel};
use crate::bookshelf::thumbnail::ThumbnailData;
use crate::bookshelf::{BookshelfBook, BookshelfDirectory, LibrarySortDirection, LibrarySortKey};
use crate::covers::CoverBookIdentity;
use crate::favorites::{FavoriteEntry, FavoriteIdentity};
use crate::history::HistoryEntry;
use adw::prelude::*;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

const NORMAL_TEXTURE_CACHE_BUDGET: usize = 64 * 1024 * 1024;
const ARCHIVE_TEXTURE_CACHE_BUDGET: usize = 64 * 1024 * 1024;

struct TextureCacheEntry<T> {
    texture: T,
    bytes: usize,
    recency: u64,
}

struct TextureLru<T> {
    entries: HashMap<ThumbnailTextureKey, TextureCacheEntry<T>>,
    total_bytes: usize,
    next_recency: u64,
    budget: usize,
}

impl<T> TextureLru<T> {
    fn new(budget: usize) -> Self {
        Self {
            entries: HashMap::new(),
            total_bytes: 0,
            next_recency: 0,
            budget,
        }
    }

    fn get(&self, key: &ThumbnailTextureKey) -> Option<&T> {
        self.entries.get(key).map(|entry| &entry.texture)
    }

    fn contains(&self, key: &ThumbnailTextureKey) -> bool {
        self.entries.contains_key(key)
    }

    fn insert(&mut self, key: ThumbnailTextureKey, texture: T, bytes: usize) {
        if let Some(previous) = self.entries.remove(&key) {
            self.total_bytes = self.total_bytes.saturating_sub(previous.bytes);
        }
        let recency = self.bump_recency();
        self.total_bytes = self.total_bytes.saturating_add(bytes);
        self.entries.insert(
            key,
            TextureCacheEntry {
                texture,
                bytes,
                recency,
            },
        );
    }

    fn touch(&mut self, key: &ThumbnailTextureKey) {
        let recency = self.bump_recency();
        if let Some(entry) = self.entries.get_mut(key) {
            entry.recency = recency;
        }
    }

    fn retain_sources(
        &mut self,
        retained: &HashSet<ThumbnailTextureKey>,
    ) -> Vec<ThumbnailTextureKey> {
        let removed = self
            .entries
            .keys()
            .filter(|source| !retained.contains(*source))
            .cloned()
            .collect::<Vec<_>>();
        for source in &removed {
            self.remove(source);
        }
        removed
    }

    fn trim(
        &mut self,
        pinned: &HashSet<ThumbnailTextureKey>,
        associated: &HashSet<ThumbnailTextureKey>,
    ) -> Vec<ThumbnailTextureKey> {
        let mut removed = Vec::new();
        while self.total_bytes > self.budget {
            let candidate = self
                .entries
                .iter()
                .filter(|(source, _)| !pinned.contains(*source))
                .min_by_key(|(source, entry)| (associated.contains(*source), entry.recency))
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

    fn remove(&mut self, key: &ThumbnailTextureKey) {
        if let Some(entry) = self.entries.remove(key) {
            self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
        }
    }

    fn bump_recency(&mut self) -> u64 {
        self.next_recency = self.next_recency.saturating_add(1);
        self.next_recency
    }
}

impl<T> Default for TextureLru<T> {
    fn default() -> Self {
        Self::new(NORMAL_TEXTURE_CACHE_BUDGET)
    }
}

fn associated_sources(
    directories: &HashMap<PathBuf, HashSet<ThumbnailTextureKey>>,
) -> HashSet<ThumbnailTextureKey> {
    directories
        .values()
        .flat_map(|sources| sources.iter().cloned())
        .collect()
}

fn target_texture_key(target: &ThumbnailTarget) -> ThumbnailTextureKey {
    ThumbnailTextureKey::new(target.source.clone(), target.kind)
}

#[derive(Clone)]
struct NormalLibraryShell {
    root: gtk::Box,
    folder_section: gtk::Box,
    folder_wrap: gtk::Box,
    folder_shelf: ShelfContainer,
    book_section: gtk::Box,
    book_flow: gtk::FlowBox,
    book_shelf: ShelfContainer,
}

impl NormalLibraryShell {
    fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 2);
        root.set_valign(gtk::Align::Start);
        let folder_section = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_start(LIBRARY_HORIZONTAL_MARGIN)
            .margin_end(LIBRARY_HORIZONTAL_MARGIN)
            .build();
        let folder_wrap = folder_wrap();
        let folder_shelf = bookshelf_container(&folder_wrap);
        folder_section.append(&folder_shelf);
        folder_section.set_visible(false);
        root.append(&folder_section);

        let book_section = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_start(LIBRARY_HORIZONTAL_MARGIN)
            .margin_end(LIBRARY_HORIZONTAL_MARGIN)
            .build();
        let book_flow = gtk::FlowBox::builder()
            .css_classes(["bookshelf-card-flow"])
            .selection_mode(gtk::SelectionMode::None)
            .column_spacing(COVER_COLUMN_SPACING)
            .row_spacing(COVER_ROW_SPACING)
            .min_children_per_line(1)
            .max_children_per_line(u32::MAX)
            .valign(gtk::Align::Start)
            .build();
        let book_shelf = bookshelf_container(&book_flow);
        book_section.append(&book_shelf);
        book_section.set_visible(false);
        root.append(&book_section);

        Self {
            root,
            folder_section,
            folder_wrap,
            folder_shelf,
            book_section,
            book_flow,
            book_shelf,
        }
    }

    fn clear_cards(&self) {
        self.folder_section.set_visible(false);
        self.book_section.set_visible(false);
        while let Some(child) = self.folder_wrap.first_child() {
            self.folder_wrap.remove(&child);
        }
        while let Some(child) = self.book_flow.first_child() {
            self.book_flow.remove(&child);
        }
        self.folder_shelf.refresh_targets();
        self.book_shelf.refresh_targets();
    }

    fn publish_cards(&self, has_folders: bool, has_books: bool) {
        self.folder_shelf.refresh_targets();
        self.book_shelf.refresh_targets();
        self.book_section
            .set_margin_top(if has_folders { 12 } else { 0 });
        self.folder_section.set_visible(has_folders);
        self.book_section.set_visible(has_books);
    }

    fn has_cards(&self) -> bool {
        self.folder_section.is_visible() || self.book_section.is_visible()
    }
}

pub(in crate::app) struct LibraryView {
    items: Option<gtk::Box>,
    viewport: Option<gtk::ScrolledWindow>,
    normal_shell: Option<NormalLibraryShell>,
    preserve_normal_cards_until_render: bool,
    directory: Option<PathBuf>,
    pub(super) targets: Vec<ThumbnailTarget>,
    progress_targets: Vec<ProgressTarget>,
    favorite_targets: Vec<FavoriteTarget>,
    context_popovers: Vec<gtk::Popover>,
    normal_texture_cache: TextureLru<gtk::gdk::Texture>,
    archive_texture_cache: TextureLru<gtk::gdk::Texture>,
    normal_directory_sources: HashMap<PathBuf, HashSet<ThumbnailTextureKey>>,
    normal_failed_sources: HashSet<ThumbnailTextureKey>,
    archive_failed_sources: HashSet<PathBuf>,
    archive_active: bool,
    archive_backing: Option<Arc<super::super::archive_thumbnail_backing::ArchiveThumbnailBacking>>,
    thumbnail_evaluation_after_paint: Option<Rc<Cell<bool>>>,
}

impl Default for LibraryView {
    fn default() -> Self {
        Self {
            items: None,
            viewport: None,
            normal_shell: None,
            preserve_normal_cards_until_render: false,
            directory: None,
            targets: Vec::new(),
            progress_targets: Vec::new(),
            favorite_targets: Vec::new(),
            context_popovers: Vec::new(),
            normal_texture_cache: TextureLru::new(NORMAL_TEXTURE_CACHE_BUDGET),
            archive_texture_cache: TextureLru::new(ARCHIVE_TEXTURE_CACHE_BUDGET),
            normal_directory_sources: HashMap::new(),
            normal_failed_sources: HashSet::new(),
            archive_failed_sources: HashSet::new(),
            archive_active: false,
            archive_backing: None,
            thumbnail_evaluation_after_paint: None,
        }
    }
}

impl LibraryView {
    pub(in crate::app) fn invalidate_cover_textures(&mut self) {
        self.normal_texture_cache.clear();
        self.archive_texture_cache.clear();
        self.normal_failed_sources.clear();
        self.archive_failed_sources.clear();
        for target in &self.targets {
            show_target_unloaded(target);
        }
    }

    pub(in crate::app) fn reset_normal_cache(&mut self) {
        self.normal_texture_cache.clear();
        self.normal_directory_sources.clear();
        self.normal_failed_sources.clear();
    }

    pub(in crate::app) fn retain_normal_directories(&mut self, retained: &[PathBuf]) {
        let retained = retained.iter().cloned().collect::<HashSet<_>>();
        self.normal_directory_sources
            .retain(|directory, _| retained.contains(directory));
        self.drop_unassociated_normal_sources();
    }

    pub(in crate::app) fn attach(
        &mut self,
        items: gtk::Box,
        viewport: gtk::ScrolledWindow,
        sender: AppSender,
    ) {
        let frame_signal_connected = Rc::new(Cell::new(false));
        let last_reported_size = Rc::new(Cell::new((i32::MIN, i32::MIN)));
        let thumbnail_evaluation_after_paint = Rc::new(Cell::new(false));
        viewport.connect_map({
            let thumbnail_evaluation_after_paint = thumbnail_evaluation_after_paint.clone();
            move |viewport| {
                if frame_signal_connected.get() {
                    return;
                }
                let Some(frame_clock) = viewport.frame_clock() else {
                    return;
                };
                frame_signal_connected.set(true);
                let viewport = viewport.downgrade();
                let sender = sender.clone();
                let last_reported_size = last_reported_size.clone();
                let thumbnail_evaluation_after_paint = thumbnail_evaluation_after_paint.clone();
                frame_clock.connect_after_paint(move |_| {
                    if let Some(viewport) = viewport.upgrade() {
                        let size = (viewport.width(), viewport.height());
                        let size_changed = last_reported_size.replace(size) != size;
                        let evaluation_pending = thumbnail_evaluation_after_paint.replace(false);
                        if size_changed || evaluation_pending {
                            sender.input(Msg::LibraryThumbnailViewportChanged);
                        }
                    }
                });
            }
        });
        let normal_shell = NormalLibraryShell::new();
        items.append(&normal_shell.root);
        self.items = Some(items);
        self.viewport = Some(viewport);
        self.normal_shell = Some(normal_shell);
        self.thumbnail_evaluation_after_paint = Some(thumbnail_evaluation_after_paint);
    }

    pub(in crate::app) fn begin_directory(&mut self, directory: Option<&Path>) {
        self.preserve_normal_cards_until_render = false;
        if let Some(directory) = directory {
            self.archive_active = false;
            self.archive_backing = None;
            self.directory = Some(directory.to_path_buf());
        } else {
            self.archive_active = true;
            self.archive_backing = None;
            self.directory = None;
            self.archive_texture_cache.clear();
            self.archive_failed_sources.clear();
        }
        self.clear();
    }

    pub(in crate::app) fn begin_pending_directory(&mut self, directory: &Path) -> bool {
        let retain_current = !self.archive_active
            && self
                .normal_shell
                .as_ref()
                .is_some_and(NormalLibraryShell::has_cards);
        self.archive_active = false;
        self.archive_backing = None;
        self.directory = Some(directory.to_path_buf());
        self.preserve_normal_cards_until_render = retain_current;
        if retain_current {
            if let Some(viewport) = &self.viewport {
                viewport.set_opacity(1.0);
            }
        } else {
            self.clear();
        }
        retain_current
    }

    pub(in crate::app) fn has_visible_normal_items(&self) -> bool {
        self.normal_shell
            .as_ref()
            .is_some_and(NormalLibraryShell::has_cards)
    }

    pub(in crate::app) fn is_holding_previous_normal_display(&self) -> bool {
        self.preserve_normal_cards_until_render
    }

    pub(in crate::app) fn hide_for_initial_reveal(&self) {
        if let Some(viewport) = &self.viewport {
            // Opacity suppresses drawing without unmapping the widget subtree, so
            // FlowBox and thumbnail slots continue to receive allocations.
            viewport.set_opacity(0.0);
        }
    }

    pub(in crate::app) fn reveal(&self) {
        if let Some(viewport) = &self.viewport {
            viewport.set_opacity(1.0);
        }
    }

    fn clear_current_state(&mut self) {
        for popover in self.context_popovers.drain(..) {
            if popover.parent().is_some() {
                popover.unparent();
            }
        }
        self.targets.clear();
        self.progress_targets.clear();
        self.favorite_targets.clear();
    }

    pub(in crate::app) fn clear(&mut self) {
        self.preserve_normal_cards_until_render = false;
        self.clear_current_state();
        if let Some(shell) = &self.normal_shell {
            shell.clear_cards();
        }
        let Some(items) = &self.items else {
            return;
        };
        if self.archive_active {
            if let Some(shell) = &self.normal_shell {
                if shell.root.parent().is_some() {
                    items.remove(&shell.root);
                }
            }
            while let Some(child) = items.first_child() {
                items.remove(&child);
            }
        } else if let Some(shell) = &self.normal_shell
            && shell.root.parent().is_none()
        {
            while let Some(child) = items.first_child() {
                items.remove(&child);
            }
            items.append(&shell.root);
        }
    }

    pub(in crate::app) fn render(
        &mut self,
        directory: Option<&BookshelfDirectory>,
        history_entries: &[HistoryEntry],
        favorite_entries: &[FavoriteEntry],
        sizes: BookshelfDisplaySizes,
        sort_key: LibrarySortKey,
        sort_direction: LibrarySortDirection,
        replace_preserved_cards: bool,
        sender: &AppSender,
    ) -> bool {
        if self.preserve_normal_cards_until_render && !replace_preserved_cards {
            return false;
        }
        let replaced_preserved_cards =
            replace_preserved_cards && std::mem::take(&mut self.preserve_normal_cards_until_render);
        self.clear_current_state();
        let Some(shell) = self.normal_shell.clone() else {
            return false;
        };
        shell.clear_cards();
        let Some(directory) = directory else {
            shell.publish_cards(false, false);
            self.request_thumbnail_evaluation_after_paint();
            request_visible_thumbnail_evaluation(sender);
            return replaced_preserved_cards;
        };
        if self.directory.as_ref() != Some(&directory.path) {
            self.directory = Some(directory.path.clone());
        }
        self.archive_active = false;
        let has_folders = directory
            .child_shelves
            .iter()
            .any(|shelf| shelf.image_document_entry.is_none());
        if has_folders {
            for shelf in directory
                .sorted_child_shelves(sort_key, sort_direction)
                .into_iter()
                .filter(|shelf| shelf.image_document_entry.is_none())
            {
                let button = self.folder_button(
                    &shelf.path,
                    &shelf.preview_candidates,
                    shelf.preview_item_count,
                    sizes,
                    sender,
                );
                append_folder_card(&shell.folder_wrap, &button);
            }
        }

        let books = if directory.is_image_document_directory {
            let mut books =
                directory.sorted_books(LibrarySortKey::Name, LibrarySortDirection::Ascending);
            books.sort_by(|left, right| {
                let cover = |book: &BookshelfBook<'_>| match book {
                    BookshelfBook::File(path) => crate::archive::is_cover_image(path),
                    BookshelfBook::ImageFolder(_) => false,
                };
                cover(right).cmp(&cover(left))
            });
            books
        } else {
            directory.sorted_books(sort_key, sort_direction)
        };
        let show_book_progress = !directory.is_image_document_directory;
        let has_books = !books.is_empty();
        if has_books {
            for book in books {
                let button = match book {
                    BookshelfBook::File(path) => {
                        let path = path.clone();
                        if directory.is_image_document_directory
                            && crate::archive::is_image_ext(&path)
                        {
                            let cover_only = crate::archive::is_cover_image(&path);
                            let button = self.content_image_button(
                                &path,
                                &path,
                                ThumbnailSourceKind::DirectImage,
                                cover_only,
                                sizes,
                                true,
                            );
                            if !cover_only {
                                button.connect_clicked({
                                    let path = path.clone();
                                    let sender = sender.clone();
                                    move |_| sender.input(direct_file_message(path.clone()))
                                });
                            }
                            let popover = folder_image_cover_popover(
                                &button,
                                directory.path.clone(),
                                path,
                                sender,
                            );
                            self.context_popovers.push(popover);
                            shell.book_flow.insert(&button, -1);
                            continue;
                        }
                        let Some(identity) = FavoriteIdentity::from_document_path(&path) else {
                            continue;
                        };
                        let indicator_enabled = !directory.is_image_document_directory;
                        let (button, favorite_state, favorite_ready) = self.cover_button(
                            &path,
                            &path,
                            if CoverBookIdentity::archive(&path).is_some() {
                                ThumbnailSourceKind::BookCover
                            } else {
                                ThumbnailSourceKind::DirectImage
                            },
                            identity.clone(),
                            history_entries,
                            show_book_progress,
                            is_favorite(favorite_entries, &identity),
                            indicator_enabled,
                            sizes,
                        );
                        button.connect_clicked({
                            let path = path.clone();
                            let sender = sender.clone();
                            move |_| sender.input(direct_file_message(path.clone()))
                        });
                        let (popover, favorite_button) = direct_file_context_popover(
                            &button,
                            path,
                            identity.clone(),
                            is_favorite(favorite_entries, &identity),
                            sender,
                        );
                        self.favorite_targets.push(FavoriteTarget {
                            identity,
                            button: favorite_button,
                            indicator: self
                                .favorite_indicator_for(&button)
                                .expect("book card has favorite indicator"),
                            indicator_enabled,
                            favorite_state,
                            favorite_ready,
                        });
                        self.context_popovers.push(popover);
                        button
                    }
                    BookshelfBook::ImageFolder(shelf) => {
                        let folder = shelf.path.clone();
                        let entry = shelf
                            .image_document_entry
                            .clone()
                            .expect("image folder book has entry");
                        let cover_source = shelf.image_document_cover.as_ref().unwrap_or(&entry);
                        let identity = FavoriteIdentity::image_folder(&folder);
                        let indicator_enabled = !directory.is_image_document_directory;
                        let (button, favorite_state, favorite_ready) = self.cover_button(
                            &folder,
                            cover_source,
                            ThumbnailSourceKind::BookCover,
                            identity.clone(),
                            history_entries,
                            show_book_progress,
                            is_favorite(favorite_entries, &identity),
                            indicator_enabled,
                            sizes,
                        );
                        button.connect_clicked({
                            let folder = folder.clone();
                            let entry = entry.clone();
                            let sender = sender.clone();
                            move |_| {
                                sender.input(folder_message(folder.clone(), Some(entry.clone())))
                            }
                        });
                        let (popover, favorite_button) = image_folder_context_popover(
                            &button,
                            folder,
                            entry,
                            identity.clone(),
                            is_favorite(favorite_entries, &identity),
                            sender,
                        );
                        self.favorite_targets.push(FavoriteTarget {
                            identity,
                            button: favorite_button,
                            indicator: self
                                .favorite_indicator_for(&button)
                                .expect("book card has favorite indicator"),
                            indicator_enabled,
                            favorite_state,
                            favorite_ready,
                        });
                        self.context_popovers.push(popover);
                        button
                    }
                };
                shell.book_flow.insert(&button, -1);
            }
        }

        shell.publish_cards(has_folders, has_books);

        let sources = self
            .targets
            .iter()
            .map(target_texture_key)
            .collect::<HashSet<_>>();
        self.normal_directory_sources
            .insert(directory.path.clone(), sources.clone());
        self.drop_unassociated_normal_sources();
        for key in &sources {
            self.normal_texture_cache.touch(key);
        }

        self.request_thumbnail_evaluation_after_paint();
        request_visible_thumbnail_evaluation(sender);
        replaced_preserved_cards
    }

    pub(in crate::app) fn render_archive(
        &mut self,
        level: Option<&ArchiveContentLevel>,
        backing: Option<Arc<super::super::archive_thumbnail_backing::ArchiveThumbnailBacking>>,
        sizes: BookshelfDisplaySizes,
        sender: &AppSender,
    ) {
        self.clear();
        let (Some(items), Some(level)) = (self.items.clone(), level) else {
            return;
        };
        let directory_key = level
            .location
            .archive
            .join(
                level
                    .location
                    .archives
                    .iter()
                    .fold(PathBuf::new(), |path, part| path.join(part)),
            )
            .join(&level.location.directory);
        if self.directory.as_ref() != Some(&directory_key) {
            self.directory = Some(directory_key);
            self.archive_texture_cache.clear();
            self.archive_failed_sources.clear();
        }
        self.archive_active = true;
        self.archive_backing = backing;
        let section = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_start(LIBRARY_HORIZONTAL_MARGIN)
            .margin_end(LIBRARY_HORIZONTAL_MARGIN)
            .build();
        section.append(&section_label("内容"));
        let flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .column_spacing(COVER_COLUMN_SPACING)
            .row_spacing(COVER_ROW_SPACING)
            .min_children_per_line(1)
            .max_children_per_line(u32::MAX)
            .valign(gtk::Align::Start)
            .build();
        for item in &level.items {
            let button = match item.kind {
                ArchiveContentItemKind::Folder => {
                    let button =
                        self.archive_container_button(&item.path, "folder-symbolic", sizes);
                    button.connect_clicked({
                        let mut location = level.location.clone();
                        location.directory = item.path.clone();
                        let sender = sender.clone();
                        move |_| sender.input(Msg::NavigateArchiveContents(location.clone()))
                    });
                    button
                }
                ArchiveContentItemKind::NestedArchive => {
                    let button = self.archive_container_button(
                        &item.path,
                        "package-x-generic-symbolic",
                        sizes,
                    );
                    button.connect_clicked({
                        let mut location = level.location.clone();
                        location.archives.push(item.path.clone());
                        location.directory = PathBuf::new();
                        let sender = sender.clone();
                        move |_| sender.input(Msg::NavigateArchiveContents(location.clone()))
                    });
                    button
                }
                ArchiveContentItemKind::Image { cover_only } => {
                    let id = level.location.image_id(item.path.clone());
                    let key = super::super::archive_library::ArchiveLibraryState::thumbnail_key(
                        &level.location.archive,
                        &id,
                    );
                    let button = self.content_image_button(
                        &item.path,
                        &key,
                        ThumbnailSourceKind::ArchiveEntry,
                        cover_only,
                        sizes,
                        false,
                    );
                    if !cover_only {
                        button.connect_clicked({
                            let archive = level.location.archive.clone();
                            let id = id.clone();
                            let sender = sender.clone();
                            move |_| {
                                sender.input(Msg::ArchiveImageSelected {
                                    archive: archive.clone(),
                                    id: id.clone(),
                                })
                            }
                        });
                    }
                    let popover = archive_image_cover_popover(
                        &button,
                        level.location.archive.clone(),
                        id,
                        sender,
                    );
                    self.context_popovers.push(popover);
                    button
                }
            };
            flow.insert(&button, -1);
        }
        section.append(&flow);
        items.append(&section);
        self.request_thumbnail_evaluation_after_paint();
        request_visible_thumbnail_evaluation(sender);
    }

    pub(in crate::app) fn visible_thumbnail_demands(&mut self) -> ThumbnailDemandEvaluation {
        if self.preserve_normal_cards_until_render {
            return ThumbnailDemandEvaluation::AllocationPending;
        }
        if self.targets.is_empty() {
            return ThumbnailDemandEvaluation::Ready(Vec::new());
        }
        let Some(visible_keys) = self.visible_texture_keys() else {
            return ThumbnailDemandEvaluation::AllocationPending;
        };
        if self.archive_active {
            for key in &visible_keys {
                self.archive_texture_cache.touch(key);
            }
            self.trim_archive_cache(&visible_keys);
        } else {
            for key in &visible_keys {
                self.normal_texture_cache.touch(key);
            }
            self.trim_normal_cache(&visible_keys);
        }
        let mut demands = Vec::new();
        let mut demanded = HashSet::new();
        for target in &self.targets {
            let cached = if self.archive_active {
                self.archive_texture_cache
                    .contains(&target_texture_key(target))
            } else {
                self.normal_texture_cache
                    .contains(&target_texture_key(target))
            };
            if visible_keys.contains(&target_texture_key(target))
                && !cached
                && demanded.insert(target_texture_key(target))
            {
                demands.push(ThumbnailDemand::new(target.source.clone(), target.kind));
            }
        }
        ThumbnailDemandEvaluation::Ready(demands)
    }

    fn visible_texture_keys(&self) -> Option<HashSet<ThumbnailTextureKey>> {
        self.visible_texture_keys_for(self.targets.iter())
    }

    fn visible_texture_keys_for<'a>(
        &self,
        targets: impl IntoIterator<Item = &'a ThumbnailTarget>,
    ) -> Option<HashSet<ThumbnailTextureKey>> {
        let (Some(items), Some(viewport)) = (&self.items, &self.viewport) else {
            return None;
        };
        let adjustment = viewport.vadjustment();
        let visible_top = adjustment.value() as f32;
        let page_size = if adjustment.page_size() > 0.0 {
            adjustment.page_size()
        } else {
            f64::from(viewport.height().max(0))
        } as f32;
        if page_size <= 0.0 {
            return None;
        }
        let visible_bottom = visible_top + page_size;
        let mut sources = HashSet::new();
        for target in targets {
            let Some(bounds) = target.slot.compute_bounds(items) else {
                return None;
            };
            if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
                return None;
            }
            let top = bounds.y();
            let bottom = top + bounds.height();
            if bottom > visible_top && top < visible_bottom {
                sources.insert(target_texture_key(target));
            }
        }
        Some(sources)
    }

    fn request_thumbnail_evaluation_after_paint(&self) {
        if let Some(pending) = &self.thumbnail_evaluation_after_paint {
            pending.set(true);
        }
        if let Some(viewport) = &self.viewport {
            viewport.queue_draw();
        }
    }

    pub(in crate::app) fn thumbnails_ready(
        &mut self,
        thumbnails: Vec<(ThumbnailTextureKey, ThumbnailData)>,
    ) -> Vec<ThumbnailTextureKey> {
        let mut failed = Vec::new();
        for (key, thumbnail) in thumbnails {
            let bytes = thumbnail.pixels.len();
            let Some(texture) = texture_from_thumbnail(thumbnail) else {
                failed.push(key);
                continue;
            };
            if self.archive_active {
                self.archive_failed_sources.remove(&key.source);
                self.archive_texture_cache
                    .insert(key.clone(), texture.clone(), bytes);
            } else {
                self.normal_failed_sources.remove(&key);
                self.normal_texture_cache
                    .insert(key.clone(), texture.clone(), bytes);
            }
            for target in self
                .targets
                .iter()
                .filter(|target| target_texture_key(target) == key)
            {
                show_target_texture(target, &texture);
            }
        }
        if self.archive_active {
            if let Some(visible_keys) = self.visible_texture_keys() {
                for key in &visible_keys {
                    self.archive_texture_cache.touch(key);
                }
                self.trim_archive_cache(&visible_keys);
            } else {
                self.trim_archive_cache(&HashSet::new());
                self.request_thumbnail_evaluation_after_paint();
            }
        } else {
            self.drop_unassociated_normal_sources();
            if let Some(visible_keys) = self.visible_texture_keys() {
                for key in &visible_keys {
                    self.normal_texture_cache.touch(key);
                }
                self.trim_normal_cache(&visible_keys);
            } else {
                self.request_thumbnail_evaluation_after_paint();
            }
        }
        failed
    }

    pub(in crate::app) fn schedule_cache_hit_flush(
        &self,
        generation: u64,
        sender: &AppSender,
    ) -> bool {
        let Some(viewport) = &self.viewport else {
            return false;
        };
        let sender = sender.clone();
        viewport.add_tick_callback(move |_, _| {
            sender.input(Msg::FlushLibraryThumbnailCacheHits { generation });
            gtk::glib::ControlFlow::Break
        });
        viewport.queue_draw();
        true
    }

    pub(in crate::app) fn thumbnail_failed(&mut self, key: &ThumbnailTextureKey) {
        if self.archive_active {
            self.archive_failed_sources.insert(key.source.clone());
        } else if self
            .normal_directory_sources
            .values()
            .any(|sources| sources.contains(key))
        {
            self.normal_failed_sources.insert(key.clone());
        }
        for target in self
            .targets
            .iter()
            .filter(|target| target_texture_key(target) == *key)
        {
            show_target_failure(target);
        }
    }

    pub(super) fn cached_texture(
        &self,
        path: &Path,
        kind: ThumbnailSourceKind,
    ) -> Option<&gtk::gdk::Texture> {
        if self.archive_active {
            self.archive_texture_cache
                .get(&ThumbnailTextureKey::new(path.to_path_buf(), kind))
        } else {
            self.normal_texture_cache
                .get(&ThumbnailTextureKey::new(path.to_path_buf(), kind))
        }
    }

    pub(super) fn source_failed(&self, path: &Path, kind: ThumbnailSourceKind) -> bool {
        if self.archive_active {
            self.archive_failed_sources.contains(path)
        } else {
            self.normal_failed_sources
                .contains(&ThumbnailTextureKey::new(path.to_path_buf(), kind))
        }
    }

    fn associated_normal_sources(&self) -> HashSet<ThumbnailTextureKey> {
        let mut associated = associated_sources(&self.normal_directory_sources);
        if self.preserve_normal_cards_until_render {
            associated.extend(self.targets.iter().map(target_texture_key));
        }
        associated
    }

    fn drop_unassociated_normal_sources(&mut self) {
        let associated = self.associated_normal_sources();
        let removed = self.normal_texture_cache.retain_sources(&associated);
        self.normal_failed_sources
            .retain(|source| associated.contains(source));
        self.release_evicted_targets(&removed, &HashSet::new());
    }

    fn trim_normal_cache(&mut self, visible_sources: &HashSet<ThumbnailTextureKey>) {
        let associated = self.associated_normal_sources();
        let removed = self.normal_texture_cache.trim(visible_sources, &associated);
        self.release_evicted_targets(&removed, visible_sources);
    }

    fn trim_archive_cache(&mut self, visible_sources: &HashSet<ThumbnailTextureKey>) {
        if !self
            .archive_backing
            .as_ref()
            .is_some_and(|backing| backing.is_usable())
        {
            return;
        }
        let associated = self
            .targets
            .iter()
            .map(target_texture_key)
            .collect::<HashSet<_>>();
        let removed = self
            .archive_texture_cache
            .trim(visible_sources, &associated);
        self.release_evicted_targets(&removed, visible_sources);
    }

    fn release_evicted_targets(
        &self,
        evicted: &[ThumbnailTextureKey],
        visible_sources: &HashSet<ThumbnailTextureKey>,
    ) {
        if evicted.is_empty() {
            return;
        }
        let evicted = evicted.iter().collect::<HashSet<_>>();
        for target in &self.targets {
            let key = target_texture_key(target);
            if evicted.contains(&key) && !visible_sources.contains(&key) {
                show_target_unloaded(target);
            }
        }
    }

    pub(super) fn register_thumbnail_target(&mut self, target: ThumbnailTarget) {
        self.targets.push(target);
    }

    pub(super) fn register_progress_target(&mut self, target: ProgressTarget) {
        self.progress_targets.push(target);
    }

    pub(super) fn progress_targets(&self) -> &[ProgressTarget] {
        &self.progress_targets
    }

    pub(super) fn favorite_targets(&self) -> &[FavoriteTarget] {
        &self.favorite_targets
    }
}

const FOLDER_COLUMN_SPACING: i32 = 32;

fn folder_wrap() -> gtk::Box {
    let wrap = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .hexpand(true)
        .valign(gtk::Align::Start)
        .css_name("flowbox")
        .css_classes(["bookshelf-card-flow"])
        .build();
    wrap.set_layout_manager(Some(FolderWrapLayout::new()));
    wrap
}

fn append_folder_card(wrap: &gtk::Box, child: &impl IsA<gtk::Widget>) {
    // Preserve the existing card CSS without placing a FlowBoxChild outside a FlowBox.
    let wrapper = gtk::Box::builder().css_name("flowboxchild").build();
    wrapper.append(child);
    wrap.append(&wrapper);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FolderWrapPosition {
    x: i32,
    y: i32,
}

fn pack_folder_cards(
    available_width: i32,
    cards: &[(i32, i32)],
    column_gap: i32,
    row_gap: i32,
) -> Vec<FolderWrapPosition> {
    let available_width = available_width.max(1);
    let mut positions = Vec::with_capacity(cards.len());
    let (mut x, mut y, mut row_height) = (0, 0, 0);
    for &(width, height) in cards {
        let width = width.max(1);
        let height = height.max(1);
        let next_x = if x == 0 { 0 } else { x + column_gap };
        if x != 0 && next_x + width > available_width {
            x = 0;
            y += row_height + row_gap;
            row_height = 0;
        } else {
            x = next_x;
        }
        positions.push(FolderWrapPosition { x, y });
        x += width;
        row_height = row_height.max(height);
    }
    positions
}

fn folder_card_sizes(widget: &gtk::Widget) -> Vec<(gtk::Widget, i32, i32)> {
    let mut cards = Vec::new();
    let mut child = widget.first_child();
    while let Some(card) = child {
        let (_, width, _, _) = card.measure(gtk::Orientation::Horizontal, -1);
        let (_, height, _, _) = card.measure(gtk::Orientation::Vertical, width);
        cards.push((card.clone(), width.max(1), height.max(1)));
        child = card.next_sibling();
    }
    cards
}

fn packed_folder_height(available_width: i32, cards: &[(i32, i32)]) -> i32 {
    pack_folder_cards(
        available_width,
        cards,
        FOLDER_COLUMN_SPACING,
        COVER_ROW_SPACING as i32,
    )
    .into_iter()
    .zip(cards)
    .map(|(position, (_, height))| position.y + height.max(&1))
    .max()
    .unwrap_or(0)
}

mod folder_wrap_layout {
    use super::*;
    use gtk::subclass::prelude::*;

    mod imp {
        use super::*;

        #[derive(Default)]
        pub struct FolderWrapLayout;

        #[gtk::glib::object_subclass]
        impl ObjectSubclass for FolderWrapLayout {
            const NAME: &'static str = "AgnamFolderWrapLayout";
            type Type = super::FolderWrapLayout;
            type ParentType = gtk::LayoutManager;
        }

        impl ObjectImpl for FolderWrapLayout {}

        impl LayoutManagerImpl for FolderWrapLayout {
            fn request_mode(&self, _: &gtk::Widget) -> gtk::SizeRequestMode {
                gtk::SizeRequestMode::HeightForWidth
            }

            fn measure(
                &self,
                widget: &gtk::Widget,
                orientation: gtk::Orientation,
                for_size: i32,
            ) -> (i32, i32, i32, i32) {
                let cards = folder_card_sizes(widget);
                let sizes = cards
                    .iter()
                    .map(|(_, width, height)| (*width, *height))
                    .collect::<Vec<_>>();
                match orientation {
                    gtk::Orientation::Horizontal => {
                        let minimum = sizes.iter().map(|(width, _)| *width).max().unwrap_or(0);
                        let natural = sizes.iter().map(|(width, _)| *width).sum::<i32>()
                            + (sizes.len().saturating_sub(1) as i32) * FOLDER_COLUMN_SPACING;
                        (minimum, natural.max(minimum), -1, -1)
                    }
                    gtk::Orientation::Vertical => {
                        let width = if for_size >= 0 {
                            for_size
                        } else {
                            sizes.iter().map(|(width, _)| *width).sum::<i32>()
                                + (sizes.len().saturating_sub(1) as i32) * FOLDER_COLUMN_SPACING
                        };
                        let height = packed_folder_height(width.max(1), &sizes);
                        (height, height, -1, -1)
                    }
                    _ => unreachable!(),
                }
            }

            fn allocate(&self, widget: &gtk::Widget, width: i32, _: i32, _: i32) {
                let cards = folder_card_sizes(widget);
                let sizes = cards
                    .iter()
                    .map(|(_, width, height)| (*width, *height))
                    .collect::<Vec<_>>();
                let positions = pack_folder_cards(
                    width.max(1),
                    &sizes,
                    FOLDER_COLUMN_SPACING,
                    COVER_ROW_SPACING as i32,
                );
                for ((card, card_width, card_height), position) in cards.into_iter().zip(positions)
                {
                    let transform = gtk::gsk::Transform::new().translate(
                        &gtk::graphene::Point::new(position.x as f32, position.y as f32),
                    );
                    card.allocate(card_width, card_height, -1, Some(transform));
                }
            }
        }
    }

    gtk::glib::wrapper! {
        pub struct FolderWrapLayout(ObjectSubclass<imp::FolderWrapLayout>) @extends gtk::LayoutManager;
    }

    impl FolderWrapLayout {
        pub fn new() -> Self {
            gtk::glib::Object::new()
        }
    }
}

use folder_wrap_layout::FolderWrapLayout;

fn bookshelf_container(content: &(impl IsA<gtk::Widget> + Clone)) -> ShelfContainer {
    ShelfContainer::new(content)
}

impl Drop for LibraryView {
    fn drop(&mut self) {
        for popover in self.context_popovers.drain(..) {
            if popover.parent().is_some() {
                popover.unparent();
            }
        }
    }
}

fn request_visible_thumbnail_evaluation(sender: &AppSender) {
    let sender = sender.clone();
    gtk::glib::idle_add_local_once(move || {
        sender.input(Msg::LibraryThumbnailViewportChanged);
    });
}

#[cfg(test)]
mod folder_wrap_tests {
    use super::{
        ARCHIVE_TEXTURE_CACHE_BUDGET, FolderWrapPosition, NORMAL_TEXTURE_CACHE_BUDGET, TextureLru,
        associated_sources, pack_folder_cards, packed_folder_height,
    };
    use crate::app::library_thumbnail::{ThumbnailSourceKind, ThumbnailTextureKey};
    use std::collections::{HashMap, HashSet};
    use std::path::{Path, PathBuf};

    fn key(path: &str) -> ThumbnailTextureKey {
        ThumbnailTextureKey::new(PathBuf::from(path), ThumbnailSourceKind::BookCover)
    }

    fn sources(paths: &[&str]) -> HashSet<ThumbnailTextureKey> {
        paths.iter().map(|path| key(path)).collect()
    }

    #[test]
    fn cards_pack_left_without_column_alignment() {
        assert_eq!(
            pack_folder_cards(100, &[(30, 10), (50, 10), (20, 10), (60, 10)], 12, 16),
            [
                FolderWrapPosition { x: 0, y: 0 },
                FolderWrapPosition { x: 42, y: 0 },
                FolderWrapPosition { x: 0, y: 26 },
                FolderWrapPosition { x: 32, y: 26 },
            ]
        );
    }

    #[test]
    fn an_oversized_first_card_does_not_prevent_following_wraps() {
        assert_eq!(
            pack_folder_cards(80, &[(100, 12), (20, 12)], 12, 16),
            [
                FolderWrapPosition { x: 0, y: 0 },
                FolderWrapPosition { x: 0, y: 28 },
            ]
        );
    }

    #[test]
    fn packed_height_tracks_the_rows_for_a_given_allocation_width() {
        assert_eq!(
            packed_folder_height(100, &[(30, 10), (50, 10), (20, 10)]),
            62
        );
    }

    #[test]
    fn texture_lru_stays_within_budget_and_evicts_oldest_unpinned() {
        let mut cache = TextureLru::new(10);
        cache.insert(key("a"), (), 4);
        cache.insert(key("b"), (), 4);
        assert!(
            cache
                .trim(&HashSet::new(), &sources(&["a", "b"]))
                .is_empty()
        );

        cache.insert(key("c"), (), 4);
        assert_eq!(
            cache.trim(&sources(&["b"]), &sources(&["a", "b", "c"])),
            [key("a")]
        );
        assert_eq!(cache.total_bytes, 8);
    }

    #[test]
    fn texture_lru_touch_makes_an_entry_recent() {
        let mut cache = TextureLru::new(8);
        cache.insert(key("a"), (), 4);
        cache.insert(key("b"), (), 4);
        cache.touch(&key("a"));
        cache.insert(key("c"), (), 4);

        assert_eq!(
            cache.trim(&HashSet::new(), &sources(&["a", "b", "c"])),
            [key("b")]
        );
    }

    #[test]
    fn texture_lru_allows_visible_only_overage_until_unpinned() {
        let mut cache = TextureLru::new(8);
        cache.insert(key("a"), (), 6);
        cache.insert(key("b"), (), 6);
        let associated = sources(&["a", "b"]);

        assert!(cache.trim(&associated, &associated).is_empty());
        assert_eq!(cache.total_bytes, 12);
        assert_eq!(cache.trim(&sources(&["a"]), &associated), [key("b")]);
        assert_eq!(cache.total_bytes, 6);
    }

    #[test]
    fn normal_and_archive_texture_caches_have_independent_budgets() {
        assert_eq!(NORMAL_TEXTURE_CACHE_BUDGET, 64 * 1024 * 1024);
        assert_eq!(ARCHIVE_TEXTURE_CACHE_BUDGET, 64 * 1024 * 1024);
        let mut normal = TextureLru::new(NORMAL_TEXTURE_CACHE_BUDGET);
        let mut archive = TextureLru::new(ARCHIVE_TEXTURE_CACHE_BUDGET);
        normal.insert(key("shared"), (), NORMAL_TEXTURE_CACHE_BUDGET);
        archive.insert(key("shared"), (), ARCHIVE_TEXTURE_CACHE_BUDGET);

        normal.clear();
        assert!(!normal.contains(&key("shared")));
        assert!(archive.contains(&key("shared")));
    }

    #[test]
    fn shared_directory_source_survives_child_association_pruning() {
        let mut directories = HashMap::from([
            (PathBuf::from("parent"), sources(&["a"])),
            (PathBuf::from("child"), sources(&["a", "b"])),
        ]);
        let mut cache = TextureLru::new(100);
        cache.insert(key("a"), (), 4);
        cache.insert(key("b"), (), 4);

        directories.remove(PathBuf::from("child").as_path());
        assert_eq!(
            cache.retain_sources(&associated_sources(&directories)),
            [key("b")]
        );
        assert!(cache.contains(&key("a")));
    }

    #[test]
    fn same_source_with_different_kinds_uses_distinct_entries_and_associations() {
        let source = PathBuf::from("shared.jpg");
        let cover = ThumbnailTextureKey::new(source.clone(), ThumbnailSourceKind::BookCover);
        let image = ThumbnailTextureKey::new(source, ThumbnailSourceKind::DirectImage);
        let mut cache = TextureLru::new(100);
        cache.insert(cover.clone(), (), 4);
        cache.insert(image.clone(), (), 4);

        let mut directories = HashMap::from([
            (PathBuf::from("parent"), HashSet::from([cover.clone()])),
            (PathBuf::from("child"), HashSet::from([image.clone()])),
        ]);
        directories.remove(Path::new("child"));

        assert_eq!(
            cache.retain_sources(&associated_sources(&directories)),
            [image]
        );
        assert!(cache.contains(&cover));
    }
}
