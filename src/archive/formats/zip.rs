use super::archive_error;
use crate::archive::safety::safe_archive_output_path;
use crate::archive::{
    IMAGE_DIMENSION_PROBE_SIZE, first_image_entry_name, is_archive_ext, is_image_ext,
    is_macos_metadata, is_viewable_archive_entry, is_wide_from_bytes,
};
use crate::document::{Document, ImageLayout, ImageSource};
use crate::error::AppError;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

pub(super) const fn access_strategy(_archive_path: &Path) -> crate::archive::ArchiveAccessStrategy {
    crate::archive::ArchiveAccessStrategy::RandomAccess
}

pub(super) const fn supports_sequential_progress(_archive_path: &Path) -> bool {
    false
}

pub(super) struct RandomAccessReader {
    archive: zip::ZipArchive<std::fs::File>,
}

impl RandomAccessReader {
    pub(super) fn open(archive_path: &Path) -> Option<Self> {
        let file = std::fs::File::open(archive_path).ok()?;
        let archive = zip::ZipArchive::new(file).ok()?;
        Some(Self { archive })
    }

    pub(super) fn read_entry(
        &mut self,
        limit: crate::archive::ArchiveExpansionLimit,
        entry_index: usize,
        expected_name: &str,
    ) -> Result<Option<gtk::glib::Bytes>, AppError> {
        read_entry_bytes(limit, &mut self.archive, entry_index, expected_name)
    }
}

fn read_entry_bytes<R>(
    limit: crate::archive::ArchiveExpansionLimit,
    archive: &mut zip::ZipArchive<R>,
    entry_index: usize,
    expected_name: &str,
) -> Result<Option<gtk::glib::Bytes>, AppError>
where
    R: Read + Seek,
{
    let mut budget = crate::archive::resource::ResourceBudget::for_expansion_limit(limit);
    read_entry_bytes_with_budget(archive, entry_index, expected_name, &mut budget)
        .map(|bytes| bytes.map(gtk::glib::Bytes::from_owned))
}

fn read_entry_bytes_with_budget<R>(
    archive: &mut zip::ZipArchive<R>,
    entry_index: usize,
    expected_name: &str,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<Option<Vec<u8>>, AppError>
where
    R: Read + Seek,
{
    let mut entry = match archive.by_index(entry_index) {
        Ok(entry) => entry,
        Err(error) => return Err(archive_error(error)),
    };
    if entry.name() != expected_name {
        return Ok(None);
    }
    let size = entry.size();
    budget.read_all(&mut entry, Some(size)).map(Some)
}

pub(super) fn has_nested_archives(archive_path: &Path) -> bool {
    let file = match std::fs::File::open(archive_path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(archive) => archive,
        Err(_) => return false,
    };

    for index in 0..archive.len() {
        if let Ok(entry) = archive.by_index(index) {
            if entry.is_dir() {
                continue;
            }
            let name = entry.name();
            if is_macos_metadata(name) {
                continue;
            }
            if is_archive_ext(Path::new(name)) {
                return true;
            }
        }
    }
    false
}

pub(super) fn has_viewable_content(archive_path: &Path) -> Result<bool, AppError> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(archive_error)?;

    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(archive_error)?;
        if !entry.is_dir() && is_viewable_archive_entry(entry.name()) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn extract_to_dir(
    archive_path: &Path,
    destination: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<(), AppError> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(archive_error)?;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(archive_error)?;
        let entry_name = entry.name().to_string();
        budget.entry(archive_path, index, is_image_ext(Path::new(&entry_name)))?;
        if is_macos_metadata(&entry_name) {
            continue;
        }
        let output_path = safe_archive_output_path(destination, Path::new(&entry_name))?;
        if entry.is_dir() {
            std::fs::create_dir_all(&output_path)?;
            continue;
        }
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let size = entry.size();
        budget.copy_materialized_to_path(&mut entry, &output_path, Some(size))?;
    }
    Ok(())
}

pub(super) fn entry_paths(
    archive_path: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<Vec<std::path::PathBuf>, AppError> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(archive_error)?;
    let mut paths = Vec::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(archive_error)?;
        let path = Path::new(entry.name());
        budget.entry(archive_path, index, is_image_ext(path))?;
        if !entry.is_dir() && !is_macos_metadata(entry.name()) {
            paths.push(path.to_path_buf());
        }
    }
    Ok(paths)
}

pub(super) fn cover_entry_bytes(
    archive_path: &Path,
    entry_path: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<Option<Vec<u8>>, AppError> {
    let expected = entry_path.to_string_lossy();
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(archive_error)?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(archive_error)?;
        if !entry.is_dir() && entry.name() == expected {
            let size = entry.size();
            let bytes = budget.read_all(&mut entry, Some(size))?;
            return Ok(Some(bytes));
        }
    }
    Ok(None)
}

pub(super) fn materialize_nested_entry(
    archive_path: &Path,
    entry_path: &Path,
    output: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<(), AppError> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(archive_error)?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(archive_error)?;
        if !entry.is_dir() && Path::new(entry.name()) == entry_path {
            let size = entry.size();
            budget.copy_materialized_to_path(&mut entry, output, Some(size))?;
            return Ok(());
        }
    }
    Err(AppError::Archive("Nested ZIP entry is missing".into()))
}

pub(super) fn load_document(
    limit: crate::archive::ArchiveExpansionLimit,
    archive_path: &Path,
) -> Result<Document, AppError> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(archive_error)?;
    let mut page_entries = Vec::new();
    let mut all_entries = Vec::new();

    let mut budget = crate::archive::resource::ResourceBudget::for_expansion_limit(limit);
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(archive_error)?;
        if entry.is_dir() {
            budget.entry(archive_path, index, false)?;
            continue;
        }

        let entry_name = entry.name().to_string();
        budget.entry(archive_path, index, is_image_ext(Path::new(&entry_name)))?;
        if is_macos_metadata(&entry_name) {
            continue;
        }
        all_entries.push(Path::new(&entry_name).to_path_buf());
        if !is_image_ext(Path::new(&entry_name)) {
            continue;
        }
        page_entries.push((index, entry_name));
    }

    let cover_only = crate::archive::cover_only_paths(&all_entries);
    page_entries.retain(|(_, name)| !cover_only.contains(Path::new(name)));

    page_entries.sort_by(|left, right| natord::compare(&left.1, &right.1));

    let archive_path = archive_path.to_path_buf();
    let mut document = Document::new(archive_path.clone(), None);
    for (entry_index, entry_name) in page_entries {
        let entry = archive.by_index(entry_index).map_err(archive_error)?;
        if entry.name() != entry_name {
            return Err(AppError::Archive(format!(
                "ZIPエントリ一覧と取得結果が一致しません: expected {entry_name:?}, got {:?}",
                entry.name()
            )));
        }
        let size = entry.size();
        let buffer = budget.read_all(
            &mut entry.take(IMAGE_DIMENSION_PROBE_SIZE as u64),
            Some(size),
        )?;
        let is_wide = is_wide_from_bytes(&buffer);

        let image = PathBuf::from(&entry_name);
        let source = ImageSource::ArchiveEntry {
            archive_path: archive_path.clone(),
            entry_index,
            entry_name,
        };
        document.add_archive_asset(
            source,
            ImageLayout::from_is_wide(is_wide),
            Vec::new(),
            image,
        );
    }
    Ok(document)
}

pub(super) fn first_image_bytes(
    archive_path: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<Option<Vec<u8>>, AppError> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(archive_error)?;
    let mut names = Vec::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(archive_error)?;
        if !entry.is_dir() {
            names.push(entry.name().to_string());
        }
    }
    let Some(first_name) = first_image_entry_name(names) else {
        return Ok(None);
    };

    let mut entry = archive.by_name(&first_name).map_err(archive_error)?;
    let size = entry.size();
    let bytes = budget.read_all(&mut entry, Some(size))?;
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_archive(path: &Path, entry_names: &[&str]) {
        let file = std::fs::File::create(path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        for entry_name in entry_names {
            archive
                .start_file(*entry_name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(b"content").unwrap();
        }
        archive.finish().unwrap();
    }

    #[test]
    fn bounded_lazy_reads_use_independent_budgets_and_check_expanded_size() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("limits.cbz");
        let file = std::fs::File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in [("exact.jpg", vec![7; 4]), ("expanded.jpg", vec![9; 5])] {
            writer.start_file(name, options).unwrap();
            writer.write_all(&data).unwrap();
        }
        writer.finish().unwrap();

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let limits = |single| crate::archive::resource::ResourceLimits {
            single_entry: single,
            cumulative: 16,
            temp_writes: 16,
            temp_occupancy: 16,
            entries: 2,
            images: 2,
        };
        for _ in 0..2 {
            assert_eq!(
                read_entry_bytes_with_budget(
                    &mut archive,
                    0,
                    "exact.jpg",
                    &mut crate::archive::resource::ResourceBudget::new(limits(4)),
                )
                .unwrap()
                .unwrap(),
                vec![7; 4]
            );
        }
        assert!(matches!(
            read_entry_bytes_with_budget(
                &mut archive,
                1,
                "expanded.jpg",
                &mut crate::archive::resource::ResourceBudget::new(limits(4)),
            ),
            Err(AppError::ArchiveResourceLimit(
                crate::archive::ResourceLimitKind::SingleEntryBytes
            ))
        ));
    }

    #[test]
    fn content_probe_uses_entry_names_without_reading_entry_data() {
        let directory = tempfile::tempdir().unwrap();
        for (name, entries, expected) in [
            ("image.zip", &["001.jpg"][..], true),
            ("nested.zip", &["inner.cbz"][..], true),
            ("epub.zip", &["book.epub"][..], false),
            ("other.zip", &["setup.exe", "memo.txt"][..], false),
        ] {
            let path = directory.path().join(name);
            write_archive(&path, entries);
            assert_eq!(has_viewable_content(&path).unwrap(), expected);
        }
    }

    #[test]
    fn random_access_reader_decodes_deflated_entry() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("deflated.zip");
        let file = std::fs::File::create(&archive_path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        archive.start_file("page.jpg", options).unwrap();
        archive.write_all(b"deflated content").unwrap();
        archive.finish().unwrap();

        let mut reader = RandomAccessReader::open(&archive_path).unwrap();
        assert_eq!(
            reader
                .read_entry(Default::default(), 0, "page.jpg")
                .unwrap()
                .unwrap()
                .as_ref(),
            b"deflated content"
        );
    }

    #[test]
    fn extraction_rejects_parent_directory_entries() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("unsafe.zip");
        let file = std::fs::File::create(&archive_path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file("../escaped.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"escaped").unwrap();
        archive.finish().unwrap();

        let destination = directory.path().join("destination");
        let error = extract_to_dir(
            &archive_path,
            &destination,
            &mut crate::archive::resource::ResourceBudget::default(),
        )
        .unwrap_err();

        assert!(matches!(error, AppError::Archive(_)));
        assert!(!directory.path().join("escaped.txt").exists());
    }

    #[test]
    fn first_image_reads_only_the_naturally_first_valid_entry() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("cover.zip");
        let file = std::fs::File::create(&archive_path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, bytes) in [
            ("10.jpg", &b"ten"[..]),
            ("memo.txt", &b"memo"[..]),
            ("__MACOSX/0.jpg", &b"metadata"[..]),
            ("._0.jpg", &b"dot metadata"[..]),
            ("2.png", &b"two"[..]),
            ("1.webp", &b"one"[..]),
        ] {
            archive.start_file(name, options).unwrap();
            archive.write_all(bytes).unwrap();
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
}
