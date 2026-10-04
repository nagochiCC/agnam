use super::thumbnail::{
    COVER_MAX_HEIGHT, COVER_MAX_WIDTH, ThumbnailData, ThumbnailGenerationError,
    generate_from_bytes_with_cancel,
};
use crate::archive::ArchiveFormat;
use crate::cache_file::{atomic_write, encode_hex, stable_cache_key};
use crate::covers::{CoverSource, CoverSourceLoadError, CoverStore};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const CACHE_FORMAT_VERSION: u32 = 3;
const CACHE_APPLICATION_DIRECTORY: &str = "agnam";
const CACHE_BOOKSHELF_DIRECTORY: &str = "bookshelf";
const CACHE_AUTO_DIRECTORY: &str = "auto";

#[derive(Debug, thiserror::Error)]
pub(crate) enum BookshelfThumbnailError {
    #[error(transparent)]
    Source(#[from] CoverSourceLoadError),
    #[error(transparent)]
    Generation(#[from] ThumbnailGenerationError),
}

#[derive(Debug, Clone)]
pub(crate) struct BookshelfThumbnailCache {
    directory: PathBuf,
}

pub(crate) fn load_cached(source: impl Into<CoverSource>) -> Option<ThumbnailData> {
    let source = source.into();
    let mut store = CoverStore::load();
    BookshelfThumbnailCache::for_user().load_cached(resolve_source(source, &mut store))
}

pub(crate) fn load_cached_direct(source: &Path) -> Option<ThumbnailData> {
    BookshelfThumbnailCache::for_user().load_cached(CoverSource::File(source.to_path_buf()))
}

#[cfg(test)]
pub(crate) fn generate_and_cache(
    source: impl Into<CoverSource>,
) -> Result<ThumbnailData, BookshelfThumbnailError> {
    Ok(generate_and_cache_with_cancel(source, &|| false)?.expect("uncancelled Cover generation"))
}

pub(crate) fn generate_and_cache_with_cancel(
    source: impl Into<CoverSource>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<ThumbnailData>, BookshelfThumbnailError> {
    if cancelled() {
        return Ok(None);
    }
    let source = source.into();
    let mut store = CoverStore::load();
    let resolved = resolve_source(source, &mut store);
    let cache = BookshelfThumbnailCache::for_user();
    generate_with_store_cancel(&cache, &mut store, resolved, cancelled)
}

pub(crate) fn generate_and_cache_direct(
    source: &Path,
) -> Result<ThumbnailData, BookshelfThumbnailError> {
    BookshelfThumbnailCache::for_user().generate_and_cache(CoverSource::File(source.to_path_buf()))
}

#[cfg(test)]
fn generate_with_store(
    cache: &BookshelfThumbnailCache,
    store: &mut CoverStore,
    resolved: CoverSource,
) -> Result<ThumbnailData, BookshelfThumbnailError> {
    Ok(
        generate_with_store_cancel(cache, store, resolved, &|| false)?
            .expect("uncancelled Cover generation"),
    )
}

fn generate_with_store_cancel(
    cache: &BookshelfThumbnailCache,
    store: &mut CoverStore,
    resolved: CoverSource,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<ThumbnailData>, BookshelfThumbnailError> {
    if cancelled() {
        return Ok(None);
    }
    let result = cache.generate_and_cache_with_cancel(resolved.clone(), cancelled);
    if cancelled() {
        return Ok(None);
    }
    match result {
        Err(BookshelfThumbnailError::Source(CoverSourceLoadError::InvalidOverride(identity))) => {
            if cancelled() {
                return Ok(None);
            }
            let replacement = store
                .source_after_invalid(&identity, &resolved)
                .ok_or_else(|| {
                    BookshelfThumbnailError::Source(CoverSourceLoadError::InvalidOverride(
                        identity.clone(),
                    ))
                })?;
            cache.generate_and_cache_with_cancel(replacement, cancelled)
        }
        Err(BookshelfThumbnailError::Generation(_)) if resolved.external_identity().is_some() => {
            if cancelled() {
                return Ok(None);
            }
            let identity = resolved.external_identity().cloned().unwrap();
            let replacement = store
                .source_after_invalid(&identity, &resolved)
                .ok_or_else(|| {
                    BookshelfThumbnailError::Source(CoverSourceLoadError::InvalidOverride(
                        identity.clone(),
                    ))
                })?;
            cache.generate_and_cache_with_cancel(replacement, cancelled)
        }
        result => result,
    }
}

fn resolve_source(source: CoverSource, store: &mut CoverStore) -> CoverSource {
    match source {
        CoverSource::ArchiveAuto(path) => store.source_for_archive(&path),
        CoverSource::File(path) if ArchiveFormat::from_path(&path).is_some() => {
            store.source_for_archive(&path)
        }
        CoverSource::File(path) => path
            .parent()
            .and_then(|folder| store.override_source_for_image_folder(folder))
            .unwrap_or(CoverSource::File(path)),
        source @ (CoverSource::FolderOverride { .. }
        | CoverSource::ArchiveOverride { .. }
        | CoverSource::ExternalOverride { .. }) => source,
    }
}

impl BookshelfThumbnailCache {
    pub(crate) fn for_user() -> Self {
        Self::new(
            glib::user_cache_dir()
                .join(CACHE_APPLICATION_DIRECTORY)
                .join(CACHE_BOOKSHELF_DIRECTORY)
                .join(CACHE_AUTO_DIRECTORY),
        )
    }

    pub(crate) fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// Loads and validates an existing thumbnail without opening the source
    /// image or archive. Any cache or source metadata problem is a cache miss.
    pub(crate) fn load_cached(&self, source: impl Into<CoverSource>) -> Option<ThumbnailData> {
        let source = source.into();
        let fingerprint = SourceFingerprint::read(&source).ok()?;
        let paths = CachePaths::new(&self.directory, &source);
        load_cached_from_paths(&paths, &fingerprint)
    }

    /// Generates a thumbnail from the source and attempts to save it. Cache
    /// write failures never hide successfully generated data from the caller.
    pub(crate) fn generate_and_cache(
        &self,
        source: impl Into<CoverSource>,
    ) -> Result<ThumbnailData, BookshelfThumbnailError> {
        Ok(self
            .generate_and_cache_with_cancel(source, &|| false)?
            .expect("uncancelled Cover generation"))
    }

    pub(crate) fn generate_and_cache_with_cancel(
        &self,
        source: impl Into<CoverSource>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<ThumbnailData>, BookshelfThumbnailError> {
        if cancelled() {
            return Ok(None);
        }
        let source = source.into();
        let cache_target = SourceFingerprint::read(&source)
            .ok()
            .map(|fingerprint| (CachePaths::new(&self.directory, &source), fingerprint));
        if cancelled() {
            return Ok(None);
        }
        let source_bytes = source.load_bytes()?;
        if cancelled() {
            return Ok(None);
        }
        let Some(thumbnail) = generate_from_bytes_with_cancel(&source_bytes, cancelled)? else {
            return Ok(None);
        };
        if cancelled() {
            return Ok(None);
        }
        if let Some((paths, fingerprint)) = cache_target {
            let _ = save_cached(&paths, &fingerprint, &thumbnail);
        }
        Ok(Some(thumbnail))
    }
}

#[derive(Debug, PartialEq, Eq)]
struct SourceFingerprint {
    path_hex: String,
    size: u64,
    modified_nanos: u128,
}

impl SourceFingerprint {
    fn read(source: &CoverSource) -> std::io::Result<Self> {
        let path = source.fingerprint_path();
        let metadata = std::fs::metadata(&path)?;
        let modified = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        Ok(Self {
            path_hex: encode_hex(source.cache_key().as_bytes()),
            size: metadata.len(),
            modified_nanos: modified.as_nanos(),
        })
    }

    fn serialize(&self) -> String {
        format!(
            "version={CACHE_FORMAT_VERSION}\nsource-path={path}\nsource-size={size}\nsource-modified-nanos={modified}\n",
            path = self.path_hex,
            size = self.size,
            modified = self.modified_nanos,
        )
    }

    fn matches_serialized(&self, serialized: &str) -> bool {
        let fields = serialized
            .lines()
            .filter_map(|line| line.split_once('='))
            .collect::<HashMap<_, _>>();
        fields
            .get("version")
            .and_then(|value| value.parse::<u32>().ok())
            == Some(CACHE_FORMAT_VERSION)
            && fields.get("source-path").copied() == Some(self.path_hex.as_str())
            && fields
                .get("source-size")
                .and_then(|value| value.parse::<u64>().ok())
                == Some(self.size)
            && fields
                .get("source-modified-nanos")
                .and_then(|value| value.parse::<u128>().ok())
                == Some(self.modified_nanos)
    }
}

struct CachePaths {
    cover: PathBuf,
    metadata: PathBuf,
}

impl CachePaths {
    fn new(directory: &Path, source: &CoverSource) -> Self {
        let key = stable_cache_key(source.cache_key().as_bytes());
        Self {
            cover: directory.join(format!("{key}.cover.png")),
            metadata: directory.join(format!("{key}.meta")),
        }
    }
}

fn load_cached_from_paths(
    paths: &CachePaths,
    fingerprint: &SourceFingerprint,
) -> Option<ThumbnailData> {
    let metadata = std::fs::read_to_string(&paths.metadata).ok()?;
    if !fingerprint.matches_serialized(&metadata) {
        return None;
    }

    let cover = read_png(&paths.cover)?;
    if cover.width > COVER_MAX_WIDTH || cover.height > COVER_MAX_HEIGHT {
        return None;
    }
    Some(cover)
}

fn save_cached(
    paths: &CachePaths,
    fingerprint: &SourceFingerprint,
    thumbnail: &ThumbnailData,
) -> std::io::Result<()> {
    let directory = paths.metadata.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "cache path has no parent")
    })?;
    std::fs::create_dir_all(directory)?;

    atomic_write(&paths.cover, &encode_png(thumbnail)?)?;
    atomic_write(&paths.metadata, fingerprint.serialize().as_bytes())
}

fn read_png(path: &Path) -> Option<ThumbnailData> {
    let bytes = std::fs::read(path).ok()?;
    let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .ok()?
        .into_rgb8();
    let stride = usize::try_from(image.width().checked_mul(3)?).ok()?;
    Some(ThumbnailData {
        width: image.width(),
        height: image.height(),
        stride,
        pixels: image.into_raw(),
    })
}

fn encode_png(thumbnail: &ThumbnailData) -> std::io::Result<Vec<u8>> {
    let image =
        image::RgbImage::from_raw(thumbnail.width, thumbnail.height, thumbnail.pixels.clone())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "thumbnail pixel buffer has an invalid length",
                )
            })?;
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .map_err(std::io::Error::other)?;
    Ok(bytes.into_inner())
}

#[cfg(all(test, unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(all(test, not(unix)))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().as_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::ffi::OsStr;
    use std::io::{Cursor, Write};

    fn write_source(path: &Path, width: u32, height: u32, color: [u8; 3]) {
        let image = image::RgbImage::from_pixel(width, height, image::Rgb(color));
        image
            .save_with_format(path, image::ImageFormat::Png)
            .unwrap();
    }

    fn cache_fixture() -> (tempfile::TempDir, PathBuf, BookshelfThumbnailCache) {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("日本語/表紙.png");
        std::fs::create_dir(source.parent().unwrap()).unwrap();
        write_source(&source, 120, 180, [10, 20, 30]);
        let cache = BookshelfThumbnailCache::new(directory.path().join("missing/cache/auto"));
        (directory, source, cache)
    }

    #[test]
    fn cancelled_after_decode_leaves_no_cache_and_can_be_retried() {
        let (_directory, source, cache) = cache_fixture();
        let checks = Cell::new(0);
        let cancelled = || {
            checks.set(checks.get() + 1);
            checks.get() == 5 // after the decoder returns
        };
        assert!(
            cache
                .generate_and_cache_with_cancel(&source, &cancelled)
                .unwrap()
                .is_none()
        );
        assert!(cache.load_cached(&source).is_none());
        assert!(cache.generate_and_cache(&source).is_ok());
        assert!(cache.load_cached(&source).is_some());
    }

    #[test]
    fn generated_thumbnail_is_available_to_the_cache_only_path() {
        let (_directory, source, cache) = cache_fixture();

        assert!(cache.load_cached(&source).is_none());
        let generated = cache.generate_and_cache(&source).unwrap();
        let cached = cache.load_cached(&source).unwrap();

        assert_eq!(generated, cached);
        assert!(cache.directory.is_dir());
    }

    #[test]
    fn source_change_invalidates_cache() {
        let (_directory, source, cache) = cache_fixture();
        cache.generate_and_cache(&source).unwrap();

        write_source(&source, 121, 181, [200, 30, 40]);
        assert!(cache.load_cached(&source).is_none());
        let changed = cache.generate_and_cache(&source).unwrap();

        assert_eq!(changed.pixels[0], 200);
    }

    #[test]
    fn version_mismatch_and_corrupt_png_are_cache_misses() {
        let (_directory, source, cache) = cache_fixture();
        cache.generate_and_cache(&source).unwrap();
        let paths = CachePaths::new(&cache.directory, &CoverSource::from(&source));

        let metadata = std::fs::read_to_string(&paths.metadata).unwrap();
        std::fs::write(
            &paths.metadata,
            metadata.replace(&format!("version={CACHE_FORMAT_VERSION}"), "version=999"),
        )
        .unwrap();
        assert!(cache.load_cached(&source).is_none());
        cache.generate_and_cache(&source).unwrap();
        assert!(cache.load_cached(&source).is_some());

        std::fs::write(&paths.cover, b"broken PNG").unwrap();
        assert!(cache.load_cached(&source).is_none());
        cache.generate_and_cache(&source).unwrap();
        assert!(cache.load_cached(&source).is_some());
    }

    #[test]
    fn incomplete_cache_artifacts_are_misses() {
        let (_directory, source, cache) = cache_fixture();
        cache.generate_and_cache(&source).unwrap();
        let paths = CachePaths::new(&cache.directory, &CoverSource::from(&source));
        let metadata = std::fs::read(&paths.metadata).unwrap();
        std::fs::remove_file(&paths.metadata).unwrap();
        assert!(cache.load_cached(&source).is_none()); // cover without metadata
        std::fs::write(&paths.metadata, metadata).unwrap();
        std::fs::remove_file(&paths.cover).unwrap();
        assert!(cache.load_cached(&source).is_none()); // metadata without cover
        std::fs::write(paths.cover.with_extension("png.tmp-1"), b"unfinished").unwrap();
        assert!(cache.load_cached(&source).is_none()); // temporary file ignored
    }

    #[test]
    fn cover_only_cache_uses_one_png() {
        let (_directory, source, cache) = cache_fixture();
        let generated = cache.generate_and_cache(&source).unwrap();
        let paths = CachePaths::new(&cache.directory, &CoverSource::from(&source));

        assert!(paths.cover.is_file());
        assert_eq!(generated.width, 120);
    }

    #[test]
    fn stable_key_is_deterministic_and_filesystem_safe() {
        let key = "/本棚/a\\b/とても長い表紙名.png";
        let first = stable_cache_key(key.as_bytes());
        let second = stable_cache_key(key.as_bytes());

        assert_eq!(first, second);
        assert_eq!(first.len(), 32);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(!first.contains('/'));
        assert!(!first.contains('\\'));
    }

    #[test]
    fn cache_write_failure_does_not_hide_generated_data() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.png");
        write_source(&source, 100, 150, [1, 2, 3]);
        let not_a_directory = directory.path().join("cache-file");
        std::fs::write(&not_a_directory, b"file").unwrap();
        let cache = BookshelfThumbnailCache::new(not_a_directory.join("auto"));

        let result = cache.generate_and_cache(&source).unwrap();

        assert_eq!((result.width, result.height), (100, 150));
    }

    #[test]
    fn missing_archive_override_is_deleted_and_retried_as_automatic() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("book.cbz");
        let mut archive = zip::ZipWriter::new(std::fs::File::create(&archive_path).unwrap());
        let image = image::RgbImage::from_pixel(2, 3, image::Rgb([9, 0, 0]));
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        archive
            .start_file("001.png", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(&bytes.into_inner()).unwrap();
        archive.finish().unwrap();
        let identity = crate::covers::CoverBookIdentity::Archive(archive_path.clone());
        let mut store = CoverStore::load_from_paths(
            directory.path().join("config/covers.ini"),
            directory.path().join("data/covers"),
        );
        let missing = crate::archive::ArchiveImageId {
            archives: Vec::new(),
            image: PathBuf::from("missing.png"),
        };
        assert!(store.set_archive_entry(&archive_path, missing.clone()));
        let cache = BookshelfThumbnailCache::new(directory.path().join("cache"));

        let thumbnail = generate_with_store(
            &cache,
            &mut store,
            CoverSource::ArchiveOverride {
                archive: archive_path,
                id: missing,
            },
        )
        .unwrap();

        assert_eq!(thumbnail.pixels[0], 9);
        assert!(store.internal_override(&identity).is_none());
        let reloaded = CoverStore::load_from_paths(
            directory.path().join("config/covers.ini"),
            directory.path().join("data/covers"),
        );
        assert!(reloaded.internal_override(&identity).is_none());
    }

    #[test]
    fn corrupt_external_override_is_deleted_and_retried_as_automatic() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("book.cbz");
        let mut archive = zip::ZipWriter::new(std::fs::File::create(&archive_path).unwrap());
        let image = image::RgbImage::from_pixel(2, 3, image::Rgb([6, 0, 0]));
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        archive
            .start_file("001.png", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(&bytes.get_ref()).unwrap();
        archive.finish().unwrap();
        let external = directory.path().join("external.png");
        std::fs::write(&external, bytes.into_inner()).unwrap();
        let identity = crate::covers::CoverBookIdentity::Archive(archive_path.clone());
        let mut store = CoverStore::load_from_paths(
            directory.path().join("config/covers.ini"),
            directory.path().join("data/covers"),
        );
        store.set_external(identity.clone(), &external).unwrap();
        let resolved = store.source_for_archive(&archive_path);
        let copied = match &resolved {
            CoverSource::ExternalOverride { path, .. } => path.clone(),
            _ => unreachable!(),
        };
        std::fs::write(&copied, b"corrupt").unwrap();
        let cache = BookshelfThumbnailCache::new(directory.path().join("cache"));

        let thumbnail = generate_with_store(&cache, &mut store, resolved).unwrap();

        assert_eq!(thumbnail.pixels[0], 6);
        assert!(!copied.exists());
        assert!(matches!(
            store.source_for_archive(&archive_path),
            CoverSource::ArchiveAuto(_)
        ));
    }

    #[test]
    fn metadata_path_encoding_supports_non_utf8_on_unix() {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let path = Path::new(OsStr::from_bytes(b"/tmp/cover-\xff.png"));
            assert_eq!(
                encode_hex(&path_bytes(path)),
                "2f746d702f636f7665722dff2e706e67"
            );
        }
    }

    #[test]
    fn cached_png_round_trip_preserves_pixels() {
        let thumbnail = ThumbnailData {
            pixels: vec![1, 2, 3, 4, 5, 6],
            width: 2,
            height: 1,
            stride: 6,
        };
        let encoded = encode_png(&thumbnail).unwrap();
        let decoded = image::load(Cursor::new(encoded), image::ImageFormat::Png)
            .unwrap()
            .into_rgb8();
        assert_eq!(decoded.into_raw(), thumbnail.pixels);
    }

    #[test]
    fn missing_source_returns_an_error_without_panicking() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("missing.png");
        let cache = BookshelfThumbnailCache::new(directory.path().join("cache"));

        assert!(cache.load_cached(&source).is_none());
        assert!(cache.generate_and_cache(&source).is_err());
    }
}
