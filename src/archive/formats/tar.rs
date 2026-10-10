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

pub(super) fn has_nested_archives(archive_path: &Path) -> bool {
    let file = match std::fs::File::open(archive_path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    let mut archive = tar::Archive::new(file);
    let entries = match archive.entries() {
        Ok(entries) => entries,
        Err(_) => return false,
    };

    for entry in entries.flatten() {
        let Ok(path) = entry.path() else {
            continue;
        };
        let path_string = path.to_string_lossy().to_string();
        if entry.header().entry_type().is_file()
            && !is_macos_metadata(&path_string)
            && is_archive_ext(&path)
        {
            return true;
        }
    }
    false
}

pub(super) fn has_viewable_content(archive_path: &Path) -> Result<bool, AppError> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = tar::Archive::new(file);

    for entry_result in archive.entries().map_err(archive_error)? {
        let entry = entry_result.map_err(archive_error)?;
        if entry.header().entry_type().is_file()
            && is_viewable_archive_entry(&entry.path().map_err(archive_error)?.to_string_lossy())
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn entry_paths(
    archive_path: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<Vec<std::path::PathBuf>, AppError> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = tar::Archive::new(file);
    let mut paths = Vec::new();
    for (index, entry) in archive.entries().map_err(archive_error)?.enumerate() {
        let entry = entry.map_err(archive_error)?;
        let path = entry.path().map_err(archive_error)?;
        budget.entry(
            archive_path,
            index,
            entry.header().entry_type().is_file() && is_image_ext(&path),
        )?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().map_err(archive_error)?.into_owned();
        if !is_macos_metadata(&path.to_string_lossy()) {
            paths.push(path);
        }
    }
    Ok(paths)
}

pub(super) fn automatic_cover_entry_bytes(
    archive_path: &Path,
    entry_path: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<super::CoverEntryBytes, AppError> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = tar::Archive::new(file);
    for entry in archive.entries().map_err(archive_error)? {
        let mut entry = entry.map_err(archive_error)?;
        if entry.header().entry_type().is_file()
            && entry.path().map_err(archive_error)?.as_ref() == entry_path
        {
            let size = entry.size();
            let bytes = budget.read_all(&mut entry, Some(size))?;
            return Ok(super::CoverEntryBytes::Bytes(bytes));
        }
    }
    Ok(super::CoverEntryBytes::Missing)
}

// Keep tar's existing unpack/link/sparse handling; interrupt its bounded reads.
struct CancelRead<'a, 'cancel> {
    file: std::fs::File,
    budget: &'a crate::archive::resource::ResourceBudget<'cancel>,
}
impl std::io::Read for CancelRead<'_, '_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.budget
            .check_cancel()
            .map_err(|_| std::io::ErrorKind::Other)?;
        std::io::Read::read(&mut self.file, bytes)
    }
}

pub(super) fn extract_to_dir(
    archive_path: &Path,
    destination: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<(), AppError> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = tar::Archive::new(file);
    for (index, entry) in archive.entries().map_err(archive_error)?.enumerate() {
        let entry = entry.map_err(archive_error)?;
        let path = entry.path().map_err(archive_error)?;
        let is_file = entry.header().entry_type().is_file();
        budget.entry(archive_path, index, is_file && is_image_ext(&path))?;
        if is_file {
            let output_path = safe_archive_output_path(destination, &path)?;
            budget.account_known_temp_path(&output_path, entry.size())?;
        }
    }
    // tar 0.4.46 builds each regular entry's payload reader with `Read::take(entry_size)`;
    // unpack copies only that bounded reader, and sparse padding is limited to the validated
    // logical entry size. Pre-accounting therefore caps every payload before `unpack` writes.
    // A truncated payload makes `unpack` return an error after a partial write; the caller's
    // TempDir owns and removes that partial tree.
    let file = std::fs::File::open(archive_path)?;
    let mut archive = tar::Archive::new(CancelRead { file, budget });
    let result = archive.unpack(destination);
    budget.check_cancel()?;
    result.map_err(AppError::Io)
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
    let file = std::fs::File::open(archive_path)?;
    let mut archive = tar::Archive::new(file);
    let mut images = SequentialImageStorage::with_limit_in(0, memory_limit, spill_root);
    let mut budget = crate::archive::resource::ResourceBudget::for_expansion_limit(limit);

    for (index, entry_result) in archive
        .entries()
        .map_err(|error| AppError::Archive(error.to_string()))?
        .enumerate()
    {
        let mut entry = entry_result.map_err(archive_error)?;
        let path = entry.path().map_err(archive_error)?;
        let path_string = path.to_string_lossy().to_string();
        let is_file = entry.header().entry_type().is_file();
        budget.entry(archive_path, index, is_file && is_image_ext(&path))?;

        if is_file && !is_macos_metadata(&path_string) && is_image_ext(&path) {
            let size = entry.size();
            let data = budget.read_all(&mut entry, Some(size))?;
            images.push_with_budget(path_string, data, &mut budget)?;
        }
    }
    images.into_naturally_ordered_document(archive_path)
}

pub(super) fn first_image_bytes(
    archive_path: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<Option<Vec<u8>>, AppError> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = tar::Archive::new(file);
    let mut names = Vec::new();
    for (index, entry_result) in archive.entries().map_err(archive_error)?.enumerate() {
        let entry = entry_result.map_err(archive_error)?;
        let path = entry.path().map_err(archive_error)?;
        let is_file = entry.header().entry_type().is_file();
        budget.entry(archive_path, index, is_file && is_image_ext(&path))?;
        if is_file {
            names.push(path.to_string_lossy().into_owned());
        }
    }
    let Some(first_name) = first_image_entry_name(names) else {
        return Ok(None);
    };

    let file = std::fs::File::open(archive_path)?;
    let mut archive = tar::Archive::new(file);
    for entry_result in archive.entries().map_err(archive_error)? {
        let mut entry = entry_result.map_err(archive_error)?;
        if entry.header().entry_type().is_file()
            && entry.path().map_err(archive_error)?.to_string_lossy() == first_name
        {
            let size = entry.size();
            let bytes = budget.read_all(&mut entry, Some(size))?;
            return Ok(Some(bytes));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

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
    fn backend_is_sequential_without_progress_notifications() {
        let path = Path::new("missing.tar");
        assert_eq!(
            access_strategy(path),
            crate::archive::ArchiveAccessStrategy::Sequential
        );
        assert!(!supports_sequential_progress(path));
    }

    fn write_archive(path: &Path, entry_names: &[&str]) {
        let file = std::fs::File::create(path).unwrap();
        let mut archive = tar::Builder::new(file);
        for entry_name in entry_names {
            let bytes = b"content";
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive
                .append_data(&mut header, entry_name, &bytes[..])
                .unwrap();
        }
        archive.finish().unwrap();
    }

    #[test]
    fn content_probe_uses_headers_without_reading_entry_data() {
        let directory = tempfile::tempdir().unwrap();
        for (name, entries, expected) in [
            ("image.tar", &["001.webp"][..], true),
            ("nested.tar", &["inner.rar"][..], true),
            ("epub.tar", &["book.epub"][..], false),
            ("other.tar", &["setup.exe", "memo.txt"][..], false),
        ] {
            let path = directory.path().join(name);
            write_archive(&path, entries);
            assert_eq!(has_viewable_content(&path).unwrap(), expected);
        }
    }

    #[test]
    fn extraction_checks_temp_and_cumulative_limits_before_unpacking() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("bounded.tar");
        write_archive(&archive_path, &["page.jpg"]);
        let destination = directory.path().join("out");
        std::fs::create_dir(&destination).unwrap();
        let limits = crate::archive::resource::ResourceLimits {
            single_entry: 4,
            cumulative: 4,
            temp_writes: 4,
            temp_occupancy: 4,
            entries: 8,
            images: 8,
        };
        assert!(matches!(
            extract_to_dir(
                &archive_path,
                &destination,
                &mut crate::archive::resource::ResourceBudget::new(limits),
            ),
            Err(AppError::ArchiveResourceLimit(
                crate::archive::ResourceLimitKind::SingleEntryBytes
            ))
        ));
        assert!(!destination.join("page.jpg").exists());
    }

    #[test]
    fn unpack_never_writes_more_than_the_tar_entry_header_size() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("overlong-payload.tar");
        let mut archive = tar::Builder::new(std::fs::File::create(&archive_path).unwrap());
        let mut header = tar::Header::new_gnu();
        header.set_size(4);
        header.set_mode(0o644);
        header.set_cksum();
        archive
            .append_data(&mut header, "payload.bin", &b"more than four bytes"[..])
            .unwrap();
        archive.finish().unwrap();

        let destination = tempfile::tempdir().unwrap();
        let mut budget = crate::archive::resource::ResourceBudget::new(
            crate::archive::resource::ResourceLimits {
                single_entry: 4,
                cumulative: 4,
                temp_writes: 4,
                temp_occupancy: 4,
                entries: 1,
                images: 1,
            },
        );
        extract_to_dir(&archive_path, destination.path(), &mut budget).unwrap();

        assert_eq!(
            std::fs::metadata(destination.path().join("payload.bin"))
                .unwrap()
                .len(),
            4
        );
    }

    #[test]
    fn first_image_uses_natural_order_and_filters_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("cover.tar");
        let file = std::fs::File::create(&archive_path).unwrap();
        let mut archive = tar::Builder::new(file);
        for (name, bytes) in [
            ("10.jpg", &b"ten"[..]),
            ("memo.txt", &b"memo"[..]),
            ("__MACOSX/0.jpg", &b"metadata"[..]),
            ("._0.jpg", &b"dot metadata"[..]),
            ("2.png", &b"two"[..]),
            ("1.webp", &b"one"[..]),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive.append_data(&mut header, name, bytes).unwrap();
        }
        archive.finish().unwrap();

        assert_eq!(
            first_image_bytes(
                &archive_path,
                &mut crate::archive::resource::ResourceBudget::default()
            )
            .unwrap(),
            Some(b"one".to_vec())
        );
    }

    #[test]
    fn automatic_cover_reads_the_transparent_wrapper_entry() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("cover.tar");
        let file = std::fs::File::create(&archive_path).unwrap();
        let mut archive = tar::Builder::new(file);
        for (name, bytes) in [
            ("book/002.png", &b"two"[..]),
            ("book/Cover.PNG", &b"cover"[..]),
            ("book/001.png", &b"one"[..]),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive.append_data(&mut header, name, bytes).unwrap();
        }
        archive.finish().unwrap();

        assert_eq!(
            super::super::automatic_cover_hint_bytes(
                &archive_path,
                &mut crate::archive::resource::ResourceBudget::default()
            )
            .unwrap()
            .unwrap(),
            b"cover"
        );
    }

    #[test]
    fn automatic_cover_fallback_counts_tar_entries_only_once() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("fallback.tar");
        write_archive(&archive_path, &["page.jpg"]);
        let limits = crate::archive::resource::ResourceLimits {
            single_entry: 16,
            cumulative: 16,
            temp_writes: 16,
            temp_occupancy: 16,
            entries: 1,
            images: 1,
        };
        let mut budget = crate::archive::resource::ResourceBudget::new(limits);

        assert!(
            super::super::automatic_cover_hint_bytes(&archive_path, &mut budget)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            first_image_bytes(&archive_path, &mut budget).unwrap(),
            Some(b"content".to_vec())
        );
    }

    #[test]
    fn load_spills_images_above_the_injected_limit() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("pages.tar");
        let first = png(4, 8);
        let second = png(8, 4);
        let file = std::fs::File::create(&archive_path).unwrap();
        let mut archive = tar::Builder::new(file);
        for (name, bytes) in [("10.png", first.as_slice()), ("2.png", second.as_slice())] {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive.append_data(&mut header, name, bytes).unwrap();
        }
        archive.finish().unwrap();

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
