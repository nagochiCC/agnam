mod lha;
mod rar;
pub(super) mod rar_native;
mod seven_zip;
mod tar;
mod zip;

use super::format::ArchiveFormat;
use super::{IMAGE_DIMENSION_PROBE_SIZE, is_wide_from_bytes};
use crate::document::{Document, ImageLayout, ImageSource};
use crate::error::AppError;
use gtk::glib;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) const SEQUENTIAL_ARCHIVE_MEMORY_LIMIT_BYTES: usize = 256 * 1024 * 1024;
const CACHE_APPLICATION_DIRECTORY: &str = "agnam";
const SEQUENTIAL_TEMP_DIRECTORY: &str = "tmp";

fn sequential_spill_root() -> PathBuf {
    glib::user_cache_dir()
        .join(CACHE_APPLICATION_DIRECTORY)
        .join(SEQUENTIAL_TEMP_DIRECTORY)
}

pub(super) fn access_strategy(
    archive_path: &Path,
) -> Result<Option<crate::archive::ArchiveAccessStrategy>, AppError> {
    Ok(match ArchiveFormat::from_path(archive_path) {
        Some(ArchiveFormat::Zip) => Some(zip::access_strategy(archive_path)),
        Some(ArchiveFormat::Rar) => Some(rar::access_strategy(archive_path)),
        Some(ArchiveFormat::SevenZip) => Some(seven_zip::access_strategy(archive_path)?),
        Some(ArchiveFormat::Tar) => Some(tar::access_strategy(archive_path)),
        Some(ArchiveFormat::Lha) => Some(lha::access_strategy(archive_path)),
        None => None,
    })
}

pub(super) fn supports_sequential_progress(archive_path: &Path) -> bool {
    match ArchiveFormat::from_path(archive_path) {
        Some(ArchiveFormat::Zip) => zip::supports_sequential_progress(archive_path),
        Some(ArchiveFormat::Rar) => rar::supports_sequential_progress(archive_path),
        Some(ArchiveFormat::SevenZip) => seven_zip::supports_sequential_progress(archive_path),
        Some(ArchiveFormat::Tar) => tar::supports_sequential_progress(archive_path),
        Some(ArchiveFormat::Lha) => lha::supports_sequential_progress(archive_path),
        None => false,
    }
}

pub(super) struct RandomAccessArchiveReader {
    backend: RandomAccessArchiveReaderBackend,
}

enum RandomAccessArchiveReaderBackend {
    Zip(zip::RandomAccessReader),
    SevenZip(seven_zip::RandomAccessReader),
}

impl RandomAccessArchiveReader {
    pub(super) fn open(archive_path: &Path) -> Option<Self> {
        let backend = match ArchiveFormat::from_path(archive_path)? {
            ArchiveFormat::Zip => {
                RandomAccessArchiveReaderBackend::Zip(zip::RandomAccessReader::open(archive_path)?)
            }
            ArchiveFormat::SevenZip => RandomAccessArchiveReaderBackend::SevenZip(
                seven_zip::RandomAccessReader::open(archive_path)?,
            ),
            ArchiveFormat::Rar | ArchiveFormat::Tar | ArchiveFormat::Lha => return None,
        };
        Some(Self { backend })
    }

    pub(super) fn read_entry(
        &mut self,
        limit: crate::archive::ArchiveExpansionLimit,
        entry_index: usize,
        expected_name: &str,
    ) -> Result<Option<glib::Bytes>, AppError> {
        match &mut self.backend {
            RandomAccessArchiveReaderBackend::Zip(reader) => {
                reader.read_entry(limit, entry_index, expected_name)
            }
            RandomAccessArchiveReaderBackend::SevenZip(reader) => {
                reader.read_entry(limit, entry_index, expected_name)
            }
        }
    }
}

pub(super) fn has_nested_archives(format: ArchiveFormat, archive_path: &Path) -> bool {
    match format {
        ArchiveFormat::Zip => zip::has_nested_archives(archive_path),
        ArchiveFormat::Rar => rar::has_nested_archives(archive_path),
        ArchiveFormat::SevenZip => seven_zip::has_nested_archives(archive_path),
        ArchiveFormat::Tar => tar::has_nested_archives(archive_path),
        ArchiveFormat::Lha => lha::has_nested_archives(archive_path),
    }
}

pub(super) fn has_viewable_content(
    format: ArchiveFormat,
    archive_path: &Path,
) -> Result<bool, AppError> {
    match format {
        ArchiveFormat::Zip => zip::has_viewable_content(archive_path),
        ArchiveFormat::Rar => rar::has_viewable_content(archive_path),
        ArchiveFormat::SevenZip => seven_zip::has_viewable_content(archive_path),
        ArchiveFormat::Tar => tar::has_viewable_content(archive_path),
        ArchiveFormat::Lha => lha::has_viewable_content(archive_path),
    }
}

pub(super) fn extract_to_dir_with_budget(
    archive_path: &Path,
    destination: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<(), AppError> {
    budget.check_cancel()?;
    match ArchiveFormat::from_path(archive_path) {
        Some(ArchiveFormat::Zip) => zip::extract_to_dir(archive_path, destination, budget),
        Some(ArchiveFormat::Rar) => rar::extract_to_dir(archive_path, destination, budget),
        Some(ArchiveFormat::SevenZip) => {
            seven_zip::extract_to_dir(archive_path, destination, budget)
        }
        Some(ArchiveFormat::Tar) => tar::extract_to_dir(archive_path, destination, budget),
        Some(ArchiveFormat::Lha) => lha::extract_to_dir(archive_path, destination, budget),
        None => Err(unsupported_archive_error()),
    }
}

/// Materializes an actual nested archive to a workspace-owned path. Image
/// byte APIs never receive the larger limit. Sequential backends retain their
/// existing whole-level extraction semantics.
pub(super) fn materialize_nested_entry(
    archive: &Path,
    entry: &Path,
    destination: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<Option<PathBuf>, AppError> {
    if !crate::archive::is_normal_relative_path(entry) || !crate::archive::is_archive_ext(entry) {
        return Err(AppError::Archive("Invalid nested archive entry".into()));
    }
    budget.check_cancel()?;
    // Count the containing level before large writes. Missing logical
    // candidates remain distinct from corrupt/decode/I/O failure.
    if !entry_paths_with_budget(archive, budget)?
        .iter()
        .any(|path| path == entry)
    {
        return Ok(None);
    }
    let output = crate::archive::safety::safe_archive_output_path(destination, entry)?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match ArchiveFormat::from_path(archive) {
        Some(ArchiveFormat::Zip) => zip::materialize_nested_entry(archive, entry, &output, budget)?,
        Some(ArchiveFormat::SevenZip)
            if seven_zip::access_strategy(archive)?
                == crate::archive::ArchiveAccessStrategy::RandomAccess =>
        {
            seven_zip::materialize_nested_entry(archive, entry, &output, budget)?;
        }
        _ => extract_to_dir_with_budget(archive, destination, budget)?,
    }
    if !output.is_file() {
        return Ok(None);
    }
    Ok(Some(output))
}

/// Lists file entry names from archive metadata without decoding image bytes.
/// The returned paths are still validated by the archive-content model before
/// they are exposed to the UI.
pub(super) fn entry_paths_with_budget(
    archive_path: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<Vec<PathBuf>, AppError> {
    match ArchiveFormat::from_path(archive_path) {
        Some(ArchiveFormat::Zip) => zip::entry_paths(archive_path, budget),
        Some(ArchiveFormat::Rar) => rar::entry_paths(archive_path, budget),
        Some(ArchiveFormat::SevenZip) => seven_zip::entry_paths(archive_path, budget),
        Some(ArchiveFormat::Tar) => tar::entry_paths(archive_path, budget),
        Some(ArchiveFormat::Lha) => lha::entry_paths(archive_path, budget),
        None => Err(unsupported_archive_error()),
    }
}

pub(super) enum CoverEntryBytes {
    Unsupported,
    Missing,
    Bytes(Vec<u8>),
}

/// Reads one logical entry for a backend with independent entry access.
pub(super) fn cover_entry_bytes(
    archive_path: &Path,
    entry_path: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<CoverEntryBytes, AppError> {
    match ArchiveFormat::from_path(archive_path) {
        Some(ArchiveFormat::Zip) => Ok(
            match zip::cover_entry_bytes(archive_path, entry_path, budget)? {
                Some(bytes) => CoverEntryBytes::Bytes(bytes),
                None => CoverEntryBytes::Missing,
            },
        ),
        Some(ArchiveFormat::SevenZip) => {
            seven_zip::cover_entry_bytes(archive_path, entry_path, budget)
        }
        Some(ArchiveFormat::Rar | ArchiveFormat::Tar | ArchiveFormat::Lha) => {
            Ok(CoverEntryBytes::Unsupported)
        }
        None => Err(unsupported_archive_error()),
    }
}

pub(super) fn load_document(
    limit: crate::archive::ArchiveExpansionLimit,
    archive_path: &Path,
) -> Result<Document, AppError> {
    match ArchiveFormat::from_path(archive_path) {
        Some(ArchiveFormat::Zip) => zip::load_document(limit, archive_path),
        Some(ArchiveFormat::Rar) => rar::load_document(limit, archive_path),
        Some(ArchiveFormat::SevenZip) => seven_zip::load_document(limit, archive_path),
        Some(ArchiveFormat::Tar) => tar::load_document(limit, archive_path),
        Some(ArchiveFormat::Lha) => lha::load_document(limit, archive_path),
        None => Err(unsupported_archive_error()),
    }
}

pub(super) fn load_sequential_document_with_progress(
    limit: crate::archive::ArchiveExpansionLimit,
    archive_path: &Path,
    cancel_token: &crate::archive::ProgressiveArchiveCancelToken,
    on_image: &mut dyn FnMut(crate::archive::ProgressiveArchiveImage),
) -> Result<crate::archive::ProgressiveArchiveLoadOutcome<Document>, AppError> {
    match ArchiveFormat::from_path(archive_path) {
        Some(ArchiveFormat::Rar) => {
            rar::load_document_with_progress(limit, archive_path, cancel_token, on_image)
        }
        Some(ArchiveFormat::SevenZip) => {
            seven_zip::load_document_with_progress(limit, archive_path, cancel_token, on_image)
        }
        _ => Err(unsupported_archive_error()),
    }
}

pub(super) fn stream_sequential_images(
    limit: crate::archive::ArchiveExpansionLimit,
    archive_path: &Path,
    cancel_token: &crate::archive::ProgressiveArchiveCancelToken,
    on_image: &mut dyn FnMut(crate::archive::SequentialArchiveImage),
) -> Result<crate::archive::ProgressiveArchiveLoadOutcome<()>, AppError> {
    match ArchiveFormat::from_path(archive_path) {
        Some(ArchiveFormat::Rar) => rar::stream_images(limit, archive_path, cancel_token, on_image),
        Some(ArchiveFormat::SevenZip) => {
            seven_zip::stream_images(limit, archive_path, cancel_token, on_image)
        }
        _ => Err(unsupported_archive_error()),
    }
}

pub(super) fn first_image_bytes(
    archive_path: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<Option<Vec<u8>>, AppError> {
    match ArchiveFormat::from_path(archive_path) {
        Some(ArchiveFormat::Zip) => zip::first_image_bytes(archive_path, budget),
        Some(ArchiveFormat::Rar) => rar::first_image_bytes(archive_path, budget),
        Some(ArchiveFormat::SevenZip) => seven_zip::first_image_bytes(archive_path, budget),
        Some(ArchiveFormat::Tar) => tar::first_image_bytes(archive_path, budget),
        Some(ArchiveFormat::Lha) => lha::first_image_bytes(archive_path, budget),
        None => Err(unsupported_archive_error()),
    }
}

pub(super) fn automatic_cover_hint_bytes(
    archive_path: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<Option<Vec<u8>>, AppError> {
    let entries = entry_paths_with_budget(archive_path, budget)?;
    let cover_only = crate::archive::cover_only_paths(&entries);
    let mut covers = cover_only.into_iter().collect::<Vec<_>>();
    covers
        .sort_by(|left, right| natord::compare(&left.to_string_lossy(), &right.to_string_lossy()));
    let Some(cover) = covers.first() else {
        return Ok(None);
    };
    match automatic_cover_entry_bytes(archive_path, cover, budget)? {
        CoverEntryBytes::Bytes(bytes) => Ok(Some(bytes)),
        CoverEntryBytes::Missing => Err(AppError::Archive(format!(
            "表紙entryが見つかりません: {:?}",
            cover
        ))),
        CoverEntryBytes::Unsupported => Err(AppError::Archive(
            "表紙entryの個別取得に対応していません".into(),
        )),
    }
}

fn automatic_cover_entry_bytes(
    archive_path: &Path,
    entry_path: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<CoverEntryBytes, AppError> {
    match ArchiveFormat::from_path(archive_path) {
        Some(ArchiveFormat::Zip) => Ok(
            match zip::cover_entry_bytes(archive_path, entry_path, budget)? {
                Some(bytes) => CoverEntryBytes::Bytes(bytes),
                None => CoverEntryBytes::Missing,
            },
        ),
        Some(ArchiveFormat::Rar) => {
            rar::automatic_cover_entry_bytes(archive_path, entry_path, budget)
        }
        Some(ArchiveFormat::SevenZip) => {
            seven_zip::automatic_cover_entry_bytes(archive_path, entry_path, budget)
        }
        Some(ArchiveFormat::Tar) => {
            tar::automatic_cover_entry_bytes(archive_path, entry_path, budget)
        }
        Some(ArchiveFormat::Lha) => {
            lha::automatic_cover_entry_bytes(archive_path, entry_path, budget)
        }
        None => Err(unsupported_archive_error()),
    }
}

pub(super) struct SequentialImageStorage {
    images: Vec<Option<StoredSequentialImage>>,
    retained_bytes: usize,
    memory_limit: usize,
    spill_root: PathBuf,
    temp_dir: Option<Arc<tempfile::TempDir>>,
}

struct StoredSequentialImage {
    name: String,
    backing: crate::archive::SequentialImageBacking,
    layout: ImageLayout,
}

impl SequentialImageStorage {
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self::with_limit_in(
            capacity,
            SEQUENTIAL_ARCHIVE_MEMORY_LIMIT_BYTES,
            sequential_spill_root(),
        )
    }

    pub(super) fn with_limit_in(capacity: usize, memory_limit: usize, spill_root: PathBuf) -> Self {
        Self {
            images: std::iter::repeat_with(|| None).take(capacity).collect(),
            retained_bytes: 0,
            memory_limit,
            spill_root,
            temp_dir: None,
        }
    }

    pub(super) fn push_with_budget(
        &mut self,
        name: String,
        data: Vec<u8>,
        budget: &mut crate::archive::resource::ResourceBudget,
    ) -> Result<usize, AppError> {
        let index = self.images.len();
        self.images.push(None);
        self.insert_with_budget(index, name, data, budget)?;
        Ok(index)
    }

    #[cfg(test)]
    fn push(&mut self, name: String, data: Vec<u8>) -> Result<usize, AppError> {
        self.push_with_budget(
            name,
            data,
            &mut crate::archive::resource::ResourceBudget::default(),
        )
    }

    #[cfg(test)]
    fn insert(&mut self, index: usize, name: String, data: Vec<u8>) -> Result<(), AppError> {
        self.insert_with_budget(
            index,
            name,
            data,
            &mut crate::archive::resource::ResourceBudget::default(),
        )
    }

    pub(super) fn insert_with_budget(
        &mut self,
        index: usize,
        name: String,
        data: Vec<u8>,
        budget: &mut crate::archive::resource::ResourceBudget,
    ) -> Result<(), AppError> {
        if index >= self.images.len() || self.images[index].is_some() {
            return Err(AppError::Archive(format!(
                "Sequential画像の保持indexが不正です: {index}"
            )));
        }

        let bytes = glib::Bytes::from_owned(data);
        let probe_length = bytes.len().min(IMAGE_DIMENSION_PROBE_SIZE);
        let layout = ImageLayout::from_is_wide(is_wide_from_bytes(&bytes[..probe_length]));
        let spills = self.temp_dir.is_none()
            && self
                .retained_bytes
                .checked_add(bytes.len())
                .is_none_or(|total| total > self.memory_limit);
        if spills {
            self.spill_with(index, name, bytes, layout, budget)?;
        } else if let Some(temp_dir) = &self.temp_dir {
            let path = crate::archive::sequential_image_path(temp_dir.path(), index);
            budget.write_temp(&path, bytes.as_ref())?;
            self.images[index] = Some(StoredSequentialImage {
                name,
                backing: {
                    let backing = crate::archive::SequentialImageBacking::memory(bytes);
                    backing.replace_with_file(path, temp_dir.clone());
                    backing
                },
                layout,
            });
        } else {
            self.retained_bytes += bytes.len();
            self.images[index] = Some(StoredSequentialImage {
                name,
                backing: crate::archive::SequentialImageBacking::memory(bytes),
                layout,
            });
        }
        Ok(())
    }

    fn spill_with(
        &mut self,
        index: usize,
        name: String,
        bytes: glib::Bytes,
        layout: ImageLayout,
        budget: &mut crate::archive::resource::ResourceBudget,
    ) -> Result<(), AppError> {
        std::fs::create_dir_all(&self.spill_root)?;
        let temp_dir = Arc::new(
            tempfile::Builder::new()
                .prefix("agnam-sequential-")
                .tempdir_in(&self.spill_root)?,
        );
        for (existing_index, existing) in self.images.iter().enumerate() {
            let Some(existing) = existing else { continue };
            let ImageSource::Memory(existing_bytes) = existing.backing.source() else {
                unreachable!("Sequential storage spills only once")
            };
            budget.write_temp(
                &crate::archive::sequential_image_path(temp_dir.path(), existing_index),
                existing_bytes.as_ref(),
            )?;
        }
        let image_path = crate::archive::sequential_image_path(temp_dir.path(), index);
        budget.write_temp(&image_path, bytes.as_ref())?;

        for (existing_index, existing) in self.images.iter_mut().enumerate() {
            let Some(existing) = existing else { continue };
            existing.backing.replace_with_file(
                crate::archive::sequential_image_path(temp_dir.path(), existing_index),
                temp_dir.clone(),
            );
        }
        self.images[index] = Some(StoredSequentialImage {
            name,
            backing: {
                let backing = crate::archive::SequentialImageBacking::memory(bytes);
                backing.replace_with_file(image_path, temp_dir.clone());
                backing
            },
            layout,
        });
        self.retained_bytes = 0;
        self.temp_dir = Some(temp_dir);
        Ok(())
    }

    pub(super) fn progressive_image(
        &self,
        physical_index: usize,
        total_physical_images: usize,
    ) -> Result<crate::archive::ProgressiveArchiveImage, AppError> {
        let image = self
            .images
            .get(physical_index)
            .and_then(Option::as_ref)
            .ok_or_else(|| {
                AppError::Archive(format!(
                    "Sequential画像の保持結果が不足しています: physical index {physical_index}"
                ))
            })?;
        Ok(crate::archive::ProgressiveArchiveImage {
            physical_index,
            total_physical_images,
            backing: image.backing.clone(),
            layout: image.layout,
        })
    }

    pub(super) fn into_ordered_document(self, archive_path: &Path) -> Result<Document, AppError> {
        let images = self
            .images
            .into_iter()
            .enumerate()
            .map(|(physical_index, image)| {
                image.ok_or_else(|| {
                    AppError::Archive(format!(
                        "Sequential画像の保持結果が不足しています: physical index {physical_index}"
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(create_document_from_ordered_stored_images(
            archive_path,
            images,
            self.temp_dir,
        ))
    }

    pub(super) fn into_naturally_ordered_document(
        self,
        archive_path: &Path,
    ) -> Result<Document, AppError> {
        let mut images = self.images.into_iter().flatten().collect::<Vec<_>>();
        let cover_only = crate::archive::cover_only_paths(
            &images
                .iter()
                .map(|image| PathBuf::from(&image.name))
                .collect::<Vec<_>>(),
        );
        images.retain(|image| !cover_only.contains(Path::new(&image.name)));
        images.sort_by(|left, right| natord::compare(&left.name, &right.name));
        Ok(create_document_from_ordered_stored_images(
            archive_path,
            images,
            self.temp_dir,
        ))
    }
}

fn create_document_from_ordered_stored_images(
    archive_path: &Path,
    images: Vec<StoredSequentialImage>,
    temp_dir: Option<Arc<tempfile::TempDir>>,
) -> Document {
    let mut document = Document::new(archive_path.to_path_buf(), temp_dir);
    for StoredSequentialImage {
        name,
        backing,
        layout,
    } in images
    {
        document.add_archive_asset(backing.source(), layout, Vec::new(), PathBuf::from(name));
    }
    document
}

pub(super) fn archive_error(error: impl std::fmt::Display) -> AppError {
    AppError::Archive(error.to_string())
}

fn unsupported_archive_error() -> AppError {
    AppError::Archive("未対応のアーカイブ形式です".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::PagePart;
    use std::io::Cursor;
    use std::time::{Duration, Instant};

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            width,
            height,
            image::Rgb([32, 64, 128]),
        ))
        .write_to(&mut output, image::ImageFormat::Png)
        .unwrap();
        output.into_inner()
    }

    #[test]
    fn production_spill_root_uses_the_xdg_user_cache_directory() {
        assert_eq!(
            sequential_spill_root(),
            glib::user_cache_dir().join("agnam").join("tmp")
        );
    }

    #[test]
    fn sequential_images_keep_natural_order_memory_sources_and_wide_layout() {
        let cache = tempfile::tempdir().unwrap();
        let spill_root = cache.path().join("agnam/tmp");
        let mut storage = SequentialImageStorage::with_limit_in(0, usize::MAX, spill_root.clone());
        storage.push("10.png".into(), png(4, 8)).unwrap();
        storage.push("2.png".into(), png(8, 4)).unwrap();
        let document = storage
            .into_naturally_ordered_document(Path::new("pages.rar"))
            .unwrap();

        assert_eq!(document.assets.len(), 2);
        assert!(matches!(document.assets[0].source, ImageSource::Memory(_)));
        assert!(matches!(document.assets[1].source, ImageSource::Memory(_)));
        assert!(!spill_root.exists());
        assert_eq!(
            document
                .pages
                .iter()
                .map(|page| page.part)
                .collect::<Vec<_>>(),
            [PagePart::Right, PagePart::Left, PagePart::Whole]
        );
    }

    #[test]
    fn sequential_storage_spills_all_images_and_stays_file_backed() {
        let cache = tempfile::tempdir().unwrap();
        let spill_root = cache.path().join("agnam/tmp");
        let mut storage = SequentialImageStorage::with_limit_in(0, 4, spill_root.clone());
        storage.push("1.jpg".into(), vec![1, 2]).unwrap();
        let queued_notification = storage.progressive_image(0, 4).unwrap();
        storage.push("2.jpg".into(), vec![3, 4]).unwrap();
        assert!(storage.temp_dir.is_none());
        assert!(storage.images.iter().all(|image| matches!(
            image.as_ref().map(|image| image.backing.source()),
            Some(ImageSource::Memory(_))
        )));

        storage.push("3.jpg".into(), vec![5]).unwrap();
        let temp_path = storage.temp_dir.as_ref().unwrap().path().to_path_buf();
        assert!(temp_path.starts_with(&spill_root));
        assert!(matches!(
            queued_notification.backing.source(),
            ImageSource::File(path) if path == crate::archive::sequential_image_path(&temp_path, 0)
        ));
        assert!(storage.images.iter().all(|image| matches!(
            image.as_ref().map(|image| image.backing.source()),
            Some(ImageSource::File(path)) if path.starts_with(&temp_path) && path.exists()
        )));

        storage.push("4.jpg".into(), vec![6]).unwrap();
        assert!(matches!(
            storage.images[3].as_ref().map(|image| image.backing.source()),
            Some(ImageSource::File(path)) if path.exists()
        ));

        let document = storage
            .into_ordered_document(Path::new("pages.rar"))
            .unwrap();
        assert!(document.assets.iter().all(|asset| matches!(
            &asset.source,
            ImageSource::File(path) if path.exists()
        )));
        assert!(temp_path.exists());
        drop(queued_notification);
        drop(document);
        assert!(!temp_path.exists());
    }

    #[test]
    fn spill_threshold_is_independent_from_the_hard_temp_limit_and_failure_cleans_up() {
        let directory = tempfile::tempdir().unwrap();
        let spill_root = directory.path().join("spill-root");
        let mut storage = SequentialImageStorage::with_limit_in(0, 2, spill_root.clone());
        let mut budget = crate::archive::resource::ResourceBudget::new(
            crate::archive::resource::ResourceLimits {
                single_entry: 8,
                cumulative: 8,
                temp_writes: 3,
                temp_occupancy: 3,
                entries: 8,
                images: 8,
            },
        );
        storage
            .push_with_budget("a.png".into(), vec![1, 2, 3], &mut budget)
            .unwrap();
        assert!(storage.temp_dir.is_some());
        assert!(matches!(
            storage.push_with_budget("b.png".into(), vec![4], &mut budget),
            Err(AppError::ArchiveResourceLimit(
                crate::archive::ResourceLimitKind::TemporaryWrites
            ))
        ));
        drop(storage);
        assert_eq!(std::fs::read_dir(spill_root).unwrap().count(), 0);
    }

    #[test]
    fn failed_spill_does_not_publish_partial_backing() {
        let root = tempfile::tempdir().unwrap();
        let spill_root = root.path().join("spill");
        let mut storage = SequentialImageStorage::with_limit_in(3, 4, spill_root.clone());
        let mut budget = crate::archive::resource::ResourceBudget::new(
            crate::archive::resource::ResourceLimits {
                single_entry: 8,
                cumulative: 8,
                temp_writes: 2,
                temp_occupancy: 8,
                entries: 8,
                images: 8,
            },
        );
        storage
            .insert_with_budget(0, "a.jpg".into(), vec![1, 2], &mut budget)
            .unwrap();
        storage
            .insert_with_budget(1, "b.jpg".into(), vec![3, 4], &mut budget)
            .unwrap();
        let progressive = storage.progressive_image(0, 3).unwrap();
        assert!(matches!(
            storage.insert_with_budget(2, "c.jpg".into(), vec![5], &mut budget),
            Err(AppError::ArchiveResourceLimit(
                crate::archive::ResourceLimitKind::TemporaryWrites
            ))
        ));
        assert!(storage.temp_dir.is_none());
        assert_eq!(storage.retained_bytes, 4);
        assert!(matches!(
            progressive.backing.source(),
            ImageSource::Memory(_)
        ));
        assert!(
            storage
                .into_ordered_document(Path::new("pages.rar"))
                .is_err()
        );
        assert_eq!(std::fs::read_dir(spill_root).unwrap().count(), 0);
    }

    #[test]
    fn glib_bytes_and_backing_clone_share_the_owned_buffer() {
        let data = vec![7; 1024];
        let pointer = data.as_ptr();
        let bytes = glib::Bytes::from_owned(data);
        assert_eq!(bytes.as_ref().as_ptr(), pointer);
        let backing = crate::archive::SequentialImageBacking::memory(bytes);
        let ImageSource::Memory(clone) = backing.source() else {
            panic!("memory backing")
        };
        assert_eq!(clone.as_ref().as_ptr(), pointer);
    }

    #[test]
    fn spill_preserves_natural_order_layout_and_archive_identity() {
        let first = png(4, 8);
        let cache = tempfile::tempdir().unwrap();
        let mut storage =
            SequentialImageStorage::with_limit_in(0, first.len(), cache.path().join("agnam/tmp"));
        storage.push("10.png".into(), first).unwrap();
        storage.push("2.png".into(), png(8, 4)).unwrap();
        let document = storage
            .into_naturally_ordered_document(Path::new("pages.tar"))
            .unwrap();

        assert_eq!(document.assets[0].layout, ImageLayout::Spread);
        assert_eq!(document.assets[1].layout, ImageLayout::Single);
        assert_eq!(
            document
                .assets
                .iter()
                .map(|asset| asset.archive_identity.as_ref().unwrap().image.clone())
                .collect::<Vec<_>>(),
            [PathBuf::from("2.png"), PathBuf::from("10.png")]
        );
    }

    #[test]
    fn common_sequential_entry_dispatches_current_backends() {
        for path in [Path::new("missing.rar"), Path::new("missing.7z")] {
            let cancel_token = crate::archive::ProgressiveArchiveCancelToken::default();
            cancel_token.cancel();
            let outcome = load_sequential_document_with_progress(
                Default::default(),
                path,
                &cancel_token,
                &mut |_| panic!("cancelled load must not notify"),
            )
            .unwrap();

            assert!(matches!(
                outcome,
                crate::archive::ProgressiveArchiveLoadOutcome::Cancelled
            ));
        }
    }

    // Run with: cargo test archive::formats::tests::measure_sequential_spill -- --ignored --nocapture
    #[test]
    #[ignore]
    fn measure_sequential_spill() {
        fn median(mut times: Vec<Duration>) -> Duration {
            times.sort();
            times[times.len() / 2]
        }
        let root = tempfile::tempdir().unwrap();
        let threshold = 16 * 1024 * 1024;
        for (label, count, image_size) in [("few", 2, 8 * 1024 * 1024), ("many", 256, 64 * 1024)] {
            let mut before = Vec::new();
            let mut crossing = Vec::new();
            let mut after = Vec::new();
            let mut path_construction = Vec::new();
            let template = (0..count + 2)
                .map(|index| {
                    let mut state = index as u64 + 1;
                    let mut bytes = vec![0; image_size];
                    for byte in &mut bytes {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        *byte = state as u8;
                    }
                    bytes
                })
                .collect::<Vec<_>>();
            for _ in 0..5 {
                let mut storage = SequentialImageStorage::with_limit_in(
                    count + 2,
                    threshold,
                    root.path().join("spill"),
                );
                let mut budget = crate::archive::resource::ResourceBudget::default();
                let mut data = template.clone();
                let now = Instant::now();
                for index in 0..count {
                    storage
                        .insert_with_budget(
                            index,
                            format!("{index}.jpg"),
                            std::mem::take(&mut data[index]),
                            &mut budget,
                        )
                        .unwrap();
                }
                before.push(now.elapsed());
                assert_eq!(storage.retained_bytes, threshold);
                assert!(storage.temp_dir.is_none());
                let now = Instant::now();
                storage
                    .insert_with_budget(
                        count,
                        "cross.jpg".into(),
                        std::mem::take(&mut data[count]),
                        &mut budget,
                    )
                    .unwrap();
                crossing.push(now.elapsed());
                assert_eq!(storage.retained_bytes, 0);
                assert_eq!(
                    budget.temp_counters(),
                    (
                        (threshold + image_size) as u64,
                        (threshold + image_size) as u64
                    )
                );
                let now = Instant::now();
                storage
                    .insert_with_budget(
                        count + 1,
                        "after.jpg".into(),
                        std::mem::take(&mut data[count + 1]),
                        &mut budget,
                    )
                    .unwrap();
                after.push(now.elapsed());
                assert_eq!(
                    budget.temp_counters(),
                    (
                        (threshold + 2 * image_size) as u64,
                        (threshold + 2 * image_size) as u64
                    )
                );
                assert!(storage.images.iter().all(|image| matches!(
                    image.as_ref().map(|image| image.backing.source()),
                    Some(ImageSource::File(_))
                )));
                let now = Instant::now();
                for index in 0..count {
                    std::hint::black_box(crate::archive::sequential_image_path(root.path(), index));
                }
                path_construction.push(now.elapsed());
            }
            eprintln!(
                "spill fixture={label} images_before={count} image_bytes={image_size} threshold={threshold} before_total_us={} crossing_us={} after_us={} crossing_written={} retained_before={threshold} retained_after=0 path_construction_us={}",
                median(before).as_micros(),
                median(crossing).as_micros(),
                median(after).as_micros(),
                threshold + image_size,
                median(path_construction).as_micros()
            );
        }
    }
}
