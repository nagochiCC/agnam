use super::library::ScanCompletion;
use super::library_thumbnail::{ThumbnailSourceKind, ThumbnailTextureKey};
use super::*;
use std::sync::Arc;

impl App {
    pub(super) fn select_external_cover(
        &mut self,
        identity: crate::covers::CoverBookIdentity,
        sender: &AppSender,
    ) {
        let request_id = self.library.external_cover_requests.begin(&identity);
        super::file_dialog::show_external_cover_dialog(identity, request_id, sender.clone());
    }

    pub(super) fn cover_override_changed(&mut self, sender: &AppSender) {
        let search_selected = self.navigation.panel.is_selected(NavigationPanel::Search);
        self.library.invalidate_cover_override(search_selected);
        self.navigation.panel.mark_cover_override_dirty();
        self.render_current_library_content(sender);
        self.update_library_thumbnail_demand(sender);

        if search_selected {
            self.render_library_search_results(sender);
        }
        let jobs = self.navigation.history_view.invalidate_cover_textures();
        Self::spawn_history_cover_jobs(jobs, sender);
        let jobs = self.navigation.favorites_view.invalidate_cover_textures();
        Self::spawn_favorites_cover_jobs(jobs, sender);
        self.mark_history_panel_dirty(sender);
        self.mark_favorites_panel_dirty(sender);
    }
    fn bookshelf_display_sizes(&self) -> BookshelfDisplaySizes {
        BookshelfDisplaySizes::new(self.settings.library_book_height)
    }

    pub(super) fn library_status_message(&self) -> Option<String> {
        if self.library.archive.is_active() {
            self.library.archive.status_message()
        } else if self.library.view.is_holding_previous_normal_display() {
            None
        } else {
            self.library.model.status_message()
        }
    }

    pub(super) fn library_status_spinning(&self) -> bool {
        if self.library.archive.is_active() {
            self.library.archive.is_loading()
        } else {
            self.library.model.is_scanning()
        }
    }

    pub(super) fn library_has_visible_items(&self) -> bool {
        if self.library.archive.is_active() {
            self.library.archive.has_items()
        } else {
            self.library.model.has_directory_items() || self.library.view.has_visible_normal_items()
        }
    }

    fn begin_library_thumbnail_scope(&mut self) -> u64 {
        self.library.begin_thumbnail_scope()
    }

    pub(super) fn render_current_library_content(&mut self, sender: &AppSender) -> bool {
        self.render_current_library_content_with_preserved_replacement(sender, false)
    }

    fn render_current_library_content_with_preserved_replacement(
        &mut self,
        sender: &AppSender,
        replace_preserved_cards: bool,
    ) -> bool {
        if self.library.archive.is_active() {
            self.library.view.render_archive(
                self.library.archive.level(),
                self.library.archive.thumbnail_backing(),
                self.bookshelf_display_sizes(),
                sender,
            );
            false
        } else {
            self.library.view.render(
                self.library.model.directory(),
                self.navigation.history.entries(),
                self.navigation.favorites.entries(),
                self.bookshelf_display_sizes(),
                self.settings.library_sort_key,
                self.settings.library_sort_direction,
                replace_preserved_cards,
                sender,
            )
        }
    }

    pub(super) fn open_file(&mut self, sender: &AppSender) {
        self.close_navigation_panel(sender);
        self.invalidate_document_boundary();
        show_file_dialog(sender.clone());
    }

    pub(super) fn select_bookshelf_root(&self, sender: &AppSender) {
        show_bookshelf_folder_dialog(sender.clone());
    }

    pub(super) fn set_bookshelf_root(&mut self, root: PathBuf, sender: &AppSender) {
        let root_changed = self.library.model.root() != Some(root.as_path());
        self.close_navigation_panel(sender);
        if root_changed {
            self.reset_library_search();
        }
        self.settings.bookshelf_root = Some(root.clone());
        self.settings.save();
        self.shell.settings_dialog.sync(&self.settings);
        self.library.model.set_root(Some(root.clone()));
        if root_changed {
            self.library.view.reset_normal_cache();
        }
        self.library.archive.leave();
        self.sync_library_search_action();
        self.refresh_breadcrumb(sender);

        if self.is_library_active() {
            self.navigate_library(root, sender);
        } else {
            self.library.view.begin_directory(Some(&root));
            self.begin_library_thumbnail_scope();
            self.library.view.hide_for_initial_reveal();
        }
    }

    pub(super) fn navigate_library(&mut self, path: PathBuf, sender: &AppSender) {
        let Some(request) = self.library.model.navigate(path) else {
            return;
        };
        let cached = self.library.model.directory().is_some();
        self.library.archive.leave();
        self.close_navigation_panel(sender);
        self.invalidate_document_boundary();
        self.document.sibling_lookup.invalidate();
        self.cancel_progressive_archive_display();
        self.set_app_view(AppView::Library);
        self.dismiss_document_toast();
        self.document.load.cancel();
        self.hide_hover_preview();
        let retaining_previous = if cached {
            self.library.view.begin_directory(Some(&request.path));
            false
        } else {
            self.library.view.begin_pending_directory(&request.path)
        };
        self.library
            .view
            .retain_normal_directories(&self.library.model.cached_directory_paths());
        self.begin_library_thumbnail_scope();
        if cached {
            self.render_current_library_content(sender);
            self.library.view.reveal();
            if let Some(timeout) = self.library.initial_reveal.begin_render() {
                Self::schedule_library_initial_reveal_timeout(timeout, sender);
            }
            self.update_library_thumbnail_demand(sender);
        } else if !retaining_previous {
            self.library.view.hide_for_initial_reveal();
        }
        self.refresh_breadcrumb(sender);
        Self::spawn_library_scan(request, sender);
    }

    pub(super) fn open_archive_contents(&mut self, archive: PathBuf, sender: &AppSender) {
        self.navigate_archive_contents(crate::archive::ArchiveLocation::root(archive), sender);
    }

    pub(super) fn navigate_archive_contents(
        &mut self,
        location: crate::archive::ArchiveLocation,
        sender: &AppSender,
    ) {
        if self.library.archive.location().is_some_and(|current| {
            current.archive == location.archive && current.archives == location.archives
        }) && self
            .library
            .archive
            .navigate_directory(location.directory.clone())
        {
            self.close_navigation_panel(sender);
            self.set_app_view(AppView::Library);
            self.library.view.begin_directory(None);
            self.begin_library_thumbnail_scope();
            self.render_current_library_content(sender);
            self.refresh_breadcrumb(sender);
            return;
        }

        let request = self.library.archive.begin(location);
        self.close_navigation_panel(sender);
        self.invalidate_document_boundary();
        self.document.sibling_lookup.invalidate();
        self.cancel_progressive_archive_display();
        self.set_app_view(AppView::Library);
        self.dismiss_document_toast();
        self.document.load.cancel();
        self.hide_hover_preview();
        self.library.view.begin_directory(None);
        self.begin_library_thumbnail_scope();
        self.library.view.hide_for_initial_reveal();
        self.refresh_breadcrumb(sender);
        let sender = sender.clone();
        spawn_background(move || {
            let result =
                crate::archive::ArchiveContentLevel::open(request.location).map_err(|error| {
                    match error {
                        crate::error::AppError::ArchiveResourceLimit(_) => {
                            super::document_workflow::ARCHIVE_RESOURCE_LIMIT_MESSAGE.into()
                        }
                        error => error.to_string(),
                    }
                });
            sender.input(Msg::ArchiveContentsLoaded {
                request_id: request.id,
                result,
            });
        });
    }

    pub(super) fn archive_contents_loaded(
        &mut self,
        request_id: u64,
        result: Result<crate::archive::ArchiveContentLevel, String>,
        sender: &AppSender,
    ) {
        if self.library.archive.apply(request_id, result) {
            let progressive_plan = self
                .library
                .archive
                .level()
                .and_then(crate::archive::ArchiveContentLevel::progressive_thumbnail_plan);
            let progressive_identity = self
                .library
                .archive
                .location()
                .map(|location| (location.archive.clone(), location.archives.clone()));
            self.render_current_library_content(sender);
            self.library.view.reveal();
            self.refresh_breadcrumb(sender);
            if let (Some((archive, images)), Some((root_archive, archives))) =
                (progressive_plan, progressive_identity)
            {
                let token = self.library.archive.begin_progressive_thumbnails();
                Self::spawn_progressive_archive_content_thumbnails(
                    request_id,
                    archive,
                    root_archive,
                    archives,
                    images,
                    self.library.archive.thumbnail_backing(),
                    token,
                    sender,
                );
            } else {
                self.update_library_thumbnail_demand(sender);
            }
        }
    }

    fn spawn_progressive_archive_content_thumbnails(
        session_id: u64,
        archive: PathBuf,
        root_archive: PathBuf,
        archives: Vec<PathBuf>,
        images: Vec<PathBuf>,
        backing: Option<Arc<super::archive_thumbnail_backing::ArchiveThumbnailBacking>>,
        token: crate::archive::ProgressiveArchiveCancelToken,
        sender: &AppSender,
    ) {
        let sender = sender.clone();
        spawn_background(move || {
            let progress_sender = sender.clone();
            let progress_token = token.clone();
            let result = crate::archive::stream_sequential_archive_images(
                &archive,
                &token,
                move |image| {
                    if progress_token.is_cancelled() {
                        return;
                    }
                    if image.total_images != images.len() {
                        return;
                    }
                    let Some(path) = images.get(image.natural_index) else {
                        return;
                    };
                    let Ok(thumbnail) =
                        crate::bookshelf::thumbnail::generate_from_bytes(&image.bytes)
                    else {
                        return;
                    };
                    let id = crate::archive::ArchiveImageId {
                        archives: archives.clone(),
                        image: path.clone(),
                    };
                    let source = super::archive_library::ArchiveLibraryState::thumbnail_key(
                        &root_archive,
                        &id,
                    );
                    let backed = backing.as_ref().is_some_and(|backing| {
                        if let Err(error) = backing.save(&source, &thumbnail) {
                            eprintln!(
                                "archive thumbnail session backingへ保存できませんでした ({}): {error}",
                                source.display()
                            );
                            false
                        } else {
                            true
                        }
                    });
                    progress_sender.input(Msg::ArchiveContentThumbnailReady {
                        session_id,
                        source,
                        backed,
                        thumbnail,
                    });
                },
            );
            if let Err(error) = result {
                eprintln!("archive内容thumbnailの逐次生成に失敗しました: {error}");
                if matches!(error, crate::error::AppError::ArchiveResourceLimit(_)) {
                    sender.input(Msg::ArchiveContentThumbnailResourceLimit { session_id });
                }
            }
            sender.input(Msg::ArchiveContentThumbnailsFinished { session_id });
        });
    }

    pub(super) fn archive_content_thumbnail_ready(
        &mut self,
        session_id: u64,
        source: PathBuf,
        backed: bool,
        thumbnail: crate::bookshelf::thumbnail::ThumbnailData,
    ) {
        if self.library.archive.accepts_session(session_id) {
            if backed {
                self.library
                    .thumbnails
                    .cache_entry_became_available(&source, ThumbnailSourceKind::ArchiveEntry);
            }
            self.apply_library_thumbnails(vec![(
                ThumbnailTextureKey::new(source, ThumbnailSourceKind::ArchiveEntry),
                thumbnail,
            )]);
        }
    }

    pub(super) fn archive_content_thumbnails_finished(
        &mut self,
        session_id: u64,
        sender: &AppSender,
    ) {
        if self
            .library
            .archive
            .finish_progressive_thumbnails(session_id)
        {
            self.update_library_thumbnail_demand(sender);
        }
    }

    pub(super) fn archive_content_thumbnail_resource_limit(
        &mut self,
        session_id: u64,
        _sender: &AppSender,
    ) {
        if self.is_library_active() && self.library.archive.accepts_session(session_id) {
            self.show_archive_resource_limit_dialog(
                "安全上限に達したため、アーカイブ内容のサムネイル生成を停止しました",
            );
        }
    }

    fn spawn_library_scan(request: ScanRequest, sender: &AppSender) {
        let sender = sender.clone();
        spawn_background(move || {
            let result = scan_directory(&request.path).map_err(|error| error.to_string());
            if request.persist_snapshot
                && let Ok(directory) = &result
            {
                crate::bookshelf::snapshot::save_root_snapshot(directory);
            }
            sender.input(Msg::LibraryScanFinished {
                request_id: request.id,
                path: request.path,
                result,
            });
        });
    }

    pub(super) fn library_scan_finished(
        &mut self,
        request_id: u64,
        path: PathBuf,
        result: Result<crate::bookshelf::BookshelfDirectory, String>,
        sender: &AppSender,
    ) {
        match self.library.model.apply_scan(request_id, &path, result) {
            ScanCompletion::InitialSuccess | ScanCompletion::InitialFailed => {
                let replaced_preserved_cards =
                    self.render_current_library_content_with_preserved_replacement(sender, true);
                if replaced_preserved_cards {
                    self.library.initial_reveal.begin_render();
                    self.library
                        .initial_reveal
                        .begin_demand(&ThumbnailDemandEvaluation::Ready(Vec::new()));
                    self.library.view.reveal();
                } else if let Some(timeout) = self.library.initial_reveal.begin_render() {
                    Self::schedule_library_initial_reveal_timeout(timeout, sender);
                }
            }
            ScanCompletion::RefreshChanged => {
                self.render_current_library_content(sender);
            }
            ScanCompletion::Stale
            | ScanCompletion::RefreshUnchanged
            | ScanCompletion::RefreshFailed => {}
        }
    }

    fn schedule_library_initial_reveal_timeout(timeout: InitialRevealTimeout, sender: &AppSender) {
        let sender = sender.clone();
        gtk::glib::timeout_add_local_once(LIBRARY_INITIAL_REVEAL_TIMEOUT, move || {
            sender.input(Msg::LibraryInitialRevealTimeout {
                generation: timeout.generation,
                revision: timeout.revision,
            });
        });
    }

    pub(super) fn update_library_thumbnail_demand(&mut self, sender: &AppSender) {
        let demands = if library_thumbnail_scope(self.shell.view) == LibraryThumbnailScope::Normal {
            self.library.view.visible_thumbnail_demands()
        } else {
            ThumbnailDemandEvaluation::Ready(Vec::new())
        };
        let generation_enabled = !self.library.archive.progressive_thumbnails_active();
        let reveal = self.library.initial_reveal.begin_demand(&demands);
        self.library
            .thumbnails
            .forget_ready_for_missing_textures(&demands);
        let jobs = self
            .library
            .thumbnails
            .update_demand_with_generation(demands, generation_enabled);
        if let Some(cohort) = jobs.cache_cohort.clone() {
            self.begin_library_thumbnail_cache_cohort(cohort, sender);
        }
        self.spawn_library_thumbnail_jobs(jobs, sender);
        if reveal {
            self.finish_library_initial_reveal();
        }
    }

    fn begin_library_thumbnail_cache_cohort(
        &mut self,
        cohort: CacheLookupCohort,
        sender: &AppSender,
    ) {
        let generation = cohort.generation;
        if self.library.thumbnail_cache_hits.begin_cohort(cohort) {
            self.schedule_library_thumbnail_cache_hit_flush(generation, sender);
        }
    }

    fn spawn_library_thumbnail_jobs(&self, jobs: ScheduledJobs, sender: &AppSender) {
        for job in jobs.cache_loads {
            let archive_backing = (job.kind == ThumbnailSourceKind::ArchiveEntry)
                .then(|| self.library.archive.thumbnail_source(&job.source))
                .flatten()
                .and_then(|source| source.backing);
            let sender = sender.clone();
            spawn_background(move || {
                let thumbnail = match job.kind {
                    ThumbnailSourceKind::BookCover => load_cached_bookshelf_thumbnail(&job.source),
                    ThumbnailSourceKind::DirectImage => {
                        load_cached_direct_bookshelf_thumbnail(&job.source)
                    }
                    ThumbnailSourceKind::ArchiveEntry => {
                        archive_backing.and_then(|backing| backing.load(&job.source))
                    }
                };
                sender.input(Msg::LibraryThumbnailCacheLoadFinished {
                    generation: job.generation,
                    cohort_id: job.cohort_id,
                    source: job.source,
                    kind: job.kind,
                    thumbnail,
                });
            });
        }
        for job in jobs.generations {
            let cancel = self.library.thumbnails.cancellation_token();
            let archive_source = (job.kind == ThumbnailSourceKind::ArchiveEntry)
                .then(|| self.library.archive.thumbnail_source(&job.source))
                .flatten();
            let sender = sender.clone();
            spawn_background(move || {
                let result = match job.kind {
                    ThumbnailSourceKind::BookCover => {
                        generate_and_cache_bookshelf_thumbnail_with_cancel(&job.source, &|| cancel.cancelled())
                            .map_err(|error| error.to_string())
                    }
                    ThumbnailSourceKind::DirectImage => {
                        generate_direct_bookshelf_thumbnail(&job.source)
                            .map(Some)
                            .map_err(|error| error.to_string())
                    }
                    ThumbnailSourceKind::ArchiveEntry => archive_source
                        .ok_or_else(|| "archive thumbnail source is stale".to_string())
                        .and_then(|source| {
                            if let Some(thumbnail) = source
                                .backing
                                .as_ref()
                                .and_then(|backing| backing.load(&job.source))
                            {
                                return Ok(thumbnail);
                            }
                            let bytes = source
                                .reader
                                .read(&source.entry)
                                .map_err(|error| error.to_string())?;
                            let thumbnail = crate::bookshelf::thumbnail::generate_from_bytes(&bytes)
                                .map_err(|error| error.to_string())?;
                            if let Some(backing) = &source.backing
                                && let Err(error) = backing.save(&job.source, &thumbnail)
                            {
                                eprintln!(
                                    "archive thumbnail session backingへ保存できませんでした ({}): {error}",
                                    job.source.display()
                                );
                            }
                            Ok(thumbnail)
                        }).map(Some),
                };
                sender.input(Msg::LibraryThumbnailGenerationFinished {
                    generation: job.generation,
                    source: job.source,
                    kind: job.kind,
                    result,
                });
            });
        }
    }

    pub(super) fn library_thumbnail_cache_load_finished(
        &mut self,
        generation: u64,
        cohort_id: u64,
        source: PathBuf,
        kind: ThumbnailSourceKind,
        thumbnail: Option<crate::bookshelf::thumbnail::ThumbnailData>,
        sender: &AppSender,
    ) {
        let accepted = self
            .library
            .thumbnails
            .accepts_cache_load_completion_with_kind(generation, cohort_id, &source, kind);
        let cache_hit = thumbnail.is_some();
        let completion = self
            .library
            .thumbnails
            .complete_cache_load_with_kind(generation, cohort_id, &source, kind, cache_hit);
        debug_assert_eq!(completion.accepted, accepted);
        if accepted {
            let update = self.library.thumbnail_cache_hits.complete_lookup(
                generation,
                cohort_id,
                ThumbnailTextureKey::new(source.clone(), kind),
                thumbnail,
            );
            self.apply_library_thumbnail_cache_lookup_update(generation, update, sender);
            if self
                .library
                .initial_reveal
                .complete_lookup(generation, &ThumbnailTextureKey::new(source.clone(), kind))
            {
                self.finish_library_initial_reveal();
            }
        }
        self.spawn_library_thumbnail_jobs(completion.jobs, sender);
    }

    pub(super) fn library_thumbnail_generation_finished(
        &mut self,
        generation: u64,
        source: PathBuf,
        kind: ThumbnailSourceKind,
        result: Result<Option<crate::bookshelf::thumbnail::ThumbnailData>, String>,
        sender: &AppSender,
    ) {
        if matches!(result, Ok(None)) {
            let jobs = self
                .library
                .thumbnails
                .cancel_generation_with_kind(generation, &source, kind);
            self.spawn_library_thumbnail_jobs(jobs, sender);
            return;
        }
        let accepted = self
            .library
            .thumbnails
            .accepts_generation_completion_with_kind(generation, &source, kind);
        let succeeded = if accepted {
            match result {
                Ok(Some(thumbnail)) => {
                    self.queue_library_thumbnail(
                        generation,
                        source.clone(),
                        kind,
                        thumbnail,
                        sender,
                    );
                    true
                }
                Ok(None) => false,
                Err(error) => {
                    eprintln!(
                        "本棚サムネイルの生成に失敗しました ({}): {error}",
                        source.display()
                    );
                    false
                }
            }
        } else {
            false
        };
        let completion = self
            .library
            .thumbnails
            .complete_generation_with_kind(generation, &source, kind, succeeded);
        debug_assert_eq!(completion.accepted, accepted);
        if completion.accepted && !succeeded {
            self.library
                .view
                .thumbnail_failed(&ThumbnailTextureKey::new(source, kind));
        }
        self.spawn_library_thumbnail_jobs(completion.jobs, sender);
    }

    fn queue_library_thumbnail(
        &mut self,
        generation: u64,
        source: PathBuf,
        kind: ThumbnailSourceKind,
        thumbnail: crate::bookshelf::thumbnail::ThumbnailData,
        sender: &AppSender,
    ) {
        let Some(timers) = self
            .library
            .thumbnail_batch
            .push_with_kind(generation, source, kind, thumbnail)
        else {
            return;
        };
        let quiet_sender = sender.clone();
        gtk::glib::timeout_add_local_once(LIBRARY_THUMBNAIL_DEBOUNCE, move || {
            quiet_sender.input(Msg::FlushLibraryThumbnails {
                generation: timers.generation,
                batch_id: timers.batch_id,
                quiet_revision: Some(timers.quiet_revision),
            });
        });
        if timers.starts_batch {
            let maximum_sender = sender.clone();
            gtk::glib::timeout_add_local_once(LIBRARY_THUMBNAIL_MAX_LATENCY, move || {
                maximum_sender.input(Msg::FlushLibraryThumbnails {
                    generation: timers.generation,
                    batch_id: timers.batch_id,
                    quiet_revision: None,
                });
            });
        }
    }

    fn apply_library_thumbnail_cache_lookup_update(
        &mut self,
        generation: u64,
        update: CacheLookupBatchUpdate,
        sender: &AppSender,
    ) {
        self.apply_library_thumbnails(update.immediate);
        if update.schedule_frame_flush {
            self.schedule_library_thumbnail_cache_hit_flush(generation, sender);
        }
    }

    fn schedule_library_thumbnail_cache_hit_flush(&self, generation: u64, sender: &AppSender) {
        if !self
            .library
            .view
            .schedule_cache_hit_flush(generation, sender)
        {
            let sender = sender.clone();
            gtk::glib::idle_add_local_once(move || {
                sender.input(Msg::FlushLibraryThumbnailCacheHits { generation });
            });
        }
    }

    pub(super) fn flush_library_thumbnail_cache_hits(&mut self, generation: u64) {
        let thumbnails = self.library.thumbnail_cache_hits.take_for_flush(generation);
        self.apply_library_thumbnails(thumbnails);
    }

    pub(super) fn library_initial_reveal_timeout(&mut self, timeout: InitialRevealTimeout) {
        if self.library.initial_reveal.timeout(timeout) {
            self.finish_library_initial_reveal();
        }
    }

    fn finish_library_initial_reveal(&mut self) {
        let generation = self.library.initial_reveal.generation();
        let thumbnails = self
            .library
            .thumbnail_cache_hits
            .take_available_for_initial_reveal(generation);
        self.apply_library_thumbnails(thumbnails);
        self.library.view.reveal();
    }

    pub(super) fn flush_library_thumbnails(
        &mut self,
        generation: u64,
        batch_id: u64,
        quiet_revision: Option<u64>,
    ) {
        let thumbnails =
            self.library
                .thumbnail_batch
                .take_for_timeout(generation, batch_id, quiet_revision);
        self.apply_library_thumbnails(thumbnails);
    }

    fn apply_library_thumbnails(
        &mut self,
        thumbnails: Vec<(
            ThumbnailTextureKey,
            crate::bookshelf::thumbnail::ThumbnailData,
        )>,
    ) {
        for key in self.library.view.thumbnails_ready(thumbnails) {
            eprintln!(
                "本棚サムネイルのTexture変換に失敗しました: {}",
                key.source.display()
            );
            self.library.view.thumbnail_failed(&key);
        }
    }
}
