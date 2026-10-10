use super::disk_cache::ThumbnailDiskCache;
use super::schedule::{BACKGROUND_BURST_SIZE, distributed_page_order};
use super::{AssetThumbnails, Thumbnail, make_asset_thumbnails_from_bytes_shared};
use crate::archive::image_loader::ThumbnailImageLoader;
use crate::document::{AssetId, ImageAsset, Page};
use std::collections::HashSet;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

pub(crate) struct ThumbnailResult {
    pub(crate) document_generation: u64,
    pub(crate) generation: u64,
    pub(crate) index: usize,
    pub(crate) thumbnail: Thumbnail,
}

pub(crate) struct ThumbnailWorker {
    assets: Vec<ImageAsset>,
    pages: Vec<Page>,
    document_generation: u64,
    generation: u64,
    cancel: Arc<AtomicU64>,
    state: Option<ThumbnailWorkerState>,
    page_order: Option<Vec<usize>>,
    next_page: usize,
    disk_cache: Option<ThumbnailDiskCache>,
    preparation: Option<crate::viewer::SmartCropPreparationHandle>,
}

struct ThumbnailWorkerState {
    generated: HashSet<AssetId>,
    loader: ThumbnailImageLoader,
    decompressor: turbojpeg::Decompressor,
}

impl ThumbnailWorkerState {
    fn with_generated(generated: HashSet<AssetId>) -> Option<Self> {
        Some(Self {
            generated,
            loader: ThumbnailImageLoader::new(),
            decompressor: turbojpeg::Decompressor::new().ok()?,
        })
    }

    fn take_asset_for_page(&mut self, pages: &[Page], page_index: usize) -> Option<AssetId> {
        let asset_id = pages.get(page_index)?.asset_id;
        self.generated.insert(asset_id).then_some(asset_id)
    }
}

impl ThumbnailWorker {
    pub(crate) fn new_skipping(
        assets: Vec<ImageAsset>,
        pages: Vec<Page>,
        document_generation: u64,
        generation: u64,
        cancel: Arc<AtomicU64>,
        already_generated: HashSet<AssetId>,
    ) -> Self {
        Self {
            assets,
            pages,
            document_generation,
            generation,
            cancel,
            state: ThumbnailWorkerState::with_generated(already_generated),
            page_order: None,
            next_page: 0,
            disk_cache: None,
            preparation: None,
        }
    }

    pub(crate) fn with_preparation(
        mut self,
        preparation: Option<crate::viewer::SmartCropPreparationHandle>,
    ) -> Self {
        self.preparation = preparation;
        self
    }

    pub(crate) fn with_disk_cache(mut self, document: &Path) -> Self {
        let cache = ThumbnailDiskCache::for_document(document);
        cache.opened(
            self.assets
                .iter()
                .any(|asset| asset.archive_identity.is_some()),
        );
        self.disk_cache = Some(cache);
        self
    }

    #[cfg(test)]
    fn run<F>(self, mut emit: F)
    where
        F: FnMut(ThumbnailResult) -> bool,
    {
        let mut worker = self;
        loop {
            match worker.run_next(Default::default(), &mut emit) {
                ThumbnailStep::More => {}
                ThumbnailStep::Finished | ThumbnailStep::OutputClosed => return,
            }
        }
    }

    /// Processes at most one page-schedule entry. Since a Spread's second page
    /// maps to an already generated Asset, the step may complete without decode.
    /// The background scheduler calls this at cooperative preemption boundaries.
    pub(crate) fn run_next<F>(
        &mut self,
        limit: crate::archive::ArchiveExpansionLimit,
        emit: &mut F,
    ) -> ThumbnailStep
    where
        F: FnMut(ThumbnailResult) -> bool,
    {
        if self.is_cancelled() {
            return ThumbnailStep::Finished;
        }
        let Some(state) = self.state.as_mut() else {
            return ThumbnailStep::Finished;
        };
        let order = self
            .page_order
            .get_or_insert_with(|| distributed_page_order(self.pages.len()));
        let Some(&page_index) = order.get(self.next_page) else {
            return ThumbnailStep::Finished;
        };
        self.next_page += 1;

        let output_open = Self::generate_at(
            limit,
            &self.assets,
            &self.pages,
            self.document_generation,
            self.generation,
            &self.cancel,
            page_index,
            state,
            self.disk_cache.as_ref(),
            self.preparation.as_ref(),
            emit,
        );
        if self.next_page % BACKGROUND_BURST_SIZE == 0 || self.next_page == order.len() {
            std::thread::yield_now();
        }
        if !output_open {
            ThumbnailStep::OutputClosed
        } else if self.next_page == order.len() || self.is_cancelled() {
            ThumbnailStep::Finished
        } else {
            ThumbnailStep::More
        }
    }

    /// Generates one nearby Asset from bytes already obtained by Viewer
    /// preload. Only a fully emitted result is added to the shared generated
    /// set, so a decode failure remains eligible for the distributed pass.
    pub(crate) fn run_near<F>(
        &mut self,
        asset_id: AssetId,
        bytes: &gtk::glib::Bytes,
        emit: &mut F,
    ) -> NearThumbnailStep
    where
        F: FnMut(ThumbnailResult) -> bool,
    {
        if self.is_cancelled() {
            return NearThumbnailStep::Cancelled;
        }
        let Some(state) = self.state.as_mut() else {
            return NearThumbnailStep::Failed;
        };
        if state.generated.contains(&asset_id) {
            return NearThumbnailStep::Skipped;
        }
        let Some(asset) = self.assets.get(asset_id.0) else {
            return NearThumbnailStep::Failed;
        };
        let entry = self
            .disk_cache
            .as_ref()
            .and_then(|cache| cache.entry(asset, asset_id.0));
        let cached = entry
            .as_ref()
            .and_then(|entry| self.disk_cache.as_ref()?.load(entry));
        let from_disk = cached.is_some();
        let Some(thumbnails) = cached.or_else(|| {
            make_asset_thumbnails_from_bytes_shared(
                &mut state.decompressor,
                bytes.as_ref(),
                asset.layout,
                self.preparation
                    .as_ref()
                    .map(|handle| (handle, self.document_generation, asset_id)),
            )
        }) else {
            return NearThumbnailStep::Failed;
        };
        if generation_is_cancelled(&self.cancel, self.generation) {
            return NearThumbnailStep::Cancelled;
        }

        match Self::emit_thumbnails(
            self.document_generation,
            self.generation,
            &self.cancel,
            asset.first_page,
            &thumbnails,
            emit,
        ) {
            ThumbnailEmission::Generated => {
                if !from_disk && let (Some(cache), Some(entry)) = (&self.disk_cache, entry) {
                    cache.save_later(entry, thumbnails);
                }
                state.generated.insert(asset_id);
                NearThumbnailStep::Generated
            }
            ThumbnailEmission::Cancelled => NearThumbnailStep::Cancelled,
            ThumbnailEmission::OutputClosed => NearThumbnailStep::OutputClosed,
        }
    }

    /// Generates an explicitly demanded Asset even when it was generated
    /// earlier in this worker generation and has since left the UI cache.
    pub(crate) fn run_demand<F>(
        &mut self,
        limit: crate::archive::ArchiveExpansionLimit,
        asset_id: AssetId,
        bytes: Option<&gtk::glib::Bytes>,
        emit: &mut F,
    ) -> DemandThumbnailStep
    where
        F: FnMut(ThumbnailResult) -> bool,
    {
        if self.is_cancelled() {
            return DemandThumbnailStep::Cancelled;
        }
        let Some(state) = self.state.as_mut() else {
            return DemandThumbnailStep::Failed;
        };
        let Some(asset) = self.assets.get(asset_id.0) else {
            return DemandThumbnailStep::Failed;
        };
        let entry = self
            .disk_cache
            .as_ref()
            .and_then(|cache| cache.entry(asset, asset_id.0));
        let cached = entry
            .as_ref()
            .and_then(|entry| self.disk_cache.as_ref()?.load(entry));
        let from_disk = cached.is_some();
        let thumbnails = cached.or_else(|| {
            if let Some(bytes) = bytes {
                make_asset_thumbnails_from_bytes_shared(
                    &mut state.decompressor,
                    bytes.as_ref(),
                    asset.layout,
                    self.preparation
                        .as_ref()
                        .map(|handle| (handle, self.document_generation, asset_id)),
                )
            } else {
                let bytes = state.loader.load_image_bytes(limit, &asset.source)?;
                make_asset_thumbnails_from_bytes_shared(
                    &mut state.decompressor,
                    &bytes,
                    asset.layout,
                    self.preparation
                        .as_ref()
                        .map(|handle| (handle, self.document_generation, asset_id)),
                )
            }
        });
        let Some(thumbnails) = thumbnails else {
            return DemandThumbnailStep::Failed;
        };
        if generation_is_cancelled(&self.cancel, self.generation) {
            return DemandThumbnailStep::Cancelled;
        }

        match Self::emit_thumbnails(
            self.document_generation,
            self.generation,
            &self.cancel,
            asset.first_page,
            &thumbnails,
            emit,
        ) {
            ThumbnailEmission::Generated => {
                if !from_disk && let (Some(cache), Some(entry)) = (&self.disk_cache, entry) {
                    cache.save_later(entry, thumbnails);
                }
                state.generated.insert(asset_id);
                DemandThumbnailStep::Generated
            }
            ThumbnailEmission::Cancelled => DemandThumbnailStep::Cancelled,
            ThumbnailEmission::OutputClosed => DemandThumbnailStep::OutputClosed,
        }
    }

    fn is_cancelled(&self) -> bool {
        generation_is_cancelled(&self.cancel, self.generation)
    }

    fn generate_at<F>(
        limit: crate::archive::ArchiveExpansionLimit,
        assets: &[ImageAsset],
        pages: &[Page],
        document_generation: u64,
        generation: u64,
        cancel: &AtomicU64,
        page_index: usize,
        state: &mut ThumbnailWorkerState,
        disk_cache: Option<&ThumbnailDiskCache>,
        preparation: Option<&crate::viewer::SmartCropPreparationHandle>,
        emit: &mut F,
    ) -> bool
    where
        F: FnMut(ThumbnailResult) -> bool,
    {
        let Some(asset_id) = state.take_asset_for_page(pages, page_index) else {
            return true;
        };
        Self::generate_asset(
            limit,
            assets,
            document_generation,
            generation,
            cancel,
            asset_id,
            state,
            disk_cache,
            preparation,
            emit,
        )
    }

    fn generate_asset<F>(
        limit: crate::archive::ArchiveExpansionLimit,
        assets: &[ImageAsset],
        document_generation: u64,
        generation: u64,
        cancel: &AtomicU64,
        asset_id: AssetId,
        state: &mut ThumbnailWorkerState,
        disk_cache: Option<&ThumbnailDiskCache>,
        preparation: Option<&crate::viewer::SmartCropPreparationHandle>,
        emit: &mut F,
    ) -> bool
    where
        F: FnMut(ThumbnailResult) -> bool,
    {
        let Some(asset) = assets.get(asset_id.0) else {
            return true;
        };
        let entry = disk_cache.and_then(|cache| cache.entry(asset, asset_id.0));
        let cached = entry.as_ref().and_then(|entry| disk_cache?.load(entry));
        let from_disk = cached.is_some();
        let Some(thumbnails) = cached.or_else(|| {
            let bytes = state.loader.load_image_bytes(limit, &asset.source)?;
            make_asset_thumbnails_from_bytes_shared(
                &mut state.decompressor,
                &bytes,
                asset.layout,
                preparation.map(|handle| (handle, document_generation, asset_id)),
            )
        }) else {
            return true;
        };
        if generation_is_cancelled(cancel, generation) {
            return true;
        }

        let emitted = Self::emit_thumbnails(
            document_generation,
            generation,
            cancel,
            asset.first_page,
            &thumbnails,
            emit,
        );
        if emitted == ThumbnailEmission::Generated
            && !from_disk
            && let (Some(cache), Some(entry)) = (disk_cache, entry)
        {
            cache.save_later(entry, thumbnails);
        }
        emitted != ThumbnailEmission::OutputClosed
    }

    fn emit_thumbnails<F>(
        document_generation: u64,
        generation: u64,
        cancel: &AtomicU64,
        first_page: usize,
        thumbnails: &AssetThumbnails,
        emit: &mut F,
    ) -> ThumbnailEmission
    where
        F: FnMut(ThumbnailResult) -> bool,
    {
        match thumbnails {
            AssetThumbnails::Single(thumbnail) => {
                if !emit(ThumbnailResult {
                    document_generation,
                    generation,
                    index: first_page,
                    thumbnail: thumbnail.clone(),
                }) {
                    return ThumbnailEmission::OutputClosed;
                }
            }
            AssetThumbnails::Spread { right, left } => {
                if !emit(ThumbnailResult {
                    document_generation,
                    generation,
                    index: first_page,
                    thumbnail: right.clone(),
                }) {
                    return ThumbnailEmission::OutputClosed;
                }
                if generation_is_cancelled(cancel, generation) {
                    return ThumbnailEmission::Cancelled;
                }
                if !emit(ThumbnailResult {
                    document_generation,
                    generation,
                    index: first_page + 1,
                    thumbnail: left.clone(),
                }) {
                    return ThumbnailEmission::OutputClosed;
                }
            }
        }
        if generation_is_cancelled(cancel, generation) {
            ThumbnailEmission::Cancelled
        } else {
            ThumbnailEmission::Generated
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThumbnailEmission {
    Generated,
    Cancelled,
    OutputClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NearThumbnailStep {
    Generated,
    Skipped,
    Failed,
    Cancelled,
    OutputClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DemandThumbnailStep {
    Generated,
    Failed,
    Cancelled,
    OutputClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThumbnailStep {
    More,
    Finished,
    OutputClosed,
}

fn generation_is_cancelled(cancel: &AtomicU64, generation: u64) -> bool {
    cancel.load(Ordering::Relaxed) != generation
}

#[cfg(test)]
mod tests {
    use super::super::disk_cache::ThumbnailDiskCache;
    use super::*;
    use crate::archive::load_document_from_path;
    use crate::document::{ImageLayout, ImageSource, PagePart};
    use std::io::{Cursor, Write};

    fn page(asset_id: usize, part: PagePart) -> Page {
        Page {
            asset_id: AssetId(asset_id),
            part,
        }
    }

    fn jpeg_bytes(width: u32, height: u32) -> gtk::glib::Bytes {
        let mut jpeg = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            width,
            height,
            image::Rgb([32, 64, 128]),
        ))
        .write_to(&mut jpeg, image::ImageFormat::Jpeg)
        .unwrap();
        gtk::glib::Bytes::from_owned(jpeg.into_inner())
    }

    fn spread_worker(generation: u64, cancel: Arc<AtomicU64>) -> ThumbnailWorker {
        ThumbnailWorker::new_skipping(
            vec![ImageAsset {
                source: ImageSource::Memory(jpeg_bytes(8, 4)),
                first_page: 0,
                layout: ImageLayout::Spread,
                archive_identity: None,
            }],
            vec![page(0, PagePart::Right), page(0, PagePart::Left)],
            3,
            generation,
            cancel,
            HashSet::new(),
        )
    }

    #[test]
    fn png_webp_thumbnail_paths_reuse_live_pixels_without_publishing_crop_or_retaining_images() {
        use crate::viewer::ANALYSIS_COUNT;
        use crate::viewer::SmartCropPreparationScheduler;
        for format in [image::ImageFormat::Png, image::ImageFormat::WebP] {
            let mut encoded = Cursor::new(Vec::new());
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                80,
                400,
                image::Rgb([20, 30, 40]),
            ))
            .write_to(&mut encoded, format)
            .unwrap();
            let bytes = gtk::glib::Bytes::from_owned(encoded.into_inner());
            for path in [0, 1, 2] {
                let scheduler = SmartCropPreparationScheduler::new(|_| true);
                scheduler.activate_document(3);
                let handle = scheduler.handle();
                let decoded = handle.decode_png_webp(3, AssetId(0), &bytes).unwrap();
                let weak = Arc::downgrade(&decoded);
                let analyses = ANALYSIS_COUNT.get();
                let mut worker = ThumbnailWorker::new_skipping(
                    vec![ImageAsset {
                        source: ImageSource::Memory(bytes.clone()),
                        first_page: 0,
                        layout: ImageLayout::Single,
                        archive_identity: None,
                    }],
                    vec![page(0, PagePart::Whole)],
                    3,
                    7,
                    Arc::new(AtomicU64::new(7)),
                    HashSet::new(),
                )
                .with_preparation(Some(handle.clone()));
                let mut results = Vec::new();
                let mut emit = |result: ThumbnailResult| {
                    results.push(result);
                    true
                };
                match path {
                    0 => assert_eq!(
                        worker.run_near(AssetId(0), &bytes, &mut emit),
                        NearThumbnailStep::Generated
                    ),
                    1 => assert_eq!(
                        worker.run_demand(Default::default(), AssetId(0), None, &mut emit),
                        DemandThumbnailStep::Generated
                    ),
                    _ => assert_eq!(
                        worker.run_next(Default::default(), &mut emit),
                        ThumbnailStep::Finished
                    ),
                }
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].thumbnail.height, 200);
                assert_eq!(handle.decode_count(), 1);
                assert_eq!(ANALYSIS_COUNT.get(), analyses);
                assert!(handle.take_ready(3, AssetId(0)).is_none());
                drop(decoded);
                assert!(
                    weak.upgrade().is_none(),
                    "thumbnail worker retains only scaled RGB pixels"
                );
                let _fresh = handle.decode_png_webp(3, AssetId(0), &bytes).unwrap();
                assert_eq!(handle.decode_count(), 2);
            }
        }
    }

    #[test]
    fn jpeg_thumbnails_do_not_decode_or_analyze_through_smart_crop() {
        use crate::viewer::SmartCropPreparationScheduler;
        use crate::viewer::{ANALYSIS_COUNT, CPU_DECODE_COUNT};
        let scheduler = SmartCropPreparationScheduler::new(|_| true);
        scheduler.activate_document(3);
        let handle = scheduler.handle();
        let mut worker =
            spread_worker(7, Arc::new(AtomicU64::new(7))).with_preparation(Some(handle.clone()));
        let counts = (
            ANALYSIS_COUNT.get(),
            CPU_DECODE_COUNT.get(),
            handle.decode_count(),
        );
        let mut results = Vec::new();
        assert_eq!(
            worker.run_demand(Default::default(), AssetId(0), None, &mut |result| {
                results.push(result);
                true
            }),
            DemandThumbnailStep::Generated
        );
        assert_eq!(results.len(), 2);
        assert_eq!(
            (
                ANALYSIS_COUNT.get(),
                CPU_DECODE_COUNT.get(),
                handle.decode_count()
            ),
            counts
        );
        assert!(handle.take_ready(3, AssetId(0)).is_none());
    }

    #[test]
    fn zip_and_non_solid_7z_disk_hits_skip_archive_reader_and_keep_generation() {
        let temp = tempfile::tempdir().unwrap();
        let mut image = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(3, 4, image::Rgb([12, 34, 56])))
            .write_to(&mut image, image::ImageFormat::Png)
            .unwrap();
        let image = image.into_inner();
        for extension in ["cbz", "cb7"] {
            let path = temp.path().join(format!("book.{extension}"));
            if extension == "cbz" {
                let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
                writer
                    .start_file("page.png", zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(&image).unwrap();
                writer.finish().unwrap();
            } else {
                let mut writer = sevenz_rust::SevenZWriter::create(&path).unwrap();
                let mut entry = sevenz_rust::SevenZArchiveEntry::new();
                entry.name = "page.png".into();
                entry.has_stream = true;
                writer
                    .push_archive_entry(entry, Some(Cursor::new(&image)))
                    .unwrap();
                writer.finish().unwrap();
            }
            let (document, _) = load_document_from_path(Default::default(), &path).unwrap();
            let cache = ThumbnailDiskCache::new(temp.path().join("cache"), &path);
            let cancel = Arc::new(AtomicU64::new(7));
            let mut first = ThumbnailWorker::new_skipping(
                document.assets.clone(),
                document.pages.clone(),
                18,
                7,
                cancel.clone(),
                HashSet::new(),
            );
            first.disk_cache = Some(cache.clone());
            let mut generated = Vec::new();
            assert_eq!(
                first.run_demand(Default::default(), AssetId(0), None, &mut |result| {
                    generated.push(result.thumbnail);
                    true
                }),
                DemandThumbnailStep::Generated
            );
            assert_eq!(first.state.as_ref().unwrap().loader.archive_open_count, 1);
            assert_eq!(first.state.as_ref().unwrap().loader.source_load_count, 1);
            cache.flush_writes();
            let mut worker = ThumbnailWorker::new_skipping(
                document.assets,
                document.pages,
                19,
                7,
                cancel.clone(),
                HashSet::new(),
            );
            worker.disk_cache = Some(cache);
            let scheduler = crate::viewer::SmartCropPreparationScheduler::new(|_| true);
            scheduler.activate_document(19);
            let handle = scheduler.handle();
            worker.preparation = Some(handle.clone());
            let analyses = crate::viewer::ANALYSIS_COUNT.get();
            let mut results = Vec::new();
            assert_eq!(
                worker.run_demand(Default::default(), AssetId(0), None, &mut |result| {
                    results.push(result);
                    true
                }),
                DemandThumbnailStep::Generated
            );
            assert_eq!(results.len(), 1);
            assert_eq!(
                (results[0].document_generation, results[0].generation),
                (19, 7)
            );
            assert_eq!(results[0].thumbnail, generated.remove(0));
            assert_eq!(worker.state.as_ref().unwrap().loader.archive_open_count, 0);
            assert_eq!(worker.state.as_ref().unwrap().loader.source_load_count, 0);
            assert_eq!(handle.decode_count(), 0);
            assert_eq!(crate::viewer::ANALYSIS_COUNT.get(), analyses);
            assert!(handle.take_ready(19, AssetId(0)).is_none());
            cancel.store(8, Ordering::Relaxed);
            assert_eq!(
                worker.run_demand(Default::default(), AssetId(0), None, &mut |_| panic!(
                    "stale disk hit"
                )),
                DemandThumbnailStep::Cancelled
            );
        }
    }

    #[test]
    fn image_folder_reopen_hits_disk_and_write_failure_keeps_generated_result() {
        let temp = tempfile::tempdir().unwrap();
        let pages = temp.path().join("pages");
        std::fs::create_dir(&pages).unwrap();
        let mut image = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(3, 4, image::Rgb([12, 34, 56])))
            .write_to(&mut image, image::ImageFormat::Png)
            .unwrap();
        let source = pages.join("one.png");
        std::fs::write(&source, image.get_ref()).unwrap();
        std::fs::write(pages.join("two.png"), image.get_ref()).unwrap();
        let (document, _) = load_document_from_path(Default::default(), &source).unwrap();
        assert_eq!(document.assets.len(), 2);
        let cache = ThumbnailDiskCache::new(temp.path().join("cache"), &source);
        let cancel = Arc::new(AtomicU64::new(7));
        let mut first = ThumbnailWorker::new_skipping(
            document.assets.clone(),
            document.pages.clone(),
            1,
            7,
            cancel.clone(),
            HashSet::new(),
        );
        first.disk_cache = Some(cache.clone());
        let mut generated = Vec::new();
        assert_eq!(
            first.run_demand(Default::default(), AssetId(0), None, &mut |result| {
                generated.push(result.thumbnail);
                true
            }),
            DemandThumbnailStep::Generated
        );
        assert_eq!(first.state.as_ref().unwrap().loader.source_load_count, 1);
        cache.flush_writes();
        let mut reopened = ThumbnailWorker::new_skipping(
            document.assets,
            document.pages,
            2,
            7,
            cancel,
            HashSet::new(),
        );
        reopened.disk_cache = Some(cache);
        let mut hit = Vec::new();
        assert_eq!(
            reopened.run_demand(Default::default(), AssetId(0), None, &mut |result| {
                hit.push(result.thumbnail);
                true
            }),
            DemandThumbnailStep::Generated
        );
        assert_eq!(hit, generated);
        assert_eq!(reopened.state.as_ref().unwrap().loader.source_load_count, 0);

        let blocked = temp.path().join("blocked");
        std::fs::write(&blocked, b"file").unwrap();
        reopened.disk_cache = Some(ThumbnailDiskCache::new(blocked.join("cache"), &source));
        let mut available = false;
        assert_eq!(
            reopened.run_demand(Default::default(), AssetId(0), None, &mut |_| {
                available = true;
                true
            }),
            DemandThumbnailStep::Generated
        );
        reopened.disk_cache.as_ref().unwrap().flush_writes();
        assert!(available);
    }

    #[test]
    fn nested_archive_reopen_uses_logical_identity_across_temp_directories() {
        fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
            let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
            for (name, bytes) in entries {
                writer
                    .start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(bytes).unwrap();
            }
            writer.finish().unwrap().into_inner()
        }
        let temp = tempfile::tempdir().unwrap();
        let mut image = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(3, 4, image::Rgb([12, 34, 56])))
            .write_to(&mut image, image::ImageFormat::Png)
            .unwrap();
        let inner = zip(&[("page.png", image.get_ref())]);
        let outer = temp.path().join("outer.cbz");
        std::fs::write(&outer, zip(&[("inner.cbz", &inner)])).unwrap();
        let (document, _) = load_document_from_path(Default::default(), &outer).unwrap();
        assert_eq!(
            document.assets[0]
                .archive_identity
                .as_ref()
                .unwrap()
                .archives,
            vec![std::path::PathBuf::from("inner.cbz")]
        );
        let first_temp = document.assets[0]
            .source
            .as_file_path()
            .unwrap()
            .to_path_buf();
        let cache = ThumbnailDiskCache::new(temp.path().join("cache"), &outer);
        let mut first = ThumbnailWorker::new_skipping(
            document.assets.clone(),
            document.pages.clone(),
            1,
            7,
            Arc::new(AtomicU64::new(7)),
            HashSet::new(),
        );
        first.disk_cache = Some(cache.clone());
        let mut generated = Vec::new();
        assert_eq!(
            first.run_demand(Default::default(), AssetId(0), None, &mut |result| {
                generated.push(result.thumbnail);
                true
            }),
            DemandThumbnailStep::Generated
        );
        cache.flush_writes();
        drop(first);
        drop(document);
        let (reopened, _) = load_document_from_path(Default::default(), &outer).unwrap();
        assert_ne!(
            reopened.assets[0].source.as_file_path().unwrap(),
            first_temp
        );
        assert!(
            cache
                .load(&cache.entry(&reopened.assets[0], 0).unwrap())
                .is_some()
        );
        let mut second = ThumbnailWorker::new_skipping(
            reopened.assets.clone(),
            reopened.pages.clone(),
            2,
            7,
            Arc::new(AtomicU64::new(7)),
            HashSet::new(),
        );
        second.disk_cache = Some(cache);
        let mut hit = Vec::new();
        assert_eq!(
            second.run_demand(Default::default(), AssetId(0), None, &mut |result| {
                hit.push(result.thumbnail);
                true
            }),
            DemandThumbnailStep::Generated
        );
        assert_eq!(hit, generated);
        assert_eq!(second.state.as_ref().unwrap().loader.source_load_count, 0);
    }

    #[test]
    fn sequential_tar_lha_and_solid_7z_reopen_from_disk() {
        fn lha_entry(output: &mut Vec<u8>, name: &str, data: &[u8]) {
            let mut header = Vec::new();
            header.extend_from_slice(b"-lh0-");
            header.extend_from_slice(&(data.len() as u32).to_le_bytes());
            header.extend_from_slice(&(data.len() as u32).to_le_bytes());
            header.extend_from_slice(&0_u32.to_le_bytes());
            header.push(0x20);
            header.push(0);
            header.push(name.len() as u8);
            header.extend_from_slice(name.as_bytes());
            header.extend_from_slice(&0_u16.to_le_bytes());
            output.push(header.len() as u8);
            output.push(
                header
                    .iter()
                    .fold(0_u8, |sum, byte| sum.wrapping_add(*byte)),
            );
            output.extend_from_slice(&header);
            output.extend_from_slice(data);
        }
        let temp = tempfile::tempdir().unwrap();
        let mut image = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(3, 4, image::Rgb([12, 34, 56])))
            .write_to(&mut image, image::ImageFormat::Png)
            .unwrap();
        let bytes = image.into_inner();
        for extension in ["tar", "lha", "cb7"] {
            let path = temp.path().join(format!("sequential.{extension}"));
            match extension {
                "tar" => {
                    let mut writer = tar::Builder::new(std::fs::File::create(&path).unwrap());
                    let mut header = tar::Header::new_gnu();
                    header.set_size(bytes.len() as u64);
                    header.set_mode(0o644);
                    header.set_cksum();
                    writer
                        .append_data(&mut header, "page.png", bytes.as_slice())
                        .unwrap();
                    writer.finish().unwrap();
                }
                "lha" => {
                    let mut output = Vec::new();
                    lha_entry(&mut output, "page.png", &bytes);
                    output.push(0);
                    std::fs::write(&path, output).unwrap();
                }
                _ => {
                    let mut writer = sevenz_rust::SevenZWriter::create(&path).unwrap();
                    let entries = ["page.png", "second.png"]
                        .map(|name| {
                            let mut entry = sevenz_rust::SevenZArchiveEntry::new();
                            entry.name = name.into();
                            entry.has_stream = true;
                            entry
                        })
                        .to_vec();
                    let readers = vec![
                        sevenz_rust::SourceReader::new(Cursor::new(bytes.clone())),
                        sevenz_rust::SourceReader::new(Cursor::new(bytes.clone())),
                    ];
                    writer
                        .push_archive_entries(entries, sevenz_rust::SeqReader::new(readers))
                        .unwrap();
                    writer.finish().unwrap();
                }
            }
            let (document, _) = load_document_from_path(Default::default(), &path).unwrap();
            assert!(matches!(
                document.assets[0].source,
                ImageSource::Memory(_) | ImageSource::File(_)
            ));
            let cache = ThumbnailDiskCache::new(temp.path().join("cache"), &path);
            let mut first = ThumbnailWorker::new_skipping(
                document.assets,
                document.pages,
                1,
                7,
                Arc::new(AtomicU64::new(7)),
                HashSet::new(),
            );
            first.disk_cache = Some(cache.clone());
            let mut generated = Vec::new();
            assert_eq!(
                first.run_demand(Default::default(), AssetId(0), None, &mut |result| {
                    generated.push(result.thumbnail);
                    true
                }),
                DemandThumbnailStep::Generated
            );
            assert_eq!(first.state.as_ref().unwrap().loader.source_load_count, 1);
            cache.flush_writes();
            let (reopened, _) = load_document_from_path(Default::default(), &path).unwrap();
            let mut second = ThumbnailWorker::new_skipping(
                reopened.assets,
                reopened.pages,
                2,
                7,
                Arc::new(AtomicU64::new(7)),
                HashSet::new(),
            );
            second.disk_cache = Some(cache);
            let mut hit = Vec::new();
            assert_eq!(
                second.run_demand(Default::default(), AssetId(0), None, &mut |result| {
                    hit.push(result.thumbnail);
                    true
                }),
                DemandThumbnailStep::Generated
            );
            assert_eq!(hit, generated);
            assert_eq!(second.state.as_ref().unwrap().loader.source_load_count, 0);
        }
    }

    #[test]
    fn rar_reopen_uses_sequential_disk_identity() {
        // RAR5 with one 3x4 PNG, generated once with `rar a` for this fixture.
        const RAR: &[u8] = &[
            0x52, 0x61, 0x72, 0x21, 0x1a, 0x07, 0x01, 0x00, 0x33, 0x92, 0xb5, 0xe5, 0x0a, 0x01,
            0x05, 0x06, 0x00, 0x05, 0x01, 0x01, 0x80, 0x80, 0x00, 0x4d, 0x8f, 0xcd, 0x31, 0x34,
            0x02, 0x03, 0x0b, 0xaa, 0x01, 0x04, 0xaa, 0x01, 0xa4, 0x83, 0x02, 0xb5, 0xf7, 0x63,
            0x10, 0x80, 0x00, 0x01, 0x16, 0x61, 0x67, 0x6e, 0x61, 0x6d, 0x2d, 0x69, 0x73, 0x73,
            0x75, 0x65, 0x38, 0x38, 0x2d, 0x70, 0x61, 0x67, 0x65, 0x2e, 0x70, 0x6e, 0x67, 0x0a,
            0x03, 0x13, 0xcb, 0x52, 0xbb, 0x6a, 0x40, 0x97, 0x89, 0x2e, 0x89, 0x50, 0x4e, 0x47,
            0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00,
            0x00, 0x03, 0x00, 0x00, 0x00, 0x04, 0x08, 0x02, 0x00, 0x00, 0x00, 0xc4, 0x4f, 0x12,
            0x50, 0x00, 0x00, 0x00, 0x01, 0x73, 0x52, 0x47, 0x42, 0x00, 0xae, 0xce, 0x1c, 0xe9,
            0x00, 0x00, 0x00, 0x44, 0x65, 0x58, 0x49, 0x66, 0x4d, 0x4d, 0x00, 0x2a, 0x00, 0x00,
            0x00, 0x08, 0x00, 0x01, 0x87, 0x69, 0x00, 0x04, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
            0x00, 0x1a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xa0, 0x01, 0x00, 0x03, 0x00, 0x00,
            0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0xa0, 0x02, 0x00, 0x04, 0x00, 0x00, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x03, 0xa0, 0x03, 0x00, 0x04, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
            0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0xe5, 0xfe, 0x82, 0x49, 0x00, 0x00, 0x00, 0x14,
            0x49, 0x44, 0x41, 0x54, 0x08, 0x1d, 0x63, 0xe4, 0x51, 0xb2, 0x60, 0x00, 0x03, 0x26,
            0x08, 0x05, 0x24, 0xb1, 0xb1, 0x00, 0x0f, 0xc0, 0x00, 0x6e, 0x64, 0x74, 0x98, 0x4b,
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82, 0x1d, 0x77,
            0x56, 0x51, 0x03, 0x05, 0x04, 0x00,
        ];
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("book.cbr");
        std::fs::write(&path, RAR).unwrap();
        let (document, _) = load_document_from_path(Default::default(), &path).unwrap();
        assert_eq!(document.assets.len(), 1);
        let cache = ThumbnailDiskCache::new(temp.path().join("cache"), &path);
        let mut first = ThumbnailWorker::new_skipping(
            document.assets,
            document.pages,
            1,
            7,
            Arc::new(AtomicU64::new(7)),
            HashSet::new(),
        );
        first.disk_cache = Some(cache.clone());
        let mut generated = Vec::new();
        assert_eq!(
            first.run_demand(Default::default(), AssetId(0), None, &mut |result| {
                generated.push(result.thumbnail);
                true
            }),
            DemandThumbnailStep::Generated
        );
        assert_eq!(first.state.as_ref().unwrap().loader.source_load_count, 1);
        cache.flush_writes();
        let (reopened, _) = load_document_from_path(Default::default(), &path).unwrap();
        let mut second = ThumbnailWorker::new_skipping(
            reopened.assets,
            reopened.pages,
            2,
            7,
            Arc::new(AtomicU64::new(7)),
            HashSet::new(),
        );
        second.disk_cache = Some(cache);
        let mut hit = Vec::new();
        assert_eq!(
            second.run_demand(Default::default(), AssetId(0), None, &mut |result| {
                hit.push(result.thumbnail);
                true
            }),
            DemandThumbnailStep::Generated
        );
        assert_eq!(hit, generated);
        assert_eq!(second.state.as_ref().unwrap().loader.source_load_count, 0);
    }

    #[test]
    fn near_spread_uses_bytes_once_and_distributed_skips_the_asset() {
        let cancel = Arc::new(AtomicU64::new(7));
        let mut worker = spread_worker(7, cancel);
        let bytes = jpeg_bytes(8, 4);
        let mut results = Vec::new();

        assert_eq!(
            worker.run_near(AssetId(0), &bytes, &mut |result| {
                results.push(result.index);
                true
            }),
            NearThumbnailStep::Generated
        );
        while worker.run_next(Default::default(), &mut |_| panic!("distributed duplicate"))
            == ThumbnailStep::More
        {}

        assert_eq!(results, vec![0, 1]);
        assert!(
            worker
                .state
                .as_ref()
                .is_some_and(|state| state.generated.contains(&AssetId(0)))
        );
    }

    #[test]
    fn failed_near_decode_remains_eligible_for_distributed_retry() {
        let cancel = Arc::new(AtomicU64::new(7));
        let mut worker = spread_worker(7, cancel);
        let invalid = gtk::glib::Bytes::from_static(b"not an image");

        assert_eq!(
            worker.run_near(AssetId(0), &invalid, &mut |_| true),
            NearThumbnailStep::Failed
        );
        assert!(
            worker
                .state
                .as_ref()
                .is_some_and(|state| !state.generated.contains(&AssetId(0)))
        );

        let mut results = Vec::new();
        while worker.run_next(Default::default(), &mut |result| {
            results.push(result.index);
            true
        }) == ThumbnailStep::More
        {}
        assert_eq!(results, vec![0, 1]);
    }

    #[test]
    fn cancelled_near_work_emits_nothing() {
        let cancel = Arc::new(AtomicU64::new(8));
        let mut worker = spread_worker(7, cancel);
        let bytes = jpeg_bytes(8, 4);

        assert_eq!(
            worker.run_near(AssetId(0), &bytes, &mut |_| panic!("stale result")),
            NearThumbnailStep::Cancelled
        );
    }

    #[test]
    fn spread_asset_is_taken_once_regardless_of_requested_side() {
        let pages = vec![page(0, PagePart::Right), page(0, PagePart::Left)];

        let mut right_first = ThumbnailWorkerState::with_generated(HashSet::new()).unwrap();
        assert_eq!(right_first.take_asset_for_page(&pages, 0), Some(AssetId(0)));
        assert_eq!(right_first.take_asset_for_page(&pages, 1), None);
        assert_eq!(right_first.generated, HashSet::from([AssetId(0)]));

        let mut left_first = ThumbnailWorkerState::with_generated(HashSet::new()).unwrap();
        assert_eq!(left_first.take_asset_for_page(&pages, 1), Some(AssetId(0)));
        assert_eq!(left_first.take_asset_for_page(&pages, 0), None);
        assert_eq!(left_first.generated, HashSet::from([AssetId(0)]));
    }

    #[test]
    fn spread_asset_pages_are_not_scheduled_twice() {
        let pages = vec![page(0, PagePart::Right), page(0, PagePart::Left)];
        let mut state = ThumbnailWorkerState::with_generated(HashSet::new()).unwrap();

        assert_eq!(state.take_asset_for_page(&pages, 0), Some(AssetId(0)));
        assert_eq!(state.take_asset_for_page(&pages, 1), None);
        assert_eq!(state.generated, HashSet::from([AssetId(0)]));
    }

    #[test]
    fn initially_generated_assets_are_skipped_without_changing_schedule_order() {
        let cancel = Arc::new(AtomicU64::new(7));
        let worker = spread_worker(7, cancel);
        let worker = ThumbnailWorker::new_skipping(
            worker.assets,
            worker.pages,
            worker.document_generation,
            worker.generation,
            worker.cancel,
            HashSet::from([AssetId(0)]),
        );
        let mut results = Vec::new();

        worker.run(|result| {
            results.push(result.index);
            true
        });

        assert!(results.is_empty());
    }

    #[test]
    fn explicit_demand_regenerates_an_asset_in_generated_history() {
        let cancel = Arc::new(AtomicU64::new(7));
        let mut worker = spread_worker(7, cancel);
        worker.state.as_mut().unwrap().generated.insert(AssetId(0));
        let bytes = jpeg_bytes(8, 4);
        let mut results = Vec::new();

        assert_eq!(
            worker.run_demand(
                Default::default(),
                AssetId(0),
                Some(&bytes),
                &mut |result| {
                    results.push(result.index);
                    true
                }
            ),
            DemandThumbnailStep::Generated
        );
        assert_eq!(results, vec![0, 1]);
    }

    #[test]
    fn assets_missing_from_the_success_set_remain_eligible_for_background_retry() {
        let pages = vec![page(0, PagePart::Whole), page(1, PagePart::Whole)];
        let mut state = ThumbnailWorkerState::with_generated(HashSet::from([AssetId(0)])).unwrap();

        assert_eq!(state.take_asset_for_page(&pages, 0), None);
        assert_eq!(state.take_asset_for_page(&pages, 1), Some(AssetId(1)));
    }

    #[test]
    fn generation_change_cancels_the_worker_generation() {
        let cancel = AtomicU64::new(4);

        assert!(!generation_is_cancelled(&cancel, 4));
        assert!(generation_is_cancelled(&cancel, 3));

        cancel.store(5, Ordering::Relaxed);
        assert!(generation_is_cancelled(&cancel, 4));
    }

    #[test]
    fn spread_results_are_emitted_right_then_left_once() {
        let cancel = Arc::new(AtomicU64::new(7));
        let worker = spread_worker(7, cancel);
        let mut results = Vec::new();

        worker.run(|result| {
            results.push((result.document_generation, result.generation, result.index));
            true
        });

        assert_eq!(results, vec![(3, 7, 0), (3, 7, 1)]);
    }

    #[test]
    fn cancellation_between_spread_results_suppresses_left() {
        let cancel = Arc::new(AtomicU64::new(7));
        let worker = spread_worker(7, cancel.clone());
        let mut results = Vec::new();

        worker.run(|result| {
            results.push(result.index);
            cancel.store(8, Ordering::Relaxed);
            true
        });

        assert_eq!(results, vec![0]);
    }

    #[test]
    fn failed_right_delivery_skips_left() {
        let cancel = Arc::new(AtomicU64::new(7));
        let worker = spread_worker(7, cancel);
        let mut results = Vec::new();

        worker.run(|result| {
            results.push(result.index);
            false
        });

        assert_eq!(results, vec![0]);
    }

    #[test]
    fn cancelled_generation_emits_no_results() {
        let cancel = Arc::new(AtomicU64::new(8));
        let worker = spread_worker(7, cancel);
        let mut result_count = 0;

        worker.run(|_| {
            result_count += 1;
            true
        });

        assert_eq!(result_count, 0);
    }
}
