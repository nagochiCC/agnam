use super::identity::HistoryIdentity;
use super::store::{HistoryEntry, HistoryStore};
use gtk::glib;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const HISTORY_DIRECTORY: &str = "agnam";
const HISTORY_FILENAME: &str = "history.ini";
pub(super) const HISTORY_GROUP: &str = "History";
const ENTRY_GROUP_PREFIX: &str = "Entry ";
pub(super) const FORMAT_VERSION: i32 = 2;

impl HistoryStore {
    pub(crate) fn load_or_empty_from_path(path: PathBuf) -> Self {
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
                    "履歴を読み込めないため空の履歴を使用します ({}): {error}",
                    path.display()
                );
                Self {
                    entries: Vec::new(),
                    storage_path: path,
                }
            }
        }
    }

    pub(super) fn load_with_bookshelf_root_from_path(
        path: PathBuf,
        bookshelf_root: Option<&Path>,
    ) -> Self {
        let mut history = Self::load_or_empty_from_path(path);
        if history.normalize_bookshelf_entries(bookshelf_root) {
            history.save();
        }
        history
    }

    fn load_from_path(path: &Path) -> Result<Vec<HistoryEntry>, glib::Error> {
        let key_file = glib::KeyFile::new();
        key_file.load_from_file(path, glib::KeyFileFlags::NONE)?;
        if key_file.integer(HISTORY_GROUP, "format-version").ok() != Some(FORMAT_VERSION) {
            return Ok(Vec::new());
        }
        let entry_count = key_file
            .integer(HISTORY_GROUP, "entry-count")
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

        Ok(entry_indices
            .into_iter()
            .filter_map(|index| load_entry(&key_file, index))
            .collect())
    }

    pub(super) fn save_to_path(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let key_file = glib::KeyFile::new();
        key_file.set_integer(HISTORY_GROUP, "format-version", FORMAT_VERSION);
        let entry_count = i32::try_from(self.entries.len()).unwrap_or(i32::MAX);
        key_file.set_integer(HISTORY_GROUP, "entry-count", entry_count);
        for (index, entry) in self.entries.iter().take(entry_count as usize).enumerate() {
            let group = entry_group(index);
            key_file.set_string(&group, "identity-kind", entry.identity.storage_kind());
            key_file.set_string(
                &group,
                "identity-path",
                entry.identity.path().to_string_lossy().as_ref(),
            );
            if let Some(work_path) = &entry.work_path {
                key_file.set_string(&group, "work-path", work_path.to_string_lossy().as_ref());
            }
            key_file.set_string(&group, "title", &entry.title);
            key_file.set_string(
                &group,
                "last-document",
                entry.last_document_path.to_string_lossy().as_ref(),
            );
            key_file.set_string(&group, "page-index", &entry.page_index.to_string());
            key_file.set_string(&group, "page-count", &entry.page_count.to_string());
            key_file.set_boolean(&group, "at-document-end", entry.at_document_end);
            key_file.set_string(
                &group,
                "last-viewed-unix-ms",
                &entry.last_viewed_unix_ms.to_string(),
            );
        }
        key_file.save_to_file(path)?;
        Ok(())
    }
}

fn load_entry(key_file: &glib::KeyFile, index: usize) -> Option<HistoryEntry> {
    let group = entry_group(index);
    let identity_path = required_path(key_file, &group, "identity-path")?;
    let kind = key_file.string(&group, "identity-kind").ok()?;
    let identity = match kind.as_str() {
        "bookshelf-work" => HistoryIdentity::BookshelfWork(identity_path),
        "document" => HistoryIdentity::Document(identity_path),
        _ => return None,
    };
    let work_path = match &identity {
        HistoryIdentity::BookshelfWork(path) => Some(
            key_file
                .string(&group, "work-path")
                .ok()
                .filter(|value| !value.is_empty())
                .map(|value| PathBuf::from(value.as_str()))
                .unwrap_or_else(|| path.clone()),
        ),
        HistoryIdentity::Document(_) => None,
    };
    let title = key_file.string(&group, "title").ok()?.to_string();
    if title.is_empty() {
        return None;
    }

    Some(HistoryEntry {
        identity,
        work_path,
        title,
        last_document_path: required_path(key_file, &group, "last-document")?,
        page_index: required_number(key_file, &group, "page-index")?,
        page_count: required_number(key_file, &group, "page-count")?,
        at_document_end: key_file.boolean(&group, "at-document-end").unwrap_or(false),
        last_viewed_unix_ms: required_number(key_file, &group, "last-viewed-unix-ms")?,
    })
}

fn required_path(key_file: &glib::KeyFile, group: &str, key: &str) -> Option<PathBuf> {
    key_file
        .string(group, key)
        .ok()
        .filter(|value| !value.is_empty())
        .map(|value| PathBuf::from(value.as_str()))
}

fn required_number<T: std::str::FromStr>(
    key_file: &glib::KeyFile,
    group: &str,
    key: &str,
) -> Option<T> {
    key_file.string(group, key).ok()?.parse().ok()
}

fn entry_group(index: usize) -> String {
    format!("{ENTRY_GROUP_PREFIX}{index}")
}

pub(super) fn history_path() -> PathBuf {
    glib::user_config_dir()
        .join(HISTORY_DIRECTORY)
        .join(HISTORY_FILENAME)
}

pub(super) fn current_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
