//! Book-scoped cover resolution and persistence.
use crate::archive::{self, ArchiveFormat, ArchiveImageId};
use crate::error::AppError;
use gtk::glib;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

const APPLICATION_DIRECTORY: &str = "agnam";
const STORE_FILENAME: &str = "covers.ini";
const STORE_GROUP: &str = "Covers";
const ENTRY_GROUP_PREFIX: &str = "Entry ";
const FORMAT_VERSION: i32 = 2;
static STORE_MUTATION_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum CoverBookIdentity {
    Archive(PathBuf),
    ImageFolder(PathBuf),
}

impl CoverBookIdentity {
    pub(crate) fn archive(path: &Path) -> Option<Self> {
        ArchiveFormat::from_path(path).map(|_| Self::Archive(path.to_path_buf()))
    }

    pub(crate) fn image_folder(path: &Path) -> Self {
        Self::ImageFolder(path.to_path_buf())
    }

    pub(crate) fn path(&self) -> &Path {
        match self {
            Self::Archive(path) | Self::ImageFolder(path) => path,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Archive(_) => "archive",
            Self::ImageFolder(_) => "image-folder",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum CoverSource {
    File(PathBuf),
    ArchiveAuto(PathBuf),
    FolderOverride {
        folder: PathBuf,
        relative: PathBuf,
    },
    ArchiveOverride {
        archive: PathBuf,
        id: ArchiveImageId,
    },
    ExternalOverride {
        identity: CoverBookIdentity,
        path: PathBuf,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CoverSourceLoadError {
    #[error("保存された手動表紙を解決できません")]
    InvalidOverride(CoverBookIdentity),
    #[error(transparent)]
    Source(#[from] AppError),
}

impl CoverSource {
    pub(crate) fn cache_key(&self) -> String {
        match self {
            Self::File(path) => format!("file:{}", path_key(path)),
            Self::ArchiveAuto(path) => format!("archive-auto:{}", path_key(path)),
            Self::FolderOverride { folder, relative } => {
                format!("folder-entry:{}:{}", path_key(folder), path_key(relative))
            }
            Self::ArchiveOverride { archive, id } => {
                let chain = id
                    .archives
                    .iter()
                    .map(|path| path_key(path))
                    .collect::<Vec<_>>()
                    .join(":");
                format!(
                    "archive-entry:{}:{}:{}",
                    path_key(archive),
                    chain,
                    path_key(&id.image)
                )
            }
            Self::ExternalOverride { identity, path } => format!(
                "external:{}:{}:{}",
                identity.kind(),
                path_key(identity.path()),
                path_key(path)
            ),
        }
    }

    pub(crate) fn fingerprint_path(&self) -> PathBuf {
        match self {
            Self::File(path) | Self::ArchiveAuto(path) => path.clone(),
            Self::FolderOverride { folder, relative } => folder.join(relative),
            Self::ArchiveOverride { archive, .. } => archive.clone(),
            Self::ExternalOverride { path, .. } => path.clone(),
        }
    }

    pub(crate) fn load_bytes(&self) -> Result<Vec<u8>, CoverSourceLoadError> {
        match self {
            Self::File(path) => Ok(fs::read(path).map_err(AppError::from)?),
            Self::ArchiveAuto(path) => Ok(archive::load_cover_source_bytes(path)?),
            Self::FolderOverride { folder, relative } => {
                let identity = CoverBookIdentity::image_folder(folder);
                let path = folder.join(relative);
                if !archive::is_normal_relative_path(relative)
                    || !path.is_file()
                    || !archive::is_image_ext(&path)
                {
                    return Err(CoverSourceLoadError::InvalidOverride(identity));
                }
                Ok(fs::read(path).map_err(AppError::from)?)
            }
            Self::ArchiveOverride { archive, id } => archive::load_archive_entry_bytes(archive, id)
                .map_err(|error| match error {
                    archive::ArchiveEntryLoadError::Missing => {
                        CoverSourceLoadError::InvalidOverride(CoverBookIdentity::Archive(
                            archive.clone(),
                        ))
                    }
                    archive::ArchiveEntryLoadError::Other(error) => error.into(),
                }),
            Self::ExternalOverride { identity, path } => {
                if !path.is_file() || !archive::is_image_ext(path) {
                    return Err(CoverSourceLoadError::InvalidOverride(identity.clone()));
                }
                Ok(fs::read(path).map_err(AppError::from)?)
            }
        }
    }

    pub(crate) fn external_identity(&self) -> Option<&CoverBookIdentity> {
        match self {
            Self::ExternalOverride { identity, .. } => Some(identity),
            _ => None,
        }
    }
}

impl From<&Path> for CoverSource {
    fn from(path: &Path) -> Self {
        if ArchiveFormat::from_path(path).is_some() {
            Self::ArchiveAuto(path.to_path_buf())
        } else {
            Self::File(path.to_path_buf())
        }
    }
}

impl From<&PathBuf> for CoverSource {
    fn from(path: &PathBuf) -> Self {
        Self::from(path.as_path())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CoverOverride {
    FolderImage(PathBuf),
    ArchiveEntry(ArchiveImageId),
    External(PathBuf),
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InternalCoverOverride {
    Folder(PathBuf),
    Archive(ArchiveImageId),
}

#[derive(Debug)]
pub(crate) struct CoverStore {
    overrides: HashMap<CoverBookIdentity, CoverOverride>,
    storage_path: PathBuf,
    data_directory: PathBuf,
    next_external_id: u64,
}

#[derive(Debug)]
pub(crate) struct PreparedExternalCover {
    identity: CoverBookIdentity,
    path: Option<PathBuf>,
}

impl PreparedExternalCover {
    pub(crate) fn discard(self) {
        drop(self);
    }
}

impl Drop for PreparedExternalCover {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(path);
        }
    }
}

impl CoverStore {
    pub(crate) fn load() -> Self {
        Self::load_from_paths(
            glib::user_config_dir()
                .join(APPLICATION_DIRECTORY)
                .join(STORE_FILENAME),
            glib::user_data_dir()
                .join(APPLICATION_DIRECTORY)
                .join("covers"),
        )
    }

    pub(crate) fn load_from_paths(storage_path: PathBuf, data_directory: PathBuf) -> Self {
        let mut store = Self {
            overrides: HashMap::new(),
            storage_path,
            data_directory,
            next_external_id: 0,
        };
        let key_file = glib::KeyFile::new();
        if key_file
            .load_from_file(&store.storage_path, glib::KeyFileFlags::NONE)
            .is_err()
            || key_file.integer(STORE_GROUP, "format-version").ok() != Some(FORMAT_VERSION)
        {
            return store;
        }
        let count = key_file
            .integer(STORE_GROUP, "entry-count")
            .ok()
            .unwrap_or(0)
            .max(0) as usize;
        for index in 0..count {
            let group = format!("{ENTRY_GROUP_PREFIX}{index}");
            let Some(identity) = read_identity(&key_file, &group) else {
                continue;
            };
            let Some(override_) = read_override(&key_file, &group) else {
                continue;
            };
            if let CoverOverride::External(path) = &override_
                && let Some(id) = path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .and_then(|value| value.parse().ok())
            {
                store.next_external_id = store.next_external_id.max(id);
            }
            store.overrides.insert(identity, override_);
        }
        store
    }

    pub(crate) fn source_for_archive(&mut self, archive: &Path) -> CoverSource {
        self.resolve(&CoverBookIdentity::Archive(archive.to_path_buf()))
            .unwrap_or_else(|| CoverSource::ArchiveAuto(archive.to_path_buf()))
    }

    pub(crate) fn override_source_for_image_folder(
        &mut self,
        folder: &Path,
    ) -> Option<CoverSource> {
        self.resolve(&CoverBookIdentity::ImageFolder(folder.to_path_buf()))
    }

    #[cfg(test)]
    pub(crate) fn internal_override(
        &self,
        identity: &CoverBookIdentity,
    ) -> Option<InternalCoverOverride> {
        match self.overrides.get(identity) {
            Some(CoverOverride::FolderImage(path)) => {
                Some(InternalCoverOverride::Folder(path.clone()))
            }
            Some(CoverOverride::ArchiveEntry(id)) => {
                Some(InternalCoverOverride::Archive(id.clone()))
            }
            _ => None,
        }
    }

    pub(crate) fn set_folder_image(&mut self, folder: &Path, image: &Path) -> bool {
        let Ok(relative) = image.strip_prefix(folder) else {
            return false;
        };
        if !archive::is_normal_relative_path(relative)
            || !archive::is_image_ext(image)
            || !image.is_file()
        {
            return false;
        }
        self.replace_checked(
            CoverBookIdentity::image_folder(folder),
            CoverOverride::FolderImage(relative.to_path_buf()),
        )
        .is_ok()
    }

    pub(crate) fn set_archive_entry(&mut self, archive: &Path, id: ArchiveImageId) -> bool {
        if !id.is_valid() || ArchiveFormat::from_path(archive).is_none() {
            return false;
        }
        self.replace_checked(
            CoverBookIdentity::Archive(archive.to_path_buf()),
            CoverOverride::ArchiveEntry(id),
        )
        .is_ok()
    }

    #[cfg(test)]
    pub(crate) fn set_external(
        &mut self,
        identity: CoverBookIdentity,
        source: &Path,
    ) -> Result<(), AppError> {
        let prepared = self.prepare_external(identity, source)?;
        self.commit_external(prepared)
    }

    pub(crate) fn prepare_external(
        &mut self,
        identity: CoverBookIdentity,
        source: &Path,
    ) -> Result<PreparedExternalCover, AppError> {
        if !archive::is_image_ext(source) {
            return Err(AppError::Archive("対応していない表紙画像形式です".into()));
        }
        let source_bytes = fs::read(source)?;
        crate::bookshelf::thumbnail::generate_from_bytes(&source_bytes)
            .map_err(|error| AppError::Archive(error.to_string()))?;
        fs::create_dir_all(&self.data_directory)?;
        let extension = source
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("img")
            .to_ascii_lowercase();
        let destination = loop {
            self.next_external_id = self.next_external_id.wrapping_add(1);
            let destination = self
                .data_directory
                .join(format!("{}.{}", self.next_external_id, extension));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)
            {
                Ok(mut output) => {
                    let result = output
                        .write_all(&source_bytes)
                        .and_then(|_| output.sync_all());
                    if let Err(error) = result {
                        drop(output);
                        let _ = fs::remove_file(&destination);
                        return Err(error.into());
                    }
                    break destination;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        };
        Ok(PreparedExternalCover {
            identity,
            path: Some(destination),
        })
    }

    pub(crate) fn commit_external(
        &mut self,
        mut prepared: PreparedExternalCover,
    ) -> Result<(), AppError> {
        let path = prepared.path.take().expect("prepared cover has a path");
        self.replace_checked(prepared.identity.clone(), CoverOverride::External(path))
    }

    pub(crate) fn clear(&mut self, identity: &CoverBookIdentity) {
        let _guard = store_mutation_guard();
        let mut latest = self.reload();
        if let Some(old) = latest.overrides.remove(identity) {
            match latest.save_to_path(&latest.storage_path) {
                Ok(()) => latest.remove_external(&old),
                Err(error) => {
                    latest.overrides.insert(identity.clone(), old);
                    eprintln!("表紙設定を保存できませんでした: {error}");
                }
            }
        }
        self.sync_from(latest);
    }

    /// Removes an invalid override only if it is still the source observed by
    /// the background job. If the user changed it meanwhile, return the newer
    /// source so the caller can render that instead of falling back to auto.
    pub(crate) fn source_after_invalid(
        &mut self,
        identity: &CoverBookIdentity,
        invalid: &CoverSource,
    ) -> Option<CoverSource> {
        let _guard = store_mutation_guard();
        let mut latest = self.reload();
        let current = latest
            .overrides
            .get(identity)
            .and_then(|override_| source_for_override(identity, override_));
        if current.as_ref() == Some(invalid) {
            let old = latest.overrides.remove(identity).expect("override exists");
            match latest.save_to_path(&latest.storage_path) {
                Ok(()) => latest.remove_external(&old),
                Err(error) => {
                    latest.overrides.insert(identity.clone(), old);
                    eprintln!("表紙設定を保存できませんでした: {error}");
                }
            }
        }
        let source = latest
            .overrides
            .get(identity)
            .and_then(|override_| source_for_override(identity, override_))
            .or_else(|| automatic_source(identity));
        self.sync_from(latest);
        source
    }

    fn resolve(&mut self, identity: &CoverBookIdentity) -> Option<CoverSource> {
        let override_ = self.overrides.get(identity)?.clone();
        let source = match (identity, &override_) {
            (CoverBookIdentity::ImageFolder(folder), CoverOverride::FolderImage(relative)) => {
                let path = folder.join(relative);
                (archive::is_normal_relative_path(relative)
                    && path.is_file()
                    && archive::is_image_ext(&path))
                .then(|| CoverSource::FolderOverride {
                    folder: folder.clone(),
                    relative: relative.clone(),
                })
            }
            (CoverBookIdentity::Archive(archive), CoverOverride::ArchiveEntry(id)) => {
                archive.is_file().then(|| CoverSource::ArchiveOverride {
                    archive: archive.clone(),
                    id: id.clone(),
                })
            }
            (_, CoverOverride::External(path)) => (path.is_file() && archive::is_image_ext(path))
                .then(|| CoverSource::ExternalOverride {
                    identity: identity.clone(),
                    path: path.clone(),
                }),
            _ => None,
        };
        if source.is_none() {
            self.clear_override_if_matches(identity, &override_);
        }
        source
    }

    fn clear_override_if_matches(
        &mut self,
        identity: &CoverBookIdentity,
        expected: &CoverOverride,
    ) {
        let _guard = store_mutation_guard();
        let mut latest = self.reload();
        if latest.overrides.get(identity) == Some(expected) {
            let old = latest.overrides.remove(identity).expect("override exists");
            match latest.save_to_path(&latest.storage_path) {
                Ok(()) => latest.remove_external(&old),
                Err(error) => {
                    latest.overrides.insert(identity.clone(), old);
                    eprintln!("表紙設定を保存できませんでした: {error}");
                }
            }
        }
        self.sync_from(latest);
    }

    fn replace_checked(
        &mut self,
        identity: CoverBookIdentity,
        new_override: CoverOverride,
    ) -> Result<(), AppError> {
        let _guard = store_mutation_guard();
        let mut latest = self.reload();
        let old = latest
            .overrides
            .insert(identity.clone(), new_override.clone());
        if let Err(error) = latest.save_to_path(&latest.storage_path) {
            match old {
                Some(old) => {
                    latest.overrides.insert(identity, old);
                }
                None => {
                    latest.overrides.remove(&identity);
                }
            }
            latest.remove_external(&new_override);
            self.sync_from(latest);
            return Err(AppError::Archive(error.to_string()));
        }
        if let Some(old) = old {
            latest.remove_external(&old);
        }
        self.sync_from(latest);
        Ok(())
    }

    fn reload(&self) -> Self {
        Self::load_from_paths(self.storage_path.clone(), self.data_directory.clone())
    }

    fn sync_from(&mut self, latest: Self) {
        self.overrides = latest.overrides;
        self.next_external_id = latest.next_external_id;
    }

    fn remove_external(&self, override_: &CoverOverride) {
        if let CoverOverride::External(path) = override_ {
            let _ = fs::remove_file(path);
        }
    }

    fn save_to_path(&self, path: &Path) -> Result<(), glib::Error> {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let key_file = glib::KeyFile::new();
        key_file.set_integer(STORE_GROUP, "format-version", FORMAT_VERSION);
        key_file.set_integer(STORE_GROUP, "entry-count", self.overrides.len() as i32);
        for (index, (identity, override_)) in self.overrides.iter().enumerate() {
            let group = format!("{ENTRY_GROUP_PREFIX}{index}");
            key_file.set_string(&group, "identity-kind", identity.kind());
            key_file.set_string(
                &group,
                "identity-path",
                identity.path().to_string_lossy().as_ref(),
            );
            match override_ {
                CoverOverride::FolderImage(path) => {
                    key_file.set_string(&group, "kind", "folder-image");
                    key_file.set_string(&group, "path", path.to_string_lossy().as_ref());
                }
                CoverOverride::ArchiveEntry(id) => {
                    key_file.set_string(&group, "kind", "archive-entry");
                    key_file.set_integer(&group, "archive-count", id.archives.len() as i32);
                    for (chain_index, path) in id.archives.iter().enumerate() {
                        key_file.set_string(
                            &group,
                            &format!("archive-{chain_index}"),
                            path.to_string_lossy().as_ref(),
                        );
                    }
                    key_file.set_string(&group, "image", id.image.to_string_lossy().as_ref());
                }
                CoverOverride::External(path) => {
                    key_file.set_string(&group, "kind", "external");
                    key_file.set_string(&group, "path", path.to_string_lossy().as_ref());
                }
            }
        }
        key_file.save_to_file(path)
    }
}

fn store_mutation_guard() -> MutexGuard<'static, ()> {
    STORE_MUTATION_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn source_for_override(
    identity: &CoverBookIdentity,
    override_: &CoverOverride,
) -> Option<CoverSource> {
    match (identity, override_) {
        (CoverBookIdentity::ImageFolder(folder), CoverOverride::FolderImage(relative)) => {
            Some(CoverSource::FolderOverride {
                folder: folder.clone(),
                relative: relative.clone(),
            })
        }
        (CoverBookIdentity::Archive(archive), CoverOverride::ArchiveEntry(id)) => {
            Some(CoverSource::ArchiveOverride {
                archive: archive.clone(),
                id: id.clone(),
            })
        }
        (_, CoverOverride::External(path)) => Some(CoverSource::ExternalOverride {
            identity: identity.clone(),
            path: path.clone(),
        }),
        _ => None,
    }
}

pub(crate) fn automatic_source(identity: &CoverBookIdentity) -> Option<CoverSource> {
    match identity {
        CoverBookIdentity::Archive(path) => Some(CoverSource::ArchiveAuto(path.clone())),
        CoverBookIdentity::ImageFolder(path) => {
            archive::cover_image_in_folder(path).map(CoverSource::File)
        }
    }
}

fn read_identity(key_file: &glib::KeyFile, group: &str) -> Option<CoverBookIdentity> {
    let path = PathBuf::from(key_file.string(group, "identity-path").ok()?.as_str());
    match key_file.string(group, "identity-kind").ok()?.as_str() {
        "archive" => Some(CoverBookIdentity::Archive(path)),
        "image-folder" => Some(CoverBookIdentity::ImageFolder(path)),
        _ => None,
    }
}

fn read_override(key_file: &glib::KeyFile, group: &str) -> Option<CoverOverride> {
    match key_file.string(group, "kind").ok()?.as_str() {
        "folder-image" => {
            let path = PathBuf::from(key_file.string(group, "path").ok()?.as_str());
            archive::is_normal_relative_path(&path).then_some(CoverOverride::FolderImage(path))
        }
        "archive-entry" => {
            let count = key_file.integer(group, "archive-count").ok()?.max(0) as usize;
            let archives = (0..count)
                .map(|index| {
                    key_file
                        .string(group, &format!("archive-{index}"))
                        .ok()
                        .map(|path| PathBuf::from(path.as_str()))
                })
                .collect::<Option<Vec<_>>>()?;
            let image = PathBuf::from(key_file.string(group, "image").ok()?.as_str());
            let id = ArchiveImageId { archives, image };
            id.is_valid().then_some(CoverOverride::ArchiveEntry(id))
        }
        "external" => Some(CoverOverride::External(PathBuf::from(
            key_file.string(group, "path").ok()?.as_str(),
        ))),
        _ => None,
    }
}

fn path_key(path: &Path) -> String {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes()
    };
    #[cfg(not(unix))]
    let lossy = path.to_string_lossy();
    #[cfg(not(unix))]
    let bytes = lossy.as_bytes();
    crate::cache_file::encode_hex(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn png(red: u8) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 3, image::Rgb([red, 0, 0])))
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    fn test_store(directory: &tempfile::TempDir) -> CoverStore {
        CoverStore::load_from_paths(
            directory.path().join("config/covers.ini"),
            directory.path().join("data/covers"),
        )
    }

    #[test]
    fn folder_and_archive_internal_overrides_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("book");
        fs::create_dir(&folder).unwrap();
        let image = folder.join("Cover.PNG");
        fs::write(&image, png(1)).unwrap();
        let archive = directory.path().join("book.cbz");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        zip.start_file("nested.cbz", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"placeholder").unwrap();
        zip.finish().unwrap();
        let archive_id = ArchiveImageId {
            archives: vec![PathBuf::from("nested.cbz")],
            image: PathBuf::from("001.jpg"),
        };

        let mut store = test_store(&directory);
        assert!(store.set_folder_image(&folder, &image));
        assert!(store.set_archive_entry(&archive, archive_id.clone()));
        let mut loaded = test_store(&directory);

        assert!(matches!(
            loaded.override_source_for_image_folder(&folder),
            Some(CoverSource::FolderOverride { relative, .. }) if relative == Path::new("Cover.PNG")
        ));
        assert!(matches!(
            loaded.source_for_archive(&archive),
            CoverSource::ArchiveOverride { id, .. } if id == archive_id
        ));
    }

    #[test]
    fn external_copy_survives_source_removal_and_clear_deletes_copy() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.png");
        fs::write(&source, png(4)).unwrap();
        let identity = CoverBookIdentity::Archive(directory.path().join("book.cbz"));
        let mut store = test_store(&directory);
        store.set_external(identity.clone(), &source).unwrap();
        fs::remove_file(&source).unwrap();

        let mut loaded = test_store(&directory);
        let CoverSource::ExternalOverride { path, .. } = loaded.source_for_archive(identity.path())
        else {
            panic!("external override should round-trip");
        };
        assert!(path.is_file());
        assert_eq!(fs::read(&path).unwrap(), png(4));
        loaded.clear(&identity);
        assert!(!path.exists());
    }

    #[test]
    fn failed_external_decode_keeps_existing_override_and_replacement_is_collision_free() {
        let directory = tempfile::tempdir().unwrap();
        let identity = CoverBookIdentity::Archive(directory.path().join("book.cbz"));
        let first = directory.path().join("first.png");
        let second = directory.path().join("second.png");
        let broken = directory.path().join("broken.png");
        fs::write(&first, png(1)).unwrap();
        fs::write(&second, png(2)).unwrap();
        fs::write(&broken, b"broken").unwrap();
        let mut store = test_store(&directory);
        store.set_external(identity.clone(), &first).unwrap();
        let first_copy = match store.resolve(&identity).unwrap() {
            CoverSource::ExternalOverride { path, .. } => path,
            _ => unreachable!(),
        };

        assert!(store.set_external(identity.clone(), &broken).is_err());
        assert!(first_copy.is_file());
        assert!(matches!(
            store.resolve(&identity),
            Some(CoverSource::ExternalOverride { path, .. }) if path == first_copy
        ));

        store.set_external(identity.clone(), &second).unwrap();
        let second_copy = match store.resolve(&identity).unwrap() {
            CoverSource::ExternalOverride { path, .. } => path,
            _ => unreachable!(),
        };
        assert_ne!(first_copy, second_copy);
        assert!(!first_copy.exists());
        assert_eq!(fs::read(second_copy).unwrap(), png(2));
    }

    #[test]
    fn invalid_folder_and_missing_external_overrides_are_removed_from_store() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("folder");
        fs::create_dir(&folder).unwrap();
        let image = folder.join("001.png");
        fs::write(&image, png(1)).unwrap();
        let folder_identity = CoverBookIdentity::image_folder(&folder);
        let external_identity = CoverBookIdentity::Archive(directory.path().join("external.cbz"));
        let external = directory.path().join("external.png");
        fs::write(&external, png(2)).unwrap();
        let mut store = test_store(&directory);
        assert!(store.set_folder_image(&folder, &image));
        store
            .set_external(external_identity.clone(), &external)
            .unwrap();
        let external_copy = match store.resolve(&external_identity).unwrap() {
            CoverSource::ExternalOverride { path, .. } => path,
            _ => unreachable!(),
        };
        fs::remove_file(image).unwrap();
        fs::remove_file(external_copy).unwrap();

        let mut loaded = test_store(&directory);
        assert!(loaded.resolve(&folder_identity).is_none());
        assert!(loaded.resolve(&external_identity).is_none());
        let reloaded = test_store(&directory);
        assert!(!reloaded.overrides.contains_key(&folder_identity));
        assert!(!reloaded.overrides.contains_key(&external_identity));
    }

    #[test]
    fn source_cache_keys_separate_every_source_kind() {
        let identity = CoverBookIdentity::Archive(PathBuf::from("book.cbz"));
        let sources = [
            CoverSource::File(PathBuf::from("book.cbz")),
            CoverSource::ArchiveAuto(PathBuf::from("book.cbz")),
            CoverSource::FolderOverride {
                folder: PathBuf::from("book"),
                relative: PathBuf::from("001.png"),
            },
            CoverSource::ArchiveOverride {
                archive: PathBuf::from("book.cbz"),
                id: ArchiveImageId {
                    archives: Vec::new(),
                    image: PathBuf::from("001.png"),
                },
            },
            CoverSource::ExternalOverride {
                identity,
                path: PathBuf::from("copy.png"),
            },
        ];
        let keys = sources
            .iter()
            .map(CoverSource::cache_key)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(keys.len(), sources.len());
    }

    #[test]
    fn stale_store_instances_merge_changes_for_different_books() {
        let directory = tempfile::tempdir().unwrap();
        let folder_a = directory.path().join("a");
        let folder_b = directory.path().join("b");
        fs::create_dir(&folder_a).unwrap();
        fs::create_dir(&folder_b).unwrap();
        let image_a = folder_a.join("001.png");
        let image_b = folder_b.join("001.png");
        fs::write(&image_a, png(1)).unwrap();
        fs::write(&image_b, png(2)).unwrap();
        let mut first_snapshot = test_store(&directory);
        let mut second_snapshot = test_store(&directory);

        assert!(first_snapshot.set_folder_image(&folder_a, &image_a));
        assert!(second_snapshot.set_folder_image(&folder_b, &image_b));

        let loaded = test_store(&directory);
        assert!(
            loaded
                .overrides
                .contains_key(&CoverBookIdentity::image_folder(&folder_a))
        );
        assert!(
            loaded
                .overrides
                .contains_key(&CoverBookIdentity::image_folder(&folder_b))
        );
    }

    #[test]
    fn stale_invalid_result_does_not_clear_newer_override() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("book");
        fs::create_dir(&folder).unwrap();
        let first = folder.join("001.png");
        let second = folder.join("002.png");
        fs::write(&first, png(1)).unwrap();
        fs::write(&second, png(2)).unwrap();
        let identity = CoverBookIdentity::image_folder(&folder);
        let mut stale_job_store = test_store(&directory);
        assert!(stale_job_store.set_folder_image(&folder, &first));
        let invalid_source = stale_job_store.resolve(&identity).unwrap();

        let mut user_store = test_store(&directory);
        assert!(user_store.set_folder_image(&folder, &second));
        let replacement = stale_job_store
            .source_after_invalid(&identity, &invalid_source)
            .unwrap();

        assert!(matches!(
            replacement,
            CoverSource::FolderOverride { relative, .. } if relative == Path::new("002.png")
        ));
        let mut loaded = test_store(&directory);
        assert!(matches!(
            loaded.resolve(&identity),
            Some(CoverSource::FolderOverride { relative, .. })
                if relative == Path::new("002.png")
        ));
    }

    #[test]
    fn discarded_external_preparation_removes_uncommitted_copy() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.png");
        fs::write(&source, png(3)).unwrap();
        let identity = CoverBookIdentity::Archive(directory.path().join("book.cbz"));
        let mut store = test_store(&directory);
        let prepared = store.prepare_external(identity, &source).unwrap();
        let copy = prepared.path.clone().unwrap();
        assert!(copy.is_file());

        prepared.discard();

        assert!(!copy.exists());
        assert!(test_store(&directory).overrides.is_empty());
    }
}
