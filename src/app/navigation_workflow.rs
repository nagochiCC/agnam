use super::*;

fn record_history_document(
    history: &mut HistoryStore,
    document_path: &Path,
    bookshelf_root: Option<&Path>,
    page_index: usize,
    page_count: usize,
    at_document_end: bool,
) -> HistoryIdentity {
    let identity =
        history.record_document(document_path, bookshelf_root, page_index, page_count, false);
    if history_at_document_end(document_path, &identity, at_document_end) {
        history.update_position(&identity, page_index, page_count, true);
    }
    identity
}

fn update_history_entry_position(
    history: &mut HistoryStore,
    identity: &HistoryIdentity,
    document_path: &Path,
    page_index: usize,
    page_count: usize,
    at_document_end: bool,
) -> bool {
    let at_document_end = history_at_document_end(document_path, identity, at_document_end);
    history.update_position(identity, page_index, page_count, at_document_end)
}

impl App {
    pub(super) fn record_current_document_in_history(&mut self, sender: &AppSender) {
        let Some(document_path) = self.viewer.session.document_path().map(ToOwned::to_owned) else {
            return;
        };
        let identity = record_history_document(
            &mut self.navigation.history,
            &document_path,
            self.settings.bookshelf_root.as_deref(),
            self.viewer.session.current_index(),
            self.viewer.session.page_count(),
            self.viewer.session.at_document_end(),
        );
        self.navigation.current_history_identity = Some(identity);
        self.navigation.history.save();
        self.sync_library_progress();
        self.mark_history_panel_dirty(sender);
    }

    pub(super) fn update_history_position(&mut self, sender: &AppSender) {
        let Some(identity) = self.navigation.current_history_identity.as_ref() else {
            return;
        };
        let Some(document_path) = self.viewer.session.document_path() else {
            return;
        };
        if update_history_entry_position(
            &mut self.navigation.history,
            identity,
            document_path,
            self.viewer.session.current_index(),
            self.viewer.session.page_count(),
            self.viewer.session.at_document_end(),
        ) {
            self.navigation.history.save();
            self.sync_library_progress();
            self.mark_history_panel_dirty(sender);
        }
    }

    pub(super) fn toggle_navigation_panel(
        &mut self,
        selected: NavigationPanel,
        sender: &AppSender,
    ) {
        if selected == NavigationPanel::Search && self.library.model.root().is_none() {
            return;
        }
        let panel = self.navigation.panel.toggled_selection(selected);
        self.set_navigation_panel(panel, sender);
    }

    fn set_navigation_panel(&mut self, panel: Option<NavigationPanel>, sender: &AppSender) {
        if self.navigation.panel.selected() == panel {
            return;
        }

        match self.navigation.panel.selected() {
            Some(NavigationPanel::Search) => {
                self.library.search_view.pause();
                self.suspend_library_search_thumbnail_demand(sender);
            }
            Some(NavigationPanel::History) => self.navigation.history_view.pause(),
            Some(NavigationPanel::Favorites) => self.navigation.favorites_view.pause(),
            None => {}
        }
        self.navigation.panel.set_selected(panel);
        self.sync_viewer_accelerators();

        match panel {
            Some(NavigationPanel::Search) => self.open_library_search_panel(sender),
            Some(NavigationPanel::History) => self.open_history_panel(sender),
            Some(NavigationPanel::Favorites) => self.open_favorites_panel(sender),
            None => {}
        }
    }

    pub(super) fn close_navigation_panel(&mut self, sender: &AppSender) {
        self.set_navigation_panel(None, sender);
    }

    fn open_library_search_panel(&mut self, sender: &AppSender) {
        let Some(root) = self.library.model.root().map(ToOwned::to_owned) else {
            return;
        };
        if let Some(request) = self.library.search.begin(&root) {
            self.navigation.panel.mark_dirty(NavigationPanel::Search);
            Self::spawn_library_search_build(request, sender);
        }
        if self.navigation.panel.is_dirty(NavigationPanel::Search) {
            self.render_library_search_results(sender);
        } else {
            self.library.search_view.resume();
        }
        self.focus_library_search_entry();
    }

    fn focus_library_search_entry(&self) {
        if let Some(entry) = self.library.search_entry.clone() {
            if entry.text().as_str() != self.library.search.query() {
                entry.set_text(self.library.search.query());
            }
            gtk::glib::idle_add_local_once(move || {
                entry.grab_focus();
            });
        }
    }

    pub(super) fn reset_library_search(&mut self) {
        self.library.search.reset();
        if let Some(entry) = &self.library.search_entry {
            entry.set_text("");
        }
        let generation = self.library.search_thumbnails.reset_session();
        self.library.search_view.reset_session(generation);
        self.navigation.panel.mark_dirty(NavigationPanel::Search);
    }

    fn open_history_panel(&mut self, sender: &AppSender) {
        if self.navigation.panel.is_dirty(NavigationPanel::History) {
            self.render_history_panel(sender);
        } else {
            let jobs = self.navigation.history_view.resume();
            Self::spawn_history_cover_jobs(self.settings.archive_expansion_limit, jobs, sender);
        }
    }

    pub(super) fn mark_history_panel_dirty(&mut self, sender: &AppSender) {
        self.navigation.panel.mark_dirty(NavigationPanel::History);
        if self.navigation.panel.is_selected(NavigationPanel::History) {
            self.render_history_panel(sender);
        }
    }

    fn sync_library_progress(&self) {
        if self.is_library_active() {
            self.library
                .view
                .sync_progress(self.navigation.history.entries());
        }
    }

    fn render_history_panel(&mut self, sender: &AppSender) {
        let resume_sender = sender.clone();
        let on_resume = Rc::new(move |identity, path, page_index, at_document_end| {
            resume_sender.input(Msg::ResumeHistory {
                identity,
                path,
                page_index,
                at_document_end,
            });
        });
        let remove_sender = sender.clone();
        let on_remove = Rc::new(move |identity| {
            remove_sender.input(Msg::RemoveHistory(identity));
        });
        let jobs = self.navigation.history_view.render(
            self.navigation.history.entries(),
            on_resume,
            on_remove,
            sender.clone(),
        );
        self.navigation.panel.mark_clean(NavigationPanel::History);
        Self::spawn_history_cover_jobs(self.settings.archive_expansion_limit, jobs, sender);
    }

    pub(super) fn spawn_history_cover_jobs(
        limit: crate::archive::ArchiveExpansionLimit,
        jobs: Vec<HistoryCoverJob>,
        sender: &AppSender,
    ) {
        for job in jobs {
            match job {
                HistoryCoverJob::LoadCache { generation, source } => {
                    let sender = sender.clone();
                    spawn_background(move || {
                        let thumbnail =
                            load_cached_bookshelf_thumbnail(&source).and_then(|thumbnail| {
                                crate::bookshelf::thumbnail::fit_thumbnail_data(
                                    thumbnail,
                                    super::history::HISTORY_COVER_WIDTH as u32,
                                    super::history::HISTORY_COVER_HEIGHT as u32,
                                )
                                .ok()
                            });
                        sender.input(Msg::HistoryCoverCacheLoadFinished {
                            generation,
                            source,
                            thumbnail,
                        });
                    });
                }
                HistoryCoverJob::Generate {
                    generation,
                    source,
                    cancel,
                } => {
                    let sender = sender.clone();
                    spawn_background(move || {
                        let result = generate_and_cache_bookshelf_thumbnail_with_cancel(
                            limit,
                            &source,
                            &|| cancel.cancelled(),
                        )
                        .map_err(|error| error.to_string());
                        let result = cover_cancel::fit_generated_cover(
                            result,
                            super::history::HISTORY_COVER_WIDTH as u32,
                            super::history::HISTORY_COVER_HEIGHT as u32,
                            &cancel,
                        );
                        sender.input(Msg::HistoryCoverGenerationFinished {
                            generation,
                            source,
                            result,
                        });
                    });
                }
            }
        }
    }

    fn open_favorites_panel(&mut self, sender: &AppSender) {
        if self.navigation.panel.is_dirty(NavigationPanel::Favorites) {
            self.render_favorites_panel(sender);
        } else {
            let jobs = self.navigation.favorites_view.resume();
            Self::spawn_favorites_cover_jobs(self.settings.archive_expansion_limit, jobs, sender);
        }
    }

    pub(super) fn mark_favorites_panel_dirty(&mut self, sender: &AppSender) {
        self.navigation.panel.mark_dirty(NavigationPanel::Favorites);
        if self
            .navigation
            .panel
            .is_selected(NavigationPanel::Favorites)
        {
            self.render_favorites_panel(sender);
        }
    }

    fn render_favorites_panel(&mut self, sender: &AppSender) {
        let open_sender = sender.clone();
        let on_open = Rc::new(move |path| open_sender.input(Msg::OpenFavorite(path)));
        let remove_sender = sender.clone();
        let on_remove = Rc::new(move |path| remove_sender.input(Msg::RemoveFavorite(path)));
        let jobs = self.navigation.favorites_view.render(
            self.navigation.favorites.entries(),
            self.settings.bookshelf_root.as_deref(),
            on_open,
            on_remove,
            sender.clone(),
        );
        self.navigation.panel.mark_clean(NavigationPanel::Favorites);
        Self::spawn_favorites_cover_jobs(self.settings.archive_expansion_limit, jobs, sender);
    }

    pub(super) fn spawn_favorites_cover_jobs(
        limit: crate::archive::ArchiveExpansionLimit,
        jobs: Vec<FavoritesCoverJob>,
        sender: &AppSender,
    ) {
        for job in jobs {
            match job {
                FavoritesCoverJob::LoadCache {
                    generation,
                    identity,
                } => {
                    let sender = sender.clone();
                    spawn_background(move || {
                        let source = match &identity {
                            FavoriteIdentity::ImageFolderDocument(folder) => {
                                crate::archive::cover_image_in_folder(folder)
                            }
                            FavoriteIdentity::FileDocument(_) => identity.resolve_entry_path(),
                        };
                        let thumbnail = source
                            .as_deref()
                            .and_then(load_cached_bookshelf_thumbnail)
                            .and_then(|thumbnail| {
                                crate::bookshelf::thumbnail::fit_thumbnail_data(
                                    thumbnail,
                                    super::favorites::COVER_WIDTH as u32,
                                    super::favorites::COVER_HEIGHT as u32,
                                )
                                .ok()
                            });
                        sender.input(Msg::FavoritesCoverCacheLoadFinished {
                            generation,
                            identity,
                            source,
                            thumbnail,
                        });
                    });
                }
                FavoritesCoverJob::Generate {
                    generation,
                    identity,
                    source,
                    cancel,
                } => {
                    let sender = sender.clone();
                    spawn_background(move || {
                        let result = generate_and_cache_bookshelf_thumbnail_with_cancel(
                            limit,
                            &source,
                            &|| cancel.cancelled(),
                        )
                        .map_err(|error| error.to_string());
                        let result = cover_cancel::fit_generated_cover(
                            result,
                            super::favorites::COVER_WIDTH as u32,
                            super::favorites::COVER_HEIGHT as u32,
                            &cancel,
                        );
                        sender.input(Msg::FavoritesCoverGenerationFinished {
                            generation,
                            identity,
                            source,
                            result,
                        });
                    });
                }
            }
        }
    }

    pub(super) fn toggle_favorite(&mut self, identity: FavoriteIdentity, sender: &AppSender) {
        let changed = if self.navigation.favorites.contains(&identity) {
            self.navigation.favorites.remove(&identity)
        } else {
            self.navigation.favorites.add(identity)
        };
        if changed {
            self.navigation.favorites.save();
            self.library
                .view
                .sync_favorites(self.navigation.favorites.entries());
            self.mark_favorites_panel_dirty(sender);
        }
    }

    pub(super) fn remove_favorite(&mut self, identity: &FavoriteIdentity, sender: &AppSender) {
        if self.navigation.favorites.remove(identity) {
            self.navigation.favorites.save();
            self.library
                .view
                .sync_favorites(self.navigation.favorites.entries());
            self.mark_favorites_panel_dirty(sender);
        }
    }

    pub(super) fn open_favorite(&mut self, identity: FavoriteIdentity, sender: &AppSender) {
        self.close_navigation_panel(sender);
        match &identity {
            FavoriteIdentity::FileDocument(path) => {
                self.favorite_open_resolved(identity.clone(), Some(path.clone()), sender)
            }
            FavoriteIdentity::ImageFolderDocument(_) => {
                let sender = sender.clone();
                spawn_background(move || {
                    let entry_path = identity.resolve_entry_path();
                    sender.input(Msg::FavoriteOpenResolved {
                        identity,
                        entry_path,
                    });
                });
            }
        }
    }

    pub(super) fn favorite_open_resolved(
        &mut self,
        identity: FavoriteIdentity,
        entry_path: Option<PathBuf>,
        sender: &AppSender,
    ) {
        let Some(path) = entry_path else {
            self.show_favorite_open_failed_dialog(identity, sender);
            return;
        };
        let initial_page = favorite_progress(self.navigation.history.entries(), &identity)
            .map(|progress| InitialPage::Saved(progress.page_index()))
            .unwrap_or(InitialPage::LoaderDefault);
        self.start_path_load(
            path,
            initial_page,
            LoadPurpose::Favorite { identity },
            sender,
        );
    }

    pub(super) fn open_image_folder(&mut self, folder: &Path, entry: PathBuf, sender: &AppSender) {
        self.close_navigation_panel(sender);
        let initial_page = image_folder_progress(self.navigation.history.entries(), folder)
            .map(|progress| InitialPage::Saved(progress.page_index()))
            .unwrap_or(InitialPage::LoaderDefault);
        self.start_path_load(entry, initial_page, LoadPurpose::Normal, sender);
    }

    pub(super) fn remove_history(&mut self, identity: HistoryIdentity, sender: &AppSender) {
        if self.navigation.history.remove(&identity) {
            self.navigation.history.save();
            self.sync_library_progress();
            self.mark_history_panel_dirty(sender);
        }
    }

    pub(super) fn request_clear_history_confirmation(&self, sender: &AppSender) {
        if self.navigation.history.entries().is_empty() {
            return;
        }
        self.confirm_clear_history(sender);
    }

    fn confirm_clear_history(&self, sender: &AppSender) {
        if self.navigation.history.entries().is_empty() {
            return;
        }
        let Some(window) = self.shell.main_window.as_ref() else {
            return;
        };
        let dialog = adw::AlertDialog::new(
            Some("閲覧履歴をすべて削除しますか？"),
            Some("この操作は元に戻せません。"),
        );
        dialog.add_responses(&[("cancel", "キャンセル"), ("clear", "すべて削除")]);
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
        let sender = sender.clone();
        dialog.choose(window, adw::gio::Cancellable::NONE, move |response| {
            if response == "clear" {
                sender.input(Msg::ClearHistory);
            }
        });
    }

    pub(super) fn clear_history(&mut self, sender: &AppSender) {
        if self.navigation.history.clear() {
            self.navigation.history.save();
            self.sync_library_progress();
            self.mark_history_panel_dirty(sender);
        }
    }

    pub(super) fn show_history_open_failed_dialog(
        &self,
        identity: HistoryIdentity,
        path: PathBuf,
        sender: &AppSender,
    ) -> bool {
        self.show_history_failed_dialog(identity, path.display().to_string(), sender)
    }

    pub(super) fn show_history_resource_limit_dialog(
        &self,
        identity: HistoryIdentity,
        path: PathBuf,
        message: &str,
        sender: &AppSender,
    ) -> bool {
        self.show_history_failed_dialog(
            identity,
            format!("{message}\n\n{}", path.display()),
            sender,
        )
    }

    fn show_history_failed_dialog(
        &self,
        identity: HistoryIdentity,
        body: String,
        sender: &AppSender,
    ) -> bool {
        let Some(window) = self.shell.main_window.as_ref() else {
            return false;
        };
        let dialog = adw::AlertDialog::new(Some("履歴を開けません"), Some(&body));
        dialog.add_responses(&[("close", "閉じる"), ("remove", "履歴から削除")]);
        dialog.set_close_response("close");
        dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        let sender = sender.clone();
        dialog.choose(window, adw::gio::Cancellable::NONE, move |response| {
            if response == "remove" {
                sender.input(Msg::RemoveHistory(identity));
            }
        });
        true
    }

    pub(super) fn show_favorite_open_failed_dialog(
        &self,
        identity: FavoriteIdentity,
        sender: &AppSender,
    ) -> bool {
        let body = identity.path().display().to_string();
        self.show_favorite_failed_dialog(identity, body, sender)
    }

    pub(super) fn show_favorite_resource_limit_dialog(
        &self,
        identity: FavoriteIdentity,
        message: &str,
        sender: &AppSender,
    ) -> bool {
        let body = format!("{message}\n\n{}", identity.path().display());
        self.show_favorite_failed_dialog(identity, body, sender)
    }

    fn show_favorite_failed_dialog(
        &self,
        identity: FavoriteIdentity,
        body: String,
        sender: &AppSender,
    ) -> bool {
        let Some(window) = self.shell.main_window.as_ref() else {
            return false;
        };
        let dialog = adw::AlertDialog::new(Some("お気に入りを開けません"), Some(&body));
        dialog.add_responses(&[("close", "閉じる"), ("remove", "お気に入りから削除")]);
        dialog.set_close_response("close");
        dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        let sender = sender.clone();
        dialog.choose(window, adw::gio::Cancellable::NONE, move |response| {
            if response == "remove" {
                sender.input(Msg::RemoveFavorite(identity));
            }
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::history_identity_for_document;

    #[test]
    fn recording_final_document_preserves_completion_and_reopen_behavior() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("books");
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let first = work.join("01.cbz");
        let last = work.join("02.cbz");
        std::fs::write(&first, []).unwrap();
        std::fs::write(&last, []).unwrap();
        let mut history =
            HistoryStore::load_or_empty_from_path(directory.path().join("history.ini"));

        let first_identity =
            record_history_document(&mut history, &first, Some(&root), 9, 10, true);
        assert!(!history.entries()[0].at_document_end);
        let last_identity = record_history_document(&mut history, &last, Some(&root), 9, 10, true);
        assert_eq!(last_identity, first_identity);
        assert!(history.entries()[0].at_document_end);
        assert_eq!(history.entries()[0].page_index, 9);

        record_history_document(&mut history, &last, Some(&root), 0, 10, false);
        assert!(!history.entries()[0].at_document_end);
        assert_eq!(history.entries()[0].page_index, 0);
    }

    #[test]
    fn position_updates_keep_the_identity_adopted_before_bookshelf_root_changes() {
        let directory = tempfile::tempdir().unwrap();
        let old_root = directory.path().join("old-root");
        let new_root = directory.path().join("new-root");
        let work = old_root.join("series");
        let first = work.join("01.cbz");
        let second = work.join("02.cbz");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(&first, []).unwrap();
        std::fs::write(&second, []).unwrap();
        let mut history =
            HistoryStore::load_or_empty_from_path(directory.path().join("history.ini"));
        let adopted_identity = history.record_document(&first, Some(&old_root), 0, 10, false);
        std::fs::create_dir_all(&new_root).unwrap();

        assert_ne!(
            history_identity_for_document(&first, Some(&new_root)),
            adopted_identity
        );
        assert!(update_history_entry_position(
            &mut history,
            &adopted_identity,
            &first,
            4,
            10,
            true,
        ));

        assert_eq!(history.entries().len(), 1);
        assert_eq!(history.entries()[0].identity, adopted_identity);
        assert!(!history.entries()[0].at_document_end);
        assert_eq!(history.entries()[0].page_index, 4);
    }
}
