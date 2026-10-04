use super::formats::RandomAccessArchiveReader;
use crate::document::ImageSource;
use crate::error::AppError;
use gtk::glib;
use std::path::PathBuf;

pub(crate) fn load_image_bytes(source: &ImageSource) -> Option<glib::Bytes> {
    try_load_image_bytes(source).ok().flatten()
}

pub(crate) fn try_load_image_bytes(source: &ImageSource) -> Result<Option<glib::Bytes>, AppError> {
    match source {
        ImageSource::File(path) => {
            let file = std::fs::File::open(path)?;
            // SAFETY: The mapping owns its file-backed region and is moved into `glib::Bytes`.
            let mapping = unsafe { memmap2::Mmap::map(&file)? };
            Ok(Some(glib::Bytes::from_owned(mapping)))
        }
        ImageSource::ArchiveEntry {
            archive_path,
            entry_index,
            entry_name,
        } => {
            let Some(mut archive) = RandomAccessArchiveReader::open(archive_path) else {
                return Ok(None);
            };
            archive.read_entry(*entry_index, entry_name)
        }
        ImageSource::Memory(data) => Ok(Some(data.clone())),
    }
}

pub(crate) struct ThumbnailImageLoader {
    archive: Option<(PathBuf, RandomAccessArchiveReader)>,
    #[cfg(test)]
    pub(crate) archive_open_count: usize,
    #[cfg(test)]
    pub(crate) source_load_count: usize,
}

impl ThumbnailImageLoader {
    pub(crate) fn new() -> Self {
        Self {
            archive: None,
            #[cfg(test)]
            archive_open_count: 0,
            #[cfg(test)]
            source_load_count: 0,
        }
    }

    pub(crate) fn load_image_bytes(&mut self, source: &ImageSource) -> Option<glib::Bytes> {
        #[cfg(test)]
        {
            self.source_load_count += 1;
        }
        let ImageSource::ArchiveEntry {
            archive_path,
            entry_index,
            entry_name,
        } = source
        else {
            return load_image_bytes(source);
        };

        let needs_open = self
            .archive
            .as_ref()
            .is_none_or(|(path, _)| path != archive_path);
        if needs_open {
            let archive = RandomAccessArchiveReader::open(archive_path)?;
            self.archive = Some((archive_path.clone(), archive));
            #[cfg(test)]
            {
                self.archive_open_count += 1;
            }
        }

        let Some((_, archive)) = self.archive.as_mut() else {
            return None;
        };
        archive.read_entry(*entry_index, entry_name).ok().flatten()
    }
}

impl Default for ThumbnailImageLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_with_entries(archive_path: &std::path::Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(archive_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for &(name, bytes) in entries {
            writer.start_file(name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    fn seven_zip_with_entries(archive_path: &std::path::Path, entries: &[(&str, &[u8])]) {
        let mut writer = sevenz_rust::SevenZWriter::create(archive_path).unwrap();
        for &(name, bytes) in entries {
            let mut entry = sevenz_rust::SevenZArchiveEntry::new();
            entry.name = name.to_string();
            entry.has_stream = true;
            writer
                .push_archive_entry(entry, Some(std::io::Cursor::new(bytes)))
                .unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn loads_file_memory_and_format_independent_archive_entry_sources() {
        let directory = tempfile::tempdir().unwrap();
        let file_path = directory.path().join("page.jpg");
        std::fs::write(&file_path, b"file").unwrap();

        let archive_path = directory.path().join("pages.cbz");
        zip_with_entries(&archive_path, &[("page.jpg", b"zip")]);

        let file = ImageSource::File(file_path);
        let memory = ImageSource::Memory(glib::Bytes::from_static(b"memory"));
        let zip = ImageSource::ArchiveEntry {
            archive_path,
            entry_index: 0,
            entry_name: "page.jpg".into(),
        };

        assert_eq!(load_image_bytes(&file).unwrap().as_ref(), b"file");
        assert_eq!(load_image_bytes(&memory).unwrap().as_ref(), b"memory");
        assert_eq!(load_image_bytes(&zip).unwrap().as_ref(), b"zip");
    }

    #[test]
    fn thumbnail_loader_reuses_an_open_random_access_archive() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("pages.cbz");
        zip_with_entries(
            &archive_path,
            &[("first.jpg", b"first"), ("second.jpg", b"second")],
        );

        let first = ImageSource::ArchiveEntry {
            archive_path: archive_path.clone(),
            entry_index: 0,
            entry_name: "first.jpg".into(),
        };
        let second = ImageSource::ArchiveEntry {
            archive_path,
            entry_index: 1,
            entry_name: "second.jpg".into(),
        };
        let mut loader = ThumbnailImageLoader::new();

        assert_eq!(loader.load_image_bytes(&first).unwrap().as_ref(), b"first");
        assert_eq!(
            loader.load_image_bytes(&second).unwrap().as_ref(),
            b"second"
        );
        assert_eq!(loader.archive_open_count, 1);
    }

    #[test]
    fn thumbnail_loader_reuses_non_solid_seven_zip_metadata_and_source() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("pages.7z");
        seven_zip_with_entries(
            &archive_path,
            &[("first.jpg", b"first"), ("second.jpg", b"second")],
        );
        let first = ImageSource::ArchiveEntry {
            archive_path: archive_path.clone(),
            entry_index: 0,
            entry_name: "first.jpg".into(),
        };
        let second = ImageSource::ArchiveEntry {
            archive_path,
            entry_index: 1,
            entry_name: "second.jpg".into(),
        };
        let mut loader = ThumbnailImageLoader::new();

        assert_eq!(loader.load_image_bytes(&first).unwrap().as_ref(), b"first");
        assert_eq!(
            loader.load_image_bytes(&second).unwrap().as_ref(),
            b"second"
        );
        assert_eq!(loader.archive_open_count, 1);
    }
}
