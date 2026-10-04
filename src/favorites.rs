use crate::archive::{first_direct_image_path, is_image_ext};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const FAVORITES_DIRECTORY: &str = "agnam";
const FAVORITES_FILENAME: &str = "favorites.ini";
const FAVORITES_GROUP: &str = "Favorites";
const ENTRY_GROUP_PREFIX: &str = "Entry ";
const FORMAT_VERSION: i32 = 2;
const LEGACY_FORMAT_VERSION: i32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum FavoriteIdentity {
    FileDocument(PathBuf),
    ImageFolderDocument(PathBuf),
}

impl FavoriteIdentity {
    pub(crate) fn from_document_path(path: &Path) -> Option<Self> {
        if is_image_ext(path) {
            let parent = path.parent()?.to_path_buf();
            if parent.as_os_str().is_empty() {
                return None;
            }
            Some(Self::ImageFolderDocument(parent))
        } else {
            Some(Self::FileDocument(path.to_path_buf()))
        }
    }

    pub(crate) fn image_folder(path: &Path) -> Self {
        Self::ImageFolderDocument(path.to_path_buf())
    }

    pub(crate) fn path(&self) -> &Path {
        match self {
            Self::FileDocument(path) | Self::ImageFolderDocument(path) => path,
        }
    }

    pub(crate) fn resolve_entry_path(&self) -> Option<PathBuf> {
        match self {
            Self::FileDocument(path) => Some(path.clone()),
            Self::ImageFolderDocument(path) => first_direct_image_path(path),
        }
    }

    fn storage_kind(&self) -> &'static str {
        match self {
            Self::FileDocument(_) => "file-document",
            Self::ImageFolderDocument(_) => "image-folder-document",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FavoriteEntry {
    pub(crate) identity: FavoriteIdentity,
}

#[derive(Debug)]
pub(crate) struct FavoritesStore {
    entries: Vec<FavoriteEntry>,
    storage_path: PathBuf,
}

impl FavoritesStore {
    pub(crate) fn load() -> Self {
        Self::load_or_empty_from_path(favorites_path())
    }

    pub(crate) fn entries(&self) -> &[FavoriteEntry] {
        &self.entries
    }

    pub(crate) fn contains(&self, identity: &FavoriteIdentity) -> bool {
        self.entries.iter().any(|entry| &entry.identity == identity)
    }

    pub(crate) fn add(&mut self, identity: FavoriteIdentity) -> bool {
        if self.contains(&identity) {
            return false;
        }
        self.entries.insert(0, FavoriteEntry { identity });
        true
    }

    pub(crate) fn remove(&mut self, identity: &FavoriteIdentity) -> bool {
        let Some(position) = self
            .entries
            .iter()
            .position(|entry| &entry.identity == identity)
        else {
            return false;
        };
        self.entries.remove(position);
        true
    }

    pub(crate) fn save(&self) {
        if let Err(error) = self.save_to_path(&self.storage_path) {
            eprintln!(
                "お気に入りを保存できませんでした ({}): {error}",
                self.storage_path.display()
            );
        }
    }

    fn load_or_empty_from_path(path: PathBuf) -> Self {
        if !path.exists() {
            return Self {
                entries: Vec::new(),
                storage_path: path,
            };
        }

        match Self::load_from_path(&path) {
            Ok(entries) => Self {
                entries,
                storage_path: path,
            },
            Err(error) => {
                eprintln!(
                    "お気に入りを読み込めないため空のお気に入りを使用します ({}): {error}",
                    path.display()
                );
                Self {
                    entries: Vec::new(),
                    storage_path: path,
                }
            }
        }
    }

    fn load_from_path(path: &Path) -> Result<Vec<FavoriteEntry>, glib::Error> {
        let key_file = glib::KeyFile::new();
        key_file.load_from_file(path, glib::KeyFileFlags::NONE)?;
        let version = key_file.integer(FAVORITES_GROUP, "format-version").ok();
        if !matches!(version, Some(FORMAT_VERSION) | Some(LEGACY_FORMAT_VERSION)) {
            return Ok(Vec::new());
        }
        let entry_count = key_file
            .integer(FAVORITES_GROUP, "entry-count")
            .ok()
            .filter(|count| *count >= 0)
            .unwrap_or(0) as usize;
        let mut entry_indices = key_file
            .groups()
            .iter()
            .filter_map(|group| {
                group
                    .strip_prefix(ENTRY_GROUP_PREFIX)?
                    .parse::<usize>()
                    .ok()
            })
            .filter(|index| *index < entry_count)
            .collect::<Vec<_>>();
        entry_indices.sort_unstable();
        entry_indices.dedup();

        let mut identities = HashSet::new();
        Ok(entry_indices
            .into_iter()
            .filter_map(|index| {
                let group = entry_group(index);
                let value = key_file
                    .string(&group, "path")
                    .ok()
                    .filter(|value| !value.is_empty())?;
                let path = PathBuf::from(value.as_str());
                let identity = if version == Some(LEGACY_FORMAT_VERSION) {
                    FavoriteIdentity::from_document_path(&path)?
                } else {
                    match key_file.string(&group, "kind").ok()?.as_str() {
                        "file-document" => FavoriteIdentity::FileDocument(path),
                        "image-folder-document" => FavoriteIdentity::ImageFolderDocument(path),
                        _ => return None,
                    }
                };
                identities
                    .insert(identity.clone())
                    .then_some(FavoriteEntry { identity })
            })
            .collect())
    }

    fn save_to_path(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let key_file = glib::KeyFile::new();
        key_file.set_integer(FAVORITES_GROUP, "format-version", FORMAT_VERSION);
        let entry_count = i32::try_from(self.entries.len()).unwrap_or(i32::MAX);
        key_file.set_integer(FAVORITES_GROUP, "entry-count", entry_count);
        for (index, entry) in self.entries.iter().take(entry_count as usize).enumerate() {
            let group = entry_group(index);
            key_file.set_string(&group, "kind", entry.identity.storage_kind());
            key_file.set_string(
                &group,
                "path",
                entry.identity.path().to_string_lossy().as_ref(),
            );
        }
        key_file.save_to_file(path)?;
        Ok(())
    }
}

fn entry_group(index: usize) -> String {
    format!("{ENTRY_GROUP_PREFIX}{index}")
}

fn favorites_path() -> PathBuf {
    glib::user_config_dir()
        .join(FAVORITES_DIRECTORY)
        .join(FAVORITES_FILENAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(path: PathBuf) -> FavoritesStore {
        FavoritesStore {
            entries: Vec::new(),
            storage_path: path,
        }
    }

    fn file(path: &str) -> FavoriteIdentity {
        FavoriteIdentity::FileDocument(PathBuf::from(path))
    }

    fn folder(path: &str) -> FavoriteIdentity {
        FavoriteIdentity::ImageFolderDocument(PathBuf::from(path))
    }

    #[test]
    fn document_paths_normalize_to_file_and_image_folder_identities() {
        assert_eq!(
            FavoriteIdentity::from_document_path(Path::new("/books/01.cbz")),
            Some(file("/books/01.cbz"))
        );
        assert_eq!(
            FavoriteIdentity::from_document_path(Path::new("/books/01/001.jpg")),
            Some(folder("/books/01"))
        );
    }

    #[test]
    fn images_in_the_same_folder_are_one_favorite() {
        let directory = tempfile::tempdir().unwrap();
        let mut favorites = store(directory.path().join("favorites.ini"));
        let first = FavoriteIdentity::from_document_path(Path::new("/books/01/001.jpg")).unwrap();
        let second = FavoriteIdentity::from_document_path(Path::new("/books/01/002.jpg")).unwrap();

        assert!(favorites.add(first));
        assert!(!favorites.add(second));
        assert_eq!(favorites.entries.len(), 1);
    }

    #[test]
    fn add_remove_and_readd_preserve_expected_order() {
        let directory = tempfile::tempdir().unwrap();
        let mut favorites = store(directory.path().join("favorites.ini"));
        let first = file("/books/01.cbz");
        let second = folder("/books/02");

        assert!(favorites.add(first.clone()));
        assert!(favorites.add(second.clone()));
        assert_eq!(favorites.entries[0].identity, second);
        assert!(favorites.remove(&first));
        assert!(!favorites.remove(&first));
        assert!(favorites.add(first.clone()));
        assert_eq!(favorites.entries[0].identity, first);
    }

    #[test]
    fn file_and_folder_favorites_round_trip_with_explicit_kinds() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("favorites.ini");
        let mut favorites = store(path.clone());
        favorites.add(file("/books/01.cbz"));
        favorites.add(folder("/books/02"));
        favorites.save_to_path(&path).unwrap();

        let loaded = FavoritesStore::load_or_empty_from_path(path);
        assert_eq!(loaded.entries, favorites.entries);
    }

    #[test]
    fn legacy_image_paths_normalize_and_deduplicate_by_parent_folder() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("favorites.ini");
        let key_file = glib::KeyFile::new();
        key_file.set_integer(FAVORITES_GROUP, "format-version", LEGACY_FORMAT_VERSION);
        key_file.set_integer(FAVORITES_GROUP, "entry-count", 3);
        key_file.set_string("Entry 0", "path", "/books/01/001.jpg");
        key_file.set_string("Entry 1", "path", "/books/01/002.jpg");
        key_file.set_string("Entry 2", "path", "/books/02.cbz");
        key_file.save_to_file(&path).unwrap();

        let loaded = FavoritesStore::load_or_empty_from_path(path);
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries[0].identity, folder("/books/01"));
        assert_eq!(loaded.entries[1].identity, file("/books/02.cbz"));
    }

    #[test]
    fn malformed_missing_and_duplicate_entries_are_skipped_safely() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("favorites.ini");
        let key_file = glib::KeyFile::new();
        key_file.set_integer(FAVORITES_GROUP, "format-version", FORMAT_VERSION);
        key_file.set_integer(FAVORITES_GROUP, "entry-count", 5);
        key_file.set_string("Entry 0", "kind", "file-document");
        key_file.set_string("Entry 0", "path", "/books/valid.cbz");
        key_file.set_string("Entry 1", "kind", "unknown");
        key_file.set_string("Entry 1", "path", "/books/ignored.cbz");
        key_file.set_string("Entry 3", "kind", "file-document");
        key_file.set_string("Entry 3", "path", "/books/valid.cbz");
        key_file.save_to_file(&path).unwrap();

        let loaded = FavoritesStore::load_or_empty_from_path(path);
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].identity, file("/books/valid.cbz"));
    }

    #[test]
    fn unreadable_file_falls_back_to_empty_store() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("favorites.ini");
        fs::write(&path, b"not a key file\0").unwrap();

        assert!(
            FavoritesStore::load_or_empty_from_path(path)
                .entries
                .is_empty()
        );
    }
}
