use super::message::DispatchDomain;
use super::*;

impl App {
    pub(super) fn dispatch(&mut self, msg: Msg, sender: &AppSender) {
        let route = msg.route();
        if !route
            .input_policy
            .allows(self.is_viewer_active(), self.viewer_navigation_enabled())
        {
            return;
        }

        match route.domain {
            DispatchDomain::Navigation => self.dispatch_navigation(msg, sender),
            DispatchDomain::History => self.dispatch_history(msg, sender),
            DispatchDomain::Favorites => self.dispatch_favorites(msg, sender),
            DispatchDomain::Library => self.dispatch_library(msg, sender),
            DispatchDomain::LibrarySearch => self.dispatch_library_search(msg, sender),
            DispatchDomain::Document => self.dispatch_document(msg, sender),
            DispatchDomain::Viewer => self.dispatch_viewer(msg, sender),
            DispatchDomain::SettingsWindow => self.dispatch_settings_window(msg, sender),
        }
    }

    fn dispatch_navigation(&mut self, msg: Msg, sender: &AppSender) {
        let sender = sender.clone();
        match msg {
            Msg::OpenFile => self.open_file(&sender),
            Msg::NavigateUp => self.navigate_up(&sender),
            Msg::ToggleSearchPanel => {
                self.toggle_navigation_panel(NavigationPanel::Search, &sender)
            }
            Msg::ToggleHistoryPanel => {
                self.toggle_navigation_panel(NavigationPanel::History, &sender)
            }
            Msg::ToggleFavoritesPanel => {
                self.toggle_navigation_panel(NavigationPanel::Favorites, &sender)
            }
            Msg::CloseNavigationPanel | Msg::NavigationDrawerClosed => {
                self.close_navigation_panel(&sender)
            }
            unexpected => unreachable!("navigation domain received {unexpected:?}"),
        }
    }

    fn dispatch_history(&mut self, msg: Msg, sender: &AppSender) {
        let sender = sender.clone();
        match msg {
            Msg::HistoryCoverDemandChanged => {
                let jobs = self.navigation.history_view.demand_changed();
                Self::spawn_history_cover_jobs(jobs, &sender);
            }
            Msg::HistoryCoverCacheLoadFinished {
                generation,
                source,
                thumbnail,
            } => {
                let jobs = self
                    .navigation
                    .history_view
                    .cache_finished(generation, source, thumbnail);
                Self::spawn_history_cover_jobs(jobs, &sender);
            }
            Msg::HistoryCoverGenerationFinished {
                generation,
                source,
                result,
            } => {
                let jobs = self
                    .navigation
                    .history_view
                    .generation_finished(generation, source, result);
                Self::spawn_history_cover_jobs(jobs, &sender);
            }
            Msg::ResumeHistory {
                identity,
                path,
                page_index,
                at_document_end,
            } => {
                self.close_navigation_panel(&sender);
                self.start_path_load(
                    path.clone(),
                    history_resume_initial_page(at_document_end, page_index),
                    LoadPurpose::History { identity, path },
                    &sender,
                );
            }
            Msg::RemoveHistory(identity) => self.remove_history(identity, &sender),
            Msg::ConfirmClearHistory => self.request_clear_history_confirmation(&sender),
            Msg::ClearHistory => self.clear_history(&sender),
            unexpected => unreachable!("history domain received {unexpected:?}"),
        }
    }

    fn dispatch_favorites(&mut self, msg: Msg, sender: &AppSender) {
        let sender = sender.clone();
        match msg {
            Msg::FavoritesCoverDemandChanged => {
                let jobs = self.navigation.favorites_view.demand_changed();
                Self::spawn_favorites_cover_jobs(jobs, &sender);
            }
            Msg::FavoritesCoverCacheLoadFinished {
                generation,
                identity,
                source,
                thumbnail,
            } => {
                let jobs = self
                    .navigation
                    .favorites_view
                    .cache_finished(generation, identity, source, thumbnail);
                Self::spawn_favorites_cover_jobs(jobs, &sender);
            }
            Msg::FavoritesCoverGenerationFinished {
                generation,
                identity,
                source,
                result,
            } => {
                let jobs = self
                    .navigation
                    .favorites_view
                    .generation_finished(generation, identity, source, result);
                Self::spawn_favorites_cover_jobs(jobs, &sender);
            }
            Msg::OpenFavorite(path) => self.open_favorite(path, &sender),
            Msg::FavoriteOpenResolved {
                identity,
                entry_path,
            } => self.favorite_open_resolved(identity, entry_path, &sender),
            Msg::ToggleFavorite(identity) => self.toggle_favorite(identity, &sender),
            Msg::RemoveFavorite(identity) => self.remove_favorite(&identity, &sender),
            unexpected => unreachable!("favorites domain received {unexpected:?}"),
        }
    }

    fn dispatch_library_search(&mut self, msg: Msg, sender: &AppSender) {
        let sender = sender.clone();
        match msg {
            Msg::LibrarySearchQueryChanged(query) => self.set_library_search_query(query, &sender),
            Msg::LibrarySearchBuildFinished {
                request_id,
                root,
                result,
            } => self.library_search_build_finished(request_id, root, result, &sender),
            Msg::LibrarySearchThumbnailViewportChanged => {
                self.update_library_search_thumbnail_demand(&sender)
            }
            Msg::LibrarySearchThumbnailCacheLoadFinished {
                generation,
                source,
                thumbnail,
            } => self.library_search_thumbnail_cache_load_finished(
                generation, source, thumbnail, &sender,
            ),
            Msg::LibrarySearchThumbnailGenerationFinished {
                generation,
                source,
                result,
            } => self
                .library_search_thumbnail_generation_finished(generation, source, result, &sender),
            Msg::LibrarySearchFolderSelected {
                path,
                image_document_entry,
            } => {
                if let Some(entry) = image_document_entry {
                    self.open_image_folder(&path, entry, &sender);
                } else {
                    self.close_navigation_panel(&sender);
                    self.navigate_library(path, &sender);
                }
            }
            Msg::LibrarySearchArchiveSelected(path) => {
                self.close_navigation_panel(&sender);
                self.start_path_load(
                    path,
                    InitialPage::LoaderDefault,
                    LoadPurpose::Normal,
                    &sender,
                );
            }
            unexpected => unreachable!("library search domain received {unexpected:?}"),
        }
    }

    fn dispatch_library(&mut self, msg: Msg, sender: &AppSender) {
        let sender = sender.clone();
        match msg {
            Msg::SelectBookshelfRoot => self.select_bookshelf_root(&sender),
            Msg::BookshelfRootSelected(path) => self.set_bookshelf_root(path, &sender),
            Msg::SetLibrarySortKey(sort_key) => self.set_library_sort_key(sort_key, &sender),
            Msg::SetLibrarySortDirection(direction) => {
                self.set_library_sort_direction(direction, &sender)
            }
            Msg::LibraryFileSelected(path) => {
                self.close_navigation_panel(&sender);
                let initial_page = library_progress(self.navigation.history.entries(), &path)
                    .map(|progress| InitialPage::Saved(progress.page_index()))
                    .unwrap_or(InitialPage::LoaderDefault);
                self.start_path_load(path, initial_page, LoadPurpose::Normal, &sender);
            }
            Msg::LibraryFileReadFromStart(path) => {
                self.close_navigation_panel(&sender);
                self.start_path_load(
                    path,
                    InitialPage::LoaderDefault,
                    LoadPurpose::Normal,
                    &sender,
                );
            }
            Msg::LibraryImageFolderSelected { folder, entry } => {
                self.open_image_folder(&folder, entry, &sender);
            }
            Msg::LibraryImageFolderReadFromStart(entry) => {
                self.close_navigation_panel(&sender);
                self.start_path_load(
                    entry,
                    InitialPage::LoaderDefault,
                    LoadPurpose::Normal,
                    &sender,
                );
            }
            Msg::OpenArchiveContents(archive) => self.open_archive_contents(archive, &sender),
            Msg::NavigateArchiveContents(location) => {
                self.navigate_archive_contents(location, &sender)
            }
            Msg::ArchiveContentsLoaded { request_id, result } => {
                self.archive_contents_loaded(request_id, result, &sender)
            }
            Msg::ArchiveContentThumbnailReady {
                session_id,
                source,
                backed,
                thumbnail,
            } => self.archive_content_thumbnail_ready(session_id, source, backed, thumbnail),
            Msg::ArchiveContentThumbnailsFinished { session_id } => {
                self.archive_content_thumbnails_finished(session_id, &sender)
            }
            Msg::ArchiveContentThumbnailResourceLimit { session_id } => {
                self.archive_content_thumbnail_resource_limit(session_id, &sender)
            }
            Msg::ArchiveImageSelected { archive, id } => {
                self.start_archive_image_load(archive, id, &sender)
            }
            Msg::SetFolderImageCover { folder, image } => {
                let identity = crate::covers::CoverBookIdentity::image_folder(&folder);
                self.library.external_cover_requests.cancel(&identity);
                if crate::covers::CoverStore::load().set_folder_image(&folder, &image) {
                    self.cover_override_changed(&sender);
                }
            }
            Msg::SetArchiveImageCover { archive, id } => {
                let identity = crate::covers::CoverBookIdentity::Archive(archive.clone());
                self.library.external_cover_requests.cancel(&identity);
                if crate::covers::CoverStore::load().set_archive_entry(&archive, id) {
                    self.cover_override_changed(&sender);
                }
            }
            Msg::ClearCoverOverride(identity) => {
                self.library.external_cover_requests.cancel(&identity);
                crate::covers::CoverStore::load().clear(&identity);
                self.cover_override_changed(&sender);
            }
            Msg::SelectExternalCover(identity) => {
                self.select_external_cover(identity, &sender);
            }
            Msg::ExternalCoverSelected {
                identity,
                request_id,
                path,
            } => {
                if !self
                    .library
                    .external_cover_requests
                    .accepts(&identity, request_id)
                {
                    return;
                }
                let sender = sender.clone();
                crate::app::spawn_background(move || {
                    let result = crate::covers::CoverStore::load()
                        .prepare_external(identity.clone(), &path)
                        .map_err(|error| error.to_string());
                    sender.input(Msg::ExternalCoverPrepared {
                        identity,
                        request_id,
                        result,
                    });
                });
            }
            Msg::ExternalCoverPrepared {
                identity,
                request_id,
                result,
            } => match result {
                Ok(prepared)
                    if self
                        .library
                        .external_cover_requests
                        .accepts(&identity, request_id) =>
                {
                    if let Err(error) = crate::covers::CoverStore::load().commit_external(prepared)
                    {
                        eprintln!("外部表紙を保存できませんでした: {error}");
                    } else {
                        self.library
                            .external_cover_requests
                            .finish(&identity, request_id);
                        self.cover_override_changed(&sender);
                    }
                }
                Ok(prepared) => prepared.discard(),
                Err(error)
                    if self
                        .library
                        .external_cover_requests
                        .finish(&identity, request_id) =>
                {
                    eprintln!("外部表紙を保存できませんでした: {error}");
                }
                Err(_) => {}
            },
            Msg::NavigateLibrary(path) => self.navigate_library(path, &sender),
            Msg::LibraryScanFinished {
                request_id,
                path,
                result,
            } => self.library_scan_finished(request_id, path, result, &sender),
            Msg::LibraryThumbnailViewportChanged => self.update_library_thumbnail_demand(&sender),
            Msg::LibraryThumbnailCacheLoadFinished {
                generation,
                cohort_id,
                source,
                kind,
                thumbnail,
            } => self.library_thumbnail_cache_load_finished(
                generation, cohort_id, source, kind, thumbnail, &sender,
            ),
            Msg::LibraryThumbnailGenerationFinished {
                generation,
                source,
                kind,
                result,
            } => self
                .library_thumbnail_generation_finished(generation, source, kind, result, &sender),
            Msg::FlushLibraryThumbnailCacheHits { generation } => {
                self.flush_library_thumbnail_cache_hits(generation)
            }
            Msg::LibraryInitialRevealTimeout {
                generation,
                revision,
            } => self.library_initial_reveal_timeout(InitialRevealTimeout {
                generation,
                revision,
            }),
            Msg::FlushLibraryThumbnails {
                generation,
                batch_id,
                quiet_revision,
            } => self.flush_library_thumbnails(generation, batch_id, quiet_revision),
            unexpected => unreachable!("library domain received {unexpected:?}"),
        }
    }

    fn dispatch_document(&mut self, msg: Msg, sender: &AppSender) {
        let sender = sender.clone();
        match msg {
            Msg::PathDropped(path) => {
                self.close_navigation_panel(&sender);
                self.invalidate_document_boundary();
                self.document.sibling_lookup.invalidate();
                let sender = sender.clone();
                spawn_background(move || match resolve_dropped_path(&path) {
                    Ok(Some(path)) => sender.input(Msg::PathSelected(path)),
                    Ok(None) => {}
                    Err(error) => eprintln!("ドロップされたパスの確認に失敗しました: {error}"),
                });
            }
            Msg::PathSelected(path) => {
                self.close_navigation_panel(&sender);
                self.start_path_load(
                    path,
                    InitialPage::LoaderDefault,
                    LoadPurpose::Normal,
                    &sender,
                )
            }
            Msg::SiblingPathLookupFinished {
                request_id,
                adjacent_path,
            } => self.sibling_path_lookup_finished(request_id, adjacent_path, &sender),
            Msg::BoundaryPathLookupFinished {
                request_id,
                adjacent_path,
            } => self.document_boundary_lookup_finished(request_id, adjacent_path),
            Msg::OpenDocument {
                load_request_id,
                document,
                initial_index,
            } => {
                if !self.document.load.is_active(load_request_id) {
                    return;
                }
                if self
                    .document
                    .progressive_archive
                    .is_displayed_for(load_request_id)
                {
                    if !self.finish_progressive_archive_load(load_request_id, document, &sender) {
                        self.load_request_failed(load_request_id, &sender);
                    }
                    return;
                }
                self.document.progressive_archive.finish(load_request_id);
                if document.pages.is_empty()
                    && self
                        .document
                        .load
                        .purpose(load_request_id)
                        .is_some_and(|purpose| purpose.is_startup_restore() || purpose.is_history())
                {
                    self.load_request_failed(load_request_id, &sender);
                    return;
                }
                let has_pages = !document.pages.is_empty();
                self.load_document(document, initial_index, &sender);
                if has_pages {
                    self.settings.save();
                }
                self.finish_load_request(load_request_id);
            }
            Msg::ArchiveImageExtracted {
                load_request_id,
                path,
                image,
            } => self.archive_image_extracted(load_request_id, path, image, &sender),
            Msg::ShowLoading {
                load_request_id,
                revision,
            } => self.show_loading(load_request_id, revision),
            Msg::LoadRequestFinished { load_request_id } => {
                self.load_request_failed(load_request_id, &sender)
            }
            Msg::ArchiveResourceLimitReached { load_request_id } => {
                self.archive_resource_limit_failed(load_request_id, &sender)
            }
            Msg::FocusViewerAfterDocumentLoad => self.focus_viewer(),
            Msg::DocumentToastDismissed => self.restore_viewer_focus_after_toast(),
            unexpected => unreachable!("document domain received {unexpected:?}"),
        }
    }

    fn dispatch_viewer(&mut self, msg: Msg, sender: &AppSender) {
        let sender = sender.clone();
        match msg {
            Msg::NextPage => self.next_page(&sender),
            Msg::PrevPage => self.prev_page(&sender),
            Msg::NextSinglePage => self.next_single_page(&sender),
            Msg::PrevSinglePage => self.prev_single_page(&sender),
            Msg::NextFile => self.next_file(&sender),
            Msg::PrevFile => self.prev_file(false, &sender),
            Msg::ToggleViewMode => self.toggle_view_mode(&sender),
            Msg::UpdateSliderVisibility {
                distance_from_bottom,
                pointer_over_bar,
            } => self.update_slider_visibility(distance_from_bottom, pointer_over_bar),
            Msg::UpdateHeaderVisibility {
                distance_from_top,
                pointer_over_header,
            } => self.update_header_visibility(distance_from_top, pointer_over_header),
            Msg::SetSliderDragging(dragging) => self.set_slider_dragging(dragging),
            Msg::PreloadBytesReady {
                document_generation,
                asset_id,
                page_index,
                bytes,
            } => {
                self.preload_bytes_ready(document_generation, asset_id, page_index, bytes, &sender)
            }
            Msg::SmartCropPrepared {
                document_generation,
                asset_id,
                page_index,
                success,
            } => self.smart_crop_prepared(
                document_generation,
                asset_id,
                page_index,
                success,
                &sender,
            ),
            Msg::DecodeBytes {
                document_generation,
                asset_id,
                page_index,
            } => self.decode_bytes(document_generation, asset_id, page_index),
            Msg::ThumbnailReady {
                document_generation,
                generation,
                index,
                thumbnail,
            } => self.thumbnail_ready(document_generation, generation, index, thumbnail),
            Msg::ProgressiveThumbnailFinished {
                session_generation,
                preview_generation,
                asset_id,
                first_page,
                layout,
                thumbnails,
            } => self.progressive_thumbnail_finished(
                session_generation,
                preview_generation,
                asset_id,
                first_page,
                layout,
                thumbnails,
                &sender,
            ),
            Msg::HoverPreview {
                value,
                pointer_x,
                slider_width,
                slider_height,
            } => self.show_hover_preview(value, pointer_x, slider_width, slider_height, &sender),
            Msg::PreviewHoldExpired { hover_revision } => {
                self.previous_preview_hold_expired(hover_revision)
            }
            Msg::HidePreview => self.hide_hover_preview(),
            Msg::ImageClick { button, x, width } => self.handle_click(button, x, width, &sender),
            Msg::SetPage(val) => self.set_page(val, &sender),
            Msg::SetPageFromSliderPointer(val) => self.set_page_from_slider_pointer(val, &sender),
            Msg::Scroll { dy } => {
                if self.document_boundary_visible() {
                    if dy < 0.0 {
                        self.next_page(&sender);
                    } else if dy > 0.0 {
                        self.prev_page(&sender);
                    }
                } else if dy < 0.0 {
                    self.next_single_page(&sender);
                } else if dy > 0.0 {
                    self.prev_single_page(&sender);
                }
            }
            unexpected => unreachable!("viewer domain received {unexpected:?}"),
        }
    }

    fn dispatch_settings_window(&mut self, msg: Msg, sender: &AppSender) {
        let sender = sender.clone();
        match msg {
            Msg::SetClickMode(mode) => self.set_click_mode(mode),
            Msg::SetShowDocumentBoundaryPage(enabled) => {
                self.set_show_document_boundary_page(enabled)
            }
            Msg::SetScaleUp(scale_up) => self.set_scale_up(scale_up),
            Msg::SetSmartCrop(enabled) => self.set_smart_crop(enabled, &sender),
            Msg::SetViewMode(mode) => self.set_view_mode(mode, &sender),
            Msg::SetSliderAutoHide(auto_hide) => self.set_slider_auto_hide(auto_hide),
            Msg::SetHeaderAutoHide(auto_hide) => self.set_header_auto_hide(auto_hide),
            Msg::SetThumbnailsEnabled(enabled) => self.set_thumbnails_enabled(enabled, &sender),
            Msg::SetThumbnailGenerationSpeed(speed) => self.set_thumbnail_generation_speed(speed),
            Msg::FullscreenChanged(fullscreen) => self.fullscreen_changed(fullscreen),
            Msg::SetPreviewPosition(mode) => self.set_preview_position_mode(mode),
            Msg::SetLibraryBookHeight(height) => self.set_library_book_height(height, &sender),
            Msg::SetStartupBehavior(behavior) => self.set_startup_behavior(behavior),
            Msg::WindowStateChanged {
                width,
                height,
                maximized,
                fullscreened,
            } => self.update_window_state(width, height, maximized, fullscreened),
            unexpected => unreachable!("settings/window domain received {unexpected:?}"),
        }
    }
}
