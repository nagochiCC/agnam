use super::{SequentialImageStorage, archive_error};
use crate::archive::safety::safe_archive_output_path;
use crate::archive::{
    ProgressiveArchiveCancelToken, ProgressiveArchiveImage, ProgressiveArchiveLoadOutcome,
    SequentialArchiveImage, is_archive_ext, is_image_ext, is_macos_metadata,
    is_viewable_archive_entry,
};
use crate::document::Document;
use crate::error::AppError;
use gtk::glib;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct RarEntryId {
    archive_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedImage {
    entry_id: RarEntryId,
    entry_name: String,
    physical_index: usize,
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
            .filter(|(_, is_file, _)| *is_file)
            .map(|(_, _, name)| std::path::PathBuf::from(name))
            .collect::<Vec<_>>(),
    );
    let mut images = entries
        .into_iter()
        .filter(|(_, is_file, name)| {
            *is_file
                && !is_macos_metadata(name)
                && is_image_ext(Path::new(name))
                && (include_cover_only || !cover_only.contains(Path::new(name)))
        })
        .map(|(archive_index, _, entry_name)| PlannedImage {
            entry_id: RarEntryId { archive_index },
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

fn image_plan(
    archive_path: &Path,
    cancel_token: Option<&ProgressiveArchiveCancelToken>,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<ProgressiveArchiveLoadOutcome<Vec<PlannedImage>>, AppError> {
    image_plan_with_options(archive_path, cancel_token, false, budget)
}

fn image_plan_with_options(
    archive_path: &Path,
    cancel_token: Option<&ProgressiveArchiveCancelToken>,
    include_cover_only: bool,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<ProgressiveArchiveLoadOutcome<Vec<PlannedImage>>, AppError> {
    let archive = unrar::Archive::new(archive_path)
        .open_for_listing()
        .map_err(archive_error)?;
    let mut entries = Vec::new();
    for (archive_index, entry) in archive.enumerate() {
        if cancel_token.is_some_and(ProgressiveArchiveCancelToken::is_cancelled) {
            return Ok(ProgressiveArchiveLoadOutcome::Cancelled);
        }
        let entry = entry.map_err(archive_error)?;
        static_count_entry(
            archive_path,
            archive_index,
            entry.is_file(),
            &entry.filename,
            budget,
        )?;
        entries.push((
            archive_index,
            entry.is_file(),
            entry.filename.to_string_lossy().into_owned(),
        ));
    }
    Ok(ProgressiveArchiveLoadOutcome::Complete(
        plan_image_entries_with_options(entries, include_cover_only),
    ))
}

fn static_count_entry(
    archive_path: &Path,
    archive_index: usize,
    file: bool,
    name: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<(), AppError> {
    budget.entry(archive_path, archive_index, file && is_image_ext(name))
}

fn check_rar_size_before_read(
    size: u64,
    budget: &crate::archive::resource::ResourceBudget,
) -> Result<(), AppError> {
    budget.preflight(size)
}

fn first_planned_image(plan: &[PlannedImage]) -> Option<&PlannedImage> {
    plan.iter().min_by_key(|image| image.physical_index)
}

fn planned_image_for_processed_entry<'a>(
    planned_by_id: &HashMap<RarEntryId, &'a PlannedImage>,
    entry_id: RarEntryId,
    entry_name: &str,
) -> Result<&'a PlannedImage, AppError> {
    let planned = planned_by_id.get(&entry_id).copied().ok_or_else(|| {
        AppError::Archive(format!(
            "RARエントリ一覧と展開結果が一致しません: archive index {}, name {entry_name:?}",
            entry_id.archive_index
        ))
    })?;
    if planned.entry_name != entry_name {
        return Err(AppError::Archive(format!(
            "RARエントリ一覧と展開結果が一致しません: archive index {}, expected {:?}, got {:?}",
            entry_id.archive_index, planned.entry_name, entry_name
        )));
    }
    Ok(planned)
}

fn is_nested_archive_entry(is_file: bool, filename: &str) -> bool {
    is_file && !is_macos_metadata(filename) && is_archive_ext(Path::new(filename))
}

pub(super) fn has_nested_archives(archive_path: &Path) -> bool {
    let archive = match unrar::Archive::new(archive_path).open_for_listing() {
        Ok(archive) => archive,
        Err(_) => return false,
    };

    for entry in archive {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => break,
        };
        if is_nested_archive_entry(entry.is_file(), &entry.filename.to_string_lossy()) {
            return true;
        }
    }
    false
}

pub(super) fn has_viewable_content(archive_path: &Path) -> Result<bool, AppError> {
    let archive = unrar::Archive::new(archive_path)
        .open_for_listing()
        .map_err(archive_error)?;

    for entry in archive {
        let entry = entry.map_err(archive_error)?;
        if entry.is_file() && is_viewable_archive_entry(&entry.filename.to_string_lossy()) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn entry_paths(
    archive_path: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<Vec<std::path::PathBuf>, AppError> {
    let archive = unrar::Archive::new(archive_path)
        .open_for_listing()
        .map_err(archive_error)?;
    let mut paths = Vec::new();
    for (archive_index, entry) in archive.enumerate() {
        let entry = entry.map_err(archive_error)?;
        let name = entry.filename.to_string_lossy();
        budget.entry(
            archive_path,
            archive_index,
            entry.is_file() && is_image_ext(Path::new(&*name)),
        )?;
        if entry.is_file() && !is_macos_metadata(&name) {
            paths.push(entry.filename);
        }
    }
    Ok(paths)
}

pub(super) fn automatic_cover_entry_bytes(
    archive_path: &Path,
    entry_path: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<super::CoverEntryBytes, AppError> {
    let expected_name = entry_path.to_string_lossy().into_owned();
    let archive = unrar::Archive::new(archive_path)
        .open_for_listing()
        .map_err(archive_error)?;
    let mut target_index = None;
    for (archive_index, entry) in archive.enumerate() {
        let entry = entry.map_err(archive_error)?;
        if entry.is_file() && entry.filename.to_string_lossy() == expected_name {
            target_index = Some(archive_index);
            break;
        }
    }
    let Some(target_index) = target_index else {
        return Ok(super::CoverEntryBytes::Missing);
    };
    read_entry_bytes(archive_path, target_index, &expected_name, budget)
        .map(super::CoverEntryBytes::Bytes)
}

pub(super) fn extract_to_dir(
    archive_path: &Path,
    destination: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<(), AppError> {
    let mut archive = unrar::Archive::new(archive_path)
        .open_for_processing()
        .map_err(archive_error)?;
    let mut archive_index = 0usize;

    loop {
        archive = match archive.read_header() {
            Ok(Some(header)) => {
                let current_index = archive_index;
                archive_index = archive_index
                    .checked_add(1)
                    .ok_or(crate::archive::ResourceLimitKind::EntryCount)?;
                let filename = header.entry().filename.to_string_lossy().into_owned();
                budget.entry(
                    archive_path,
                    current_index,
                    header.entry().is_file() && is_image_ext(Path::new(&filename)),
                )?;
                if is_macos_metadata(&filename) {
                    header.skip().map_err(archive_error)?
                } else {
                    let output_path = safe_archive_output_path(destination, Path::new(&filename))?;
                    if !header.entry().is_file() {
                        header.skip().map_err(archive_error)?
                    } else {
                        check_rar_size_before_read(header.entry().unpacked_size, budget)?;
                        if let Some(parent) = output_path.parent() {
                            std::fs::create_dir_all(parent)?;
                        }
                        match header.read() {
                            Ok((data, next)) => {
                                budget.account_bytes(data.len() as u64)?;
                                budget.write_temp(&output_path, &data)?;
                                next
                            }
                            Err(error) => {
                                return Err(AppError::Archive(format!(
                                    "RARファイル '{filename}' の展開失敗: {error}"
                                )));
                            }
                        }
                    }
                }
            }
            Ok(None) => break,
            Err(error) => return Err(AppError::Archive(error.to_string())),
        };
    }
    Ok(())
}

pub(super) fn load_document(archive_path: &Path) -> Result<Document, AppError> {
    match load_document_impl(archive_path, None, None)? {
        ProgressiveArchiveLoadOutcome::Complete(document) => Ok(document),
        ProgressiveArchiveLoadOutcome::Cancelled => unreachable!("non-progressive RAR load"),
    }
}

pub(super) const fn access_strategy(_archive_path: &Path) -> crate::archive::ArchiveAccessStrategy {
    crate::archive::ArchiveAccessStrategy::Sequential
}

pub(super) const fn supports_sequential_progress(_archive_path: &Path) -> bool {
    true
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
    let mut budget = crate::archive::resource::ResourceBudget::default();
    let plan = match image_plan_with_options(archive_path, Some(cancel_token), true, &mut budget)? {
        ProgressiveArchiveLoadOutcome::Complete(plan) => plan,
        ProgressiveArchiveLoadOutcome::Cancelled => {
            return Ok(ProgressiveArchiveLoadOutcome::Cancelled);
        }
    };
    process_planned_images(
        archive_path,
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
    plan: &[PlannedImage],
    cancel_token: Option<&ProgressiveArchiveCancelToken>,
    budget: &mut crate::archive::resource::ResourceBudget,
    on_image: &mut dyn FnMut(&PlannedImage, usize, Vec<u8>) -> Result<(), AppError>,
) -> Result<ProgressiveArchiveLoadOutcome<()>, AppError> {
    let planned_by_id = plan
        .iter()
        .map(|image| (image.entry_id, image))
        .collect::<HashMap<_, _>>();
    let mut archive = unrar::Archive::new(archive_path)
        .open_for_processing()
        .map_err(archive_error)?;
    let mut archive_index = 0;

    loop {
        if cancel_token.is_some_and(ProgressiveArchiveCancelToken::is_cancelled) {
            return Ok(ProgressiveArchiveLoadOutcome::Cancelled);
        }
        archive = match archive.read_header() {
            Ok(Some(header)) => {
                let filename = header.entry().filename.to_string_lossy().into_owned();
                let entry_id = RarEntryId { archive_index };
                if planned_by_id.contains_key(&entry_id) {
                    check_rar_size_before_read(header.entry().unpacked_size, budget)?;
                    match header.read() {
                        Ok((data, next)) => {
                            if cancel_token.is_some_and(ProgressiveArchiveCancelToken::is_cancelled)
                            {
                                return Ok(ProgressiveArchiveLoadOutcome::Cancelled);
                            }
                            budget.account_bytes(data.len() as u64)?;
                            if cancel_token.is_some_and(ProgressiveArchiveCancelToken::is_cancelled)
                            {
                                return Ok(ProgressiveArchiveLoadOutcome::Cancelled);
                            }
                            let planned = planned_image_for_processed_entry(
                                &planned_by_id,
                                entry_id,
                                &filename,
                            )?;
                            on_image(planned, plan.len(), data)?;
                            if cancel_token.is_some_and(ProgressiveArchiveCancelToken::is_cancelled)
                            {
                                return Ok(ProgressiveArchiveLoadOutcome::Cancelled);
                            }
                            next
                        }
                        Err(error) => {
                            if cancel_token.is_some_and(ProgressiveArchiveCancelToken::is_cancelled)
                            {
                                return Ok(ProgressiveArchiveLoadOutcome::Cancelled);
                            }
                            eprintln!("[Agnam] メモリ展開に失敗しました ({filename:?}): {error}");
                            return Err(AppError::Archive(format!(
                                "ファイル '{filename}' のメモリ展開失敗: {error}"
                            )));
                        }
                    }
                } else {
                    header.skip().map_err(archive_error)?
                }
            }
            Ok(None) => break,
            Err(error) => return Err(AppError::Archive(error.to_string())),
        };
        archive_index += 1;
    }
    Ok(ProgressiveArchiveLoadOutcome::Complete(()))
}

fn load_document_impl(
    archive_path: &Path,
    cancel_token: Option<&ProgressiveArchiveCancelToken>,
    mut on_image: Option<&mut dyn FnMut(ProgressiveArchiveImage)>,
) -> Result<ProgressiveArchiveLoadOutcome<Document>, AppError> {
    if cancel_token.is_some_and(ProgressiveArchiveCancelToken::is_cancelled) {
        return Ok(ProgressiveArchiveLoadOutcome::Cancelled);
    }
    let mut budget = crate::archive::resource::ResourceBudget::default();
    let plan = match image_plan(archive_path, cancel_token, &mut budget)? {
        ProgressiveArchiveLoadOutcome::Complete(plan) => plan,
        ProgressiveArchiveLoadOutcome::Cancelled => {
            return Ok(ProgressiveArchiveLoadOutcome::Cancelled);
        }
    };
    let mut images = SequentialImageStorage::with_capacity(plan.len());
    let mut storage_budget = crate::archive::resource::ResourceBudget::default();
    let outcome = process_planned_images(
        archive_path,
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
    let mut listing_budget = crate::archive::resource::ResourceBudget::default();
    let plan = match image_plan(archive_path, None, &mut listing_budget)? {
        ProgressiveArchiveLoadOutcome::Complete(plan) => plan,
        ProgressiveArchiveLoadOutcome::Cancelled => unreachable!("cover load has no cancel token"),
    };
    let Some(first) = first_planned_image(&plan) else {
        return Ok(None);
    };
    read_entry_bytes(
        archive_path,
        first.entry_id.archive_index,
        &first.entry_name,
        budget,
    )
    .map(Some)
}

fn read_entry_bytes(
    archive_path: &Path,
    target_index: usize,
    expected_name: &str,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<Vec<u8>, AppError> {
    let mut archive = unrar::Archive::new(archive_path)
        .open_for_processing()
        .map_err(archive_error)?;
    let mut archive_index = 0;
    loop {
        archive = match archive.read_header().map_err(archive_error)? {
            Some(header) => {
                let filename = header.entry().filename.to_string_lossy().into_owned();
                if archive_index == target_index {
                    if !header.entry().is_file() || filename != expected_name {
                        return Err(AppError::Archive(format!(
                            "RAR表紙エントリ一覧と取得結果が一致しません: archive index {archive_index}, expected {expected_name:?}, got {filename:?}"
                        )));
                    }
                    check_rar_size_before_read(header.entry().unpacked_size, budget)?;
                    let (bytes, _) = header.read().map_err(archive_error)?;
                    budget.account_bytes(bytes.len() as u64)?;
                    return Ok(bytes);
                }
                header.skip().map_err(archive_error)?
            }
            None => break,
        };
        archive_index += 1;
    }
    Err(AppError::Archive(format!(
        "RAR表紙entryが見つかりません: archive index {target_index}, name {expected_name:?}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{ImageLayout, ImageSource, PagePart};
    use std::io::Cursor;

    #[test]
    fn rar_size_preflight_and_actual_size_have_distinct_checks() {
        let limits = crate::archive::resource::ResourceLimits {
            single_entry: 4,
            cumulative: 8,
            temp_writes: 8,
            temp_occupancy: 8,
            entries: 2,
            images: 1,
        };
        let mut budget = crate::archive::resource::ResourceBudget::new(limits);
        assert!(matches!(
            check_rar_size_before_read(5, &budget),
            Err(AppError::ArchiveResourceLimit(
                crate::archive::ResourceLimitKind::SingleEntryBytes
            ))
        ));
        assert!(matches!(
            budget.account_bytes(5),
            Err(AppError::ArchiveResourceLimit(
                crate::archive::ResourceLimitKind::SingleEntryBytes
            ))
        ));
        static_count_entry(
            Path::new("rar"),
            0,
            true,
            Path::new("cover.png"),
            &mut budget,
        )
        .unwrap();
        static_count_entry(
            Path::new("rar"),
            0,
            true,
            Path::new("cover.png"),
            &mut budget,
        )
        .unwrap();
        assert!(matches!(
            static_count_entry(
                Path::new("rar"),
                1,
                true,
                Path::new("page.jpg"),
                &mut budget
            ),
            Err(AppError::ArchiveResourceLimit(
                crate::archive::ResourceLimitKind::ImageEntryCount
            ))
        ));
        let mut entry_budget = crate::archive::resource::ResourceBudget::new(
            crate::archive::resource::ResourceLimits {
                entries: 1,
                images: 2,
                ..limits
            },
        );
        static_count_entry(
            Path::new("rar"),
            0,
            false,
            Path::new("notes.txt"),
            &mut entry_budget,
        )
        .unwrap();
        assert!(matches!(
            static_count_entry(
                Path::new("rar"),
                1,
                false,
                Path::new("notes.txt"),
                &mut entry_budget
            ),
            Err(AppError::ArchiveResourceLimit(
                crate::archive::ResourceLimitKind::EntryCount
            ))
        ));
    }

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

    // Small RAR5 fixtures generated once for backend tests. Keeping them
    // embedded avoids requiring a RAR writer or external command in CI.
    const RAR_WITH_COVER: &[u8] = &[
        82, 97, 114, 33, 26, 7, 1, 0, 243, 225, 130, 235, 11, 1, 5, 7, 0, 6, 1, 1, 128, 128, 128,
        0, 65, 40, 154, 117, 36, 2, 3, 11, 133, 0, 4, 133, 0, 164, 131, 2, 63, 188, 38, 164, 128,
        0, 1, 6, 49, 48, 46, 112, 110, 103, 10, 3, 19, 230, 166, 138, 106, 81, 110, 41, 26, 97,
        114, 99, 104, 10, 9, 32, 190, 18, 35, 2, 3, 11, 153, 0, 4, 153, 0, 164, 131, 2, 12, 203, 7,
        227, 128, 0, 1, 5, 50, 46, 112, 110, 103, 10, 3, 19, 230, 166, 138, 106, 142, 1, 56, 26,
        92, 83, 123, 80, 82, 69, 84, 84, 89, 95, 78, 65, 77, 69, 125, 32, 92, 114, 32, 40, 92, 108,
        41, 10, 10, 2, 103, 38, 120, 39, 2, 3, 11, 254, 0, 4, 191, 1, 164, 131, 2, 219, 130, 63,
        160, 128, 3, 1, 9, 67, 111, 118, 101, 114, 46, 80, 78, 71, 10, 3, 19, 230, 166, 138, 106,
        255, 201, 108, 26, 199, 230, 123, 32, 83, 67, 51, 246, 80, 68, 223, 56, 86, 4, 211, 110,
        135, 102, 171, 195, 232, 94, 5, 146, 188, 117, 178, 100, 140, 30, 57, 2, 119, 201, 167, 9,
        242, 79, 142, 53, 137, 98, 64, 189, 201, 208, 45, 8, 71, 189, 200, 233, 15, 131, 55, 220,
        54, 146, 4, 92, 24, 9, 193, 160, 113, 255, 11, 129, 30, 70, 105, 234, 138, 67, 233, 22,
        205, 241, 239, 200, 58, 50, 6, 148, 198, 25, 201, 69, 85, 173, 245, 101, 175, 107, 29, 211,
        45, 85, 122, 75, 71, 114, 95, 72, 123, 93, 138, 76, 173, 107, 31, 21, 24, 199, 40, 38, 179,
        126, 177, 154, 105, 194, 213, 22, 123, 122, 29, 119, 86, 81, 3, 5, 4, 0,
    ];

    const RAR_WITHOUT_COVER: &[u8] = &[
        82, 97, 114, 33, 26, 7, 1, 0, 51, 146, 181, 229, 10, 1, 5, 6, 0, 5, 1, 1, 128, 128, 0, 65,
        40, 154, 117, 36, 2, 3, 11, 133, 0, 4, 133, 0, 164, 131, 2, 63, 188, 38, 164, 128, 0, 1, 6,
        49, 48, 46, 112, 110, 103, 10, 3, 19, 230, 166, 138, 106, 81, 110, 41, 26, 97, 114, 99,
        104, 10, 9, 32, 190, 18, 35, 2, 3, 11, 153, 0, 4, 153, 0, 164, 131, 2, 12, 203, 7, 227,
        128, 0, 1, 5, 50, 46, 112, 110, 103, 10, 3, 19, 230, 166, 138, 106, 142, 1, 56, 26, 92, 83,
        123, 80, 82, 69, 84, 84, 89, 95, 78, 65, 77, 69, 125, 32, 92, 114, 32, 40, 92, 108, 41, 10,
        10, 29, 119, 86, 81, 3, 5, 4, 0,
    ];

    #[test]
    fn automatic_cover_and_natural_fallback_read_single_rar_entries() {
        let directory = tempfile::tempdir().unwrap();
        let with_cover = directory.path().join("with-cover.rar");
        std::fs::write(&with_cover, RAR_WITH_COVER).unwrap();
        let cover = crate::archive::cover::load_cover_source_bytes(&with_cover).unwrap();
        assert!(cover.starts_with(b"# Pathnames of valid login shells."));

        let without_cover = directory.path().join("without-cover.rar");
        std::fs::write(&without_cover, RAR_WITHOUT_COVER).unwrap();
        assert!(
            super::super::automatic_cover_hint_bytes(
                &without_cover,
                &mut crate::archive::resource::ResourceBudget::default()
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(
            crate::archive::cover::load_cover_source_bytes(&without_cover).unwrap(),
            b"\\S{PRETTY_NAME} \\r (\\l)\n\n"
        );
    }

    #[test]
    fn backend_declares_sequential_strategy_and_progress_capability() {
        assert_eq!(
            access_strategy(Path::new("book.rar")),
            crate::archive::ArchiveAccessStrategy::Sequential
        );
        assert!(supports_sequential_progress(Path::new("book.cbr")));
    }

    #[test]
    fn image_plan_filters_entries_and_maps_archive_order_to_natural_positions() {
        let plan = plan_image_entries([
            (0, true, "001.jpg".into()),
            (1, true, "002.jpg".into()),
            (2, true, "004.jpg".into()),
            (3, false, "images".into()),
            (4, true, "__MACOSX/000.jpg".into()),
            (5, true, "._000.jpg".into()),
            (6, true, "memo.txt".into()),
            (7, true, "003.jpg".into()),
        ]);

        assert_eq!(
            plan.iter()
                .map(|image| (
                    image.entry_id.archive_index,
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
    fn contents_plan_includes_cover_while_viewer_plan_excludes_it() {
        let entries = [
            (0, true, "cover.jpg".into()),
            (1, true, "11.jpg".into()),
            (2, true, "3.jpg".into()),
        ];

        let viewer = plan_image_entries(entries.clone());
        let contents = plan_image_entries_with_options(entries, true);

        assert_eq!(
            viewer
                .iter()
                .map(|image| image.entry_name.as_str())
                .collect::<Vec<_>>(),
            ["11.jpg", "3.jpg"]
        );
        assert_eq!(
            contents
                .iter()
                .map(|image| (image.entry_name.as_str(), image.physical_index))
                .collect::<Vec<_>>(),
            [("cover.jpg", 2), ("11.jpg", 1), ("3.jpg", 0)]
        );
    }

    #[test]
    fn nested_archive_entry_requires_a_real_supported_non_metadata_file() {
        for (is_file, name, expected) in [
            (true, "inner.cbz", true),
            (true, "INNER.RAR", true),
            (true, "nested/book.cb7", true),
            (false, "directory.rar", false),
            (true, "__MACOSX/inner.rar", false),
            (true, "nested/._inner.rar", false),
            (true, "book.epub", false),
            (true, "page.jpg", false),
        ] {
            assert_eq!(is_nested_archive_entry(is_file, name), expected, "{name}");
        }
    }

    #[test]
    fn duplicate_names_keep_distinct_entry_ids_and_stable_natural_positions() {
        let plan = plan_image_entries([
            (2, true, "2.jpg".into()),
            (5, true, "1.jpg".into()),
            (8, true, "1.jpg".into()),
        ]);

        assert_eq!(plan[0].physical_index, 2);
        assert_eq!(plan[1].physical_index, 0);
        assert_eq!(plan[2].physical_index, 1);
        assert_ne!(plan[1].entry_id, plan[2].entry_id);
    }

    #[test]
    fn processing_matches_duplicate_names_by_archive_index() {
        let plan = plan_image_entries([(2, true, "same.png".into()), (5, true, "same.png".into())]);
        let planned_by_id = plan
            .iter()
            .map(|image| (image.entry_id, image))
            .collect::<HashMap<_, _>>();

        let first = planned_image_for_processed_entry(
            &planned_by_id,
            RarEntryId { archive_index: 2 },
            "same.png",
        )
        .unwrap();
        let second = planned_image_for_processed_entry(
            &planned_by_id,
            RarEntryId { archive_index: 5 },
            "same.png",
        )
        .unwrap();

        assert_eq!(first.entry_id.archive_index, 2);
        assert_eq!(second.entry_id.archive_index, 5);
        assert_ne!(first.entry_id, second.entry_id);
        assert!(
            planned_image_for_processed_entry(
                &planned_by_id,
                RarEntryId { archive_index: 5 },
                "other.png",
            )
            .is_err()
        );
    }

    #[test]
    fn cover_plan_selects_natural_first_image_with_stable_archive_index() {
        let plan = plan_image_entries([
            (0, true, "notes.txt".into()),
            (1, true, "10.png".into()),
            (2, false, "images".into()),
            (3, true, "1.png".into()),
            (4, true, "1.png".into()),
            (5, true, "2.png".into()),
        ]);

        let first = first_planned_image(&plan).unwrap();

        assert_eq!(first.entry_id.archive_index, 3);
        assert_eq!(first.entry_name, "1.png");
        assert_eq!(first.physical_index, 0);
    }

    #[test]
    fn extracted_images_notify_immediately_and_build_the_same_ordered_document() {
        let plan = plan_image_entries([
            (0, true, "001.png".into()),
            (1, true, "002.png".into()),
            (2, true, "004.png".into()),
            (3, true, "005.png".into()),
            (4, true, "003.png".into()),
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
        assert_eq!(plan[4].entry_name, "003.png");

        let document = images
            .into_ordered_document(Path::new("pages.rar"))
            .unwrap();
        assert_eq!(document.assets.len(), 5);
        assert_eq!(document.pages.len(), 6);
        assert_eq!(document.assets[1].layout, ImageLayout::Spread);
        assert_eq!(document.pages[1].part, PagePart::Right);
        assert_eq!(document.pages[2].part, PagePart::Left);

        for notification in &notifications {
            let ImageSource::Memory(document_bytes) =
                &document.assets[notification.physical_index].source
            else {
                panic!("RAR assets should use shared memory bytes");
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
    fn already_cancelled_progressive_load_has_a_distinct_outcome() {
        let cancel_token = ProgressiveArchiveCancelToken::default();
        cancel_token.cancel();
        let outcome =
            load_document_with_progress(Path::new("missing.rar"), &cancel_token, &mut |_| {
                panic!("cancelled load must not notify")
            })
            .unwrap();

        assert!(matches!(outcome, ProgressiveArchiveLoadOutcome::Cancelled));
    }
}
