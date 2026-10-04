use crate::bookshelf::thumbnail::{ThumbnailData, fit_thumbnail_data};
use image::ColorType;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug)]
pub(super) struct ArchiveThumbnailBacking {
    directory: tempfile::TempDir,
    usable: AtomicBool,
}

impl ArchiveThumbnailBacking {
    pub(super) fn create() -> std::io::Result<Self> {
        let parent = glib::user_cache_dir().join("agnam").join("tmp");
        Self::create_in(&parent)
    }

    pub(super) fn create_in(parent: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(parent)?;
        let directory = tempfile::Builder::new()
            .prefix("agnam-archive-thumbnails-")
            .tempdir_in(parent)?;
        Ok(Self {
            directory,
            usable: AtomicBool::new(true),
        })
    }

    pub(super) fn load(&self, key: &Path) -> Option<ThumbnailData> {
        let image = image::open(self.path_for(key)).ok()?.into_rgb8();
        let width = image.width();
        let height = image.height();
        let stride = usize::try_from(width.checked_mul(3)?).ok()?;
        fit_thumbnail_data(
            ThumbnailData {
                pixels: image.into_raw(),
                width,
                height,
                stride,
            },
            u32::MAX,
            u32::MAX,
        )
        .ok()
    }

    pub(super) fn save(&self, key: &Path, thumbnail: &ThumbnailData) -> Result<(), String> {
        let expected_len = thumbnail
            .width
            .checked_mul(thumbnail.height)
            .and_then(|pixels| pixels.checked_mul(3))
            .and_then(|len| usize::try_from(len).ok());
        let valid = thumbnail.width > 0
            && thumbnail.height > 0
            && thumbnail.stride == thumbnail.width as usize * 3
            && expected_len == Some(thumbnail.pixels.len());
        let result = if valid {
            image::save_buffer_with_format(
                self.path_for(key),
                &thumbnail.pixels,
                thumbnail.width,
                thumbnail.height,
                ColorType::Rgb8,
                image::ImageFormat::Png,
            )
            .map_err(|error| error.to_string())
        } else {
            Err("thumbnail dimensions are invalid".to_string())
        };
        if result.is_err() {
            self.usable.store(false, Ordering::Release);
        }
        result
    }

    pub(super) fn is_usable(&self) -> bool {
        self.usable.load(Ordering::Acquire)
    }

    fn path_for(&self, key: &Path) -> PathBuf {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        self.directory
            .path()
            .join(format!("{:016x}.png", hasher.finish()))
    }

    #[cfg(test)]
    fn path(&self) -> &Path {
        self.directory.path()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thumbnail() -> ThumbnailData {
        ThumbnailData {
            pixels: vec![10, 20, 30, 40, 50, 60],
            width: 2,
            height: 1,
            stride: 6,
        }
    }

    #[test]
    fn save_load_roundtrip_and_cache_miss() {
        let root = tempfile::tempdir().unwrap();
        let backing = ArchiveThumbnailBacking::create_in(root.path()).unwrap();
        let key = Path::new(".agnam-archive-thumbnail-deadbeef");
        assert_eq!(backing.load(key), None);
        backing.save(key, &thumbnail()).unwrap();
        assert_eq!(backing.load(key), Some(thumbnail()));
    }

    #[test]
    fn session_drop_removes_directory() {
        let root = tempfile::tempdir().unwrap();
        let path = {
            let backing = ArchiveThumbnailBacking::create_in(root.path()).unwrap();
            let path = backing.path().to_path_buf();
            assert!(path.is_dir());
            path
        };
        assert!(!path.exists());
    }

    #[test]
    fn keys_always_map_to_safe_flat_filenames() {
        let root = tempfile::tempdir().unwrap();
        let backing = ArchiveThumbnailBacking::create_in(root.path()).unwrap();
        let path = backing.path_for(Path::new("../../unsafe/archive entry.png"));
        assert_eq!(path.parent(), Some(backing.path()));
        let name = path.file_name().unwrap().to_string_lossy();
        assert_eq!(name.len(), 20);
        assert!(name.ends_with(".png"));
        assert!(!name.contains('/'));
    }

    #[test]
    fn corrupt_backing_is_a_cache_miss() {
        let root = tempfile::tempdir().unwrap();
        let backing = ArchiveThumbnailBacking::create_in(root.path()).unwrap();
        let key = Path::new("key");
        std::fs::write(backing.path_for(key), b"not a png").unwrap();
        assert_eq!(backing.load(key), None);
    }

    #[test]
    fn write_failure_marks_the_backing_unusable_for_lru_eviction() {
        let root = tempfile::tempdir().unwrap();
        let backing = ArchiveThumbnailBacking::create_in(root.path()).unwrap();
        let invalid = ThumbnailData {
            pixels: vec![0; 1],
            width: 2,
            height: 2,
            stride: 6,
        };
        assert!(backing.save(Path::new("key"), &invalid).is_err());
        assert!(!backing.is_usable());
    }

    #[test]
    fn backing_hit_does_not_depend_on_an_archive_source() {
        let root = tempfile::tempdir().unwrap();
        let backing = ArchiveThumbnailBacking::create_in(root.path()).unwrap();
        let key = Path::new("missing-archive-entry");
        backing.save(key, &thumbnail()).unwrap();

        // No ArchiveEntryReader is created: restoration is entirely session-backed.
        assert_eq!(backing.load(key), Some(thumbnail()));
    }
}
