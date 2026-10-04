use super::{AssetThumbnails, THUMBNAIL_BYTES_PER_PIXEL, THUMBNAIL_HEIGHT, Thumbnail};
use crate::cache_file::{atomic_write, path_bytes, stable_cache_key};
use crate::document::{ImageAsset, ImageLayout, ImageSource};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, mpsc};
use std::time::{SystemTime, UNIX_EPOCH};

const VERSION: u32 = 1;
const MAGIC: &[u8; 8] = b"AGNTHMB1";
const LIMIT: u64 = 1024 * 1024 * 1024;
const MAX_FILE: u64 = 16 * 1024 * 1024;
const CLEANUP_INTERVAL: u64 = 16 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct ThumbnailDiskCache {
    root: PathBuf,
    document: PathBuf,
}

pub(super) struct CacheEntry {
    path: PathBuf,
    key: Vec<u8>,
    source: PathBuf,
    size: u64,
    modified: SystemTime,
    source_tag: Vec<u8>,
    layout: ImageLayout,
}

impl CacheEntry {
    fn source_is_current(&self) -> bool {
        fs::metadata(&self.source).is_ok_and(|metadata| {
            metadata.len() == self.size
                && metadata.modified().ok() == Some(self.modified)
                && source_tag(&metadata) == self.source_tag
        })
    }
}

enum WriteJob {
    Open(PathBuf, PathBuf),
    Save(CacheEntry, AssetThumbnails),
    #[cfg(test)]
    Flush(mpsc::Sender<()>),
}

fn writer() -> &'static mpsc::SyncSender<WriteJob> {
    static WRITER: OnceLock<mpsc::SyncSender<WriteJob>> = OnceLock::new();
    WRITER.get_or_init(|| {
        let (send, receive) = mpsc::sync_channel(16);
        std::thread::Builder::new()
            .name("slider-thumbnail-cache".into())
            .spawn(move || {
                let mut written = 0;
                let mut active = None;
                for job in receive {
                    match job {
                        WriteJob::Open(root, group) => {
                            if group.is_dir() {
                                let _ = atomic_write(&group.join(".last-used"), b"1");
                            }
                            active = Some(group.clone());
                            cleanup(&root, LIMIT, Some(&group));
                            written = 0;
                        }
                        WriteJob::Save(entry, thumbnails) => {
                            if let Ok(bytes) = save(&entry, &thumbnails) {
                                written += bytes;
                            }
                            if written >= CLEANUP_INTERVAL {
                                if let Some(root) = entry.path.parent().and_then(Path::parent) {
                                    cleanup(root, LIMIT, active.as_deref());
                                }
                                written = 0;
                            }
                        }
                        #[cfg(test)]
                        WriteJob::Flush(done) => {
                            let _ = done.send(());
                        }
                    }
                }
            })
            .expect("failed to start Slider thumbnail cache writer");
        send
    })
}

impl ThumbnailDiskCache {
    pub(super) fn for_document(document: &Path) -> Self {
        Self::new(
            glib::user_cache_dir()
                .join("agnam")
                .join("slider-thumbnails"),
            document,
        )
    }

    pub(super) fn new(root: PathBuf, document: &Path) -> Self {
        Self {
            root,
            document: document.to_path_buf(),
        }
    }

    pub(super) fn opened(&self, is_archive: bool) {
        let group = self.group(if is_archive {
            &self.document
        } else {
            self.document.parent().unwrap_or(&self.document)
        });
        let _ = writer().send(WriteJob::Open(self.root.clone(), group));
    }

    fn group(&self, path: &Path) -> PathBuf {
        self.root.join(stable_cache_key(path_bytes(path)))
    }

    pub(super) fn entry(&self, asset: &ImageAsset, physical_index: usize) -> Option<CacheEntry> {
        match &asset.source {
            ImageSource::File(path) if asset.archive_identity.is_none() => {
                self.make_entry(path, path.parent()?, path_bytes(path), asset.layout)
            }
            ImageSource::ArchiveEntry {
                archive_path,
                entry_index,
                entry_name,
            } if archive_path == &self.document => {
                let mut identity = Vec::new();
                field(&mut identity, b"random");
                field(&mut identity, &entry_index.to_le_bytes());
                field(&mut identity, entry_name.as_bytes());
                self.make_entry(archive_path, archive_path, &identity, asset.layout)
            }
            ImageSource::File(_) | ImageSource::Memory(_) => {
                let identity = asset.archive_identity.as_ref()?;
                let mut bytes = Vec::new();
                if identity.archives.is_empty() {
                    field(&mut bytes, b"sequential");
                    field(&mut bytes, &physical_index.to_le_bytes());
                } else {
                    field(&mut bytes, b"nested");
                    for archive in &identity.archives {
                        field(&mut bytes, path_bytes(archive));
                    }
                    field(&mut bytes, path_bytes(&identity.image));
                    field(&mut bytes, &physical_index.to_le_bytes());
                }
                self.make_entry(&self.document, &self.document, &bytes, asset.layout)
            }
            _ => None,
        }
    }

    // Progressive images have no entry name yet. The archive's natural physical
    // index is stable across Memory -> File spill and matches the completed
    // Sequential Document's asset order.
    pub(super) fn progressive_entry(
        &self,
        physical_index: usize,
        layout: ImageLayout,
    ) -> Option<CacheEntry> {
        let mut identity = Vec::new();
        field(&mut identity, b"sequential");
        field(&mut identity, &physical_index.to_le_bytes());
        self.make_entry(&self.document, &self.document, &identity, layout)
    }

    fn make_entry(
        &self,
        source: &Path,
        group: &Path,
        identity: &[u8],
        layout: ImageLayout,
    ) -> Option<CacheEntry> {
        let metadata = fs::metadata(source).ok()?;
        let modified = metadata.modified().ok()?;
        let nanos = modified.duration_since(UNIX_EPOCH).ok()?.as_nanos();
        let source_tag = source_tag(&metadata);
        let mut key = Vec::new();
        field(&mut key, &VERSION.to_le_bytes());
        field(&mut key, path_bytes(source));
        field(&mut key, &metadata.len().to_le_bytes());
        field(&mut key, &nanos.to_le_bytes());
        field(&mut key, &source_tag);
        field(&mut key, identity);
        field(&mut key, &[u8::from(layout == ImageLayout::Spread)]);
        let path = self
            .group(group)
            .join(format!("{}.rgb", stable_cache_key(&key)));
        Some(CacheEntry {
            path,
            key,
            source: source.to_path_buf(),
            size: metadata.len(),
            modified,
            source_tag,
            layout,
        })
    }

    pub(super) fn load(&self, entry: &CacheEntry) -> Option<AssetThumbnails> {
        let len = fs::metadata(&entry.path).ok()?.len();
        if len > MAX_FILE {
            return None;
        }
        let thumbnails = decode(&fs::read(&entry.path).ok()?, &entry.key)?;
        if !entry.source_is_current() {
            return None;
        }
        match (&thumbnails, entry.layout) {
            (AssetThumbnails::Single(_), ImageLayout::Single)
            | (AssetThumbnails::Spread { .. }, ImageLayout::Spread) => Some(thumbnails),
            _ => None,
        }
    }

    pub(super) fn save_later(&self, entry: CacheEntry, thumbnails: AssetThumbnails) {
        let _ = writer().try_send(WriteJob::Save(entry, thumbnails));
    }

    #[cfg(test)]
    pub(super) fn flush_writes(&self) {
        let (done, receive) = mpsc::channel();
        writer().send(WriteJob::Flush(done)).unwrap();
        receive.recv().unwrap();
    }

    #[cfg(test)]
    pub(super) fn save_now(
        &self,
        entry: &CacheEntry,
        thumbnails: &AssetThumbnails,
    ) -> std::io::Result<u64> {
        save(entry, thumbnails)
    }
}

fn field(output: &mut Vec<u8>, bytes: &[u8]) {
    output.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    output.extend_from_slice(bytes);
}

#[cfg(unix)]
fn source_tag(metadata: &fs::Metadata) -> Vec<u8> {
    use std::os::unix::fs::MetadataExt;
    let mut tag = Vec::with_capacity(32);
    tag.extend_from_slice(&metadata.dev().to_le_bytes());
    tag.extend_from_slice(&metadata.ino().to_le_bytes());
    tag.extend_from_slice(&metadata.ctime().to_le_bytes());
    tag.extend_from_slice(&metadata.ctime_nsec().to_le_bytes());
    tag
}

#[cfg(not(unix))]
fn source_tag(metadata: &fs::Metadata) -> Vec<u8> {
    metadata
        .created()
        .ok()
        .and_then(|created| created.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_nanos().to_le_bytes().to_vec())
        .unwrap_or_default()
}

fn save(entry: &CacheEntry, thumbnails: &AssetThumbnails) -> std::io::Result<u64> {
    if !entry.source_is_current() {
        return Ok(0);
    }
    let Some(bytes) = encode(&entry.key, thumbnails) else {
        return Ok(0);
    };
    atomic_write(&entry.path, &bytes)?;
    Ok(bytes.len() as u64)
}

fn encode(key: &[u8], thumbnails: &AssetThumbnails) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&u32::try_from(key.len()).ok()?.to_le_bytes());
    bytes.extend_from_slice(key);
    let parts: &[&Thumbnail] = match thumbnails {
        AssetThumbnails::Single(thumbnail) => &[thumbnail],
        AssetThumbnails::Spread { right, left } => &[right, left],
    };
    bytes.push(u8::try_from(parts.len()).ok()?);
    for part in parts {
        let width = u32::try_from(part.width).ok()?;
        let height = u32::try_from(part.height).ok()?;
        let stride = (width as usize).checked_mul(THUMBNAIL_BYTES_PER_PIXEL)?;
        let expected = stride.checked_mul(height as usize)?;
        if width == 0
            || height == 0
            || height > THUMBNAIL_HEIGHT
            || part.stride != stride
            || part.pixels.len() != expected
        {
            return None;
        }
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&u32::try_from(expected).ok()?.to_le_bytes());
        bytes.extend_from_slice(&part.pixels);
    }
    if bytes.len() + 32 > MAX_FILE as usize {
        return None;
    }
    let checksum = stable_cache_key(&bytes);
    bytes.extend_from_slice(checksum.as_bytes());
    Some(bytes)
}

fn decode(bytes: &[u8], key: &[u8]) -> Option<AssetThumbnails> {
    let body_len = bytes.len().checked_sub(32)?;
    if stable_cache_key(&bytes[..body_len]).as_bytes() != &bytes[body_len..] {
        return None;
    }
    let mut input = &bytes[..body_len];
    if take(&mut input, 8)? != MAGIC
        || u32::from_le_bytes(take(&mut input, 4)?.try_into().ok()?) != VERSION
    {
        return None;
    }
    let key_len = u32::from_le_bytes(take(&mut input, 4)?.try_into().ok()?) as usize;
    if take(&mut input, key_len)? != key {
        return None;
    }
    let count = *take(&mut input, 1)?.first()?;
    if count != 1 && count != 2 {
        return None;
    }
    let mut parts = Vec::new();
    for _ in 0..count {
        let width = u32::from_le_bytes(take(&mut input, 4)?.try_into().ok()?);
        let height = u32::from_le_bytes(take(&mut input, 4)?.try_into().ok()?);
        let length = u32::from_le_bytes(take(&mut input, 4)?.try_into().ok()?) as usize;
        let stride = (width as usize).checked_mul(THUMBNAIL_BYTES_PER_PIXEL)?;
        let expected = stride.checked_mul(height as usize)?;
        if width == 0 || height == 0 || height > THUMBNAIL_HEIGHT || length != expected {
            return None;
        }
        parts.push(Thumbnail {
            width: i32::try_from(width).ok()?,
            height: i32::try_from(height).ok()?,
            stride,
            pixels: take(&mut input, length)?.to_vec(),
        });
    }
    if !input.is_empty() {
        return None;
    }
    match parts.len() {
        1 => Some(AssetThumbnails::Single(parts.remove(0))),
        2 => Some(AssetThumbnails::Spread {
            right: parts.remove(0),
            left: parts.remove(0),
        }),
        _ => None,
    }
}

fn take<'a>(input: &mut &'a [u8], length: usize) -> Option<&'a [u8]> {
    if length > input.len() {
        return None;
    }
    let (head, tail) = input.split_at(length);
    *input = tail;
    Some(head)
}

fn cleanup(root: &Path, limit: u64, active: Option<&Path>) {
    let Ok(groups) = fs::read_dir(root) else {
        return;
    };
    let mut total = 0u64;
    let mut candidates = Vec::new();
    for group in groups.flatten() {
        let path = group.path();
        if !group.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(files) = fs::read_dir(&path) else {
            continue;
        };
        let size = files
            .flatten()
            .filter_map(|file| file.metadata().ok())
            .filter(|metadata| metadata.is_file())
            .map(|metadata| metadata.len())
            .fold(0u64, u64::saturating_add);
        total = total.saturating_add(size);
        let used = fs::metadata(path.join(".last-used"))
            .or_else(|_| fs::metadata(&path))
            .and_then(|metadata| metadata.modified())
            .unwrap_or(UNIX_EPOCH);
        candidates.push((used, path, size));
    }
    candidates.sort_by_key(|(used, _, _)| *used);
    for (_, path, size) in &candidates {
        if total <= limit {
            break;
        }
        if active == Some(path.as_path()) {
            continue;
        }
        if fs::remove_dir_all(path).is_ok() {
            total = total.saturating_sub(*size);
        }
    }
    if total > limit
        && let Some(active) = active
        && let Some((_, _, size)) = candidates.iter().find(|(_, path, _)| path == active)
        && fs::remove_dir_all(active).is_ok()
    {
        let _ = total.saturating_sub(*size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{ArchiveAssetIdentity, ImageLayout, PagePart};
    use std::io::Cursor;
    use std::time::Duration;

    fn thumbnail(red: u8) -> Thumbnail {
        Thumbnail {
            width: 3,
            height: 2,
            stride: 9,
            pixels: vec![red; 18],
        }
    }

    fn direct(path: &Path) -> ImageAsset {
        ImageAsset {
            source: ImageSource::File(path.to_path_buf()),
            first_page: 0,
            layout: ImageLayout::Single,
            archive_identity: None,
        }
    }

    fn sequential(source: ImageSource, image: &str) -> ImageAsset {
        ImageAsset {
            source,
            first_page: 0,
            layout: ImageLayout::Spread,
            archive_identity: Some(ArchiveAssetIdentity {
                archives: Vec::new(),
                image: image.into(),
            }),
        }
    }

    #[test]
    fn direct_file_round_trip_source_changes_and_missing_directory() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("pages/one.png");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, b"source").unwrap();
        let cache = ThumbnailDiskCache::new(temp.path().join("cache"), &source);
        let asset = direct(&source);
        let entry = cache.entry(&asset, 0).unwrap();
        assert!(cache.load(&entry).is_none());
        let original = AssetThumbnails::Single(thumbnail(42));
        cache.save_now(&entry, &original).unwrap();
        assert_eq!(cache.load(&entry), Some(original));
        assert_eq!(cache.entry(&asset, 9).unwrap().path, entry.path);

        let modified = UNIX_EPOCH + Duration::from_secs(10);
        fs::File::open(&source)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert_ne!(cache.entry(&asset, 0).unwrap().path, entry.path);
        assert!(cache.load(&cache.entry(&asset, 0).unwrap()).is_none());
        fs::write(&source, b"longer source").unwrap();
        assert_ne!(cache.entry(&asset, 0).unwrap().path, entry.path);
        assert!(cache.load(&cache.entry(&asset, 0).unwrap()).is_none());

        fs::remove_dir_all(&cache.root).unwrap();
        let current = cache.entry(&asset, 0).unwrap();
        assert!(cache.load(&current).is_none());
        cache
            .save_now(&current, &AssetThumbnails::Single(thumbnail(99)))
            .unwrap();
        assert!(cache.load(&current).is_some());
    }

    #[test]
    fn spread_is_one_atomic_file_and_corruption_is_a_miss() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("book.cbr");
        fs::write(&archive, b"archive").unwrap();
        let cache = ThumbnailDiskCache::new(temp.path().join("cache"), &archive);
        let asset = sequential(
            ImageSource::Memory(glib::Bytes::from_static(b"image")),
            "a.png",
        );
        let entry = cache.entry(&asset, 0).unwrap();
        let image = image::RgbImage::from_fn(5, 2, |x, _| image::Rgb([x as u8, 0, 0]));
        let original = AssetThumbnails::Spread {
            right: super::super::thumbnail_for_part(&image, PagePart::Right).unwrap(),
            left: super::super::thumbnail_for_part(&image, PagePart::Left).unwrap(),
        };
        cache.save_now(&entry, &original).unwrap();
        assert_eq!(cache.load(&entry), Some(original.clone()));
        let AssetThumbnails::Spread { right, left } = original else {
            unreachable!()
        };
        assert_eq!(right.pixels[0], 2);
        assert_eq!(left.pixels[0], 0);

        let mut bytes = fs::read(&entry.path).unwrap();
        bytes[8..12].copy_from_slice(&999u32.to_le_bytes());
        let checksum = stable_cache_key(&bytes[..bytes.len() - 32]);
        let end = bytes.len() - 32;
        bytes[end..].copy_from_slice(checksum.as_bytes());
        fs::write(&entry.path, &bytes).unwrap();
        assert!(cache.load(&entry).is_none());
        fs::write(&entry.path, b"broken").unwrap();
        assert!(cache.load(&entry).is_none());
        fs::remove_file(&entry.path).unwrap();
        fs::write(entry.path.with_extension("rgb.tmp-1"), b"partial").unwrap();
        assert!(cache.load(&entry).is_none());
        cache
            .save_now(&entry, &AssetThumbnails::Spread { right, left })
            .unwrap();
        assert!(cache.load(&entry).is_some());
    }

    #[test]
    fn archive_identity_separates_entries_and_ignores_temporary_backing() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("book.cb7");
        fs::write(&archive, b"archive").unwrap();
        let cache = ThumbnailDiskCache::new(temp.path().join("cache"), &archive);
        let random = |index, name: &str| ImageAsset {
            source: ImageSource::ArchiveEntry {
                archive_path: archive.clone(),
                entry_index: index,
                entry_name: name.into(),
            },
            first_page: 0,
            layout: ImageLayout::Single,
            archive_identity: Some(ArchiveAssetIdentity {
                archives: Vec::new(),
                image: name.into(),
            }),
        };
        assert_ne!(
            cache.entry(&random(0, "a.png"), 0).unwrap().path,
            cache.entry(&random(1, "a.png"), 1).unwrap().path
        );
        let memory = sequential(
            ImageSource::Memory(glib::Bytes::from_static(b"image")),
            "a.png",
        );
        let file = sequential(
            ImageSource::File(temp.path().join("random-temp/image.bin")),
            "a.png",
        );
        let progressive = cache.progressive_entry(0, ImageLayout::Spread).unwrap();
        assert_eq!(cache.entry(&memory, 0).unwrap().path, progressive.path);
        assert_eq!(cache.entry(&file, 0).unwrap().path, progressive.path);
        assert_ne!(cache.entry(&memory, 1).unwrap().path, progressive.path);
        let mut nested = file;
        nested.archive_identity.as_mut().unwrap().archives = vec!["inner.cbz".into()];
        assert_ne!(cache.entry(&nested, 0).unwrap().path, progressive.path);
        let first_nested = cache.entry(&nested, 0).unwrap().path;
        nested.source = ImageSource::File(temp.path().join("another-temp/image.bin"));
        assert_eq!(cache.entry(&nested, 0).unwrap().path, first_nested);
        nested.archive_identity.as_mut().unwrap().image = "other.png".into();
        assert_ne!(cache.entry(&nested, 0).unwrap().path, first_nested);
    }

    #[cfg(unix)]
    #[test]
    fn archive_replacement_with_same_size_and_mtime_is_a_miss() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("book.cbz");
        fs::write(&archive, b"original").unwrap();
        let modified = fs::metadata(&archive).unwrap().modified().unwrap();
        let cache = ThumbnailDiskCache::new(temp.path().join("cache"), &archive);
        let asset = ImageAsset {
            source: ImageSource::ArchiveEntry {
                archive_path: archive.clone(),
                entry_index: 0,
                entry_name: "page.png".into(),
            },
            first_page: 0,
            layout: ImageLayout::Single,
            archive_identity: None,
        };
        let before = cache.entry(&asset, 0).unwrap();
        cache
            .save_now(&before, &AssetThumbnails::Single(thumbnail(5)))
            .unwrap();
        let replacement = temp.path().join("replacement.cbz");
        fs::write(&replacement, b"replaced").unwrap();
        fs::File::open(&replacement)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        fs::rename(replacement, &archive).unwrap();
        assert_ne!(cache.entry(&asset, 0).unwrap().path, before.path);
        assert!(cache.load(&before).is_none());
        assert!(cache.load(&cache.entry(&asset, 0).unwrap()).is_none());
        assert_eq!(
            cache
                .save_now(&before, &AssetThumbnails::Single(thumbnail(6)))
                .unwrap(),
            0
        );
    }

    #[test]
    fn unsupported_memory_without_archive_identity_and_write_failure_are_safe() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("image.png");
        fs::write(&source, b"source").unwrap();
        let blocked = temp.path().join("blocked");
        fs::write(&blocked, b"file").unwrap();
        let cache = ThumbnailDiskCache::new(blocked.join("cache"), &source);
        assert!(
            cache
                .entry(
                    &ImageAsset {
                        source: ImageSource::Memory(glib::Bytes::from_static(b"image")),
                        first_page: 0,
                        layout: ImageLayout::Single,
                        archive_identity: None,
                    },
                    0
                )
                .is_none()
        );
        let entry = cache.entry(&direct(&source), 0).unwrap();
        let generated = AssetThumbnails::Single(thumbnail(7));
        assert!(cache.save_now(&entry, &generated).is_err());
        assert_eq!(generated, AssetThumbnails::Single(thumbnail(7)));
    }

    #[test]
    fn cleanup_evicts_old_groups_and_recovers_partial_groups() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("cache");
        let old = root.join("old");
        let new = root.join("new");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(old.join("broken.rgb"), vec![1; 20]).unwrap();
        fs::write(new.join("page.rgb"), vec![2; 20]).unwrap();
        fs::write(old.join(".last-used"), b"bad metadata").unwrap();
        fs::write(new.join(".last-used"), b"also bad").unwrap();
        fs::File::open(old.join(".last-used"))
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(10)))
            .unwrap();
        fs::File::open(new.join(".last-used"))
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(20)))
            .unwrap();
        cleanup(&root, 100, None);
        assert!(old.exists() && new.exists());
        let real_size = [
            old.join("broken.rgb"),
            old.join(".last-used"),
            new.join("page.rgb"),
            new.join(".last-used"),
        ]
        .iter()
        .map(|path| fs::metadata(path).unwrap().len())
        .sum::<u64>();
        assert!(real_size > 40 && real_size <= 100);
        cleanup(&root, 40, None);
        assert!(!old.exists() && new.exists());
        let remaining_size = fs::metadata(new.join("page.rgb")).unwrap().len()
            + fs::metadata(new.join(".last-used")).unwrap().len();
        assert!(remaining_size <= 40);
        cleanup(&root, 1, Some(&new));
        assert!(!new.exists());
        cleanup(&root, 1, None);
        cleanup(&root.join("missing"), 1, None);
    }

    #[test]
    #[ignore = "performance measurement; run with --ignored --nocapture"]
    fn measure_jpeg_png_webp_first_generation_write_and_second_hit() {
        let temp = tempfile::tempdir().unwrap();
        let fixture = image::RgbImage::from_fn(900, 1300, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 239) as u8, ((x + y) % 227) as u8])
        });
        for (format, extension) in [
            (image::ImageFormat::Jpeg, "jpg"),
            (image::ImageFormat::Png, "png"),
            (image::ImageFormat::WebP, "webp"),
        ] {
            let source = temp.path().join(format!("page.{extension}"));
            let mut encoded = Cursor::new(Vec::new());
            image::DynamicImage::ImageRgb8(fixture.clone())
                .write_to(&mut encoded, format)
                .unwrap();
            fs::write(&source, encoded.into_inner()).unwrap();
            let cache = ThumbnailDiskCache::new(temp.path().join("cache"), &source);
            let miss_started = std::time::Instant::now();
            let entry = cache.entry(&direct(&source), 0).unwrap();
            assert!(cache.load(&entry).is_none());
            let miss = miss_started.elapsed();
            let first = std::time::Instant::now();
            let source_bytes = fs::read(&source).unwrap();
            let mut decompressor = turbojpeg::Decompressor::new().unwrap();
            let generated = super::super::make_asset_thumbnails_from_bytes(
                &mut decompressor,
                &source_bytes,
                ImageLayout::Single,
            )
            .unwrap();
            let generation = first.elapsed();
            let write = std::time::Instant::now();
            let cache_bytes = cache.save_now(&entry, &generated).unwrap();
            let write_time = write.elapsed();
            let second = std::time::Instant::now();
            assert_eq!(cache.load(&entry), Some(generated));
            let hit = second.elapsed();
            eprintln!(
                "{extension}: miss={miss:?} first={generation:?} write={write_time:?} second={hit:?} source_reads=1 decodes=1 cache_reads=1 cache_writes=1 cache_bytes={cache_bytes}"
            );
        }
    }
}
