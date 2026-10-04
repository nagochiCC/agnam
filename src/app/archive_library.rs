use crate::archive::{
    ArchiveContentItemKind, ArchiveContentLevel, ArchiveEntryReader, ArchiveImageId,
    ArchiveLocation,
};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(super) struct ArchiveThumbnailSource {
    pub(super) reader: Arc<ArchiveEntryReader>,
    pub(super) entry: PathBuf,
    pub(super) backing: Option<Arc<super::archive_thumbnail_backing::ArchiveThumbnailBacking>>,
}

#[derive(Debug)]
pub(super) struct ArchiveLoadRequest {
    pub(super) id: u64,
    pub(super) location: ArchiveLocation,
}

#[derive(Debug, Default)]
pub(super) struct ArchiveLibraryState {
    active: bool,
    next_request_id: u64,
    active_request_id: Option<u64>,
    location: Option<ArchiveLocation>,
    level: Option<ArchiveContentLevel>,
    error: Option<String>,
    thumbnail_sources: HashMap<PathBuf, ArchiveThumbnailSource>,
    progressive_thumbnails: Option<crate::archive::ProgressiveArchiveCancelToken>,
    thumbnail_backing: Option<Arc<super::archive_thumbnail_backing::ArchiveThumbnailBacking>>,
}

impl ArchiveLibraryState {
    pub(super) fn is_active(&self) -> bool {
        self.active
    }

    pub(super) fn is_loading(&self) -> bool {
        self.active && self.active_request_id.is_some()
    }

    pub(super) fn level(&self) -> Option<&ArchiveContentLevel> {
        self.level.as_ref()
    }

    pub(super) fn location(&self) -> Option<&ArchiveLocation> {
        self.location.as_ref()
    }

    pub(super) fn status_message(&self) -> Option<String> {
        if !self.active {
            return None;
        }
        if self.is_loading() {
            return Some("アーカイブの内容を読み込んでいます…".into());
        }
        if let Some(error) = &self.error {
            return Some(format!("アーカイブの内容を表示できません\n{error}"));
        }
        self.level
            .as_ref()
            .filter(|level| level.items.is_empty())
            .map(|_| "表示できる画像やアーカイブがありません".into())
    }

    pub(super) fn has_items(&self) -> bool {
        self.level
            .as_ref()
            .is_some_and(|level| !level.items.is_empty())
    }

    pub(super) fn begin(&mut self, location: ArchiveLocation) -> ArchiveLoadRequest {
        let backing = match super::archive_thumbnail_backing::ArchiveThumbnailBacking::create() {
            Ok(backing) => Some(Arc::new(backing)),
            Err(error) => {
                eprintln!("archive thumbnail session backingを作成できませんでした: {error}");
                None
            }
        };
        self.begin_with_backing(location, backing)
    }

    fn begin_with_backing(
        &mut self,
        location: ArchiveLocation,
        backing: Option<Arc<super::archive_thumbnail_backing::ArchiveThumbnailBacking>>,
    ) -> ArchiveLoadRequest {
        self.cancel_progressive_thumbnails();
        self.active = true;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        self.active_request_id = Some(self.next_request_id);
        self.location = Some(location.clone());
        self.level = None;
        self.error = None;
        self.thumbnail_sources.clear();
        self.thumbnail_backing = backing;
        ArchiveLoadRequest {
            id: self.next_request_id,
            location,
        }
    }

    #[cfg(test)]
    pub(super) fn begin_for_test(
        &mut self,
        location: ArchiveLocation,
        backing_directory: &Path,
    ) -> ArchiveLoadRequest {
        let backing =
            super::archive_thumbnail_backing::ArchiveThumbnailBacking::create_in(backing_directory)
                .unwrap();
        self.begin_with_backing(location, Some(Arc::new(backing)))
    }

    pub(super) fn apply(
        &mut self,
        request_id: u64,
        result: Result<ArchiveContentLevel, String>,
    ) -> bool {
        if !self.active || self.active_request_id != Some(request_id) {
            return false;
        }
        self.active_request_id = None;
        match result {
            Ok(level) => {
                self.location = Some(level.location.clone());
                self.level = Some(level);
                self.error = None;
                self.rebuild_thumbnail_sources();
            }
            Err(error) => {
                self.level = None;
                self.error = Some(error);
                self.thumbnail_sources.clear();
            }
        }
        true
    }

    pub(super) fn navigate_directory(&mut self, directory: PathBuf) -> bool {
        let Some(level) = self.level.as_mut() else {
            return false;
        };
        if !level.navigate_directory(directory) {
            return false;
        }
        self.location = Some(level.location.clone());
        self.rebuild_thumbnail_sources();
        true
    }

    pub(super) fn leave(&mut self) {
        self.cancel_progressive_thumbnails();
        self.active = false;
        self.active_request_id = None;
        self.location = None;
        self.level = None;
        self.error = None;
        self.thumbnail_sources.clear();
        self.thumbnail_backing = None;
    }

    pub(super) fn begin_progressive_thumbnails(
        &mut self,
    ) -> crate::archive::ProgressiveArchiveCancelToken {
        self.cancel_progressive_thumbnails();
        let token = crate::archive::ProgressiveArchiveCancelToken::default();
        self.progressive_thumbnails = Some(token.clone());
        token
    }

    pub(super) fn progressive_thumbnails_active(&self) -> bool {
        self.progressive_thumbnails.is_some()
    }

    pub(super) fn accepts_session(&self, session_id: u64) -> bool {
        self.active && self.next_request_id == session_id
    }

    pub(super) fn finish_progressive_thumbnails(&mut self, session_id: u64) -> bool {
        if !self.accepts_session(session_id) {
            return false;
        }
        self.progressive_thumbnails = None;
        true
    }

    fn cancel_progressive_thumbnails(&mut self) {
        if let Some(token) = self.progressive_thumbnails.take() {
            token.cancel();
        }
    }

    pub(super) fn thumbnail_source(&self, key: &Path) -> Option<ArchiveThumbnailSource> {
        self.thumbnail_sources.get(key).cloned()
    }

    pub(super) fn thumbnail_backing(
        &self,
    ) -> Option<Arc<super::archive_thumbnail_backing::ArchiveThumbnailBacking>> {
        self.thumbnail_backing.clone()
    }

    pub(super) fn thumbnail_key(archive: &Path, id: &ArchiveImageId) -> PathBuf {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        archive.hash(&mut hasher);
        id.hash(&mut hasher);
        PathBuf::from(format!(".agnam-archive-thumbnail-{:016x}", hasher.finish()))
    }

    fn rebuild_thumbnail_sources(&mut self) {
        self.thumbnail_sources.clear();
        let Some(level) = self.level.as_ref() else {
            return;
        };
        for item in &level.items {
            if !matches!(item.kind, ArchiveContentItemKind::Image { .. }) {
                continue;
            }
            let id = level.location.image_id(item.path.clone());
            let key = Self::thumbnail_key(&level.location.archive, &id);
            self.thumbnail_sources.insert(
                key,
                ArchiveThumbnailSource {
                    reader: level.reader.clone(),
                    entry: item.path.clone(),
                    backing: self.thumbnail_backing.clone(),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn begin(
        state: &mut ArchiveLibraryState,
        location: ArchiveLocation,
        root: &Path,
    ) -> ArchiveLoadRequest {
        let backing =
            super::super::archive_thumbnail_backing::ArchiveThumbnailBacking::create_in(root)
                .unwrap();
        state.begin_with_backing(location, Some(Arc::new(backing)))
    }

    #[test]
    fn stale_listing_completion_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let mut state = ArchiveLibraryState::default();
        let first = begin(
            &mut state,
            ArchiveLocation::root(PathBuf::from("first.cbz")),
            root.path(),
        );
        let second = begin(
            &mut state,
            ArchiveLocation::root(PathBuf::from("second.cbz")),
            root.path(),
        );
        assert!(!state.apply(first.id, Err("stale".into())));
        assert!(state.apply(second.id, Err("current".into())));
        assert_eq!(
            state.status_message().as_deref(),
            Some("アーカイブの内容を表示できません\ncurrent")
        );
    }

    #[test]
    fn a_new_archive_session_rejects_old_progressive_thumbnail_results() {
        let root = tempfile::tempdir().unwrap();
        let mut state = ArchiveLibraryState::default();
        let first = begin(
            &mut state,
            ArchiveLocation::root(PathBuf::from("first.cbz")),
            root.path(),
        );
        state.begin_progressive_thumbnails();
        let second = begin(
            &mut state,
            ArchiveLocation::root(PathBuf::from("second.cbz")),
            root.path(),
        );

        assert!(!state.accepts_session(first.id));
        assert!(state.accepts_session(second.id));
        assert!(!state.finish_progressive_thumbnails(first.id));
    }
}
