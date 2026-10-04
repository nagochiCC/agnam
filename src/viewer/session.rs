use super::cache::ImageCache;
use super::preparation::SmartCropPreparationHandle;
use super::render::{
    CropAnalysis, RenderedAsset, analyze_smart_crop_asset, analyze_texture, render_asset,
};
use super::smart_crop::CropRect;
use super::state::{ViewMode, ViewState};
use crate::archive::ResourceLimitKind;
use crate::archive::image_loader::try_load_image_bytes;
use crate::document::{AssetId, Document, ImageLayout, ImageSource, Page};
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct ProgressiveBackup {
    document: Option<Document>,
    state: ViewState,
    cache: ImageCache,
}

pub(crate) struct ViewerSession {
    document: Option<Document>,
    state: ViewState,
    cache: ImageCache,
    document_generation: u64,
    progressive_active: bool,
    progressive_backup: Option<ProgressiveBackup>,
    archive_resource_limit: Option<ResourceLimitKind>,
}

impl ViewerSession {
    pub(crate) fn new(view_mode: ViewMode) -> Self {
        Self {
            document: None,
            state: ViewState::with_view_mode(view_mode),
            cache: ImageCache::new(),
            document_generation: 0,
            progressive_active: false,
            progressive_backup: None,
            archive_resource_limit: None,
        }
    }

    pub(crate) fn replace_document(&mut self, document: Document, initial_index: usize) {
        self.document_generation = self.document_generation.wrapping_add(1);
        self.progressive_active = false;
        self.progressive_backup = None;
        self.archive_resource_limit = None;
        self.document = Some(document);
        self.cache.clear();
        let pages: &[Page] = self
            .document
            .as_ref()
            .map_or(&[], |document| document.pages.as_slice());
        self.state.reset_for_document(pages, initial_index);
    }

    #[cfg(test)]
    pub(crate) fn begin_progressive_document(
        &mut self,
        path: PathBuf,
        images: impl IntoIterator<Item = (gtk::glib::Bytes, ImageLayout)>,
        initial_index: usize,
    ) {
        self.archive_resource_limit = None;
        let view_mode = self.state.view_mode();
        self.progressive_backup = Some(ProgressiveBackup {
            document: self.document.take(),
            state: std::mem::replace(&mut self.state, ViewState::with_view_mode(view_mode)),
            cache: std::mem::replace(&mut self.cache, ImageCache::new()),
        });
        self.progressive_active = true;
        self.document_generation = self.document_generation.wrapping_add(1);

        let mut document = Document::new(path, None);
        for (bytes, layout) in images {
            document.add_asset(ImageSource::Memory(bytes), layout);
        }
        self.document = Some(document);
        let pages = self
            .document
            .as_ref()
            .map_or(&[][..], |document| document.pages.as_slice());
        self.state
            .reset_for_progressive_document(pages, initial_index);
    }

    pub(crate) fn begin_progressive_archive_document(
        &mut self,
        path: PathBuf,
        images: impl IntoIterator<Item = (ImageSource, ImageLayout)>,
        temp_dir: Option<Arc<tempfile::TempDir>>,
        initial_index: usize,
    ) {
        self.archive_resource_limit = None;
        let view_mode = self.state.view_mode();
        self.progressive_backup = Some(ProgressiveBackup {
            document: self.document.take(),
            state: std::mem::replace(&mut self.state, ViewState::with_view_mode(view_mode)),
            cache: std::mem::replace(&mut self.cache, ImageCache::new()),
        });
        self.progressive_active = true;
        self.document_generation = self.document_generation.wrapping_add(1);

        let mut document = Document::new(path, temp_dir);
        for (source, layout) in images {
            document.add_asset(source, layout);
        }
        self.document = Some(document);
        let pages = self
            .document
            .as_ref()
            .map_or(&[][..], |document| document.pages.as_slice());
        self.state
            .reset_for_progressive_document(pages, initial_index);
    }

    pub(crate) fn append_progressive_archive_images(
        &mut self,
        images: impl IntoIterator<Item = (ImageSource, ImageLayout)>,
    ) -> bool {
        if !self.progressive_active {
            return false;
        }
        let Some(document) = self.document.as_mut() else {
            return false;
        };
        for (source, layout) in images {
            document.add_asset(source, layout);
        }
        self.state.extend_progressive_document(&document.pages);
        true
    }

    pub(crate) fn spill_progressive_document(&mut self, temp_dir: Arc<tempfile::TempDir>) -> bool {
        if !self.progressive_active {
            return false;
        }
        let Some(document) = self.document.as_mut() else {
            return false;
        };
        for (physical_index, asset) in document.assets.iter_mut().enumerate() {
            asset.source = ImageSource::File(crate::archive::sequential_image_path(
                temp_dir.path(),
                physical_index,
            ));
        }
        document.temp_dir = Some(temp_dir);
        true
    }

    pub(crate) fn finish_progressive_document(&mut self, document: Document) -> bool {
        if !self.progressive_active || self.document_path() != Some(document.path.as_path()) {
            return false;
        }
        self.document = Some(document);
        let pages = self
            .document
            .as_ref()
            .map_or(&[][..], |document| document.pages.as_slice());
        self.state.finish_progressive_document(pages);
        self.progressive_active = false;
        self.progressive_backup = None;
        true
    }

    pub(crate) fn cancel_progressive_document(&mut self) -> bool {
        if !self.progressive_active {
            return false;
        }
        self.progressive_active = false;
        if let Some(backup) = self.progressive_backup.take() {
            self.document = backup.document;
            self.state = backup.state;
            self.cache = backup.cache;
            self.document_generation = self.document_generation.wrapping_add(1);
        } else {
            let pages = self
                .document
                .as_ref()
                .map_or(&[][..], |document| document.pages.as_slice());
            self.state.finish_progressive_document(pages);
        }
        true
    }

    pub(crate) fn retain_progressive_document_for_replacement(&mut self) -> bool {
        if !self.progressive_active {
            return false;
        }
        self.progressive_active = false;
        self.progressive_backup = None;
        let pages = self
            .document
            .as_ref()
            .map_or(&[][..], |document| document.pages.as_slice());
        self.state.finish_progressive_document(pages);
        true
    }

    pub(crate) fn discard_progressive_rollback(&mut self) -> bool {
        if !self.progressive_active {
            return false;
        }
        self.progressive_backup = None;
        true
    }

    pub(crate) fn document(&self) -> Option<&Document> {
        self.document.as_ref()
    }

    pub(crate) fn document_path(&self) -> Option<&Path> {
        self.document
            .as_ref()
            .map(|document| document.path.as_path())
    }

    pub(crate) fn pages(&self) -> &[Page] {
        self.document
            .as_ref()
            .map_or(&[], |document| document.pages.as_slice())
    }

    pub(crate) fn page_count(&self) -> usize {
        self.state.available_page_count(self.pages().len())
    }

    pub(crate) fn current_index(&self) -> usize {
        self.state.current_index()
    }

    pub(crate) fn physical_image_count(&self) -> usize {
        self.document
            .as_ref()
            .map_or(0, |document| document.assets.len())
    }

    pub(crate) fn current_physical_image_index(&self) -> Option<usize> {
        self.document
            .as_ref()?
            .pages
            .get(self.state.current_index())
            .map(|page| page.asset_id.0)
    }

    pub(crate) fn view_mode(&self) -> ViewMode {
        self.state.view_mode()
    }

    pub(crate) fn view_starts(&self) -> &[usize] {
        self.state.view_starts()
    }

    pub(crate) fn document_generation(&self) -> u64 {
        self.document_generation
    }

    pub(crate) fn is_full_spread(&self) -> bool {
        self.state.is_full_spread(self.pages())
    }

    pub(crate) fn is_full_spread_at(&self, index: usize) -> bool {
        self.state.is_full_spread_at(index, self.pages())
    }

    pub(crate) fn at_document_end(&self) -> bool {
        self.state.at_document_end(self.pages())
    }

    pub(crate) fn next_page(&mut self) -> bool {
        let pages: &[Page] = self
            .document
            .as_ref()
            .map_or(&[], |document| document.pages.as_slice());
        self.state.next_page(pages)
    }

    pub(crate) fn next_single_page(&mut self) -> bool {
        let pages: &[Page] = self
            .document
            .as_ref()
            .map_or(&[], |document| document.pages.as_slice());
        self.state.next_single_page(pages)
    }

    pub(crate) fn prev_page(&mut self) -> bool {
        self.state.prev_page()
    }

    pub(crate) fn prev_single_page(&mut self) -> bool {
        let pages: &[Page] = self
            .document
            .as_ref()
            .map_or(&[], |document| document.pages.as_slice());
        self.state.prev_single_page(pages)
    }

    pub(crate) fn set_view_mode(&mut self, view_mode: ViewMode) -> bool {
        let pages: &[Page] = self
            .document
            .as_ref()
            .map_or(&[], |document| document.pages.as_slice());
        self.state.set_view_mode(view_mode, pages)
    }

    pub(crate) fn set_page(&mut self, value: f64) -> bool {
        let pages: &[Page] = self
            .document
            .as_ref()
            .map_or(&[], |document| document.pages.as_slice());
        self.state.set_page(value, pages)
    }

    pub(crate) fn set_physical_image(&mut self, physical_index: usize) -> Option<bool> {
        let document = self.document.as_ref()?;
        let first_page = document.assets.get(physical_index)?.first_page;
        Some(
            self.state
                .set_progressive_page(first_page as f64, &document.pages),
        )
    }

    pub(crate) fn progressive_preview_pages(
        &self,
        physical_index: usize,
    ) -> Option<(usize, Option<usize>)> {
        let document = self.document.as_ref()?;
        let first_page = document.assets.get(physical_index)?.first_page;
        self.state
            .progressive_view_for_page(first_page, &document.pages)
    }

    pub(crate) fn progressive_thumbnail_source(
        &self,
        asset_id: AssetId,
    ) -> Option<(
        usize,
        ImageLayout,
        ImageSource,
        Option<Arc<tempfile::TempDir>>,
    )> {
        let document = self.document.as_ref()?;
        let asset = document.assets.get(asset_id.0)?;
        let temp_dir = match &asset.source {
            ImageSource::Memory(_) => None,
            ImageSource::File(_) => Some(document.temp_dir.clone()?),
            ImageSource::ArchiveEntry { .. } => return None,
        };
        Some((
            asset.first_page,
            asset.layout,
            asset.source.clone(),
            temp_dir,
        ))
    }

    #[cfg(test)]
    pub(crate) fn current_textures(
        &mut self,
        smart_crop: bool,
    ) -> (Option<gtk::gdk::Paintable>, Option<gtk::gdk::Paintable>) {
        self.current_textures_with_preparation(smart_crop, None)
    }

    pub(crate) fn current_textures_with_preparation(
        &mut self,
        smart_crop: bool,
        preparation: Option<&SmartCropPreparationHandle>,
    ) -> (Option<gtk::gdk::Paintable>, Option<gtk::gdk::Paintable>) {
        let Some(document) = self.document.as_ref() else {
            return (None, None);
        };

        let current_index = self.state.current_index();
        let is_full_spread = self.state.is_full_spread(&document.pages);
        let right =
            self.ensure_texture_for_page_with_preparation(current_index, smart_crop, preparation);
        let left = if is_full_spread {
            self.ensure_texture_for_page_with_preparation(
                current_index + 1,
                smart_crop,
                preparation,
            )
        } else {
            None
        };

        let Some(document) = self.document.as_ref() else {
            return (right, left);
        };
        let retained_texture_indices = self.state.texture_retention_indices(&document.pages);
        self.cache
            .trim(&retained_texture_indices, current_index, document);
        (right, left)
    }

    pub(crate) fn take_archive_resource_limit(&mut self) -> Option<ResourceLimitKind> {
        self.archive_resource_limit.take()
    }

    #[cfg(test)]
    fn ensure_texture_for_page(
        &mut self,
        page_index: usize,
        smart_crop: bool,
    ) -> Option<gtk::gdk::Paintable> {
        self.ensure_texture_for_page_with_preparation(page_index, smart_crop, None)
    }

    fn ensure_texture_for_page_with_preparation(
        &mut self,
        page_index: usize,
        smart_crop: bool,
        preparation: Option<&SmartCropPreparationHandle>,
    ) -> Option<gtk::gdk::Paintable> {
        if let Some(texture) = self.cache.get_texture(page_index) {
            return Some(texture);
        }

        let document = self.document.as_ref()?;
        let page = document.pages.get(page_index)?;
        let asset = document.assets.get(page.asset_id.0)?;
        let asset_id = page.asset_id;
        let layout = asset.layout;
        let first_page = asset.first_page;
        if smart_crop && self.cache.crop_result(asset_id).is_none() {
            if let Some(analysis) = preparation.and_then(|preparation| {
                preparation.take_ready_or_cancel(self.document_generation, asset_id)
            }) {
                if analysis.layout == layout {
                    self.cache
                        .insert_crop_result(asset_id, analysis.crop_result);
                }
            }
        }
        let bytes = if let Some(bytes) = self.cache.get_bytes(page.asset_id) {
            bytes
        } else {
            let bytes = match try_load_image_bytes(&asset.source) {
                Ok(Some(bytes)) => bytes,
                Ok(None) => gtk::glib::Bytes::from_static(&[]),
                Err(crate::error::AppError::ArchiveResourceLimit(kind)) => {
                    self.archive_resource_limit = Some(kind);
                    gtk::glib::Bytes::from_static(&[])
                }
                Err(_) => gtk::glib::Bytes::from_static(&[]),
            };
            self.cache.insert_bytes(page.asset_id, bytes.clone());
            bytes
        };
        if bytes.is_empty() {
            return None;
        }

        let texture = gtk::gdk::Texture::from_bytes(&bytes).ok()?;
        let crop = if smart_crop {
            match self.cache.crop_result(asset_id) {
                Some(crop) => crop,
                None => {
                    let analysis = if let Some(preparation) = preparation {
                        preparation
                            .analyze(
                                self.document_generation,
                                asset_id,
                                bytes.as_ref(),
                                layout,
                                || false,
                            )
                            .ok()
                            .flatten()
                    } else {
                        analyze_smart_crop_asset(bytes.as_ref(), layout)
                    }
                    .or_else(|| analyze_texture(&texture, layout))?;
                    self.cache
                        .insert_crop_result(asset_id, analysis.crop_result);
                    analysis.crop_result
                }
            }
        } else {
            None
        };
        let rendered = render_asset(texture, layout, crop)?;
        self.cache_rendered(first_page, rendered);
        self.cache.get_texture(page_index)
    }

    fn cache_rendered(&mut self, first_page: usize, rendered: RenderedAsset) {
        match rendered {
            RenderedAsset::Single(texture) => {
                self.cache.insert_texture(first_page, texture);
            }
            RenderedAsset::Spread { right, left } => {
                self.cache.insert_texture(first_page, right);
                self.cache.insert_texture(first_page + 1, left);
            }
        }
    }

    pub(crate) fn has_texture(&self, index: usize) -> bool {
        self.cache.has_texture(index)
    }

    pub(crate) fn texture(&self, index: usize) -> Option<gtk::gdk::Paintable> {
        self.cache.get_texture(index)
    }

    pub(crate) fn cached_bytes(&self, asset_id: AssetId) -> Option<gtk::glib::Bytes> {
        self.cache.get_bytes(asset_id)
    }

    pub(crate) fn clear_textures(&mut self) {
        self.cache.clear_textures();
    }

    pub(crate) fn page_asset_id(&self, page_index: usize) -> Option<AssetId> {
        self.document
            .as_ref()?
            .pages
            .get(page_index)
            .map(|page| page.asset_id)
    }

    pub(crate) fn image_source_for_page(&self, page_index: usize) -> Option<&ImageSource> {
        self.document
            .as_ref()?
            .asset_for_page(page_index)
            .map(|asset| &asset.source)
    }

    pub(crate) fn image_layout_for_page(&self, page_index: usize) -> Option<ImageLayout> {
        self.document
            .as_ref()?
            .asset_for_page(page_index)
            .map(|asset| asset.layout)
    }

    pub(crate) fn cached_crop_result(&self, asset_id: AssetId) -> Option<Option<CropRect>> {
        self.cache.crop_result(asset_id)
    }

    pub(crate) fn texture_preload_indices(&self) -> Vec<usize> {
        self.state.texture_preload_indices(self.pages())
    }

    pub(crate) fn byte_preload_indices(&self) -> Vec<usize> {
        self.state.byte_preload_indices(self.pages())
    }

    pub(crate) fn texture_needed_index_for_asset(&self, asset_id: AssetId) -> Option<usize> {
        let current_index = self.state.current_index();
        let current_left = self
            .state
            .is_full_spread(self.pages())
            .then_some(current_index + 1);
        [Some(current_index), current_left]
            .into_iter()
            .flatten()
            .chain(self.texture_preload_indices())
            .find(|&index| {
                self.page_asset_id(index) == Some(asset_id) && !self.cache.has_texture(index)
            })
    }

    pub(crate) fn accept_preload_bytes(
        &mut self,
        document_generation: u64,
        asset_id: AssetId,
        page_index: usize,
        bytes: gtk::glib::Bytes,
    ) -> (bool, Option<usize>) {
        if document_generation != self.document_generation
            || self.page_asset_id(page_index) != Some(asset_id)
        {
            return (false, None);
        }

        self.cache.insert_bytes(asset_id, bytes);
        (true, self.texture_needed_index_for_asset(asset_id))
    }

    pub(crate) fn decode_preloaded(
        &mut self,
        document_generation: u64,
        asset_id: AssetId,
        page_index: usize,
        smart_crop: bool,
        preparation: Option<&SmartCropPreparationHandle>,
    ) {
        if document_generation != self.document_generation
            || self.page_asset_id(page_index) != Some(asset_id)
        {
            return;
        }
        let Some(texture_index) = self.texture_needed_index_for_asset(asset_id) else {
            return;
        };
        if self
            .ensure_texture_for_page_with_preparation(texture_index, smart_crop, preparation)
            .is_some()
        {
            let Some(document) = self.document.as_ref() else {
                return;
            };
            let retained_texture_indices = self.state.texture_retention_indices(&document.pages);
            self.cache.trim(
                &retained_texture_indices,
                self.state.current_index(),
                document,
            );
        }
    }

    pub(crate) fn accept_crop_analysis(
        &mut self,
        document_generation: u64,
        asset_id: AssetId,
        page_index: usize,
        analysis: CropAnalysis,
    ) -> bool {
        if document_generation != self.document_generation
            || self.page_asset_id(page_index) != Some(asset_id)
            || self.image_layout_for_page(page_index) != Some(analysis.layout)
        {
            return false;
        }
        self.cache
            .insert_crop_result(asset_id, analysis.crop_result);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{ImageLayout, ImageSource};
    use crate::viewer::preparation::{SmartCropPreparationRequest, SmartCropPreparationScheduler};
    use gtk::prelude::*;
    use std::io::Cursor;
    use std::sync::mpsc;
    use std::time::Duration;

    fn png_bytes(width: u32, height: u32) -> gtk::glib::Bytes {
        let image = image::RgbaImage::from_fn(width, height, |x, _| {
            image::Rgba([((x + 1) * 10) as u8, 0, 0, 255])
        });
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        gtk::glib::Bytes::from_owned(png.into_inner())
    }

    fn document_with_layouts(layouts: &[ImageLayout]) -> Document {
        let mut document = Document::new("book".into(), None);
        for &layout in layouts {
            document.add_asset(
                ImageSource::Memory(gtk::glib::Bytes::from_static(b"image")),
                layout,
            );
        }
        document
    }

    #[test]
    fn initial_state_uses_the_requested_view_mode_and_has_no_document() {
        let mut session = ViewerSession::new(ViewMode::Single);

        assert!(session.document().is_none());
        assert_eq!(session.page_count(), 0);
        assert_eq!(session.current_index(), 0);
        assert_eq!(session.view_mode(), ViewMode::Single);
        assert_eq!(session.document_generation(), 0);
        assert!(!session.next_page());
        assert!(!session.next_single_page());
        assert!(!session.prev_page());
        assert!(!session.prev_single_page());
    }

    #[test]
    fn replacing_document_resets_view_state_and_cache_and_advances_generation() {
        let mut session = ViewerSession::new(ViewMode::Spread);
        session.replace_document(
            document_with_layouts(&[
                ImageLayout::Single,
                ImageLayout::Single,
                ImageLayout::Single,
                ImageLayout::Single,
                ImageLayout::Single,
            ]),
            2,
        );
        assert_eq!(session.view_starts(), &[0, 1, 3]);
        assert_eq!(session.current_index(), 3);
        let first_generation = session.document_generation();
        let first_asset = session.page_asset_id(0).unwrap();
        let _ = session.accept_preload_bytes(
            first_generation,
            first_asset,
            0,
            gtk::glib::Bytes::from_static(b"cached"),
        );
        assert!(session.cached_bytes(first_asset).is_some());
        assert!(session.prev_single_page());
        assert_eq!(session.view_starts(), &[0, 2, 4]);

        session.replace_document(
            document_with_layouts(&[
                ImageLayout::Spread,
                ImageLayout::Single,
                ImageLayout::Single,
            ]),
            1,
        );

        assert_eq!(session.document_generation(), first_generation + 1);
        assert!(session.cached_bytes(first_asset).is_none());
        assert_eq!(session.view_starts(), &[0, 2]);
        assert_eq!(session.current_index(), 2);

        assert_eq!(
            session.accept_preload_bytes(
                first_generation,
                first_asset,
                0,
                gtk::glib::Bytes::from_static(b"stale"),
            ),
            (false, None)
        );
        assert!(session.cached_bytes(first_asset).is_none());
    }

    #[test]
    fn saved_image_document_index_is_restored_and_oversized_index_is_clamped() {
        let mut image_document = document_with_layouts(&[
            ImageLayout::Single,
            ImageLayout::Single,
            ImageLayout::Single,
        ]);
        image_document.path = "023.jpg".into();
        let mut session = ViewerSession::new(ViewMode::Single);

        session.replace_document(image_document, 2);
        assert_eq!(session.document_path(), Some(Path::new("023.jpg")));
        assert_eq!(session.current_index(), 2);

        session.replace_document(
            document_with_layouts(&[
                ImageLayout::Single,
                ImageLayout::Single,
                ImageLayout::Single,
            ]),
            usize::MAX,
        );
        assert_eq!(session.current_index(), 2);
    }

    #[test]
    fn page_movement_delegates_to_the_existing_view_state_behavior() {
        let mut session = ViewerSession::new(ViewMode::Spread);
        session.replace_document(
            document_with_layouts(&[
                ImageLayout::Single,
                ImageLayout::Single,
                ImageLayout::Single,
                ImageLayout::Single,
                ImageLayout::Single,
            ]),
            0,
        );

        assert!(session.next_page());
        assert_eq!(session.current_index(), 1);
        assert!(session.next_page());
        assert_eq!(session.current_index(), 3);
        assert!(!session.next_page());
        assert!(session.prev_page());
        assert_eq!(session.current_index(), 1);
    }

    #[test]
    fn document_end_tracks_viewer_navigation_and_backtracking() {
        let mut single = ViewerSession::new(ViewMode::Single);
        single.replace_document(
            document_with_layouts(&[
                ImageLayout::Single,
                ImageLayout::Single,
                ImageLayout::Single,
            ]),
            1,
        );
        assert!(!single.at_document_end());
        assert!(single.next_page());
        assert!(single.at_document_end());
        assert!(single.prev_page());
        assert!(!single.at_document_end());

        let mut spread = ViewerSession::new(ViewMode::Spread);
        let layouts = vec![ImageLayout::Single; 65];
        spread.replace_document(document_with_layouts(&layouts), 61);
        assert_eq!(spread.current_index(), 61);
        assert!(!spread.at_document_end());
        assert!(spread.next_page());
        assert_eq!(spread.current_index(), 63);
        assert!(spread.at_document_end());
        assert!(spread.prev_page());
        assert!(!spread.at_document_end());
    }

    #[test]
    fn rendering_either_spread_page_caches_right_then_left_together() {
        let mut document = Document::new("book".into(), None);
        let asset_id =
            document.add_asset(ImageSource::Memory(png_bytes(5, 2)), ImageLayout::Spread);
        let mut session = ViewerSession::new(ViewMode::Spread);
        session.replace_document(document, 0);

        let left = session.ensure_texture_for_page(1, false).unwrap();
        let right = session.texture(0).unwrap();

        assert_eq!((right.intrinsic_width(), left.intrinsic_width()), (3, 2));
        assert!(session.cached_bytes(asset_id).is_some());
        assert!(session.has_texture(0));
        assert!(session.has_texture(1));
    }

    #[test]
    fn texture_cache_hit_does_not_render_cached_bytes_again() {
        let mut document = Document::new("book".into(), None);
        let asset_id =
            document.add_asset(ImageSource::Memory(png_bytes(2, 2)), ImageLayout::Single);
        let mut session = ViewerSession::new(ViewMode::Single);
        session.replace_document(document, 0);

        assert!(session.current_textures(false).0.is_some());
        session
            .cache
            .insert_bytes(asset_id, gtk::glib::Bytes::from_static(&[]));

        assert!(session.current_textures(false).0.is_some());
    }

    #[test]
    fn clearing_textures_keeps_bytes_for_a_setting_driven_rerender() {
        let mut document = Document::new("book".into(), None);
        let asset_id =
            document.add_asset(ImageSource::Memory(png_bytes(80, 70)), ImageLayout::Single);
        let mut session = ViewerSession::new(ViewMode::Single);
        session.replace_document(document, 0);

        assert!(session.current_textures(false).0.is_some());
        session.clear_textures();

        assert!(!session.has_texture(0));
        assert!(session.cached_bytes(asset_id).is_some());
        assert!(session.current_textures(true).0.is_some());
    }

    #[test]
    fn failed_load_keeps_an_empty_negative_cache_entry() {
        let directory = tempfile::tempdir().unwrap();
        let image_path = directory.path().join("page.png");
        let mut document = Document::new("book".into(), None);
        let asset_id =
            document.add_asset(ImageSource::File(image_path.clone()), ImageLayout::Single);
        let mut session = ViewerSession::new(ViewMode::Single);
        session.replace_document(document, 0);

        assert!(session.current_textures(false).0.is_none());
        assert!(session.cache.get_bytes(asset_id).unwrap().is_empty());

        std::fs::write(image_path, png_bytes(2, 2).as_ref()).unwrap();
        assert!(session.current_textures(false).0.is_none());
    }

    #[test]
    fn cached_bytes_avoid_source_io_when_rendering() {
        let directory = tempfile::tempdir().unwrap();
        let mut document = Document::new("book".into(), None);
        let asset_id = document.add_asset(
            ImageSource::File(directory.path().join("missing.png")),
            ImageLayout::Single,
        );
        let mut session = ViewerSession::new(ViewMode::Single);
        session.replace_document(document, 0);
        let generation = session.document_generation();

        assert_eq!(
            session.accept_preload_bytes(generation, asset_id, 0, png_bytes(2, 2)),
            (true, Some(0))
        );
        assert!(session.current_textures(false).0.is_some());
        assert!(session.has_texture(0));
    }

    #[test]
    fn preload_decode_falls_back_to_the_loader_when_bytes_are_absent() {
        let directory = tempfile::tempdir().unwrap();
        let image_path = directory.path().join("next.png");
        std::fs::write(&image_path, png_bytes(2, 2).as_ref()).unwrap();
        let mut document = Document::new("book".into(), None);
        document.add_asset(ImageSource::Memory(png_bytes(2, 2)), ImageLayout::Single);
        let asset_id = document.add_asset(ImageSource::File(image_path), ImageLayout::Single);
        let mut session = ViewerSession::new(ViewMode::Single);
        session.replace_document(document, 0);
        let generation = session.document_generation();

        assert!(session.cached_bytes(asset_id).is_none());
        session.decode_preloaded(generation, asset_id, 1, false, None);

        assert!(session.cached_bytes(asset_id).is_some());
        assert!(session.has_texture(1));
    }

    #[test]
    fn prepared_preload_validates_generation_asset_and_layout_before_caching() {
        let first = png_bytes(80, 70);
        let second = png_bytes(80, 70);
        let mut document = Document::new("book".into(), None);
        document.add_asset(ImageSource::Memory(first), ImageLayout::Single);
        let asset_id = document.add_asset(ImageSource::Memory(second.clone()), ImageLayout::Single);
        let mut session = ViewerSession::new(ViewMode::Single);
        session.replace_document(document, 0);
        let generation = session.document_generation();
        assert_eq!(
            session.accept_preload_bytes(generation, asset_id, 1, second.clone()),
            (true, Some(1))
        );

        let stale = analyze_smart_crop_asset(second.as_ref(), ImageLayout::Single).unwrap();
        assert!(!session.accept_crop_analysis(generation - 1, asset_id, 1, stale));
        assert!(!session.has_texture(1));

        let wrong_layout = analyze_smart_crop_asset(second.as_ref(), ImageLayout::Spread).unwrap();
        assert!(!session.accept_crop_analysis(generation, asset_id, 1, wrong_layout));
        assert!(!session.has_texture(1));

        assert!(session.next_page());
        assert_eq!(session.current_index(), 1);
        let prepared = analyze_smart_crop_asset(second.as_ref(), ImageLayout::Single).unwrap();
        assert!(session.accept_crop_analysis(generation, asset_id, 1, prepared));
        assert!(!session.has_texture(1));
        session.decode_preloaded(generation, asset_id, 1, true, None);
        assert!(session.has_texture(1));
        assert!(session.cached_crop_result(asset_id).is_some());
    }

    #[test]
    fn completed_preparation_is_used_after_the_target_moves_from_next_to_current() {
        let image = image::RgbaImage::from_fn(80, 70, |x, y| {
            if (15..65).contains(&x) && (12..57).contains(&y) {
                image::Rgba([20, 20, 20, 255])
            } else {
                image::Rgba([255, 255, 255, 255])
            }
        });
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut output, image::ImageFormat::Png)
            .unwrap();
        let prepared_bytes = gtk::glib::Bytes::from_owned(output.into_inner());
        let mut document = Document::new("book".into(), None);
        document.add_asset(ImageSource::Memory(png_bytes(2, 2)), ImageLayout::Single);
        let asset_id = document.add_asset(
            ImageSource::File(PathBuf::from("missing-current-page.png")),
            ImageLayout::Single,
        );
        let mut session = ViewerSession::new(ViewMode::Single);
        session.replace_document(document, 0);
        let generation = session.document_generation();

        let (sender, receiver) = mpsc::channel();
        let scheduler =
            SmartCropPreparationScheduler::new(move |result| sender.send(result).is_ok());
        scheduler.activate_document(generation);
        scheduler.set_targets(generation, [asset_id]);
        scheduler.queue(SmartCropPreparationRequest {
            document_generation: generation,
            asset_id,
            page_index: 1,
            bytes: prepared_bytes.clone(),
            layout: ImageLayout::Single,
        });
        assert!(
            receiver
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .success
        );

        session.accept_preload_bytes(generation, asset_id, 1, prepared_bytes);
        assert!(session.next_page());
        let handle = scheduler.handle();
        let texture = session
            .current_textures_with_preparation(true, Some(&handle))
            .0
            .unwrap();
        assert_eq!(texture.intrinsic_width(), 80);
        assert!(texture.intrinsic_height() < 70);
        assert!(session.cached_bytes(asset_id).is_some());
        assert!(handle.take_ready(generation, asset_id).is_none());
    }

    #[test]
    fn crop_and_no_crop_cache_survive_toggle_and_texture_regeneration_without_cpu_work() {
        use crate::viewer::render::{ANALYSIS_COUNT, CPU_DECODE_COUNT, DOWNLOAD_COUNT};
        for format in [
            image::ImageFormat::Png,
            image::ImageFormat::WebP,
            image::ImageFormat::Jpeg,
        ] {
            for layout in [ImageLayout::Single, ImageLayout::Spread] {
                for with_border in [false, true] {
                    let image = image::RgbImage::from_fn(81, 70, |x, y| {
                        if !with_border || ((8..66).contains(&x) && (12..57).contains(&y)) {
                            image::Rgb([x as u8, 20, 30])
                        } else {
                            image::Rgb([255, 255, 255])
                        }
                    });
                    let mut encoded = Cursor::new(Vec::new());
                    image::DynamicImage::ImageRgb8(image)
                        .write_to(&mut encoded, format)
                        .unwrap();
                    let mut document = Document::new("book".into(), None);
                    let asset = document.add_asset(
                        ImageSource::Memory(gtk::glib::Bytes::from_owned(encoded.into_inner())),
                        layout,
                    );
                    let mut session = ViewerSession::new(ViewMode::Spread);
                    session.replace_document(document, 0);
                    let generation = session.document_generation();
                    let scheduler = SmartCropPreparationScheduler::new(|_| true);
                    scheduler.activate_document(generation);
                    let handle = scheduler.handle();

                    let off = session
                        .current_textures_with_preparation(false, Some(&handle))
                        .0
                        .unwrap();
                    assert_eq!(off.intrinsic_height(), 70);
                    assert_eq!(
                        session.cached_crop_result(asset),
                        None,
                        "OFF leaves crop unanalysed"
                    );
                    session.clear_textures();
                    let on = session
                        .current_textures_with_preparation(true, Some(&handle))
                        .0
                        .unwrap();
                    let cached = session.cached_crop_result(asset).unwrap();
                    assert_eq!(cached.is_some(), with_border, "{format:?}/{layout:?}");
                    assert_eq!(
                        on.intrinsic_height(),
                        cached.map_or(70, |crop| crop.height as i32)
                    );
                    if layout == ImageLayout::Single && !with_border {
                        assert!(
                            on.is::<gtk::gdk::Texture>(),
                            "cached no-crop uses the normal Texture"
                        );
                    }
                    let counts = (
                        ANALYSIS_COUNT.get(),
                        CPU_DECODE_COUNT.get(),
                        DOWNLOAD_COUNT.get(),
                        handle.decode_count(),
                    );
                    // The setting flow clears only textures, including OFF -> ON.
                    for enabled in [false, true, true] {
                        session.clear_textures();
                        assert_eq!(session.cached_crop_result(asset), Some(cached));
                        let rendered = session
                            .current_textures_with_preparation(enabled, Some(&handle))
                            .0
                            .unwrap();
                        assert_eq!(
                            rendered.intrinsic_height(),
                            if enabled { on.intrinsic_height() } else { 70 }
                        );
                        assert_eq!(session.document_generation(), generation);
                        assert_eq!(session.current_index(), 0);
                        assert_eq!(
                            (
                                ANALYSIS_COUNT.get(),
                                CPU_DECODE_COUNT.get(),
                                DOWNLOAD_COUNT.get(),
                                handle.decode_count()
                            ),
                            counts
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn idle_preload_consumes_ready_analysis_without_a_second_cpu_decode() {
        use crate::viewer::render::{ANALYSIS_COUNT, CPU_DECODE_COUNT};
        let bytes = png_bytes(80, 70);
        let mut document = Document::new("book".into(), None);
        document.add_asset(ImageSource::Memory(png_bytes(2, 2)), ImageLayout::Single);
        let asset_id = document.add_asset(ImageSource::Memory(bytes.clone()), ImageLayout::Single);
        let mut session = ViewerSession::new(ViewMode::Single);
        session.replace_document(document, 0);
        let generation = session.document_generation();
        let (sender, receiver) = mpsc::channel();
        let scheduler =
            SmartCropPreparationScheduler::new(move |result| sender.send(result).is_ok());
        scheduler.activate_document(generation);
        scheduler.set_targets(generation, [asset_id]);
        scheduler.queue(SmartCropPreparationRequest {
            document_generation: generation,
            asset_id,
            page_index: 1,
            bytes,
            layout: ImageLayout::Single,
        });
        assert!(
            receiver
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .success
        );
        let handle = scheduler.handle();
        let counts = (
            ANALYSIS_COUNT.get(),
            CPU_DECODE_COUNT.get(),
            handle.decode_count(),
        );
        session.decode_preloaded(generation, asset_id, 1, true, Some(&handle));
        assert!(session.has_texture(1));
        assert_eq!(session.cached_crop_result(asset_id), Some(None));
        assert!(handle.take_ready(generation, asset_id).is_none());
        assert_eq!(
            (
                ANALYSIS_COUNT.get(),
                CPU_DECODE_COUNT.get(),
                handle.decode_count()
            ),
            counts
        );
    }

    #[test]
    fn progressive_finish_preserves_position_mode_generation_and_cache() {
        let first = png_bytes(2, 2);
        let second = png_bytes(2, 2);
        let third = png_bytes(2, 2);
        let mut session = ViewerSession::new(ViewMode::Spread);
        session.begin_progressive_document(
            PathBuf::from("book.rar"),
            [
                (first.clone(), ImageLayout::Single),
                (second.clone(), ImageLayout::Single),
                (third.clone(), ImageLayout::Single),
            ],
            0,
        );
        let generation = session.document_generation();
        assert!(session.next_page());
        assert_eq!(session.current_index(), 1);
        assert!(session.current_textures(false).0.is_some());
        assert!(session.has_texture(1));

        let mut final_document = Document::new("book.rar".into(), None);
        for bytes in [first, second, third, png_bytes(2, 2), png_bytes(2, 2)] {
            final_document.add_asset(ImageSource::Memory(bytes), ImageLayout::Single);
        }
        assert!(session.finish_progressive_document(final_document));

        assert_eq!(session.current_index(), 1);
        assert_eq!(session.view_mode(), ViewMode::Spread);
        assert_eq!(session.document_generation(), generation);
        assert!(session.has_texture(1));
        assert_eq!(session.page_count(), 5);
    }

    #[test]
    fn physical_image_navigation_uses_each_assets_first_logical_page() {
        let mut session = ViewerSession::new(ViewMode::Single);
        session.begin_progressive_document(
            PathBuf::from("book.rar"),
            [
                (png_bytes(2, 2), ImageLayout::Single),
                (png_bytes(4, 2), ImageLayout::Spread),
                (png_bytes(2, 2), ImageLayout::Single),
            ],
            0,
        );

        assert_eq!(session.physical_image_count(), 3);
        assert_eq!(session.current_physical_image_index(), Some(0));
        assert_eq!(session.progressive_preview_pages(3), None);
        assert_eq!(session.set_physical_image(1), Some(true));
        assert_eq!(session.current_index(), 1);
        assert_eq!(session.current_physical_image_index(), Some(1));
        assert_eq!(session.set_physical_image(2), Some(true));
        assert_eq!(session.current_index(), 3);
        assert_eq!(session.current_physical_image_index(), Some(2));
        assert_eq!(session.set_physical_image(3), None);
    }

    #[test]
    fn physical_target_reveals_an_extracted_progressive_spread_tail() {
        let mut session = ViewerSession::new(ViewMode::Spread);
        session.begin_progressive_document(
            PathBuf::from("book.rar"),
            [
                (png_bytes(2, 2), ImageLayout::Single),
                (png_bytes(2, 2), ImageLayout::Single),
            ],
            0,
        );
        assert_eq!(session.page_count(), 1);
        assert!(!session.next_page());

        assert_eq!(session.set_physical_image(1), Some(true));
        assert_eq!(session.current_index(), 1);
        assert_eq!(session.current_physical_image_index(), Some(1));
        assert_eq!(session.progressive_preview_pages(1), Some((1, None)));
        let (first_page, layout, source, temp_dir) =
            session.progressive_thumbnail_source(AssetId(1)).unwrap();
        assert_eq!(first_page, 1);
        assert_eq!(layout, ImageLayout::Single);
        assert!(matches!(source, ImageSource::Memory(bytes) if !bytes.is_empty()));
        assert!(temp_dir.is_none());
    }

    #[test]
    fn progressive_thumbnail_source_returns_temp_file_info_without_reading_it() {
        let temp_dir = std::sync::Arc::new(tempfile::tempdir().unwrap());
        let path = temp_dir.path().join("not-created.png");
        let mut document = Document::new("book".into(), Some(temp_dir.clone()));
        let asset_id = document.add_asset(ImageSource::File(path.clone()), ImageLayout::Single);
        let mut session = ViewerSession::new(ViewMode::Single);
        session.replace_document(document, 0);

        let (_, _, source, guard) = session.progressive_thumbnail_source(asset_id).unwrap();
        assert!(matches!(source, ImageSource::File(source_path) if source_path == path));
        assert!(std::sync::Arc::ptr_eq(&guard.unwrap(), &temp_dir));
    }

    #[test]
    fn spilling_progressive_document_replaces_existing_memory_and_cancel_cleans_temp() {
        let mut session = ViewerSession::new(ViewMode::Single);
        session.replace_document(document_with_layouts(&[ImageLayout::Single]), 0);
        session.begin_progressive_document(
            PathBuf::from("loading.rar"),
            [
                (png_bytes(2, 2), ImageLayout::Single),
                (png_bytes(4, 2), ImageLayout::Spread),
            ],
            0,
        );

        let temp_dir = std::sync::Arc::new(tempfile::tempdir().unwrap());
        let temp_path = temp_dir.path().to_path_buf();
        for (index, bytes) in [png_bytes(2, 2), png_bytes(4, 2)].into_iter().enumerate() {
            std::fs::write(
                crate::archive::sequential_image_path(&temp_path, index),
                bytes.as_ref(),
            )
            .unwrap();
        }
        assert!(session.spill_progressive_document(temp_dir.clone()));
        assert!(
            session
                .document()
                .unwrap()
                .assets
                .iter()
                .all(|asset| matches!(
                    &asset.source,
                    ImageSource::File(path) if path.starts_with(&temp_path)
                ))
        );
        assert!(session.progressive_thumbnail_source(AssetId(0)).is_some());

        drop(temp_dir);
        assert!(session.cancel_progressive_document());
        assert_eq!(session.document_path(), Some(Path::new("book")));
        assert!(!temp_path.exists());
    }

    #[test]
    fn cancelling_progressive_document_restores_the_previous_session() {
        let mut session = ViewerSession::new(ViewMode::Single);
        session.replace_document(document_with_layouts(&[ImageLayout::Single]), 0);
        let previous_generation = session.document_generation();

        session.begin_progressive_document(
            PathBuf::from("loading.rar"),
            [(png_bytes(2, 2), ImageLayout::Single)],
            0,
        );
        assert_eq!(session.document_path(), Some(Path::new("loading.rar")));
        assert!(session.cancel_progressive_document());

        assert_eq!(session.document_path(), Some(Path::new("book")));
        assert_eq!(session.current_index(), 0);
        assert_eq!(session.view_mode(), ViewMode::Single);
        assert!(session.document_generation() > previous_generation);
    }

    #[test]
    fn replacement_retains_progressive_document_and_temp_until_direct_replace() {
        let mut session = ViewerSession::new(ViewMode::Single);
        session.replace_document(document_with_layouts(&[ImageLayout::Single]), 0);
        let temp_dir = Arc::new(tempfile::tempdir().unwrap());
        let temp_path = temp_dir.path().to_path_buf();
        let image_path = crate::archive::sequential_image_path(&temp_path, 0);
        std::fs::write(&image_path, png_bytes(2, 2).as_ref()).unwrap();
        session.begin_progressive_archive_document(
            PathBuf::from("loading.rar"),
            [(ImageSource::File(image_path), ImageLayout::Single)],
            Some(temp_dir.clone()),
            0,
        );
        drop(temp_dir);

        assert!(session.retain_progressive_document_for_replacement());
        assert_eq!(session.document_path(), Some(Path::new("loading.rar")));
        assert!(temp_path.exists());
        assert!(!session.cancel_progressive_document());

        session.replace_document(document_with_layouts(&[ImageLayout::Single]), 0);
        assert!(!temp_path.exists());
    }

    #[test]
    fn replacement_progressive_failure_keeps_the_new_display_instead_of_old_backup() {
        let mut session = ViewerSession::new(ViewMode::Single);
        session.replace_document(document_with_layouts(&[ImageLayout::Single]), 0);
        session.begin_progressive_document(
            PathBuf::from("replacement.rar"),
            [(png_bytes(2, 2), ImageLayout::Single)],
            0,
        );

        assert!(session.discard_progressive_rollback());
        assert!(session.append_progressive_archive_images([(
            ImageSource::Memory(png_bytes(2, 2)),
            ImageLayout::Single,
        )]));
        assert_eq!(session.physical_image_count(), 2);
        assert!(session.cancel_progressive_document());
        assert_eq!(session.document_path(), Some(Path::new("replacement.rar")));
    }
}
