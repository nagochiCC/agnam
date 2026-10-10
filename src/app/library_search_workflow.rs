use super::*;

impl App {
    pub(super) fn set_library_search_query(&mut self, query: String, sender: &AppSender) {
        if !self.library.search.set_query(query) {
            return;
        }
        self.navigation.panel.mark_dirty(NavigationPanel::Search);
        if self.navigation.panel.is_selected(NavigationPanel::Search) {
            self.render_library_search_results(sender);
        }
    }

    pub(super) fn spawn_library_search_build(request: SearchBuildRequest, sender: &AppSender) {
        let sender = sender.clone();
        spawn_background(move || {
            let result = crate::bookshelf::search::SearchIndex::build(&request.root)
                .map_err(|error| error.to_string());
            sender.input(Msg::LibrarySearchBuildFinished {
                request_id: request.id,
                root: request.root,
                result,
            });
        });
    }

    pub(super) fn library_search_build_finished(
        &mut self,
        request_id: u64,
        root: PathBuf,
        result: Result<crate::bookshelf::search::SearchIndex, String>,
        sender: &AppSender,
    ) {
        let error = result.as_ref().err().cloned();
        if self.library.search.apply_build(request_id, &root, result) {
            self.navigation.panel.mark_dirty(NavigationPanel::Search);
            if let Some(error) = error {
                eprintln!(
                    "本棚検索indexの構築に失敗しました ({}): {error}",
                    root.display()
                );
            }
            if self.navigation.panel.is_selected(NavigationPanel::Search) {
                self.render_library_search_results(sender);
            }
        }
    }

    pub(super) fn render_library_search_results(&mut self, sender: &AppSender) {
        let generation = self.library.search_thumbnails.begin_generation();
        self.library
            .search_view
            .render(&self.library.search, generation, sender);
        self.navigation.panel.mark_clean(NavigationPanel::Search);
    }

    pub(super) fn update_library_search_thumbnail_demand(&mut self, sender: &AppSender) {
        let evaluation = if search_thumbnail_scope_visible(
            self.navigation.panel.is_selected(NavigationPanel::Search),
            self.library.search.has_meaningful_query(),
        ) {
            self.library.search_view.visible_thumbnail_sources()
        } else {
            SearchThumbnailDemandEvaluation::Ready(Vec::new())
        };
        let jobs = self.library.search_thumbnails.update_demand(evaluation);
        Self::spawn_library_search_thumbnail_jobs(
            self.settings.archive_expansion_limit,
            jobs,
            self.library.search_thumbnails.cancellation_token(),
            sender,
        );
    }

    pub(super) fn suspend_library_search_thumbnail_demand(&mut self, sender: &AppSender) {
        let jobs = self
            .library
            .search_thumbnails
            .update_demand(SearchThumbnailDemandEvaluation::Ready(Vec::new()));
        Self::spawn_library_search_thumbnail_jobs(
            self.settings.archive_expansion_limit,
            jobs,
            self.library.search_thumbnails.cancellation_token(),
            sender,
        );
    }

    fn spawn_library_search_thumbnail_jobs(
        limit: crate::archive::ArchiveExpansionLimit,
        jobs: SearchThumbnailJobs,
        cancel: cover_cancel::CoverCancel,
        sender: &AppSender,
    ) {
        for job in jobs.cache_loads {
            let sender = sender.clone();
            spawn_background(move || {
                let thumbnail =
                    load_cached_bookshelf_thumbnail(&job.source).and_then(|thumbnail| {
                        crate::bookshelf::thumbnail::fit_thumbnail_data(
                            thumbnail,
                            super::library_search_view::SEARCH_ARCHIVE_COVER_WIDTH as u32,
                            super::library_search_view::SEARCH_ROW_THUMBNAIL_HEIGHT as u32,
                        )
                        .ok()
                    });
                sender.input(Msg::LibrarySearchThumbnailCacheLoadFinished {
                    generation: job.generation,
                    source: job.source,
                    thumbnail,
                });
            });
        }
        for job in jobs.generations {
            let sender = sender.clone();
            let cancel = cancel.clone();
            spawn_background(move || {
                let result =
                    generate_and_cache_bookshelf_thumbnail_with_cancel(limit, &job.source, &|| {
                        cancel.cancelled()
                    })
                    .map_err(|error| error.to_string());
                let result = cover_cancel::fit_generated_cover(
                    result,
                    super::library_search_view::SEARCH_ARCHIVE_COVER_WIDTH as u32,
                    super::library_search_view::SEARCH_ROW_THUMBNAIL_HEIGHT as u32,
                    &cancel,
                );
                sender.input(Msg::LibrarySearchThumbnailGenerationFinished {
                    generation: job.generation,
                    source: job.source,
                    result,
                });
            });
        }
    }

    pub(super) fn library_search_thumbnail_cache_load_finished(
        &mut self,
        generation: u64,
        source: PathBuf,
        thumbnail: Option<crate::bookshelf::thumbnail::ThumbnailData>,
        sender: &AppSender,
    ) {
        let accepted = self
            .library
            .search_thumbnails
            .accepts_cache_completion(generation, &source);
        let succeeded = accepted
            && thumbnail.is_some_and(|thumbnail| {
                self.library
                    .search_view
                    .show_thumbnail(generation, &source, thumbnail)
            });
        let completion = self
            .library
            .search_thumbnails
            .complete_cache_load(generation, &source, succeeded);
        debug_assert_eq!(completion.accepted, accepted);
        Self::spawn_library_search_thumbnail_jobs(
            self.settings.archive_expansion_limit,
            completion.jobs,
            self.library.search_thumbnails.cancellation_token(),
            sender,
        );
    }

    pub(super) fn library_search_thumbnail_generation_finished(
        &mut self,
        generation: u64,
        source: PathBuf,
        result: Result<Option<crate::bookshelf::thumbnail::ThumbnailData>, String>,
        sender: &AppSender,
    ) {
        if matches!(result, Ok(None)) {
            let jobs = self
                .library
                .search_thumbnails
                .cancel_generation(generation, &source);
            Self::spawn_library_search_thumbnail_jobs(
                self.settings.archive_expansion_limit,
                jobs,
                self.library.search_thumbnails.cancellation_token(),
                sender,
            );
            return;
        }
        let accepted = self
            .library
            .search_thumbnails
            .accepts_generation_completion(generation, &source);
        let succeeded = accepted
            && result.is_ok_and(|thumbnail| {
                thumbnail.is_some_and(|thumbnail| {
                    self.library
                        .search_view
                        .show_thumbnail(generation, &source, thumbnail)
                })
            });
        if accepted && !succeeded {
            self.library.search_view.show_failure(generation, &source);
        }
        let completion = self
            .library
            .search_thumbnails
            .complete_generation(generation, &source, succeeded);
        debug_assert_eq!(completion.accepted, accepted);
        Self::spawn_library_search_thumbnail_jobs(
            self.settings.archive_expansion_limit,
            completion.jobs,
            self.library.search_thumbnails.cancellation_token(),
            sender,
        );
    }
}
