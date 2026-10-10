use super::*;

impl App {
    pub(super) fn schedule_thumbnail_generation(&mut self, sender: &AppSender) {
        self.schedule_thumbnail_generation_skipping(Default::default(), sender);
    }

    pub(super) fn schedule_thumbnail_generation_skipping(
        &mut self,
        mut already_generated: std::collections::HashSet<AssetId>,
        _sender: &AppSender,
    ) {
        if !self.settings.thumbnails_enabled || self.progressive_archive_loading() {
            return;
        }
        let Some(document) = self.viewer.session.document() else {
            return;
        };
        let assets = document.assets.clone();
        let pages = document.pages.clone();
        already_generated.retain(|asset_id| {
            assets.get(asset_id.0).is_some_and(|asset| {
                self.viewer
                    .preview
                    .has_cached_asset(asset.first_page, asset.layout)
            })
        });
        let document_generation = self.viewer.session.document_generation();
        let (generation, cancel) = self.viewer.preview.generation_context();
        self.viewer.background.replace_thumbnails(
            document.path.clone(),
            document_generation,
            generation,
            cancel,
            assets,
            pages,
            already_generated,
        );
        if !self.viewer.preview.distributed_warmup_needed() {
            self.viewer
                .background
                .stop_distributed_warmup(document_generation, generation);
        }
    }

    pub(super) fn thumbnail_ready(
        &mut self,
        document_generation: u64,
        generation: u64,
        idx: usize,
        thumbnail: Thumbnail,
    ) {
        if !self.settings.thumbnails_enabled
            || document_generation != self.viewer.session.document_generation()
        {
            return;
        }
        let ready = self
            .viewer
            .preview
            .thumbnail_ready(generation, idx, thumbnail);
        if !ready.accepted() {
            return;
        }
        if ready.warmup_saturated() {
            self.viewer
                .background
                .stop_distributed_warmup(document_generation, generation);
        }
        self.viewer.preview.display_current_cached_if_placeholder();
    }

    pub(super) fn show_hover_preview(
        &mut self,
        val: f64,
        pointer_x: i32,
        slider_width: i32,
        slider_height: i32,
        sender: &AppSender,
    ) {
        if self.document.load.loading_visible() || self.document_boundary_visible() {
            self.hide_hover_preview();
            return;
        }
        if !self.viewer.slider.is_visible() {
            self.hide_hover_preview();
            return;
        }
        if !self.settings.thumbnails_enabled {
            return;
        }
        if self.progressive_archive_loading() {
            self.show_progressive_hover_preview(
                val,
                pointer_x,
                slider_width,
                slider_height,
                sender,
            );
            return;
        }
        let Some(index) = snap_to_view(val.round() as usize, self.viewer.session.view_starts())
        else {
            return;
        };
        let raw_target = self.preview_target_at(index);
        let raw_target_changed = self.viewer.preview.raw_hover_target() != Some(raw_target);
        let nearest_ready = if self.viewer.preview.raw_hover_target() == Some(raw_target) {
            None
        } else {
            nearest_ready_view(index, self.viewer.session.view_starts(), |candidate| {
                let target = self.preview_target_at(candidate);
                self.viewer.preview.has_cached_preview(target) || self.has_image_preview(target)
            })
            .map(|candidate| self.preview_target_at(candidate))
        };
        let (image_right, image_left) = nearest_ready
            .map(|target| self.preview_image_textures(target.first_page, target.second_page))
            .unwrap_or((None, None));
        self.present_hover_preview(
            raw_target,
            nearest_ready,
            (pointer_x, slider_width, slider_height),
            (image_right, image_left),
            sender,
        );
        if raw_target_changed {
            self.request_hover_thumbnails(raw_target);
        }
    }

    fn request_hover_thumbnails(&self, target: PreviewTarget) {
        let Some(document) = self.viewer.session.document() else {
            return;
        };
        let mut seen = std::collections::HashSet::new();
        let requests = [Some(target.first_page), target.second_page]
            .into_iter()
            .flatten()
            .filter_map(|page_index| self.viewer.session.page_asset_id(page_index))
            .filter(|asset_id| seen.insert(*asset_id))
            .filter_map(|asset_id| {
                let asset = document.assets.get(asset_id.0)?;
                (!self
                    .viewer
                    .preview
                    .has_cached_asset(asset.first_page, asset.layout))
                .then(|| HoverThumbnailRequest {
                    asset_id,
                    bytes: self.viewer.session.cached_bytes(asset_id),
                })
            })
            .collect();
        let document_generation = self.viewer.session.document_generation();
        let generation = self.viewer.preview.generation_context().0;
        self.viewer
            .background
            .replace_hover_thumbnails(document_generation, generation, requests);
    }

    fn show_progressive_hover_preview(
        &mut self,
        val: f64,
        pointer_x: i32,
        slider_width: i32,
        slider_height: i32,
        sender: &AppSender,
    ) {
        let Some(last_index) = self.slider_item_count().checked_sub(1) else {
            return;
        };
        let physical_index = (val.round() as usize).min(last_index);
        let raw_target = self
            .progressive_preview_target_at(physical_index)
            .unwrap_or_else(|| PreviewTarget::unavailable_physical(physical_index));
        let nearest_ready = if self.viewer.preview.raw_hover_target() == Some(raw_target) {
            None
        } else {
            nearest_ready_physical(
                physical_index,
                self.viewer.session.physical_image_count(),
                |candidate| {
                    self.progressive_preview_target_at(candidate)
                        .is_some_and(|target| {
                            self.viewer.preview.has_cached_preview(target)
                                || self.has_image_preview(target)
                        })
                },
            )
            .and_then(|candidate| self.progressive_preview_target_at(candidate))
        };
        let image_textures = nearest_ready
            .map(|target| self.preview_image_textures(target.first_page, target.second_page))
            .unwrap_or((None, None));
        self.present_hover_preview(
            raw_target,
            nearest_ready,
            (pointer_x, slider_width, slider_height),
            image_textures,
            sender,
        );

        if self.progressive_preview_target_at(physical_index).is_some() {
            self.request_progressive_thumbnails(raw_target, sender);
        } else {
            self.document.progressive_thumbnails.clear_pending();
        }
    }

    fn present_hover_preview(
        &mut self,
        raw_target: PreviewTarget,
        nearest_ready: Option<PreviewTarget>,
        pointer: (i32, i32, i32),
        image_textures: (Option<gtk::gdk::Paintable>, Option<gtk::gdk::Paintable>),
        sender: &AppSender,
    ) {
        let hold_revision = self.viewer.preview.show_hover_preview(
            raw_target,
            nearest_ready,
            pointer,
            self.settings.preview_position,
            image_textures,
        );
        if let Some(hover_revision) = hold_revision {
            let sender = sender.clone();
            gtk::glib::timeout_add_local_once(PREVIOUS_PREVIEW_HOLD_DURATION, move || {
                sender.input(Msg::PreviewHoldExpired { hover_revision });
            });
        }
    }

    fn progressive_preview_target_at(&self, physical_index: usize) -> Option<PreviewTarget> {
        let (first_page, second_page) = self
            .viewer
            .session
            .progressive_preview_pages(physical_index)?;
        Some(PreviewTarget::for_physical(
            physical_index,
            first_page,
            second_page,
        ))
    }

    fn request_progressive_thumbnails(&mut self, target: PreviewTarget, sender: &AppSender) {
        let (preview_generation, _) = self.viewer.preview.generation_context();
        let mut asset_ids = vec![self.viewer.session.page_asset_id(target.first_page)];
        if let Some(second_page) = target.second_page {
            asset_ids.push(self.viewer.session.page_asset_id(second_page));
        }
        let mut seen = std::collections::HashSet::new();
        let requests = asset_ids
            .into_iter()
            .flatten()
            .filter(|asset_id| seen.insert(*asset_id))
            .filter_map(|asset_id| {
                let (first_page, layout, source, temp_dir) =
                    self.viewer.session.progressive_thumbnail_source(asset_id)?;
                (!self.viewer.preview.has_cached_asset(first_page, layout)).then_some(
                    ProgressiveThumbnailRequest {
                        document_path: self.viewer.session.document_path()?.to_path_buf(),
                        asset_id,
                        first_page,
                        layout,
                        source,
                        temp_dir,
                        preview_generation,
                    },
                )
            })
            .collect::<Vec<_>>();
        if let Some(job) = self.document.progressive_thumbnails.request(requests) {
            Self::spawn_progressive_thumbnail(job, sender);
        }
    }

    pub(super) fn refresh_progressive_hover_mapping(&mut self, sender: &AppSender) {
        let Some((pointer_x, slider_width, slider_height)) = self.viewer.preview.pointer() else {
            return;
        };
        if slider_width <= 0 {
            return;
        }
        let position = (1.0 - f64::from(pointer_x) / f64::from(slider_width)).clamp(0.0, 1.0);
        let value = position * self.slider_item_count().saturating_sub(1) as f64;
        let physical_index = value.round() as usize;
        let target = self
            .progressive_preview_target_at(physical_index)
            .unwrap_or_else(|| PreviewTarget::unavailable_physical(physical_index));
        if self.viewer.preview.raw_hover_target() == Some(target) {
            return;
        }
        self.viewer.preview.reset_hover_selection_for_view_change();
        self.show_progressive_hover_preview(value, pointer_x, slider_width, slider_height, sender);
    }

    fn spawn_progressive_thumbnail(job: ProgressiveThumbnailJob, sender: &AppSender) {
        let sender = sender.clone();
        spawn_background(move || {
            let request = job.request;
            let cache = crate::thumbnail::ProgressiveThumbnailCacheAttempt::new(
                &request.document_path,
                request.asset_id.0,
                request.layout,
            );
            let cached = cache.load();
            let from_disk = cached.is_some();
            let thumbnails = cached.or_else(|| {
                request
                    .load_bytes()
                    .and_then(|bytes| make_memory_asset_thumbnails(&bytes, request.layout))
            });
            let to_save = (!from_disk).then(|| thumbnails.clone()).flatten();
            sender.input(Msg::ProgressiveThumbnailFinished {
                session_generation: job.session_generation,
                preview_generation: request.preview_generation,
                asset_id: request.asset_id,
                first_page: request.first_page,
                layout: request.layout,
                thumbnails,
            });
            if let Some(thumbnails) = to_save {
                cache.save_later(thumbnails);
            }
        });
    }

    pub(super) fn progressive_thumbnail_finished(
        &mut self,
        session_generation: u64,
        preview_generation: u64,
        asset_id: AssetId,
        first_page: usize,
        layout: crate::document::ImageLayout,
        thumbnails: Option<Vec<(usize, Thumbnail)>>,
        sender: &AppSender,
    ) {
        if !self.settings.thumbnails_enabled
            || !self
                .document
                .progressive_thumbnails
                .accepts_completion(session_generation, asset_id)
        {
            return;
        }

        let mut accepted_all = thumbnails.is_some();
        if let Some(thumbnails) = thumbnails {
            for (offset, thumbnail) in thumbnails {
                let Some(page_index) = first_page.checked_add(offset) else {
                    accepted_all = false;
                    continue;
                };
                accepted_all &= self
                    .viewer
                    .preview
                    .thumbnail_ready(preview_generation, page_index, thumbnail)
                    .accepted();
            }
        }
        let cached_successfully =
            accepted_all && self.viewer.preview.has_cached_asset(first_page, layout);
        let next = self.document.progressive_thumbnails.complete(
            session_generation,
            asset_id,
            cached_successfully,
        );
        if cached_successfully {
            self.viewer.preview.display_current_cached_if_placeholder();
        }
        if let Some(job) = next {
            Self::spawn_progressive_thumbnail(job, sender);
        }
    }

    fn preview_target_at(&self, index: usize) -> PreviewTarget {
        let second_page = self
            .viewer
            .session
            .is_full_spread_at(index)
            .then_some(index + 1);
        PreviewTarget::new(index, second_page)
    }

    fn has_image_preview(&self, target: PreviewTarget) -> bool {
        !self.settings.smart_crop
            && self.viewer.session.has_texture(target.first_page)
            && target
                .second_page
                .is_none_or(|page| self.viewer.session.has_texture(page))
    }

    fn preview_image_textures(
        &self,
        index: usize,
        second_page: Option<usize>,
    ) -> (Option<gtk::gdk::Paintable>, Option<gtk::gdk::Paintable>) {
        if self.settings.smart_crop {
            return (None, None);
        }
        let right = self.viewer.session.texture(index);
        let left = second_page.and_then(|page| self.viewer.session.texture(page));
        if second_page.is_some() && (right.is_none() || left.is_none()) {
            (None, None)
        } else {
            (right, left)
        }
    }

    pub(super) fn previous_preview_hold_expired(&mut self, hover_revision: u64) {
        self.viewer
            .preview
            .previous_preview_hold_expired(hover_revision);
    }

    pub(super) fn hide_hover_preview(&mut self) {
        self.document.progressive_thumbnails.clear_pending();
        let document_generation = self.viewer.session.document_generation();
        let generation = self.viewer.preview.generation_context().0;
        self.viewer.background.replace_hover_thumbnails(
            document_generation,
            generation,
            Vec::new(),
        );
        self.viewer.preview.hide();
    }

    pub(super) fn hide_preview_if_slider_hidden(&mut self) {
        if !self.viewer.slider.is_visible() {
            self.hide_hover_preview();
        }
    }

    pub(super) fn rebuild_hover_preview_for_view_change(&mut self, sender: &AppSender) {
        let Some((pointer_x, slider_width, slider_height)) = self.viewer.preview.pointer() else {
            return;
        };
        if slider_width <= 0 {
            return;
        }
        let position = (1.0 - f64::from(pointer_x) / f64::from(slider_width)).clamp(0.0, 1.0);
        let value = position * self.slider_item_count().saturating_sub(1) as f64;
        self.viewer.preview.reset_hover_selection_for_view_change();
        self.show_hover_preview(value, pointer_x, slider_width, slider_height, sender);
    }

    pub(super) fn schedule_preload(&mut self, sender: &AppSender) {
        if self.viewer.session.document().is_none() {
            return;
        }
        let mut queued_assets = std::collections::HashSet::new();
        let mut requests = Vec::new();
        let mut near = Vec::new();
        let document_generation = self.viewer.session.document_generation();
        let texture_preload_indices = self.viewer.session.texture_preload_indices();
        let byte_preload_indices = self.viewer.session.byte_preload_indices();
        let smart_crop_targets = self
            .settings
            .smart_crop
            .then(|| {
                byte_preload_indices
                    .iter()
                    .filter(|&&index| !self.viewer.session.has_texture(index))
                    .filter_map(|&index| self.viewer.session.page_asset_id(index))
                    .filter(|&asset_id| self.viewer.session.cached_crop_result(asset_id).is_none())
            })
            .into_iter()
            .flatten();
        self.viewer
            .smart_crop
            .set_targets(document_generation, smart_crop_targets);
        if self.settings.smart_crop {
            let handle = self.viewer.smart_crop.handle();
            for &texture_index in &texture_preload_indices {
                if self.viewer.session.has_texture(texture_index) {
                    continue;
                }
                let Some(asset_id) = self.viewer.session.page_asset_id(texture_index) else {
                    continue;
                };
                let Some(prepared) = handle.take_ready(document_generation, asset_id) else {
                    continue;
                };
                self.viewer.session.accept_crop_analysis(
                    document_generation,
                    asset_id,
                    texture_index,
                    prepared,
                );
                Self::queue_decode(texture_index, asset_id, document_generation, sender);
            }
        }
        let thumbnail_generation = (self.settings.thumbnails_enabled
            && !self.progressive_archive_loading())
        .then(|| self.viewer.preview.generation_context().0);

        for idx in byte_preload_indices {
            let Some(asset_id) = self.viewer.session.page_asset_id(idx) else {
                continue;
            };
            let Some(source) = self.viewer.session.image_source_for_page(idx) else {
                continue;
            };
            let Some(layout) = self.viewer.session.image_layout_for_page(idx) else {
                continue;
            };

            if !queued_assets.insert(asset_id) {
                continue;
            }

            let texture_index = texture_preload_indices.iter().copied().find(|&idx| {
                self.viewer.session.page_asset_id(idx) == Some(asset_id)
                    && !self.viewer.session.has_texture(idx)
            });
            let prepare_smart_crop = self.settings.smart_crop
                && !self.viewer.session.has_texture(idx)
                && self.viewer.session.cached_crop_result(asset_id).is_none();
            if let Some(bytes) = self.viewer.session.cached_bytes(asset_id) {
                if prepare_smart_crop {
                    self.viewer.smart_crop.queue(SmartCropPreparationRequest {
                        document_generation,
                        asset_id,
                        page_index: idx,
                        bytes: bytes.clone(),
                        layout,
                    });
                } else if let Some(texture_index) = texture_index {
                    // smart crop OFFの既存idle decode経路は維持する。
                    Self::queue_decode(texture_index, asset_id, document_generation, sender);
                }
                if thumbnail_generation.is_some() {
                    near.push(NearThumbnailRequest {
                        asset_id,
                        bytes: Some(bytes),
                    });
                }
            } else {
                // バイトデータがない場合、ファイル/アーカイブからの展開・読込を完全にバックグラウンドへオフロード
                requests.push(PreloadRequest {
                    asset_id,
                    page_index: idx,
                    source: source.clone(),
                });
                if thumbnail_generation.is_some() {
                    near.push(NearThumbnailRequest {
                        asset_id,
                        bytes: None,
                    });
                }
            }
        }
        self.viewer.background.replace_preloads(
            document_generation,
            thumbnail_generation,
            requests,
            near,
        );
    }

    pub(super) fn next_page(&mut self, sender: &AppSender) {
        if self.document.progressive_archive.is_waiting_for_page() {
            return;
        }
        if self.document_boundary_visible() {
            self.navigate_from_boundary(
                BoundaryDirection::Next,
                InitialPage::LoaderDefault,
                sender,
            );
            return;
        }
        if self.viewer.session.next_page() {
            self.document.boundary.cancel_lookup();
            self.sync_viewer_position(sender);
        } else if self.progressive_archive_loading() {
            self.wait_for_progressive_page(PendingPageMove::Next, sender);
        } else if self.sibling_navigation_allowed() {
            self.cross_document_edge(BoundaryDirection::Next, false, sender);
        }
    }

    pub(super) fn next_single_page(&mut self, sender: &AppSender) {
        if self.document.progressive_archive.is_waiting_for_page()
            || self.document_boundary_visible()
        {
            return;
        }
        if self.viewer.session.next_single_page() {
            self.document.boundary.cancel_lookup();
            self.sync_viewer_position(sender);
            self.rebuild_hover_preview_for_view_change(sender);
        } else if self.progressive_archive_loading() {
            self.wait_for_progressive_page(PendingPageMove::NextSingle, sender);
        }
    }

    pub(super) fn prev_page(&mut self, sender: &AppSender) {
        if self.document.progressive_archive.is_waiting_for_page() {
            return;
        }
        if self.document_boundary_visible() {
            self.navigate_from_boundary(BoundaryDirection::Prev, InitialPage::End, sender);
            return;
        }
        if self.viewer.session.prev_page() {
            self.document.boundary.cancel_lookup();
            self.sync_viewer_position(sender);
        } else if self.sibling_navigation_allowed() {
            self.cross_document_edge(BoundaryDirection::Prev, false, sender);
        }
    }

    pub(super) fn prev_single_page(&mut self, sender: &AppSender) {
        if self.document.progressive_archive.is_waiting_for_page()
            || self.document_boundary_visible()
        {
            return;
        }
        if self.viewer.session.prev_single_page() {
            self.document.boundary.cancel_lookup();
            self.sync_viewer_position(sender);
            self.rebuild_hover_preview_for_view_change(sender);
        }
    }

    fn wait_for_progressive_page(&mut self, movement: PendingPageMove, sender: &AppSender) {
        if !self.document.progressive_archive.request_move(movement) {
            return;
        }
        let Some(load_request_id) = self.document.load.active_request_id() else {
            return;
        };
        let Some(revision) = self.document.load.restart_loading_delay(load_request_id) else {
            return;
        };
        Self::schedule_loading(load_request_id, revision, sender);
    }

    pub(super) fn handle_click(&mut self, button: u32, x: f64, width: i32, sender: &AppSender) {
        match decide_click_direction(self.settings.click_mode, button, x, width) {
            ClickDirection::Next => self.next_page(sender),
            ClickDirection::Prev => self.prev_page(sender),
            ClickDirection::None => (),
        }
    }

    pub(super) fn preload_bytes_ready(
        &mut self,
        document_generation: u64,
        asset_id: AssetId,
        page_index: usize,
        bytes: gtk::glib::Bytes,
        sender: &AppSender,
    ) {
        // Byte cacheへ先に反映し、smart crop CPU処理は独立laneへ渡す。
        let (accepted, texture_index) = self.viewer.session.accept_preload_bytes(
            document_generation,
            asset_id,
            page_index,
            bytes,
        );
        if !accepted {
            return;
        }
        if self.settings.smart_crop && self.viewer.session.cached_crop_result(asset_id).is_none() {
            let preparation_index =
                self.viewer
                    .session
                    .byte_preload_indices()
                    .into_iter()
                    .find(|&index| {
                        self.viewer.session.page_asset_id(index) == Some(asset_id)
                            && !self.viewer.session.has_texture(index)
                    });
            if let Some(preparation_index) = preparation_index {
                let Some(layout) = self.viewer.session.image_layout_for_page(preparation_index)
                else {
                    return;
                };
                let Some(bytes) = self.viewer.session.cached_bytes(asset_id) else {
                    return;
                };
                self.viewer.smart_crop.queue(SmartCropPreparationRequest {
                    document_generation,
                    asset_id,
                    page_index: preparation_index,
                    bytes,
                    layout,
                });
            }
        } else if let Some(texture_index) = texture_index {
            Self::queue_decode(texture_index, asset_id, document_generation, sender);
        }
    }

    pub(super) fn smart_crop_prepared(
        &mut self,
        document_generation: u64,
        asset_id: AssetId,
        page_index: usize,
        success: bool,
        sender: &AppSender,
    ) {
        if !self.settings.smart_crop {
            return;
        }
        if document_generation != self.viewer.session.document_generation()
            || self.viewer.session.page_asset_id(page_index) != Some(asset_id)
        {
            return;
        }
        if !success {
            if let Some(texture_index) =
                self.viewer.session.texture_needed_index_for_asset(asset_id)
            {
                Self::queue_decode(texture_index, asset_id, document_generation, sender);
            }
            return;
        }
        let handle = self.viewer.smart_crop.handle();
        let Some(prepared) = handle.take_ready(document_generation, asset_id) else {
            return;
        };
        self.viewer.session.accept_crop_analysis(
            document_generation,
            asset_id,
            page_index,
            prepared,
        );
        if let Some(texture_index) = self.viewer.session.texture_needed_index_for_asset(asset_id) {
            Self::queue_decode(texture_index, asset_id, document_generation, sender);
        }
    }

    fn queue_decode(
        page_index: usize,
        asset_id: AssetId,
        document_generation: u64,
        sender: &AppSender,
    ) {
        gtk::glib::idle_add_local_once(clone!(
            #[strong]
            sender,
            move || {
                sender.input(Msg::DecodeBytes {
                    document_generation,
                    asset_id,
                    page_index,
                })
            }
        ));
    }

    pub(super) fn decode_bytes(
        &mut self,
        document_generation: u64,
        asset_id: AssetId,
        page_index: usize,
    ) {
        let preparation = self.viewer.smart_crop.handle();
        self.viewer.session.decode_preloaded(
            self.settings.archive_expansion_limit,
            document_generation,
            asset_id,
            page_index,
            self.settings.smart_crop,
            Some(&preparation),
        );
    }

    pub(super) fn set_page(&mut self, val: f64, sender: &AppSender) {
        self.set_page_for_slider_input(val, SliderNavigationInput::Other, sender);
    }

    pub(super) fn set_page_from_slider_pointer(&mut self, val: f64, sender: &AppSender) {
        self.set_page_for_slider_input(val, SliderNavigationInput::Pointer, sender);
    }

    fn set_page_for_slider_input(
        &mut self,
        val: f64,
        input: SliderNavigationInput,
        sender: &AppSender,
    ) {
        if self.document_boundary_visible() {
            return;
        }
        let displayed_preview_target = self
            .settings
            .thumbnails_enabled
            .then(|| self.viewer.preview.displayed_navigation_index())
            .flatten();
        if self.progressive_archive_loading() {
            let Some(last_index) = self.slider_item_count().checked_sub(1) else {
                return;
            };
            let raw_target = (val.round() as usize).min(last_index);
            let target = slider_navigation_target(raw_target, displayed_preview_target, input);
            if target < self.viewer.session.physical_image_count() {
                if self.viewer.session.set_physical_image(target) == Some(true) {
                    self.document.boundary.cancel_lookup();
                    self.sync_viewer_position(sender);
                }
            } else {
                self.wait_for_progressive_page(PendingPageMove::PhysicalImage(target), sender);
            }
            return;
        }
        let Some(target) = slider_page_target(val, self.viewer.session.view_starts()) else {
            return;
        };
        let target = slider_navigation_target(target, displayed_preview_target, input);
        if self.viewer.session.set_page(target as f64) {
            self.document.boundary.cancel_lookup();
            self.sync_viewer_position(sender);
        }
    }
}
