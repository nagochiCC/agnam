use crate::cache_file::{atomic_write, encode_hex, path_bytes, stable_cache_key};
use crate::error::AppError;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[cfg(test)]
use std::sync::Mutex;

const FORMAT_VERSION: u32 = 1;
const CACHE_APPLICATION_DIRECTORY: &str = "agnam";
const CACHE_BOOKSHELF_DIRECTORY: &str = "bookshelf";
const CACHE_ARCHIVE_PROBE_DIRECTORY: &str = "archive-probes";

#[cfg(test)]
static BACKEND_PROBE_TRACKER: Mutex<Option<(PathBuf, usize)>> = Mutex::new(None);

pub(super) fn probe_viewable_content(
    archive_path: &Path,
    probe: impl FnOnce() -> Result<bool, AppError>,
) -> Result<bool, AppError> {
    ArchiveProbeCache::for_user().get_or_probe(archive_path, || {
        #[cfg(test)]
        {
            let mut tracker = BACKEND_PROBE_TRACKER
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some((target, count)) = tracker.as_mut()
                && target == archive_path
            {
                *count += 1;
            }
        }
        probe()
    })
}

#[cfg(test)]
pub(crate) fn track_backend_probes_for(path: &Path) {
    *BACKEND_PROBE_TRACKER
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some((path.to_path_buf(), 0));
}

#[cfg(test)]
pub(crate) fn backend_probe_count() -> usize {
    BACKEND_PROBE_TRACKER
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .map_or(0, |(_, count)| *count)
}

#[derive(Debug, Clone)]
struct ArchiveProbeCache {
    directory: PathBuf,
}

impl ArchiveProbeCache {
    fn for_user() -> Self {
        Self::new(user_cache_directory())
    }

    fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    fn get_or_probe(
        &self,
        archive_path: &Path,
        probe: impl FnOnce() -> Result<bool, AppError>,
    ) -> Result<bool, AppError> {
        let fingerprint = ArchiveFingerprint::read(archive_path).ok();
        if let Some(fingerprint) = &fingerprint
            && let Some(result) = self.load(fingerprint)
        {
            return Ok(result);
        }

        let result = probe()?;
        if let Some(fingerprint) = fingerprint {
            let _ = self.save(&fingerprint, result);
        }
        Ok(result)
    }

    fn cache_path(&self, fingerprint: &ArchiveFingerprint) -> PathBuf {
        self.directory
            .join(format!("{}.probe", stable_cache_key(&fingerprint.path)))
    }

    fn load(&self, fingerprint: &ArchiveFingerprint) -> Option<bool> {
        let contents = std::fs::read_to_string(self.cache_path(fingerprint)).ok()?;
        fingerprint.matches_serialized(&contents)
    }

    fn save(&self, fingerprint: &ArchiveFingerprint, result: bool) -> std::io::Result<()> {
        atomic_write(
            &self.cache_path(fingerprint),
            fingerprint.serialize(result).as_bytes(),
        )
    }
}

#[cfg(not(test))]
fn user_cache_directory() -> PathBuf {
    glib::user_cache_dir()
        .join(CACHE_APPLICATION_DIRECTORY)
        .join(CACHE_BOOKSHELF_DIRECTORY)
        .join(CACHE_ARCHIVE_PROBE_DIRECTORY)
}

#[cfg(test)]
fn user_cache_directory() -> PathBuf {
    use std::sync::OnceLock;
    static DIRECTORY: OnceLock<tempfile::TempDir> = OnceLock::new();
    DIRECTORY
        .get_or_init(|| tempfile::tempdir().expect("archive probe test cache"))
        .path()
        .join(CACHE_APPLICATION_DIRECTORY)
        .join(CACHE_BOOKSHELF_DIRECTORY)
        .join(CACHE_ARCHIVE_PROBE_DIRECTORY)
}

#[derive(Debug, PartialEq, Eq)]
struct ArchiveFingerprint {
    path: Vec<u8>,
    size: u64,
    modified_nanos: u128,
}

impl ArchiveFingerprint {
    fn read(path: &Path) -> std::io::Result<Self> {
        let metadata = std::fs::metadata(path)?;
        let modified = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        Ok(Self {
            path: path_bytes(path).to_vec(),
            size: metadata.len(),
            modified_nanos: modified.as_nanos(),
        })
    }

    fn serialize(&self, result: bool) -> String {
        format!(
            "version={FORMAT_VERSION}\narchive-path={}\narchive-size={}\narchive-modified-nanos={}\nviewable={}\n",
            encode_hex(&self.path),
            self.size,
            self.modified_nanos,
            u8::from(result),
        )
    }

    fn matches_serialized(&self, serialized: &str) -> Option<bool> {
        let fields = serialized
            .lines()
            .filter_map(|line| line.split_once('='))
            .collect::<HashMap<_, _>>();
        let path_hex = encode_hex(&self.path);
        if fields
            .get("version")
            .and_then(|value| value.parse::<u32>().ok())
            != Some(FORMAT_VERSION)
            || fields.get("archive-path").copied() != Some(path_hex.as_str())
            || fields
                .get("archive-size")
                .and_then(|value| value.parse::<u64>().ok())
                != Some(self.size)
            || fields
                .get("archive-modified-nanos")
                .and_then(|value| value.parse::<u128>().ok())
                != Some(self.modified_nanos)
        {
            return None;
        }
        match fields.get("viewable").copied() {
            Some("1") => Some(true),
            Some("0") => Some(false),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::io::Write;
    use std::time::Duration;

    fn fixture() -> (tempfile::TempDir, PathBuf, ArchiveProbeCache) {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("book.zip");
        std::fs::write(&archive, b"archive").unwrap();
        let cache = ArchiveProbeCache::new(directory.path().join("cache/archive-probes"));
        (directory, archive, cache)
    }

    #[test]
    fn true_and_false_results_round_trip_without_reprobing() {
        for expected in [true, false] {
            let (_directory, archive, cache) = fixture();
            let calls = Cell::new(0);
            assert_eq!(
                cache
                    .get_or_probe(&archive, || {
                        calls.set(calls.get() + 1);
                        Ok(expected)
                    })
                    .unwrap(),
                expected
            );
            assert_eq!(
                cache
                    .get_or_probe(&archive, || {
                        calls.set(calls.get() + 1);
                        Ok(!expected)
                    })
                    .unwrap(),
                expected
            );
            assert_eq!(calls.get(), 1);
        }
    }

    #[test]
    fn size_change_is_a_cache_miss() {
        let (_directory, archive, cache) = fixture();
        assert!(cache.get_or_probe(&archive, || Ok(true)).unwrap());
        std::fs::OpenOptions::new()
            .append(true)
            .open(&archive)
            .unwrap()
            .write_all(b"changed")
            .unwrap();

        let calls = Cell::new(0);
        assert!(
            !cache
                .get_or_probe(&archive, || {
                    calls.set(calls.get() + 1);
                    Ok(false)
                })
                .unwrap()
        );
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn mtime_change_is_a_cache_miss() {
        let (_directory, archive, cache) = fixture();
        assert!(cache.get_or_probe(&archive, || Ok(true)).unwrap());
        let changed =
            std::fs::metadata(&archive).unwrap().modified().unwrap() + Duration::from_secs(2);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&archive)
            .unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(changed))
            .unwrap();

        let calls = Cell::new(0);
        assert!(
            !cache
                .get_or_probe(&archive, || {
                    calls.set(calls.get() + 1);
                    Ok(false)
                })
                .unwrap()
        );
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn corrupt_or_wrong_version_cache_is_a_miss() {
        let (_directory, archive, cache) = fixture();
        let fingerprint = ArchiveFingerprint::read(&archive).unwrap();
        cache.save(&fingerprint, true).unwrap();
        let path = cache.cache_path(&fingerprint);

        std::fs::write(&path, b"broken").unwrap();
        assert!(!cache.get_or_probe(&archive, || Ok(false)).unwrap());

        cache.save(&fingerprint, true).unwrap();
        let contents = std::fs::read_to_string(&path)
            .unwrap()
            .replace(&format!("version={FORMAT_VERSION}"), "version=999");
        std::fs::write(&path, contents).unwrap();
        assert!(!cache.get_or_probe(&archive, || Ok(false)).unwrap());
    }

    #[test]
    fn probe_errors_are_not_persisted_as_false() {
        let (_directory, archive, cache) = fixture();
        assert!(
            cache
                .get_or_probe(&archive, || Err(AppError::Archive("temporary".into())))
                .is_err()
        );

        let calls = Cell::new(0);
        assert!(
            cache
                .get_or_probe(&archive, || {
                    calls.set(calls.get() + 1);
                    Ok(true)
                })
                .unwrap()
        );
        assert_eq!(calls.get(), 1);
    }
}
