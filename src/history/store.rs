use super::identity::{HistoryIdentity, classify_document};
use super::persistence::{current_unix_millis, history_path};
use crate::archive::is_image_ext;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoryEntry {
    pub(crate) identity: HistoryIdentity,
    pub(crate) work_path: Option<PathBuf>,
    pub(crate) title: String,
    pub(crate) last_document_path: PathBuf,
    pub(crate) page_index: usize,
    pub(crate) page_count: usize,
    pub(crate) at_document_end: bool,
    pub(crate) last_viewed_unix_ms: u64,
}

#[derive(Debug)]
pub(crate) struct HistoryStore {
    pub(super) entries: Vec<HistoryEntry>,
    pub(super) storage_path: PathBuf,
}

impl HistoryStore {
    pub(crate) fn load(bookshelf_root: Option<&Path>) -> Self {
        Self::load_with_bookshelf_root_from_path(history_path(), bookshelf_root)
    }

    pub(crate) fn record_document(
        &mut self,
        document_path: &Path,
        bookshelf_root: Option<&Path>,
        page_index: usize,
        page_count: usize,
        at_document_end: bool,
    ) -> HistoryIdentity {
        self.record_document_state_at(
            document_path,
            bookshelf_root,
            page_index,
            page_count,
            at_document_end,
            current_unix_millis(),
        )
    }

    pub(crate) fn update_position(
        &mut self,
        identity: &HistoryIdentity,
        page_index: usize,
        page_count: usize,
        at_document_end: bool,
    ) -> bool {
        self.update_position_state_at(
            identity,
            page_index,
            page_count,
            at_document_end,
            current_unix_millis(),
        )
    }

    pub(crate) fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    pub(crate) fn remove(&mut self, identity: &HistoryIdentity) -> bool {
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

    pub(crate) fn clear(&mut self) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        self.entries.clear();
        true
    }

    pub(crate) fn save(&self) {
        if let Err(error) = self.save_to_path(&self.storage_path) {
            eprintln!(
                "履歴を保存できませんでした ({}): {error}",
                self.storage_path.display()
            );
        }
    }

    #[cfg(test)]
    pub(super) fn record_document_at(
        &mut self,
        document_path: &Path,
        bookshelf_root: Option<&Path>,
        page_index: usize,
        page_count: usize,
        timestamp: u64,
    ) -> HistoryIdentity {
        self.record_document_state_at(
            document_path,
            bookshelf_root,
            page_index,
            page_count,
            false,
            timestamp,
        )
    }

    pub(super) fn record_document_state_at(
        &mut self,
        document_path: &Path,
        bookshelf_root: Option<&Path>,
        page_index: usize,
        page_count: usize,
        at_document_end: bool,
        timestamp: u64,
    ) -> HistoryIdentity {
        self.normalize_bookshelf_entries(bookshelf_root);
        let (identity, work_path, title) = classify_document(document_path, bookshelf_root);
        let mut entry = self
            .entries
            .iter()
            .position(|entry| entry.identity == identity)
            .map(|position| self.entries.remove(position))
            .unwrap_or_else(|| HistoryEntry {
                identity: identity.clone(),
                work_path: work_path.clone(),
                title: title.clone(),
                last_document_path: document_path.to_path_buf(),
                page_index,
                page_count,
                at_document_end,
                last_viewed_unix_ms: timestamp,
            });
        entry.work_path = work_path;
        entry.title = title;
        entry.last_document_path = document_path.to_path_buf();
        entry.page_index = page_index;
        entry.page_count = page_count;
        entry.at_document_end = at_document_end;
        entry.last_viewed_unix_ms = timestamp;
        self.entries.insert(0, entry);
        identity
    }

    #[cfg(test)]
    pub(super) fn update_position_at(
        &mut self,
        identity: &HistoryIdentity,
        page_index: usize,
        page_count: usize,
        timestamp: u64,
    ) -> bool {
        self.update_position_state_at(identity, page_index, page_count, false, timestamp)
    }

    pub(super) fn update_position_state_at(
        &mut self,
        identity: &HistoryIdentity,
        page_index: usize,
        page_count: usize,
        at_document_end: bool,
        timestamp: u64,
    ) -> bool {
        let Some(position) = self
            .entries
            .iter()
            .position(|entry| &entry.identity == identity)
        else {
            return false;
        };
        if self.entries[position].page_index == page_index
            && self.entries[position].page_count == page_count
            && self.entries[position].at_document_end == at_document_end
        {
            return false;
        }

        let mut entry = self.entries.remove(position);
        entry.page_index = page_index;
        entry.page_count = page_count;
        entry.at_document_end = at_document_end;
        entry.last_viewed_unix_ms = timestamp;
        self.entries.insert(0, entry);
        true
    }

    pub(super) fn normalize_bookshelf_entries(&mut self, bookshelf_root: Option<&Path>) -> bool {
        let Some(root) = bookshelf_root else {
            return false;
        };
        let mut changed = false;
        let mut normalized = Vec::with_capacity(self.entries.len());
        let mut seen = HashSet::with_capacity(self.entries.len());
        for mut entry in self.entries.drain(..) {
            if is_image_ext(&entry.last_document_path) && entry.last_document_path.starts_with(root)
            {
                let (identity, work_path, title) =
                    classify_document(&entry.last_document_path, Some(root));
                if entry.identity != identity
                    || entry.work_path != work_path
                    || entry.title != title
                {
                    entry.identity = identity;
                    entry.work_path = work_path;
                    entry.title = title;
                    changed = true;
                }
            }
            if !seen.insert(entry.identity.clone()) {
                changed = true;
            } else {
                normalized.push(entry);
            }
        }
        self.entries = normalized;
        changed
    }
}
