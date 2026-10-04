use super::{BookshelfDirectory, BookshelfItemMetadata, ChildShelf};
use crate::cache_file::{atomic_write, path_bytes, stable_cache_key};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAGIC: &[u8; 8] = b"AGNAMBS\0";
const FORMAT_VERSION: u32 = 1;
const MAX_SNAPSHOT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_COLLECTION_ITEMS: usize = 1_000_000;
const CACHE_APPLICATION_DIRECTORY: &str = "agnam";
const CACHE_BOOKSHELF_DIRECTORY: &str = "bookshelf";
const CACHE_SNAPSHOT_DIRECTORY: &str = "scan-snapshots";

pub(crate) fn load_root_snapshot(root: &Path) -> Option<BookshelfDirectory> {
    if !std::fs::metadata(root).ok()?.is_dir() {
        return None;
    }
    let _available_root = std::fs::read_dir(root).ok()?;
    BookshelfSnapshotCache::for_user().load(root)
}

pub(crate) fn save_root_snapshot(directory: &BookshelfDirectory) {
    let _ = BookshelfSnapshotCache::for_user().save(directory);
}

#[derive(Debug, Clone)]
struct BookshelfSnapshotCache {
    directory: PathBuf,
}

impl BookshelfSnapshotCache {
    fn for_user() -> Self {
        Self::new(
            glib::user_cache_dir()
                .join(CACHE_APPLICATION_DIRECTORY)
                .join(CACHE_BOOKSHELF_DIRECTORY)
                .join(CACHE_SNAPSHOT_DIRECTORY),
        )
    }

    fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    fn snapshot_path(&self, root: &Path) -> PathBuf {
        self.directory
            .join(format!("{}.snapshot", stable_cache_key(path_bytes(root))))
    }

    fn load(&self, root: &Path) -> Option<BookshelfDirectory> {
        let path = self.snapshot_path(root);
        if std::fs::metadata(&path).ok()?.len() > MAX_SNAPSHOT_BYTES {
            return None;
        }
        decode_snapshot(root, &std::fs::read(path).ok()?)
    }

    fn save(&self, directory: &BookshelfDirectory) -> std::io::Result<()> {
        let bytes = encode_snapshot(directory)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_SNAPSHOT_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "bookshelf snapshot is too large",
            ));
        }
        atomic_write(&self.snapshot_path(&directory.path), &bytes)
    }
}

fn encode_snapshot(directory: &BookshelfDirectory) -> std::io::Result<Vec<u8>> {
    let mut writer = SnapshotWriter::default();
    writer.bytes.extend_from_slice(MAGIC);
    writer.u32(FORMAT_VERSION);
    writer.path(&directory.path)?;
    writer.bool(directory.is_image_document_directory);
    writer.paths(&directory.direct_files)?;

    writer.len(directory.direct_file_metadata.len())?;
    for (path, metadata) in &directory.direct_file_metadata {
        writer.path(path)?;
        writer.metadata(*metadata);
    }

    writer.len(directory.child_shelves.len())?;
    for child in &directory.child_shelves {
        writer.path(&child.path)?;
        writer.paths(&child.preview_candidates)?;
        writer.u64(u64::try_from(child.preview_item_count).map_err(invalid_data)?);
        writer.optional_path(child.image_document_entry.as_deref())?;
        writer.optional_path(child.image_document_cover.as_deref())?;
        writer.metadata(child.metadata);
    }
    Ok(writer.bytes)
}

fn decode_snapshot(root: &Path, bytes: &[u8]) -> Option<BookshelfDirectory> {
    let mut reader = SnapshotReader::new(bytes);
    if reader.take(MAGIC.len())? != MAGIC || reader.u32()? != FORMAT_VERSION {
        return None;
    }
    let path = reader.path()?;
    if path != root {
        return None;
    }
    let is_image_document_directory = reader.bool()?;
    let direct_files = reader.paths()?;

    let mut direct_file_metadata = HashMap::new();
    for _ in 0..reader.len()? {
        direct_file_metadata.insert(reader.path()?, reader.metadata()?);
    }

    let mut child_shelves = Vec::new();
    for _ in 0..reader.len()? {
        child_shelves.push(ChildShelf {
            path: reader.path()?,
            preview_candidates: reader.paths()?,
            preview_item_count: usize::try_from(reader.u64()?).ok()?,
            image_document_entry: reader.optional_path()?,
            image_document_cover: reader.optional_path()?,
            metadata: reader.metadata()?,
        });
    }
    if !reader.is_finished() {
        return None;
    }
    let directory = BookshelfDirectory {
        path,
        is_image_document_directory,
        direct_files,
        direct_file_metadata,
        child_shelves,
    };
    snapshot_paths_are_valid(root, &directory).then_some(directory)
}

fn snapshot_paths_are_valid(root: &Path, directory: &BookshelfDirectory) -> bool {
    let within_root = |path: &Path| {
        path.strip_prefix(root).is_ok_and(|relative| {
            relative.components().all(|component| {
                matches!(
                    component,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            })
        })
    };
    directory.path == root
        && directory
            .direct_files
            .iter()
            .all(|path| within_root(path) && path.parent().is_some_and(|parent| parent == root))
        && directory.direct_file_metadata.len() == directory.direct_files.len()
        && directory
            .direct_files
            .iter()
            .all(|path| directory.direct_file_metadata.contains_key(path))
        && directory.child_shelves.iter().all(|child| {
            within_root(&child.path)
                && child.path.parent().is_some_and(|parent| parent == root)
                && child
                    .preview_candidates
                    .iter()
                    .all(|path| within_root(path) && path.starts_with(&child.path))
                && child.image_document_entry.as_deref().is_none_or(|path| {
                    within_root(path) && path.parent().is_some_and(|parent| parent == child.path)
                })
                && child.image_document_cover.as_deref().is_none_or(|path| {
                    within_root(path) && path.parent().is_some_and(|parent| parent == child.path)
                })
        })
}

#[derive(Default)]
struct SnapshotWriter {
    bytes: Vec<u8>,
}

impl SnapshotWriter {
    fn bool(&mut self, value: bool) {
        self.bytes.push(u8::from(value));
    }

    fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn len(&mut self, value: usize) -> std::io::Result<()> {
        self.u32(u32::try_from(value).map_err(invalid_data)?);
        Ok(())
    }

    fn path(&mut self, path: &Path) -> std::io::Result<()> {
        let bytes = path_bytes(path);
        self.len(bytes.len())?;
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn paths(&mut self, paths: &[PathBuf]) -> std::io::Result<()> {
        self.len(paths.len())?;
        for path in paths {
            self.path(path)?;
        }
        Ok(())
    }

    fn optional_path(&mut self, path: Option<&Path>) -> std::io::Result<()> {
        self.bool(path.is_some());
        if let Some(path) = path {
            self.path(path)?;
        }
        Ok(())
    }

    fn metadata(&mut self, metadata: BookshelfItemMetadata) {
        self.time(metadata.modified);
        self.time(metadata.created);
    }

    fn time(&mut self, time: Option<SystemTime>) {
        self.bool(time.is_some());
        let Some(time) = time else {
            return;
        };
        let (before_epoch, duration) = match time.duration_since(UNIX_EPOCH) {
            Ok(duration) => (false, duration),
            Err(error) => (true, error.duration()),
        };
        self.bool(before_epoch);
        self.u64(duration.as_secs());
        self.u32(duration.subsec_nanos());
    }
}

struct SnapshotReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> SnapshotReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.position.checked_add(len)?;
        let bytes = self.bytes.get(self.position..end)?;
        self.position = end;
        Some(bytes)
    }

    fn bool(&mut self) -> Option<bool> {
        match self.take(1)?.first()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn len(&mut self) -> Option<usize> {
        let len = usize::try_from(self.u32()?).ok()?;
        (len <= MAX_COLLECTION_ITEMS).then_some(len)
    }

    fn path(&mut self) -> Option<PathBuf> {
        let len = self.len()?;
        path_from_bytes(self.take(len)?)
    }

    fn paths(&mut self) -> Option<Vec<PathBuf>> {
        let len = self.len()?;
        let mut paths = Vec::with_capacity(len.min(1024));
        for _ in 0..len {
            paths.push(self.path()?);
        }
        Some(paths)
    }

    fn optional_path(&mut self) -> Option<Option<PathBuf>> {
        if self.bool()? {
            Some(Some(self.path()?))
        } else {
            Some(None)
        }
    }

    fn metadata(&mut self) -> Option<BookshelfItemMetadata> {
        Some(BookshelfItemMetadata {
            modified: self.time()?,
            created: self.time()?,
        })
    }

    fn time(&mut self) -> Option<Option<SystemTime>> {
        if !self.bool()? {
            return Some(None);
        }
        let before_epoch = self.bool()?;
        let duration = Duration::new(self.u64()?, self.u32()?);
        let time = if before_epoch {
            UNIX_EPOCH.checked_sub(duration)?
        } else {
            UNIX_EPOCH.checked_add(duration)?
        };
        Some(Some(time))
    }

    fn is_finished(&self) -> bool {
        self.position == self.bytes.len()
    }
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(OsString::from_vec(bytes.to_vec())))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> Option<PathBuf> {
    Some(PathBuf::from(String::from_utf8(bytes.to_vec()).ok()?))
}

fn invalid_data(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(root: &Path) -> BookshelfDirectory {
        let direct = root.join("本 01.cbz");
        let child = root.join("作品");
        let metadata = BookshelfItemMetadata {
            modified: Some(UNIX_EPOCH + Duration::new(123, 456)),
            created: Some(UNIX_EPOCH - Duration::new(12, 34)),
        };
        BookshelfDirectory {
            path: root.to_path_buf(),
            is_image_document_directory: false,
            direct_files: vec![direct.clone()],
            direct_file_metadata: HashMap::from([(direct, metadata)]),
            child_shelves: vec![ChildShelf {
                path: child.clone(),
                preview_candidates: vec![child.join("01.zip")],
                preview_item_count: 7,
                image_document_entry: Some(child.join("001.jpg")),
                image_document_cover: Some(child.join("Cover.png")),
                metadata,
            }],
        }
    }

    #[test]
    fn snapshot_round_trip_preserves_scan_equality_data() {
        let root = Path::new("/mnt/本棚");
        let snapshot = sample(root);
        let bytes = encode_snapshot(&snapshot).unwrap();

        assert_eq!(decode_snapshot(root, &bytes), Some(snapshot));
    }

    #[test]
    fn root_mismatch_is_a_cache_miss() {
        let snapshot = sample(Path::new("/mnt/books-a"));
        let bytes = encode_snapshot(&snapshot).unwrap();

        assert!(decode_snapshot(Path::new("/mnt/books-b"), &bytes).is_none());
    }

    #[test]
    fn version_mismatch_is_a_cache_miss() {
        let root = Path::new("/mnt/books");
        let mut bytes = encode_snapshot(&sample(root)).unwrap();
        bytes[MAGIC.len()..MAGIC.len() + 4].copy_from_slice(&999_u32.to_le_bytes());

        assert!(decode_snapshot(root, &bytes).is_none());
    }

    #[test]
    fn corrupt_snapshot_is_a_cache_miss() {
        let root = Path::new("/mnt/books");
        let bytes = encode_snapshot(&sample(root)).unwrap();

        for corrupt in [&b"broken"[..], &bytes[..bytes.len() / 2]] {
            assert!(decode_snapshot(root, corrupt).is_none());
        }
    }

    #[test]
    fn cache_round_trip_and_unavailable_root_miss() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("books");
        std::fs::create_dir(&root).unwrap();
        let cache = BookshelfSnapshotCache::new(directory.path().join("cache/scan-snapshots"));
        let snapshot = sample(&root);

        cache.save(&snapshot).unwrap();
        assert_eq!(cache.load(&root), Some(snapshot));
        std::fs::remove_dir(&root).unwrap();
        assert!(load_root_snapshot(&root).is_none());
    }
}
