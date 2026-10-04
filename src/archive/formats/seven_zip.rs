use super::{SequentialImageStorage, archive_error};
use crate::archive::safety::safe_archive_output_path;
use crate::archive::{
    ArchiveAccessStrategy, IMAGE_DIMENSION_PROBE_SIZE, ProgressiveArchiveCancelToken,
    ProgressiveArchiveImage, ProgressiveArchiveLoadOutcome, SequentialArchiveImage,
    first_image_entry_name, is_archive_ext, is_image_ext, is_macos_metadata,
    is_viewable_archive_entry, is_wide_from_bytes,
};
use crate::document::{Document, ImageLayout, ImageSource};
use crate::error::AppError;
use gtk::glib;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SevenZipEntryId {
    file_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedImage {
    entry_id: SevenZipEntryId,
    entry_name: String,
    physical_index: usize,
}

enum SevenZipEntryDecoder {
    Standard(sevenz_rust::SevenZReader<std::fs::File>),
    Progressive(ProgressiveSevenZipReader),
}

impl SevenZipEntryDecoder {
    fn open(archive_path: &Path, progressive: bool) -> Result<Self, sevenz_rust::Error> {
        if progressive {
            ProgressiveSevenZipReader::open(archive_path).map(Self::Progressive)
        } else {
            sevenz_rust::SevenZReader::open(archive_path, sevenz_rust::Password::empty())
                .map(Self::Standard)
        }
    }

    fn archive(&self) -> &sevenz_rust::Archive {
        match self {
            Self::Standard(reader) => reader.archive(),
            Self::Progressive(reader) => &reader.archive,
        }
    }

    fn for_each_entries<F>(
        &mut self,
        cancel_token: Option<&ProgressiveArchiveCancelToken>,
        mut each: F,
    ) -> Result<bool, sevenz_rust::Error>
    where
        F: FnMut(
            &sevenz_rust::SevenZArchiveEntry,
            &mut dyn std::io::Read,
        ) -> Result<bool, sevenz_rust::Error>,
    {
        match self {
            Self::Standard(reader) => {
                reader.for_each_entries(&mut each)?;
                Ok(true)
            }
            Self::Progressive(reader) => {
                let cancel_token = cancel_token.ok_or_else(|| {
                    sevenz_rust::Error::other("progressive decoder requires a cancel token")
                })?;
                reader.for_each_entries(cancel_token, &mut each)
            }
        }
    }
}

struct ProgressiveSevenZipReader {
    source: std::fs::File,
    archive: sevenz_rust::Archive,
}

impl ProgressiveSevenZipReader {
    fn open(archive_path: &Path) -> Result<Self, sevenz_rust::Error> {
        let (source, archive) = open_archive(archive_path)?;
        Ok(Self { source, archive })
    }

    fn for_each_entries<F>(
        &mut self,
        cancel_token: &ProgressiveArchiveCancelToken,
        each: &mut F,
    ) -> Result<bool, sevenz_rust::Error>
    where
        F: FnMut(
            &sevenz_rust::SevenZArchiveEntry,
            &mut dyn std::io::Read,
        ) -> Result<bool, sevenz_rust::Error>,
    {
        // sevenz-rust 0.6.1のSevenZReaderはBlockDecoderのboolを無視するため、
        // folder loopをここで所有し、falseをarchive全体の終了として伝播する。
        for folder_index in 0..self.archive.folders.len() {
            if cancel_token.is_cancelled() {
                return Ok(false);
            }
            let decoder =
                sevenz_rust::BlockDecoder::new(folder_index, &self.archive, &[], &mut self.source);
            if !decoder.for_each_entries(each)? {
                return Ok(false);
            }
        }

        for (file_index, entry) in self.archive.files.iter().enumerate() {
            if cancel_token.is_cancelled() {
                return Ok(false);
            }
            if self.archive.stream_map.file_folder_index[file_index].is_none() {
                let empty_reader: &mut dyn std::io::Read = &mut ([0_u8; 0].as_slice());
                if !each(entry, empty_reader)? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

fn open_archive(
    archive_path: &Path,
) -> Result<(std::fs::File, sevenz_rust::Archive), sevenz_rust::Error> {
    let mut source = std::fs::File::open(archive_path)
        .map_err(|error| sevenz_rust::Error::io_msg(error, archive_path.display().to_string()))?;
    let length = source.metadata().map_err(sevenz_rust::Error::io)?.len();
    let archive = sevenz_rust::Archive::read(&mut source, length, &[])?;
    Ok((source, archive))
}

fn access_strategy_from_archive(
    archive: &sevenz_rust::Archive,
) -> Result<ArchiveAccessStrategy, sevenz_rust::Error> {
    let mut stream_counts = vec![0_usize; archive.folders.len()];
    for (file_index, entry) in archive.files.iter().enumerate() {
        if !entry.has_stream() {
            continue;
        }
        let folder_index = archive
            .stream_map
            .file_folder_index
            .get(file_index)
            .copied()
            .flatten()
            .ok_or_else(|| {
                sevenz_rust::Error::other(format!(
                    "7z stream entry has no folder: file index {file_index}"
                ))
            })?;
        let count = stream_counts.get_mut(folder_index).ok_or_else(|| {
            sevenz_rust::Error::other(format!(
                "7z stream entry has invalid folder: file index {file_index}, folder index {folder_index}"
            ))
        })?;
        *count += 1;
    }

    Ok(if stream_counts.into_iter().any(|count| count > 1) {
        ArchiveAccessStrategy::Sequential
    } else {
        ArchiveAccessStrategy::RandomAccess
    })
}

pub(super) struct RandomAccessReader {
    source: std::fs::File,
    archive: sevenz_rust::Archive,
    #[cfg(test)]
    decoded_folder_indices: Vec<usize>,
}

impl RandomAccessReader {
    fn open_result(archive_path: &Path) -> Result<Self, AppError> {
        let (source, archive) = open_archive(archive_path).map_err(archive_error)?;
        if access_strategy_from_archive(&archive).map_err(archive_error)?
            != ArchiveAccessStrategy::RandomAccess
        {
            return Err(AppError::Archive(
                "solid 7z cannot be opened as Random Access".to_string(),
            ));
        }
        Ok(Self {
            source,
            archive,
            #[cfg(test)]
            decoded_folder_indices: Vec::new(),
        })
    }

    pub(super) fn open(archive_path: &Path) -> Option<Self> {
        Self::open_result(archive_path).ok()
    }

    pub(super) fn read_entry(
        &mut self,
        entry_index: usize,
        expected_name: &str,
    ) -> Result<Option<glib::Bytes>, AppError> {
        self.read_entry_bytes(
            entry_index,
            expected_name,
            None,
            &mut crate::archive::resource::ResourceBudget::default(),
        )
        .map(|bytes| Some(glib::Bytes::from_owned(bytes)))
    }

    fn read_entry_prefix(
        &mut self,
        entry_index: usize,
        expected_name: &str,
        limit: usize,
        budget: &mut crate::archive::resource::ResourceBudget,
    ) -> Result<Vec<u8>, AppError> {
        self.read_entry_bytes(entry_index, expected_name, Some(limit), budget)
    }

    fn read_entry_bytes(
        &mut self,
        entry_index: usize,
        expected_name: &str,
        limit: Option<usize>,
        budget: &mut crate::archive::resource::ResourceBudget,
    ) -> Result<Vec<u8>, AppError> {
        let entry = self.archive.files.get(entry_index).ok_or_else(|| {
            AppError::Archive(format!("7z entry index is out of range: {entry_index}"))
        })?;
        if entry.name() != expected_name {
            return Err(AppError::Archive(format!(
                "7zエントリ一覧と取得結果が一致しません: expected {expected_name:?}, got {:?}",
                entry.name()
            )));
        }
        if !entry.has_stream() {
            return Ok(Vec::new());
        }
        let folder_index = self
            .archive
            .stream_map
            .file_folder_index
            .get(entry_index)
            .copied()
            .flatten()
            .ok_or_else(|| {
                AppError::Archive(format!(
                    "7z stream entry has no folder: file index {entry_index}"
                ))
            })?;
        let target_entry = std::ptr::from_ref(entry);
        let mut bytes = None;
        let mut resource_error = None;
        #[cfg(test)]
        self.decoded_folder_indices.push(folder_index);
        let decoder =
            sevenz_rust::BlockDecoder::new(folder_index, &self.archive, &[], &mut self.source);
        let result = decoder.for_each_entries(&mut |decoded_entry, reader| {
            if std::ptr::from_ref(decoded_entry) != target_entry {
                if decoded_entry.has_stream() {
                    return Err(sevenz_rust::Error::other(format!(
                        "Random Access 7z folder contains an unexpected stream: {:?}",
                        decoded_entry.name()
                    )));
                }
                return Ok(true);
            }
            let data = if let Some(limit) = limit {
                if let Err(error) = budget.preflight(decoded_entry.size) {
                    resource_error = Some(error);
                    return Err(sevenz_rust::Error::other("archive resource limit"));
                }
                budget.read_all(&mut reader.take(limit as u64), Some(decoded_entry.size))
            } else {
                budget.read_all(reader, Some(decoded_entry.size))
            };
            let data = match data {
                Ok(data) => data,
                Err(error @ AppError::ArchiveResourceLimit(_)) => {
                    resource_error = Some(error);
                    return Err(sevenz_rust::Error::other("archive resource limit"));
                }
                Err(error) => return Err(sevenz_rust::Error::other(error.to_string())),
            };
            bytes = Some(data);
            Ok(false)
        });
        if let Some(error) = resource_error {
            return Err(error);
        }
        result.map_err(archive_error)?;
        bytes.ok_or_else(|| {
            AppError::Archive(format!(
                "7z entry was not found in its folder: file index {entry_index}"
            ))
        })
    }
}

#[cfg(test)]
fn plan_image_entries(
    entries: impl IntoIterator<Item = (usize, bool, String)>,
) -> Vec<PlannedImage> {
    plan_image_entries_with_options(entries, false)
}

fn plan_image_entries_with_options(
    entries: impl IntoIterator<Item = (usize, bool, String)>,
    include_cover_only: bool,
) -> Vec<PlannedImage> {
    let entries = entries.into_iter().collect::<Vec<_>>();
    let cover_only = crate::archive::cover_only_paths(
        &entries
            .iter()
            .filter(|(_, is_directory, _)| !*is_directory)
            .map(|(_, _, name)| std::path::PathBuf::from(name))
            .collect::<Vec<_>>(),
    );
    let mut images = entries
        .into_iter()
        .filter(|(_, is_directory, name)| {
            !*is_directory
                && !is_macos_metadata(name)
                && is_image_ext(Path::new(name))
                && (include_cover_only || !cover_only.contains(Path::new(name)))
        })
        .map(|(file_index, _, entry_name)| PlannedImage {
            entry_id: SevenZipEntryId { file_index },
            entry_name,
            physical_index: 0,
        })
        .collect::<Vec<_>>();

    let mut natural_order = (0..images.len()).collect::<Vec<_>>();
    natural_order.sort_by(|&left, &right| {
        natord::compare(&images[left].entry_name, &images[right].entry_name)
    });
    for (physical_index, archive_order_index) in natural_order.into_iter().enumerate() {
        images[archive_order_index].physical_index = physical_index;
    }
    images
}

fn image_plan(entries: &[sevenz_rust::SevenZArchiveEntry]) -> Vec<PlannedImage> {
    image_plan_with_options(entries, false)
}

fn image_plan_with_options(
    entries: &[sevenz_rust::SevenZArchiveEntry],
    include_cover_only: bool,
) -> Vec<PlannedImage> {
    plan_image_entries_with_options(
        entries.iter().enumerate().map(|(file_index, entry)| {
            (file_index, entry.is_directory(), entry.name().to_string())
        }),
        include_cover_only,
    )
}

pub(super) fn has_nested_archives(archive_path: &Path) -> bool {
    let Ok(archive) = sevenz_rust::SevenZReader::open(archive_path, sevenz_rust::Password::empty())
    else {
        return false;
    };
    archive.archive().files.iter().any(|entry| {
        !entry.is_directory()
            && !is_macos_metadata(entry.name())
            && is_archive_ext(Path::new(entry.name()))
    })
}

pub(super) fn entry_paths(
    archive_path: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<Vec<std::path::PathBuf>, AppError> {
    let (source, archive) = open_archive(archive_path).map_err(archive_error)?;
    drop(source);
    let mut paths = Vec::new();
    for (index, entry) in archive.files.iter().enumerate() {
        budget.entry(
            archive_path,
            index,
            !entry.is_directory() && is_image_ext(Path::new(entry.name())),
        )?;
        if !entry.is_directory() && !is_macos_metadata(entry.name()) {
            paths.push(std::path::PathBuf::from(entry.name()));
        }
    }
    Ok(paths)
}

pub(super) fn cover_entry_bytes(
    archive_path: &Path,
    entry_path: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<super::CoverEntryBytes, AppError> {
    if access_strategy(archive_path)? != ArchiveAccessStrategy::RandomAccess {
        return Ok(super::CoverEntryBytes::Unsupported);
    }
    let expected = entry_path.to_string_lossy();
    let mut reader = RandomAccessReader::open_result(archive_path)?;
    let index = reader
        .archive
        .files
        .iter()
        .position(|entry| !entry.is_directory() && entry.name() == expected);
    let Some(index) = index else {
        return Ok(super::CoverEntryBytes::Missing);
    };
    reader
        .read_entry_bytes(index, &expected, None, budget)
        .map(super::CoverEntryBytes::Bytes)
}

pub(super) fn automatic_cover_entry_bytes(
    archive_path: &Path,
    entry_path: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<super::CoverEntryBytes, AppError> {
    if access_strategy(archive_path)? == ArchiveAccessStrategy::RandomAccess {
        return cover_entry_bytes(archive_path, entry_path, budget);
    }
    read_entry_bytes_sequential(archive_path, &entry_path.to_string_lossy(), budget).map(|bytes| {
        match bytes {
            Some(bytes) => super::CoverEntryBytes::Bytes(bytes),
            None => super::CoverEntryBytes::Missing,
        }
    })
}

pub(super) fn has_viewable_content(archive_path: &Path) -> Result<bool, AppError> {
    let archive = sevenz_rust::SevenZReader::open(archive_path, sevenz_rust::Password::empty())
        .map_err(archive_error)?;
    Ok(archive
        .archive()
        .files
        .iter()
        .any(|entry| !entry.is_directory() && is_viewable_archive_entry(entry.name())))
}

pub(super) fn extract_to_dir(
    archive_path: &Path,
    destination: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<(), AppError> {
    let mut resource_error = None;
    let mut entry_index = 0;
    let result = sevenz_rust::decompress_file_with_extract_fn(
        archive_path,
        destination,
        |entry, reader, _default_output_path| {
            let index = entry_index;
            entry_index += 1;
            let output_path = safe_archive_output_path(destination, Path::new(entry.name()))
                .map_err(|error| sevenz_rust::Error::other(error.to_string()))?;
            if let Err(error) = budget.entry(
                archive_path,
                index,
                !entry.is_directory() && is_image_ext(Path::new(entry.name())),
            ) {
                resource_error = Some(error);
                return Err(sevenz_rust::Error::other("archive resource limit"));
            }

            if entry.is_directory() {
                std::fs::create_dir_all(&output_path).map_err(sevenz_rust::Error::io)?;
            } else {
                if let Some(parent) = output_path.parent() {
                    std::fs::create_dir_all(parent).map_err(sevenz_rust::Error::io)?;
                }
                if let Err(error) = budget.copy_to_path(reader, &output_path, Some(entry.size)) {
                    if matches!(error, AppError::ArchiveResourceLimit(_)) {
                        resource_error = Some(error);
                    }
                    return Err(sevenz_rust::Error::other("archive extraction failed"));
                }
            }
            Ok(true)
        },
    );
    if let Some(error) = resource_error {
        return Err(error);
    }
    result.map_err(archive_error)
}

pub(super) fn load_document(archive_path: &Path) -> Result<Document, AppError> {
    match access_strategy(archive_path)? {
        ArchiveAccessStrategy::RandomAccess => load_random_access_document(archive_path),
        ArchiveAccessStrategy::Sequential => match load_document_impl(archive_path, None, None)? {
            ProgressiveArchiveLoadOutcome::Complete(document) => Ok(document),
            ProgressiveArchiveLoadOutcome::Cancelled => unreachable!("non-progressive 7z load"),
        },
    }
}

pub(super) fn access_strategy(archive_path: &Path) -> Result<ArchiveAccessStrategy, AppError> {
    let (_, archive) = open_archive(archive_path).map_err(archive_error)?;
    access_strategy_from_archive(&archive).map_err(archive_error)
}

pub(super) const fn supports_sequential_progress(_archive_path: &Path) -> bool {
    true
}

fn load_random_access_document(archive_path: &Path) -> Result<Document, AppError> {
    let mut reader = RandomAccessReader::open_result(archive_path)?;
    let mut budget = crate::archive::resource::ResourceBudget::default();
    for (index, entry) in reader.archive.files.iter().enumerate() {
        budget.entry(
            archive_path,
            index,
            !entry.is_directory() && is_image_ext(Path::new(entry.name())),
        )?;
    }
    let mut plan = image_plan(&reader.archive.files);
    plan.sort_by_key(|image| image.physical_index);

    let mut document = Document::new(archive_path.to_path_buf(), None);
    for planned in plan {
        let prefix = reader.read_entry_prefix(
            planned.entry_id.file_index,
            &planned.entry_name,
            IMAGE_DIMENSION_PROBE_SIZE,
            &mut budget,
        )?;
        let layout = ImageLayout::from_is_wide(is_wide_from_bytes(&prefix));
        document.add_archive_asset(
            ImageSource::ArchiveEntry {
                archive_path: archive_path.to_path_buf(),
                entry_index: planned.entry_id.file_index,
                entry_name: planned.entry_name.clone(),
            },
            layout,
            Vec::new(),
            std::path::PathBuf::from(planned.entry_name),
        );
    }
    Ok(document)
}

pub(super) fn load_document_with_progress(
    archive_path: &Path,
    cancel_token: &ProgressiveArchiveCancelToken,
    on_image: &mut dyn FnMut(ProgressiveArchiveImage),
) -> Result<ProgressiveArchiveLoadOutcome<Document>, AppError> {
    load_document_impl(archive_path, Some(cancel_token), Some(on_image))
}

pub(super) fn stream_images(
    archive_path: &Path,
    cancel_token: &ProgressiveArchiveCancelToken,
    on_image: &mut dyn FnMut(SequentialArchiveImage),
) -> Result<ProgressiveArchiveLoadOutcome<()>, AppError> {
    if cancel_token.is_cancelled() {
        return Ok(ProgressiveArchiveLoadOutcome::Cancelled);
    }
    let archive = SevenZipEntryDecoder::open(archive_path, true).map_err(archive_error)?;
    let plan = image_plan_with_options(&archive.archive().files, true);
    let mut budget = crate::archive::resource::ResourceBudget::default();
    process_planned_images(
        archive_path,
        archive,
        &plan,
        Some(cancel_token),
        &mut budget,
        &mut |planned, total, data| {
            on_image(SequentialArchiveImage {
                natural_index: planned.physical_index,
                total_images: total,
                bytes: glib::Bytes::from_owned(data),
            });
            Ok(())
        },
    )
}

fn process_planned_images(
    archive_path: &Path,
    mut archive: SevenZipEntryDecoder,
    plan: &[PlannedImage],
    cancel_token: Option<&ProgressiveArchiveCancelToken>,
    budget: &mut crate::archive::resource::ResourceBudget,
    on_image: &mut dyn FnMut(&PlannedImage, usize, Vec<u8>) -> Result<(), AppError>,
) -> Result<ProgressiveArchiveLoadOutcome<()>, AppError> {
    let planned_by_id = plan
        .iter()
        .map(|image| (image.entry_id, image))
        .collect::<HashMap<_, _>>();
    // sevenz-rust 0.6.1のcallbackはこのfiles配列内のentryを参照するため、
    // addressからmetadata上のstableなfile indexへ戻して同名entryも区別する。
    let entry_ids_by_address = archive
        .archive()
        .files
        .iter()
        .enumerate()
        .map(|(file_index, entry)| (std::ptr::from_ref(entry), SevenZipEntryId { file_index }))
        .collect::<HashMap<_, _>>();
    let mut processed = std::collections::HashSet::new();
    let mut resource_error = None;
    for (index, entry) in archive.archive().files.iter().enumerate() {
        budget.entry(
            archive_path,
            index,
            !entry.is_directory() && is_image_ext(Path::new(entry.name())),
        )?;
    }

    let result = archive.for_each_entries(cancel_token, |entry, reader| {
        if cancel_token.is_some_and(ProgressiveArchiveCancelToken::is_cancelled) {
            return Ok(false);
        }
        let entry_id = entry_ids_by_address
            .get(&std::ptr::from_ref(entry))
            .copied()
            .ok_or_else(|| {
                sevenz_rust::Error::other(format!(
                    "7zエントリ一覧と展開結果が一致しません: {:?}",
                    entry.name()
                ))
            })?;

        if let Some(planned) = planned_by_id.get(&entry_id) {
            if planned.entry_name != entry.name() {
                return Err(sevenz_rust::Error::other(format!(
                    "7zエントリ一覧と展開結果が一致しません: expected {:?}, got {:?}",
                    planned.entry_name,
                    entry.name()
                )));
            }
            if !processed.insert(planned.physical_index) {
                return Err(sevenz_rust::Error::other(format!(
                    "7z画像エントリが重複して展開されました: file index {}",
                    entry_id.file_index
                )));
            }

            let data = match budget.read_all(reader, Some(entry.size)) {
                Ok(data) => data,
                Err(error @ AppError::ArchiveResourceLimit(_)) => {
                    resource_error = Some(error);
                    return Err(sevenz_rust::Error::other("archive resource limit"));
                }
                Err(error) => return Err(sevenz_rust::Error::other(error.to_string())),
            };
            if cancel_token.is_some_and(ProgressiveArchiveCancelToken::is_cancelled) {
                return Ok(false);
            }
            on_image(planned, plan.len(), data)
                .map_err(|error| sevenz_rust::Error::other(error.to_string()))?;
            if cancel_token.is_some_and(ProgressiveArchiveCancelToken::is_cancelled) {
                return Ok(false);
            }
        } else if entry.has_stream() {
            // Solid blockの後続entryをdecodeできるよう、対象外streamも最後まで消費する。
            if let Err(error) = budget.drain(reader, Some(entry.size)) {
                if matches!(error, AppError::ArchiveResourceLimit(_)) {
                    resource_error = Some(error);
                }
                return Err(sevenz_rust::Error::other("archive resource limit"));
            }
            if cancel_token.is_some_and(ProgressiveArchiveCancelToken::is_cancelled) {
                return Ok(false);
            }
        }
        Ok(true)
    });
    if cancel_token.is_some_and(ProgressiveArchiveCancelToken::is_cancelled) {
        return Ok(ProgressiveArchiveLoadOutcome::Cancelled);
    }
    if let Some(error) = resource_error {
        return Err(error);
    }
    let completed = result.map_err(archive_error)?;

    if !completed || cancel_token.is_some_and(ProgressiveArchiveCancelToken::is_cancelled) {
        Ok(ProgressiveArchiveLoadOutcome::Cancelled)
    } else if processed.len() != plan.len() {
        Err(AppError::Archive(format!(
            "7z画像の展開結果が不足しています: expected {}, got {}",
            plan.len(),
            processed.len()
        )))
    } else {
        Ok(ProgressiveArchiveLoadOutcome::Complete(()))
    }
}

fn load_document_impl(
    archive_path: &Path,
    cancel_token: Option<&ProgressiveArchiveCancelToken>,
    mut on_image: Option<&mut dyn FnMut(ProgressiveArchiveImage)>,
) -> Result<ProgressiveArchiveLoadOutcome<Document>, AppError> {
    if cancel_token.is_some_and(ProgressiveArchiveCancelToken::is_cancelled) {
        return Ok(ProgressiveArchiveLoadOutcome::Cancelled);
    }
    let archive =
        SevenZipEntryDecoder::open(archive_path, cancel_token.is_some()).map_err(archive_error)?;
    let plan = image_plan(&archive.archive().files);
    let mut images = SequentialImageStorage::with_capacity(plan.len());
    let mut storage_budget = crate::archive::resource::ResourceBudget::default();
    let mut budget = crate::archive::resource::ResourceBudget::default();
    let outcome = process_planned_images(
        archive_path,
        archive,
        &plan,
        cancel_token,
        &mut budget,
        &mut |planned, total, data| {
            images.insert_with_budget(
                planned.physical_index,
                planned.entry_name.clone(),
                data,
                &mut storage_budget,
            )?;
            let extracted = images.progressive_image(planned.physical_index, total)?;
            if let Some(on_image) = on_image.as_mut() {
                on_image(extracted);
            }
            Ok(())
        },
    )?;
    if matches!(outcome, ProgressiveArchiveLoadOutcome::Cancelled) {
        return Ok(ProgressiveArchiveLoadOutcome::Cancelled);
    }

    Ok(ProgressiveArchiveLoadOutcome::Complete(
        images.into_ordered_document(archive_path)?,
    ))
}

pub(super) fn first_image_bytes(
    archive_path: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<Option<Vec<u8>>, AppError> {
    if access_strategy(archive_path)? == ArchiveAccessStrategy::RandomAccess {
        let mut reader = RandomAccessReader::open_result(archive_path)?;
        let plan = image_plan(&reader.archive.files);
        let Some(first) = plan.iter().min_by_key(|image| image.physical_index) else {
            return Ok(None);
        };
        return reader
            .read_entry_bytes(first.entry_id.file_index, &first.entry_name, None, budget)
            .map(Some);
    }

    first_image_bytes_sequential(archive_path, budget)
}

fn first_image_bytes_sequential(
    archive_path: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<Option<Vec<u8>>, AppError> {
    let archive = sevenz_rust::SevenZReader::open(archive_path, sevenz_rust::Password::empty())
        .map_err(archive_error)?;
    let names = archive
        .archive()
        .files
        .iter()
        .filter(|entry| !entry.is_directory())
        .map(|entry| entry.name().to_string())
        .collect();
    let Some(first_name) = first_image_entry_name(names) else {
        return Ok(None);
    };

    read_entry_bytes_sequential(archive_path, &first_name, budget)
}

fn read_entry_bytes_sequential(
    archive_path: &Path,
    expected_name: &str,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<Option<Vec<u8>>, AppError> {
    let mut archive = sevenz_rust::SevenZReader::open(archive_path, sevenz_rust::Password::empty())
        .map_err(archive_error)?;
    let mut selected_bytes = None;
    let mut resource_error = None;
    let result = archive.for_each_entries(|entry, reader| {
        if selected_bytes.is_some() {
            return Ok(false);
        }
        if !entry.is_directory() && entry.name() == expected_name {
            match budget.read_all(reader, Some(entry.size)) {
                Ok(bytes) => selected_bytes = Some(bytes),
                Err(error @ AppError::ArchiveResourceLimit(_)) => {
                    resource_error = Some(error);
                    return Err(sevenz_rust::Error::other("archive resource limit"));
                }
                Err(error) => return Err(sevenz_rust::Error::other(error.to_string())),
            }
            return Ok(false);
        }
        if entry.has_stream() {
            // Solid 7z blocks require preceding streams to be consumed to
            // reach the selected entry, but their bytes are never retained.
            if let Err(error) = budget.drain(reader, Some(entry.size)) {
                if matches!(error, AppError::ArchiveResourceLimit(_)) {
                    resource_error = Some(error);
                }
                return Err(sevenz_rust::Error::other("archive resource limit"));
            }
        }
        Ok(true)
    });
    if let Some(error) = resource_error {
        return Err(error);
    }
    result.map_err(archive_error)?;
    Ok(selected_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{ImageLayout, ImageSource, PagePart};
    use std::io::Cursor;
    use std::path::PathBuf;

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

    fn archive_entry(name: &str) -> sevenz_rust::SevenZArchiveEntry {
        let mut entry = sevenz_rust::SevenZArchiveEntry::new();
        entry.name = name.to_string();
        entry.has_stream = true;
        entry
    }

    fn write_non_solid_archive(path: &Path, entries: Vec<(&str, Vec<u8>)>) {
        let mut writer = sevenz_rust::SevenZWriter::create(path).unwrap();
        for (name, data) in entries {
            writer
                .push_archive_entry(archive_entry(name), Some(Cursor::new(data)))
                .unwrap();
        }
        writer.finish().unwrap();
    }

    fn write_solid_archive(path: &Path, entries: Vec<(&str, Vec<u8>)>) {
        let metadata = entries
            .iter()
            .map(|(name, _)| archive_entry(name))
            .collect::<Vec<_>>();
        let readers = entries
            .into_iter()
            .map(|(_, data)| sevenz_rust::SourceReader::new(Cursor::new(data)))
            .collect::<Vec<_>>();
        let mut writer = sevenz_rust::SevenZWriter::create(path).unwrap();
        writer
            .push_archive_entries(metadata, sevenz_rust::SeqReader::new(readers))
            .unwrap();
        writer.finish().unwrap();
    }

    fn completed_document(outcome: ProgressiveArchiveLoadOutcome<Document>) -> Document {
        match outcome {
            ProgressiveArchiveLoadOutcome::Complete(document) => document,
            ProgressiveArchiveLoadOutcome::Cancelled => panic!("load should complete"),
        }
    }

    #[test]
    fn metadata_strategy_distinguishes_non_solid_and_solid_streams() {
        let directory = tempfile::tempdir().unwrap();
        let non_solid_path = directory.path().join("non-solid.7z");
        write_non_solid_archive(
            &non_solid_path,
            vec![
                ("notes.txt", b"notes".to_vec()),
                ("001.png", png(4, 8)),
                ("002.png", png(4, 8)),
            ],
        );
        assert_eq!(
            access_strategy(&non_solid_path).unwrap(),
            ArchiveAccessStrategy::RandomAccess
        );

        let solid_path = directory.path().join("solid.7z");
        write_solid_archive(
            &solid_path,
            vec![("001.png", png(4, 8)), ("002.png", png(4, 8))],
        );
        assert_eq!(
            access_strategy(&solid_path).unwrap(),
            ArchiveAccessStrategy::Sequential
        );

        let mixed_solid_path = directory.path().join("mixed-solid.7z");
        write_solid_archive(
            &mixed_solid_path,
            vec![("notes.txt", b"notes".to_vec()), ("001.png", png(4, 8))],
        );
        assert_eq!(
            access_strategy(&mixed_solid_path).unwrap(),
            ArchiveAccessStrategy::Sequential
        );
    }

    #[test]
    fn invalid_archive_does_not_receive_a_strategy() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("broken.7z");
        std::fs::write(&archive_path, b"not a 7z archive").unwrap();

        assert!(access_strategy(&archive_path).is_err());
    }

    #[test]
    fn image_plan_filters_entries_and_maps_archive_order_to_natural_positions() {
        let plan = plan_image_entries([
            (0, false, "001.jpg".into()),
            (1, false, "002.jpg".into()),
            (2, false, "004.jpg".into()),
            (3, true, "images".into()),
            (4, false, "__MACOSX/000.jpg".into()),
            (5, false, "._000.jpg".into()),
            (6, false, "memo.txt".into()),
            (7, false, "003.jpg".into()),
        ]);

        assert_eq!(
            plan.iter()
                .map(|image| (
                    image.entry_id.file_index,
                    image.entry_name.as_str(),
                    image.physical_index,
                ))
                .collect::<Vec<_>>(),
            [
                (0, "001.jpg", 0),
                (1, "002.jpg", 1),
                (2, "004.jpg", 3),
                (7, "003.jpg", 2),
            ]
        );
    }

    #[test]
    fn duplicate_names_keep_distinct_entry_ids_and_stable_natural_positions() {
        let plan = plan_image_entries([
            (2, false, "2.jpg".into()),
            (5, false, "1.jpg".into()),
            (8, false, "1.jpg".into()),
        ]);

        assert_eq!(plan[0].physical_index, 2);
        assert_eq!(plan[1].physical_index, 0);
        assert_eq!(plan[2].physical_index, 1);
        assert_ne!(plan[1].entry_id, plan[2].entry_id);
    }

    #[test]
    fn extracted_images_match_natural_document_order_layout_and_shared_bytes() {
        let plan = plan_image_entries([
            (0, false, "001.png".into()),
            (1, false, "002.png".into()),
            (2, false, "004.png".into()),
            (3, false, "005.png".into()),
            (4, false, "003.png".into()),
        ]);
        let dimensions = [(4, 8), (8, 4), (4, 8), (4, 8), (4, 8)];
        let mut images = SequentialImageStorage::with_capacity(plan.len());
        let mut notifications = Vec::new();

        for (planned, (width, height)) in plan.iter().zip(dimensions) {
            images
                .insert(
                    planned.physical_index,
                    planned.entry_name.clone(),
                    png(width, height),
                )
                .unwrap();
            notifications.push(
                images
                    .progressive_image(planned.physical_index, plan.len())
                    .unwrap(),
            );
        }

        assert_eq!(
            notifications
                .iter()
                .map(|image| image.physical_index)
                .collect::<Vec<_>>(),
            [0, 1, 3, 4, 2]
        );
        assert_eq!(notifications[1].layout, ImageLayout::Spread);
        assert!(
            notifications
                .iter()
                .all(|image| image.total_physical_images == plan.len())
        );

        let document = images.into_ordered_document(Path::new("pages.7z")).unwrap();
        assert_eq!(document.assets.len(), 5);
        assert_eq!(document.pages.len(), 6);
        assert_eq!(document.assets[1].layout, ImageLayout::Spread);
        assert_eq!(document.pages[1].part, PagePart::Right);
        assert_eq!(document.pages[2].part, PagePart::Left);

        for notification in &notifications {
            let ImageSource::Memory(document_bytes) =
                &document.assets[notification.physical_index].source
            else {
                panic!("7z assets should use shared memory bytes");
            };
            assert_eq!(
                match notification.backing.source() {
                    ImageSource::Memory(bytes) => bytes.as_ref().as_ptr(),
                    _ => panic!("notification should stay memory-backed"),
                },
                document_bytes.as_ref().as_ptr()
            );
            assert_eq!(
                notification.layout,
                document.assets[notification.physical_index].layout
            );
        }
    }

    #[test]
    fn non_solid_archive_builds_a_lazy_natural_document() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("pages.7z");
        write_non_solid_archive(
            &archive_path,
            vec![("002.png", png(8, 4)), ("001.png", png(4, 8))],
        );
        let document = load_document(&archive_path).unwrap();

        assert_eq!(document.assets.len(), 2);
        assert_eq!(document.pages.len(), 3);
        assert_eq!(document.assets[0].layout, ImageLayout::Single);
        assert_eq!(document.assets[1].layout, ImageLayout::Spread);
        assert_eq!(
            document
                .assets
                .iter()
                .map(|asset| match &asset.source {
                    ImageSource::ArchiveEntry {
                        entry_index,
                        entry_name,
                        ..
                    } => (*entry_index, entry_name.as_str()),
                    ImageSource::File(_) | ImageSource::Memory(_) => {
                        panic!("non-solid 7z images must stay lazy")
                    }
                })
                .collect::<Vec<_>>(),
            [(1, "001.png"), (0, "002.png")]
        );
    }

    #[test]
    fn non_solid_cover_listing_uses_metadata_and_individual_entry_reads() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("picker.7z");
        let selected = png(5, 9);
        write_non_solid_archive(
            &archive_path,
            vec![
                ("10.png", png(4, 8)),
                ("2.png", selected.clone()),
                ("memo.txt", b"memo".to_vec()),
            ],
        );

        assert_eq!(
            entry_paths(
                &archive_path,
                &mut crate::archive::resource::ResourceBudget::default()
            )
            .unwrap(),
            [
                PathBuf::from("10.png"),
                PathBuf::from("2.png"),
                PathBuf::from("memo.txt")
            ]
        );
        let super::super::CoverEntryBytes::Bytes(bytes) = cover_entry_bytes(
            &archive_path,
            Path::new("2.png"),
            &mut crate::archive::resource::ResourceBudget::default(),
        )
        .unwrap() else {
            panic!("non-solid entry should be independently readable");
        };
        assert_eq!(bytes, selected);
    }

    #[test]
    fn random_access_decodes_only_the_selected_late_folder() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("late-entry.7z");
        let last_bytes = png(6, 9);
        write_non_solid_archive(
            &archive_path,
            vec![
                ("001.png", png(4, 8)),
                ("002.png", png(5, 8)),
                ("003.png", last_bytes.clone()),
            ],
        );
        let mut reader = RandomAccessReader::open_result(&archive_path).unwrap();
        let selected_folder = reader.archive.stream_map.file_folder_index[2].unwrap();
        assert!(selected_folder > 0);

        let bytes = reader.read_entry(2, "003.png").unwrap().unwrap();

        assert_eq!(bytes.as_ref(), last_bytes);
        assert_eq!(reader.decoded_folder_indices, [selected_folder]);
    }

    #[test]
    fn duplicate_names_are_loaded_by_stable_entry_index() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("duplicates.7z");
        let first_bytes = png(4, 8);
        let second_bytes = png(8, 4);
        write_non_solid_archive(
            &archive_path,
            vec![
                ("same.png", first_bytes.clone()),
                ("same.png", second_bytes.clone()),
            ],
        );
        let mut reader = RandomAccessReader::open_result(&archive_path).unwrap();

        assert_eq!(
            reader.read_entry(0, "same.png").unwrap().unwrap().as_ref(),
            first_bytes
        );
        assert_eq!(
            reader.read_entry(1, "same.png").unwrap().unwrap().as_ref(),
            second_bytes
        );

        let document = load_document(&archive_path).unwrap();
        assert_eq!(document.assets.len(), 2);
        assert_eq!(document.assets[0].layout, ImageLayout::Single);
        assert_eq!(document.assets[1].layout, ImageLayout::Spread);
        assert!(matches!(
            document.assets[0].source,
            ImageSource::ArchiveEntry { entry_index: 0, .. }
        ));
        assert!(matches!(
            document.assets[1].source,
            ImageSource::ArchiveEntry { entry_index: 1, .. }
        ));
    }

    #[test]
    fn non_solid_cover_uses_the_naturally_first_entry_by_index() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("cover.7z");
        write_non_solid_archive(
            &archive_path,
            vec![
                ("notes.txt", vec![b'x'; 32 * 1024]),
                ("10.png", png(10, 20)),
                ("1.png", png(1, 2)),
                ("2.png", png(2, 4)),
            ],
        );

        let bytes = first_image_bytes(
            &archive_path,
            &mut crate::archive::resource::ResourceBudget::default(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(image::load_from_memory(&bytes).unwrap().width(), 1);
    }

    #[test]
    fn solid_cover_keeps_the_sequential_prefix_consumption() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("solid-cover.7z");
        write_solid_archive(
            &archive_path,
            vec![
                ("notes.txt", vec![b'x'; 32 * 1024]),
                ("10.png", png(10, 20)),
                ("1.png", png(1, 2)),
            ],
        );

        let bytes = first_image_bytes(
            &archive_path,
            &mut crate::archive::resource::ResourceBudget::default(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(image::load_from_memory(&bytes).unwrap().width(), 1);
    }

    #[test]
    fn automatic_cover_reads_only_the_selected_entry_for_both_access_strategies() {
        let directory = tempfile::tempdir().unwrap();
        let cover = png(9, 12);
        for (name, solid) in [("non-solid-cover.7z", false), ("solid-cover.7z", true)] {
            let archive_path = directory.path().join(name);
            let entries = vec![
                ("book/001.png", png(1, 2)),
                ("book/Cover.PNG", cover.clone()),
                ("book/002.png", png(2, 4)),
            ];
            if solid {
                write_solid_archive(&archive_path, entries);
            } else {
                write_non_solid_archive(&archive_path, entries);
            }

            assert_eq!(
                super::super::automatic_cover_hint_bytes(
                    &archive_path,
                    &mut crate::archive::resource::ResourceBudget::default()
                )
                .unwrap()
                .unwrap(),
                cover
            );
        }
    }

    #[test]
    fn solid_archive_consumes_non_image_stream_before_later_images() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("solid.cb7");
        write_solid_archive(
            &archive_path,
            vec![
                ("notes.txt", vec![b'x'; 16 * 1024]),
                ("002.png", png(8, 4)),
                ("001.png", png(4, 8)),
            ],
        );
        let mut notifications = Vec::new();
        let cancel_token = ProgressiveArchiveCancelToken::default();

        let document = completed_document(
            load_document_with_progress(&archive_path, &cancel_token, &mut |image| {
                notifications.push(image)
            })
            .unwrap(),
        );

        assert_eq!(
            notifications
                .iter()
                .map(|image| image.physical_index)
                .collect::<Vec<_>>(),
            [1, 0]
        );
        assert!(
            notifications
                .iter()
                .all(|image| image.total_physical_images == 2)
        );
        assert_eq!(document.assets.len(), 2);
        assert_eq!(document.assets[0].layout, ImageLayout::Single);
        assert_eq!(document.assets[1].layout, ImageLayout::Spread);
    }

    #[test]
    fn solid_non_image_drain_counts_toward_the_operation_budget() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("drain-limit.7z");
        write_solid_archive(
            &archive_path,
            vec![("notes.txt", vec![b'x'; 64]), ("001.png", png(4, 8))],
        );
        let archive = SevenZipEntryDecoder::open(&archive_path, true).unwrap();
        let plan = image_plan(&archive.archive().files);
        let mut budget = crate::archive::resource::ResourceBudget::new(
            crate::archive::resource::ResourceLimits {
                single_entry: 128,
                cumulative: 32,
                temp_writes: 128,
                temp_occupancy: 128,
                entries: 4,
                images: 4,
            },
        );
        let cancel_token = ProgressiveArchiveCancelToken::default();
        let error = process_planned_images(
            &archive_path,
            archive,
            &plan,
            Some(&cancel_token),
            &mut budget,
            &mut |_, _, _| Ok(()),
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                AppError::ArchiveResourceLimit(crate::archive::ResourceLimitKind::CumulativeBytes)
            ),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn contents_stream_maps_physical_order_to_natural_positions_and_includes_cover() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("contents-solid.7z");
        write_solid_archive(
            &archive_path,
            vec![
                ("11.png", png(11, 20)),
                ("cover.png", png(1, 20)),
                ("25.png", png(25, 30)),
                ("3.png", png(3, 20)),
            ],
        );
        let cancel_token = ProgressiveArchiveCancelToken::default();
        let mut notifications = Vec::new();

        let outcome = stream_images(&archive_path, &cancel_token, &mut |image| {
            let width = image::load_from_memory(&image.bytes).unwrap().width();
            notifications.push((image.natural_index, image.total_images, width));
        })
        .unwrap();

        assert!(matches!(
            outcome,
            ProgressiveArchiveLoadOutcome::Complete(())
        ));
        assert_eq!(
            notifications,
            [(1, 4, 11), (3, 4, 1), (2, 4, 25), (0, 4, 3)]
        );

        let document = completed_document(
            load_document_with_progress(&archive_path, &cancel_token, &mut |_| {}).unwrap(),
        );
        assert_eq!(document.assets.len(), 3);
    }

    #[test]
    fn cancelling_contents_stream_stops_without_document_completion() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("cancel-contents-solid.7z");
        write_solid_archive(
            &archive_path,
            vec![
                ("001.png", png(4, 8)),
                ("002.png", png(4, 8)),
                ("003.png", png(4, 8)),
            ],
        );
        let cancel_token = ProgressiveArchiveCancelToken::default();
        let mut notifications = 0;

        let outcome = stream_images(&archive_path, &cancel_token, &mut |_| {
            notifications += 1;
            cancel_token.cancel();
        })
        .unwrap();

        assert!(matches!(outcome, ProgressiveArchiveLoadOutcome::Cancelled));
        assert_eq!(notifications, 1);
    }

    #[test]
    fn progressive_driver_propagates_callback_stop_before_later_folders() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("cancel.7z");
        write_non_solid_archive(
            &archive_path,
            vec![
                ("001.png", png(4, 8)),
                ("002.png", png(4, 8)),
                ("003.png", png(4, 8)),
            ],
        );
        let mut reader = ProgressiveSevenZipReader::open(&archive_path).unwrap();
        assert!(reader.archive.folders.len() > 1);
        let driver_cancel_token = ProgressiveArchiveCancelToken::default();
        let mut visited_entries = 0;
        let completed = reader
            .for_each_entries(&driver_cancel_token, &mut |_entry, stream| {
                std::io::copy(stream, &mut std::io::sink()).unwrap();
                visited_entries += 1;
                Ok(false)
            })
            .unwrap();
        assert!(!completed);
        assert_eq!(visited_entries, 1);
    }

    #[test]
    fn cancelled_solid_load_stops_after_the_completed_image() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("cancel-solid.7z");
        write_solid_archive(
            &archive_path,
            vec![
                ("001.png", png(4, 8)),
                ("002.png", png(4, 8)),
                ("003.png", png(4, 8)),
            ],
        );

        let cancel_token = ProgressiveArchiveCancelToken::default();
        let mut notifications = Vec::new();

        let outcome = load_document_with_progress(&archive_path, &cancel_token, &mut |image| {
            notifications.push(image);
            cancel_token.cancel();
        })
        .unwrap();

        assert!(matches!(outcome, ProgressiveArchiveLoadOutcome::Cancelled));
        assert_eq!(notifications.len(), 1);
    }
}
