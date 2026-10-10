use super::formats::{self, CoverEntryBytes};
use super::{
    ArchiveAccessStrategy, ArchiveFormat, ArchiveImageId, archive_access_strategy,
    archive_supports_sequential_progress, is_archive_ext, is_cover_image, is_image_ext,
    is_macos_metadata_path, is_normal_relative_path, sort_paths_naturally,
};
use crate::error::AppError;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

const MAX_NESTED_DEPTH: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArchiveContentItemKind {
    Folder,
    NestedArchive,
    Image { cover_only: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveContentItem {
    pub(crate) path: PathBuf,
    pub(crate) kind: ArchiveContentItemKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ArchiveLocation {
    pub(crate) archive: PathBuf,
    pub(crate) archives: Vec<PathBuf>,
    pub(crate) directory: PathBuf,
}

impl ArchiveLocation {
    pub(crate) fn root(archive: PathBuf) -> Self {
        Self {
            archive,
            archives: Vec::new(),
            directory: PathBuf::new(),
        }
    }

    pub(crate) fn image_id(&self, image: PathBuf) -> ArchiveImageId {
        ArchiveImageId {
            archives: self.archives.clone(),
            image,
        }
    }
}

#[derive(Debug)]
pub(crate) struct ArchiveContentLevel {
    pub(crate) location: ArchiveLocation,
    pub(crate) entries: Vec<PathBuf>,
    pub(crate) items: Vec<ArchiveContentItem>,
    pub(crate) reader: Arc<ArchiveEntryReader>,
}

impl ArchiveContentLevel {
    #[cfg(test)]
    pub(crate) fn open(
        limit: crate::archive::ArchiveExpansionLimit,
        location: ArchiveLocation,
    ) -> Result<Self, AppError> {
        Self::open_with_cancel(limit, location, &Default::default())
    }

    pub(crate) fn open_with_cancel(
        limit: crate::archive::ArchiveExpansionLimit,
        location: ArchiveLocation,
        cancel: &crate::archive::ProgressiveArchiveCancelToken,
    ) -> Result<Self, AppError> {
        if location.archives.len() > MAX_NESTED_DEPTH
            || !location
                .archives
                .iter()
                .all(|path| is_normal_relative_path(path))
            || !valid_directory(&location.directory)
        {
            return Err(AppError::Archive(
                "アーカイブの階層またはパスが不正です".into(),
            ));
        }

        let mut budget = crate::archive::resource::ResourceBudget::for_expansion_limit(limit);
        budget.set_cancel_token(cancel);
        budget.check_cancel()?;
        let (physical_archive, workspace) =
            materialize_archive_chain(&location.archive, &location.archives, &mut budget)?;
        let entries = formats::entry_paths_with_budget(&physical_archive, &mut budget)?
            .into_iter()
            .filter(|path| is_normal_relative_path(path) && !is_macos_metadata_path(path))
            .collect::<Vec<_>>();
        let items = content_items(&entries, &location.directory);
        let mut reader = ArchiveEntryReader::new(physical_archive);
        // Thumbnail jobs retain this reader after navigation. Keep its physical
        // nested archive alive until the last job releases the reader.
        reader._workspace = workspace;
        Ok(Self {
            location,
            entries,
            items,
            reader: Arc::new(reader),
        })
    }

    pub(crate) fn navigate_directory(&mut self, directory: PathBuf) -> bool {
        if !valid_directory(&directory) || !directory_exists(&self.entries, &directory) {
            return false;
        }
        self.location.directory = directory;
        self.items = content_items(&self.entries, &self.location.directory);
        true
    }

    /// Sequential contents loaders can stream non-nested RAR/solid 7z images.
    /// The returned logical paths are in the same natural order as their
    /// `natural_index` notifications, independent of backend order.
    pub(crate) fn progressive_thumbnail_plan(&self) -> Option<(PathBuf, Vec<PathBuf>)> {
        let archive = self.reader.archive.clone();
        if archive_access_strategy(&archive).ok().flatten()
            != Some(ArchiveAccessStrategy::Sequential)
            || !archive_supports_sequential_progress(&archive)
            || self.entries.iter().any(|path| is_archive_ext(path))
        {
            return None;
        }
        let mut images = self
            .entries
            .iter()
            .filter(|path| is_image_ext(path))
            .cloned()
            .collect::<Vec<_>>();
        sort_paths_naturally(&mut images);
        Some((archive, images))
    }
}

#[derive(Debug)]
pub(crate) struct ArchiveEntryReader {
    archive: PathBuf,
    extracted: Mutex<Option<ExtractedArchive>>,
    _workspace: Option<tempfile::TempDir>,
}

#[derive(Debug)]
struct ExtractedArchive {
    directory: tempfile::TempDir,
}

impl ArchiveEntryReader {
    fn new(archive: PathBuf) -> Self {
        Self {
            archive,
            extracted: Mutex::new(None),
            _workspace: None,
        }
    }

    /// Reads one entry directly for random-access backends. Sequential/solid
    /// backends share one retained extraction so concurrent visible-demand
    /// jobs never expand the same archive once per card.
    #[cfg(test)]
    pub(crate) fn read(
        &self,
        limit: crate::archive::ArchiveExpansionLimit,
        entry: &Path,
    ) -> Result<Vec<u8>, AppError> {
        self.read_with_budget(
            entry,
            &mut crate::archive::resource::ResourceBudget::for_expansion_limit(limit),
        )
    }

    pub(crate) fn read_with_cancel(
        &self,
        limit: crate::archive::ArchiveExpansionLimit,
        entry: &Path,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, AppError> {
        let mut budget = crate::archive::resource::ResourceBudget::for_expansion_limit(limit);
        budget.set_cancel_check(cancelled);
        budget.check_cancel()?;
        self.read_with_budget(entry, &mut budget)
    }

    fn read_with_budget(
        &self,
        entry: &Path,
        budget: &mut crate::archive::resource::ResourceBudget,
    ) -> Result<Vec<u8>, AppError> {
        if !is_normal_relative_path(entry) {
            return Err(AppError::Archive("安全でないarchive entryです".into()));
        }
        match formats::cover_entry_bytes(&self.archive, entry, budget)? {
            CoverEntryBytes::Bytes(bytes) => return Ok(bytes),
            CoverEntryBytes::Missing => {
                return Err(AppError::Archive(format!(
                    "archive entryが見つかりません: {}",
                    entry.display()
                )));
            }
            CoverEntryBytes::Unsupported => {}
        }

        let mut extracted = self
            .extracted
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if extracted.is_none() {
            let directory = tempfile::Builder::new()
                .prefix("agnam-archive-content-")
                .tempdir()?;
            formats::extract_to_dir_with_budget(&self.archive, directory.path(), budget)?;
            *extracted = Some(ExtractedArchive { directory });
        }
        let path = extracted
            .as_ref()
            .expect("extracted archive initialized")
            .directory
            .path()
            .join(entry);
        if !path.is_file() {
            return Err(AppError::Archive(format!(
                "archive entryが見つかりません: {}",
                entry.display()
            )));
        }
        budget.preflight(std::fs::metadata(&path)?.len())?;
        Ok(std::fs::read(path)?)
    }
}

fn materialize_archive_chain(
    root: &Path,
    chain: &[PathBuf],
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<(PathBuf, Option<tempfile::TempDir>), AppError> {
    if chain.is_empty() {
        return Ok((root.to_path_buf(), None));
    }
    let workspace = tempfile::Builder::new()
        .prefix("agnam-archive-level-")
        .tempdir()?;
    let mut current = root.to_path_buf();
    for (depth, entry) in chain.iter().enumerate() {
        if depth >= MAX_NESTED_DEPTH || ArchiveFormat::from_path(entry).is_none() {
            return Err(AppError::Archive(
                "アーカイブの階層が深すぎるか形式が不正です".into(),
            ));
        }
        let destination = workspace.path().join(format!("level-{depth}"));
        current = formats::materialize_nested_entry(&current, entry, &destination, budget)?
            .ok_or_else(|| AppError::Archive("Nested archive entry is missing".into()))?;
    }
    Ok((current, Some(workspace)))
}

pub(crate) fn content_items(entries: &[PathBuf], directory: &Path) -> Vec<ArchiveContentItem> {
    let cover_directory = automatic_cover_directory(entries);
    let mut folders = HashSet::new();
    let mut items = Vec::new();
    for path in entries {
        if !is_normal_relative_path(path) || (!is_image_ext(path) && !is_archive_ext(path)) {
            continue;
        }
        let Ok(relative) = path.strip_prefix(directory) else {
            continue;
        };
        let mut components = relative.components();
        let Some(Component::Normal(first)) = components.next() else {
            continue;
        };
        if components.next().is_some() {
            let folder = directory.join(first);
            if folders.insert(folder.clone()) {
                items.push(ArchiveContentItem {
                    path: folder,
                    kind: ArchiveContentItemKind::Folder,
                });
            }
            continue;
        }
        let kind = if is_image_ext(path) {
            ArchiveContentItemKind::Image {
                cover_only: cover_directory.as_deref() == path.parent() && is_cover_image(path),
            }
        } else {
            ArchiveContentItemKind::NestedArchive
        };
        items.push(ArchiveContentItem {
            path: path.clone(),
            kind,
        });
    }
    items.sort_by(|left, right| {
        let left_cover = matches!(
            left.kind,
            ArchiveContentItemKind::Image { cover_only: true }
        );
        let right_cover = matches!(
            right.kind,
            ArchiveContentItemKind::Image { cover_only: true }
        );
        right_cover.cmp(&left_cover).then_with(|| {
            natord::compare(
                &left
                    .path
                    .file_name()
                    .unwrap_or(left.path.as_os_str())
                    .to_string_lossy(),
                &right
                    .path
                    .file_name()
                    .unwrap_or(right.path.as_os_str())
                    .to_string_lossy(),
            )
        })
    });
    items
}

pub(crate) fn automatic_cover_directory(entries: &[PathBuf]) -> Option<PathBuf> {
    let entries = entries
        .iter()
        .filter(|path| is_normal_relative_path(path) && !is_macos_metadata_path(path))
        .collect::<Vec<_>>();
    let mut level = PathBuf::new();
    loop {
        if entries
            .iter()
            .any(|path| path.parent() == Some(level.as_path()) && is_cover_image(path))
        {
            return Some(level);
        }
        let mut child_directories = HashSet::new();
        let mut file_at_level = false;
        for path in &entries {
            let Ok(relative) = path.strip_prefix(&level) else {
                continue;
            };
            let mut components = relative.components();
            let Some(Component::Normal(first)) = components.next() else {
                continue;
            };
            if components.next().is_none() {
                file_at_level = true;
                break;
            }
            child_directories.insert(first.to_os_string());
        }
        if file_at_level || child_directories.len() != 1 {
            return None;
        }
        level.push(child_directories.into_iter().next().unwrap());
    }
}

pub(crate) fn cover_only_paths(entries: &[PathBuf]) -> HashSet<PathBuf> {
    let Some(directory) = automatic_cover_directory(entries) else {
        return HashSet::new();
    };
    entries
        .iter()
        .filter(|path| path.parent() == Some(directory.as_path()) && is_cover_image(path))
        .cloned()
        .collect()
}

fn directory_exists(entries: &[PathBuf], directory: &Path) -> bool {
    directory.as_os_str().is_empty()
        || entries.iter().any(|entry| {
            entry
                .strip_prefix(directory)
                .is_ok_and(|relative| relative.components().next().is_some())
        })
}

fn valid_directory(path: &Path) -> bool {
    path.as_os_str().is_empty() || is_normal_relative_path(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn paths(values: &[&str]) -> Vec<PathBuf> {
        values.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn hierarchy_filters_unsupported_files_and_keeps_required_folders() {
        let entries = paths(&[
            "chapter/10.jpg",
            "chapter/2.jpg",
            "empty/readme.txt",
            "bonus.cbz",
            "memo.txt",
        ]);
        let root = content_items(&entries, Path::new(""));
        assert_eq!(
            root.iter()
                .map(|item| item.path.as_path())
                .collect::<Vec<_>>(),
            [Path::new("bonus.cbz"), Path::new("chapter")]
        );
        let chapter = content_items(&entries, Path::new("chapter"));
        assert_eq!(
            chapter
                .iter()
                .map(|item| item.path.as_path())
                .collect::<Vec<_>>(),
            [Path::new("chapter/2.jpg"), Path::new("chapter/10.jpg")]
        );
    }

    #[test]
    fn root_and_transparent_wrapper_covers_are_cover_only_but_deep_cover_is_not() {
        let root = paths(&["cover.jpg", "001.jpg", "chapter/cover.jpg"]);
        assert_eq!(
            cover_only_paths(&root),
            HashSet::from([PathBuf::from("cover.jpg")])
        );

        let wrapper = paths(&["Book/Cover.PNG", "Book/001.jpg", "Book/deep/cover.jpg"]);
        assert_eq!(
            automatic_cover_directory(&wrapper),
            Some(PathBuf::from("Book"))
        );
        assert_eq!(
            cover_only_paths(&wrapper),
            HashSet::from([PathBuf::from("Book/Cover.PNG")])
        );
        assert!(matches!(
            content_items(&wrapper, Path::new("Book/deep"))[0].kind,
            ArchiveContentItemKind::Image { cover_only: false }
        ));
    }

    #[test]
    fn cover_group_is_first_and_each_group_is_natural() {
        let entries = paths(&["10.jpg", "Cover.PNG", "2.jpg", "cover.jpg", "1.jpg"]);
        let items = content_items(&entries, Path::new(""));
        assert_eq!(
            items
                .iter()
                .map(|item| item.path.as_path())
                .collect::<Vec<_>>(),
            [
                Path::new("Cover.PNG"),
                Path::new("cover.jpg"),
                Path::new("1.jpg"),
                Path::new("2.jpg"),
                Path::new("10.jpg")
            ]
        );
    }

    #[test]
    fn zip_listing_does_not_decode_image_data() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("book.cbz");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive_path).unwrap());
        zip.start_file("chapter/001.jpg", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"not decoded during listing").unwrap();
        zip.start_file("memo.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"memo").unwrap();
        zip.finish().unwrap();

        let level =
            ArchiveContentLevel::open(Default::default(), ArchiveLocation::root(archive_path))
                .unwrap();
        assert_eq!(level.items.len(), 1);
        assert!(matches!(
            level.items[0].kind,
            ArchiveContentItemKind::Folder
        ));
    }

    #[test]
    fn nested_location_keeps_stable_logical_identity() {
        let location = ArchiveLocation {
            archive: PathBuf::from("book.zip"),
            archives: vec![PathBuf::from("volumes/01.cbz")],
            directory: PathBuf::from("chapter"),
        };
        assert_eq!(
            location.image_id(PathBuf::from("chapter/001.jpg")),
            ArchiveImageId {
                archives: vec![PathBuf::from("volumes/01.cbz")],
                image: PathBuf::from("chapter/001.jpg")
            }
        );
    }

    #[test]
    fn materializing_nested_tar_keeps_file_backing_without_a_second_copy() {
        let mut inner = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        inner
            .start_file("page.jpg", zip::write::SimpleFileOptions::default())
            .unwrap();
        inner.write_all(b"image").unwrap();
        let inner = inner.finish().unwrap().into_inner();

        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("outer.tar");
        let mut archive = tar::Builder::new(std::fs::File::create(&archive_path).unwrap());
        let mut header = tar::Header::new_gnu();
        header.set_size(inner.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive
            .append_data(&mut header, "nested.cbz", inner.as_slice())
            .unwrap();
        archive.finish().unwrap();

        let limits = crate::archive::resource::ResourceLimits {
            single_entry: inner.len() as u64,
            cumulative: inner.len() as u64,
            temp_writes: inner.len() as u64 * 2,
            temp_occupancy: inner.len() as u64,
            entries: 4,
            images: 2,
        };
        let mut budget = crate::archive::resource::ResourceBudget::new(limits);
        let (materialized, workspace) =
            materialize_archive_chain(&archive_path, &[PathBuf::from("nested.cbz")], &mut budget)
                .unwrap();
        let workspace = workspace.unwrap();
        assert_eq!(std::fs::read(&materialized).unwrap(), inner);
        assert_eq!(
            budget.temp_counters(),
            (inner.len() as u64, inner.len() as u64)
        );

        let workspace_path = workspace.path().to_path_buf();
        drop(workspace);
        budget.release_temp_tree(&workspace_path).unwrap();
        assert_eq!(budget.temp_counters(), (inner.len() as u64, 0));
    }

    #[test]
    fn thumbnail_reader_keeps_nested_workspace_until_the_last_job_finishes() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("outer.rar");
        let inner = super::super::formats::rar_native::stored_rar(&[("page.png", b"image")]);
        std::fs::write(
            &archive_path,
            super::super::formats::rar_native::stored_rar(&[("inner.rar", &inner)]),
        )
        .unwrap();
        let level = ArchiveContentLevel::open(
            Default::default(),
            ArchiveLocation {
                archive: archive_path,
                archives: vec![PathBuf::from("inner.rar")],
                directory: PathBuf::new(),
            },
        )
        .unwrap();
        let reader = level.reader.clone();
        let physical_archive = reader.archive.clone();
        drop(level);
        assert!(physical_archive.is_file());
        assert_eq!(
            reader
                .read(Default::default(), Path::new("page.png"))
                .unwrap(),
            b"image"
        );
        assert!(matches!(
            reader.read_with_cancel(Default::default(), Path::new("page.png"), &|| true),
            Err(AppError::ArchiveCancelled)
        ));
        drop(reader);
        assert!(!physical_archive.exists());
    }

    #[test]
    fn sequential_progress_plan_separates_physical_entry_order_from_natural_positions() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("solid.7z");
        let entries = [
            ("11.png", b"eleven".to_vec()),
            ("25.png", b"twenty-five".to_vec()),
            ("3.png", b"three".to_vec()),
        ];
        let metadata = entries
            .iter()
            .map(|(name, _)| {
                let mut entry = sevenz_rust::SevenZArchiveEntry::new();
                entry.name = (*name).to_string();
                entry.has_stream = true;
                entry
            })
            .collect::<Vec<_>>();
        let readers = entries
            .into_iter()
            .map(|(_, data)| sevenz_rust::SourceReader::new(std::io::Cursor::new(data)))
            .collect::<Vec<_>>();
        let mut writer = sevenz_rust::SevenZWriter::create(&archive_path).unwrap();
        writer
            .push_archive_entries(metadata, sevenz_rust::SeqReader::new(readers))
            .unwrap();
        writer.finish().unwrap();

        let level =
            ArchiveContentLevel::open(Default::default(), ArchiveLocation::root(archive_path))
                .unwrap();
        let (_, plan) = level.progressive_thumbnail_plan().unwrap();
        assert_eq!(
            plan,
            [
                PathBuf::from("3.png"),
                PathBuf::from("11.png"),
                PathBuf::from("25.png")
            ]
        );
    }

    #[test]
    fn sequential_progress_plan_keeps_cover_card_in_the_backend_index_mapping() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("solid-cover.7z");
        let entries = [
            ("11.png", b"eleven".to_vec()),
            ("cover.png", b"cover".to_vec()),
            ("3.png", b"three".to_vec()),
        ];
        let metadata = entries
            .iter()
            .map(|(name, _)| {
                let mut entry = sevenz_rust::SevenZArchiveEntry::new();
                entry.name = (*name).to_string();
                entry.has_stream = true;
                entry
            })
            .collect::<Vec<_>>();
        let readers = entries
            .into_iter()
            .map(|(_, data)| sevenz_rust::SourceReader::new(std::io::Cursor::new(data)))
            .collect::<Vec<_>>();
        let mut writer = sevenz_rust::SevenZWriter::create(&archive_path).unwrap();
        writer
            .push_archive_entries(metadata, sevenz_rust::SeqReader::new(readers))
            .unwrap();
        writer.finish().unwrap();

        let level =
            ArchiveContentLevel::open(Default::default(), ArchiveLocation::root(archive_path))
                .unwrap();
        let (_, plan) = level.progressive_thumbnail_plan().unwrap();
        assert_eq!(
            plan,
            [
                PathBuf::from("3.png"),
                PathBuf::from("11.png"),
                PathBuf::from("cover.png")
            ]
        );
    }
}
