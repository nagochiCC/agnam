use super::{SequentialImageStorage, archive_error};
use crate::archive::safety::safe_archive_output_path;
use crate::archive::{
    first_image_entry_name, is_archive_ext, is_image_ext, is_macos_metadata,
    is_viewable_archive_entry,
};
use crate::document::Document;
use crate::error::AppError;
use std::path::Path;

pub(super) const fn access_strategy(_archive_path: &Path) -> crate::archive::ArchiveAccessStrategy {
    crate::archive::ArchiveAccessStrategy::Sequential
}

pub(super) const fn supports_sequential_progress(_archive_path: &Path) -> bool {
    false
}

fn read_current_entry(
    lha: &mut delharc::LhaDecodeReader<std::fs::File>,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<Vec<u8>, AppError> {
    let original_size = lha.header().original_size;
    budget.read_all(lha, Some(original_size))
}

pub(super) fn has_nested_archives(archive_path: &Path) -> bool {
    let mut lha = match delharc::parse_file(archive_path) {
        Ok(lha) => lha,
        Err(_) => return false,
    };

    loop {
        let header = lha.header();
        let filename = header.parse_pathname();
        let filename_string = filename.to_string_lossy().to_string();

        if !header.is_directory()
            && !is_macos_metadata(&filename_string)
            && is_archive_ext(&filename)
        {
            return true;
        }

        match lha.seek_next_file() {
            Ok(true) => {}
            _ => break,
        }
    }
    false
}

pub(super) fn has_viewable_content(archive_path: &Path) -> Result<bool, AppError> {
    let mut lha = delharc::parse_file(archive_path).map_err(archive_error)?;

    loop {
        let header = lha.header();
        if !header.is_directory()
            && lha.is_decoder_supported()
            && is_viewable_archive_entry(&header.parse_pathname().to_string_lossy())
        {
            return Ok(true);
        }
        if !lha.seek_next_file().map_err(archive_error)? {
            break;
        }
    }
    Ok(false)
}

pub(super) fn entry_paths(
    archive_path: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<Vec<std::path::PathBuf>, AppError> {
    let mut lha = delharc::parse_file(archive_path).map_err(archive_error)?;
    let mut paths = Vec::new();
    let mut index = 0;
    loop {
        let header = lha.header();
        let path = header.parse_pathname();
        budget.entry(
            archive_path,
            index,
            !header.is_directory() && is_image_ext(&path),
        )?;
        if !header.is_directory() && !is_macos_metadata(&path.to_string_lossy()) {
            paths.push(path);
        }
        if !lha.seek_next_file().map_err(archive_error)? {
            break;
        }
        index += 1;
    }
    Ok(paths)
}

pub(super) fn automatic_cover_entry_bytes(
    archive_path: &Path,
    entry_path: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<super::CoverEntryBytes, AppError> {
    let mut lha = delharc::parse_file(archive_path).map_err(archive_error)?;
    loop {
        let header = lha.header();
        if !header.is_directory()
            && lha.is_decoder_supported()
            && header.parse_pathname() == entry_path
        {
            return read_current_entry(&mut lha, budget).map(super::CoverEntryBytes::Bytes);
        }
        if !lha.seek_next_file().map_err(archive_error)? {
            break;
        }
    }
    Ok(super::CoverEntryBytes::Missing)
}

pub(super) fn extract_to_dir(
    archive_path: &Path,
    destination: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<(), AppError> {
    let mut lha = delharc::parse_file(archive_path).map_err(archive_error)?;
    let mut index = 0;
    loop {
        let header = lha.header();
        let filename = header.parse_pathname();
        let is_target = !header.is_directory() && lha.is_decoder_supported();
        budget.entry(
            archive_path,
            index,
            !header.is_directory() && is_image_ext(&filename),
        )?;
        let output_path = safe_archive_output_path(destination, &filename)?;

        if is_target {
            let original_size = header.original_size;
            if let Some(parent) = output_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            budget.copy_materialized_to_path(&mut lha, &output_path, Some(original_size))?;
        }

        if !lha.seek_next_file().map_err(archive_error)? {
            break;
        }
        index += 1;
    }
    Ok(())
}

pub(super) fn load_document(
    limit: crate::archive::ArchiveExpansionLimit,
    archive_path: &Path,
) -> Result<Document, AppError> {
    load_document_with_limit(
        limit,
        archive_path,
        super::SEQUENTIAL_ARCHIVE_MEMORY_LIMIT_BYTES,
        super::sequential_spill_root(),
    )
}

fn load_document_with_limit(
    limit: crate::archive::ArchiveExpansionLimit,
    archive_path: &Path,
    memory_limit: usize,
    spill_root: std::path::PathBuf,
) -> Result<Document, AppError> {
    let mut lha = delharc::parse_file(archive_path).map_err(archive_error)?;
    let mut images = SequentialImageStorage::with_limit_in(0, memory_limit, spill_root);
    let mut budget = crate::archive::resource::ResourceBudget::for_expansion_limit(limit);
    let mut index = 0;

    loop {
        let header = lha.header();
        let filename = header.parse_pathname();
        let filename_string = filename.to_string_lossy().to_string();
        let is_target_image = !header.is_directory()
            && !is_macos_metadata(&filename_string)
            && is_image_ext(&filename)
            && lha.is_decoder_supported();
        budget.entry(
            archive_path,
            index,
            !header.is_directory() && is_image_ext(&filename),
        )?;

        if is_target_image {
            let data = read_current_entry(&mut lha, &mut budget)?;
            images.push_with_budget(filename_string, data, &mut budget)?;
        }

        if !lha.seek_next_file().map_err(archive_error)? {
            break;
        }
        index += 1;
    }
    images.into_naturally_ordered_document(archive_path)
}

pub(super) fn first_image_bytes(
    archive_path: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<Option<Vec<u8>>, AppError> {
    let mut lha = delharc::parse_file(archive_path).map_err(archive_error)?;
    let mut names = Vec::new();
    loop {
        let header = lha.header();
        if !header.is_directory() && lha.is_decoder_supported() {
            names.push(header.parse_pathname().to_string_lossy().into_owned());
        }
        if !lha.seek_next_file().map_err(archive_error)? {
            break;
        }
    }
    let Some(first_name) = first_image_entry_name(names) else {
        return Ok(None);
    };

    let mut lha = delharc::parse_file(archive_path).map_err(archive_error)?;
    loop {
        let header = lha.header();
        let filename = header.parse_pathname().to_string_lossy().into_owned();
        if !header.is_directory() && lha.is_decoder_supported() && filename == first_name {
            return read_current_entry(&mut lha, budget).map(Some);
        }
        if !lha.seek_next_file().map_err(archive_error)? {
            break;
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

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

    fn append_lh0_entry(output: &mut Vec<u8>, name: &str, data: &[u8]) {
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

    #[test]
    fn backend_is_sequential_without_progress_notifications() {
        let path = Path::new("missing.lha");
        assert_eq!(
            access_strategy(path),
            crate::archive::ArchiveAccessStrategy::Sequential
        );
        assert!(!supports_sequential_progress(path));
    }

    #[test]
    fn automatic_cover_reads_only_the_selected_entry() {
        let mut archive = Vec::new();
        append_lh0_entry(&mut archive, "10.png", b"ten");
        append_lh0_entry(&mut archive, "Cover.PNG", b"cover");
        append_lh0_entry(&mut archive, "2.png", b"two");
        archive.push(0);
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("cover.lha");
        std::fs::File::create(&archive_path)
            .unwrap()
            .write_all(&archive)
            .unwrap();

        assert_eq!(
            super::super::automatic_cover_hint_bytes(
                &archive_path,
                &mut crate::archive::resource::ResourceBudget::default()
            )
            .unwrap()
            .unwrap(),
            b"cover"
        );
        assert_eq!(
            first_image_bytes(
                &archive_path,
                &mut crate::archive::resource::ResourceBudget::default()
            )
            .unwrap()
            .unwrap(),
            b"two"
        );
    }

    #[test]
    fn bounded_lha_reader_streams_metadata_sized_entry_and_stops_over_limit() {
        let mut archive = Vec::new();
        append_lh0_entry(&mut archive, "sample.bin", b"12345");
        archive.push(0);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bounded.lha");
        std::fs::write(&path, archive).unwrap();
        let limits = |single| crate::archive::resource::ResourceLimits {
            single_entry: single,
            cumulative: 16,
            temp_writes: 16,
            temp_occupancy: 16,
            entries: 8,
            images: 8,
        };
        let mut decoder = delharc::parse_file(&path).unwrap();
        assert_eq!(
            read_current_entry(
                &mut decoder,
                &mut crate::archive::resource::ResourceBudget::new(limits(5)),
            )
            .unwrap(),
            b"12345"
        );
        let mut decoder = delharc::parse_file(&path).unwrap();
        assert!(matches!(
            read_current_entry(
                &mut decoder,
                &mut crate::archive::resource::ResourceBudget::new(limits(4)),
            ),
            Err(AppError::ArchiveResourceLimit(
                crate::archive::ResourceLimitKind::SingleEntryBytes
            ))
        ));
    }

    #[test]
    fn load_spills_images_above_the_injected_limit() {
        let first = png(4, 8);
        let second = png(8, 4);
        let mut archive = Vec::new();
        append_lh0_entry(&mut archive, "10.png", &first);
        append_lh0_entry(&mut archive, "2.png", &second);
        archive.push(0);
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("pages.lha");
        std::fs::File::create(&archive_path)
            .unwrap()
            .write_all(&archive)
            .unwrap();

        let spill_root = directory.path().join("cache/agnam/tmp");
        let document = load_document_with_limit(
            Default::default(),
            &archive_path,
            first.len(),
            spill_root.clone(),
        )
        .unwrap();
        assert!(document.temp_dir.is_some());
        assert!(
            document
                .temp_dir
                .as_ref()
                .unwrap()
                .path()
                .starts_with(spill_root)
        );
        assert!(document
            .assets
            .iter()
            .all(|asset| matches!(&asset.source, crate::document::ImageSource::File(path) if path.exists())));
        assert_eq!(
            document.assets[0].archive_identity.as_ref().unwrap().image,
            std::path::PathBuf::from("2.png")
        );
    }
}
