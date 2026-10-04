use crate::bookshelf::BookshelfDirectory;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) struct ScanRequest {
    pub(in crate::app) id: u64,
    pub(in crate::app) path: PathBuf,
    pub(in crate::app) persist_snapshot: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum ScanCompletion {
    Stale,
    InitialSuccess,
    RefreshUnchanged,
    RefreshChanged,
    InitialFailed,
    RefreshFailed,
}

#[derive(Debug, Default)]
pub(in crate::app) struct LibraryState {
    root: Option<PathBuf>,
    current_directory: Option<PathBuf>,
    next_request_id: u64,
    active_request_id: Option<u64>,
    scanning: bool,
    directories: HashMap<PathBuf, BookshelfDirectory>,
    error: Option<String>,
}

impl LibraryState {
    pub(in crate::app) fn new(root: Option<PathBuf>) -> Self {
        let mut state = Self {
            current_directory: root.clone(),
            root,
            ..Self::default()
        };
        state.seed_root_snapshot();
        state
    }

    pub(in crate::app) fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    pub(in crate::app) fn current_directory(&self) -> Option<&Path> {
        self.current_directory.as_deref()
    }

    pub(in crate::app) fn directory(&self) -> Option<&BookshelfDirectory> {
        self.current_directory
            .as_ref()
            .and_then(|path| self.directories.get(path))
    }

    pub(in crate::app) fn cached_directory_paths(&self) -> Vec<PathBuf> {
        self.directories.keys().cloned().collect()
    }

    pub(in crate::app) fn has_directory_items(&self) -> bool {
        self.directory().is_some_and(|directory| {
            !directory.child_shelves.is_empty() || !directory.direct_files.is_empty()
        })
    }

    pub(in crate::app) fn is_scanning(&self) -> bool {
        self.scanning
    }

    pub(in crate::app) fn status_message(&self) -> Option<String> {
        if self.root.is_none() {
            return Some("本棚フォルダが設定されていません".into());
        }
        if let Some(error) = &self.error {
            return Some(format!("本棚フォルダを開けません\n{error}"));
        }
        self.directory()
            .filter(|directory| {
                directory.child_shelves.is_empty() && directory.direct_files.is_empty()
            })
            .map(|_| "表示できるフォルダやファイルがありません".into())
    }

    pub(in crate::app) fn can_select_root(&self) -> bool {
        self.root.is_none() || self.error.is_some()
    }

    pub(in crate::app) fn set_root(&mut self, root: Option<PathBuf>) {
        self.invalidate_scan();
        let root_changed = self.root != root;
        if root_changed {
            self.directories.clear();
        }
        self.root = root.clone();
        self.current_directory = root;
        if root_changed {
            self.seed_root_snapshot();
        }
        self.prune_directory_cache();
        self.error = None;
    }

    pub(in crate::app) fn begin_current_scan(&mut self) -> Option<ScanRequest> {
        let path = self.current_directory.clone()?;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        let id = self.next_request_id;
        self.active_request_id = Some(id);
        self.scanning = true;
        self.error = None;
        let persist_snapshot = self.root.as_deref() == Some(path.as_path());
        Some(ScanRequest {
            id,
            path,
            persist_snapshot,
        })
    }

    pub(in crate::app) fn navigate(&mut self, path: PathBuf) -> Option<ScanRequest> {
        let root = self.root.as_deref()?;
        if !path_is_within_root(root, &path) {
            return None;
        }
        self.current_directory = Some(path);
        self.prune_directory_cache();
        self.begin_current_scan()
    }

    pub(in crate::app) fn apply_scan(
        &mut self,
        request_id: u64,
        path: &Path,
        result: Result<BookshelfDirectory, String>,
    ) -> ScanCompletion {
        if self.active_request_id != Some(request_id)
            || self.current_directory.as_deref() != Some(path)
        {
            return ScanCompletion::Stale;
        }

        self.active_request_id = None;
        self.scanning = false;
        match result {
            Ok(directory) => {
                self.error = None;
                match self.directories.get(path) {
                    Some(cached) if cached == &directory => ScanCompletion::RefreshUnchanged,
                    Some(_) => {
                        self.directories.insert(path.to_path_buf(), directory);
                        ScanCompletion::RefreshChanged
                    }
                    None => {
                        self.directories.insert(path.to_path_buf(), directory);
                        ScanCompletion::InitialSuccess
                    }
                }
            }
            Err(error) => {
                if self.directories.contains_key(path) {
                    ScanCompletion::RefreshFailed
                } else {
                    self.error = Some(error);
                    ScanCompletion::InitialFailed
                }
            }
        }
    }

    fn prune_directory_cache(&mut self) {
        let (Some(root), Some(current)) = (&self.root, &self.current_directory) else {
            self.directories.clear();
            return;
        };
        self.directories.retain(|path, _| {
            path_is_within_root(root, path) && current.strip_prefix(path).is_ok()
        });
    }

    fn seed_root_snapshot(&mut self) {
        let Some(root) = self.root.as_deref() else {
            return;
        };
        if let Some(directory) = crate::bookshelf::snapshot::load_root_snapshot(root) {
            self.directories.insert(root.to_path_buf(), directory);
        }
    }

    #[cfg(test)]
    pub(in crate::app) fn seed_snapshot_for_test(&mut self, directory: BookshelfDirectory) {
        if self.root.as_deref() == Some(directory.path.as_path()) {
            self.directories.insert(directory.path.clone(), directory);
        }
    }

    fn invalidate_scan(&mut self) {
        self.active_request_id = None;
        self.scanning = false;
    }
}

pub(in crate::app) fn path_is_within_root(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).is_ok_and(|relative| {
        relative
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
    })
}
