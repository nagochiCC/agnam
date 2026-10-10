use crate::favorites::FavoriteIdentity;
use crate::history::HistoryIdentity;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InitialPage {
    LoaderDefault,
    End,
    Saved(usize),
}

impl InitialPage {
    pub(super) fn resolve(self, loader_default: usize, page_count: usize) -> usize {
        match self {
            Self::LoaderDefault => loader_default,
            Self::End => page_count.saturating_sub(1),
            Self::Saved(page_index) => page_index,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LoadPurpose {
    Normal,
    StartupRestore {
        fallback_root: PathBuf,
    },
    History {
        identity: HistoryIdentity,
        path: PathBuf,
    },
    Favorite {
        identity: FavoriteIdentity,
    },
}

impl LoadPurpose {
    pub(super) fn fallback_root(&self) -> Option<&Path> {
        match self {
            Self::Normal => None,
            Self::StartupRestore { fallback_root } => Some(fallback_root),
            Self::History { .. } => None,
            Self::Favorite { .. } => None,
        }
    }

    pub(super) fn is_startup_restore(&self) -> bool {
        matches!(self, Self::StartupRestore { .. })
    }

    pub(super) fn is_history(&self) -> bool {
        matches!(self, Self::History { .. })
    }

    pub(super) fn history_target(&self) -> Option<(&HistoryIdentity, &Path)> {
        match self {
            Self::History { identity, path } => Some((identity, path)),
            _ => None,
        }
    }

    pub(super) fn favorite_target(&self) -> Option<&FavoriteIdentity> {
        match self {
            Self::Favorite { identity } => Some(identity),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct ActiveLoadRequest {
    id: u64,
    purpose: LoadPurpose,
    replacement: bool,
    cancel: crate::archive::ProgressiveArchiveCancelToken,
}

#[derive(Debug, Default)]
pub(super) struct DocumentLoadController {
    request_id: u64,
    active: Option<ActiveLoadRequest>,
    loading_visible: bool,
    loading_revision: u64,
}

impl DocumentLoadController {
    pub(super) fn begin(&mut self, purpose: LoadPurpose) -> (u64, u64) {
        self.begin_with_replacement(purpose, false)
    }

    pub(super) fn begin_replacement(&mut self, purpose: LoadPurpose) -> (u64, u64) {
        self.begin_with_replacement(purpose, true)
    }

    fn begin_with_replacement(&mut self, purpose: LoadPurpose, replacement: bool) -> (u64, u64) {
        if let Some(active) = &self.active {
            active.cancel.cancel();
        }
        self.request_id = self.request_id.wrapping_add(1);
        let request_id = self.request_id;
        self.active = Some(ActiveLoadRequest {
            id: request_id,
            purpose,
            replacement,
            cancel: Default::default(),
        });
        self.loading_visible = false;
        let loading_revision = self.advance_loading_revision();
        (request_id, loading_revision)
    }

    pub(super) fn cancel_token(
        &self,
        request_id: u64,
    ) -> crate::archive::ProgressiveArchiveCancelToken {
        self.active
            .as_ref()
            .filter(|request| request.id == request_id)
            .expect("cancel token belongs to an active load")
            .cancel
            .clone()
    }

    pub(super) fn active_request_id(&self) -> Option<u64> {
        self.active.as_ref().map(|request| request.id)
    }

    pub(super) fn is_active(&self, request_id: u64) -> bool {
        self.active_request_id() == Some(request_id)
    }

    pub(super) fn has_active_request(&self) -> bool {
        self.active.is_some()
    }

    pub(super) fn is_replacement(&self, request_id: u64) -> bool {
        self.active
            .as_ref()
            .is_some_and(|request| request.id == request_id && request.replacement)
    }

    pub(super) fn loading_visible(&self) -> bool {
        self.loading_visible
    }

    pub(super) fn restart_loading_delay(&mut self, request_id: u64) -> Option<u64> {
        if !self.is_active(request_id) {
            return None;
        }
        self.loading_visible = false;
        Some(self.advance_loading_revision())
    }

    pub(super) fn show_loading(&mut self, request_id: u64, revision: u64) -> bool {
        if !self.is_active(request_id) || self.loading_revision != revision {
            return false;
        }
        self.loading_visible = true;
        true
    }

    pub(super) fn purpose(&self, request_id: u64) -> Option<&LoadPurpose> {
        self.active
            .as_ref()
            .filter(|request| request.id == request_id)
            .map(|request| &request.purpose)
    }

    pub(super) fn finish(&mut self, request_id: u64) -> Option<LoadPurpose> {
        if !self.is_active(request_id) {
            return None;
        }
        self.loading_visible = false;
        self.advance_loading_revision();
        self.active.take().map(|request| request.purpose)
    }

    pub(super) fn cancel(&mut self) {
        if let Some(active) = self.active.take() {
            active.cancel.cancel();
        }
        self.loading_visible = false;
        self.advance_loading_revision();
    }

    fn advance_loading_revision(&mut self) -> u64 {
        self.loading_revision = self.loading_revision.wrapping_add(1);
        self.loading_revision
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SiblingNavigationTarget {
    pub(super) path: PathBuf,
    pub(super) start_at_end: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SiblingLookupCompletion {
    Stale,
    Missing,
    Found(SiblingNavigationTarget),
}

#[derive(Debug)]
struct ActiveSiblingLookup {
    id: u64,
    document_path: PathBuf,
    document_generation: u64,
    start_at_end: bool,
}

#[derive(Debug, Default)]
pub(super) struct SiblingLookupController {
    request_id: u64,
    active: Option<ActiveSiblingLookup>,
}

impl SiblingLookupController {
    pub(super) fn begin(
        &mut self,
        document_path: PathBuf,
        document_generation: u64,
        start_at_end: bool,
    ) -> u64 {
        self.request_id = self.request_id.wrapping_add(1);
        let request_id = self.request_id;
        self.active = Some(ActiveSiblingLookup {
            id: request_id,
            document_path,
            document_generation,
            start_at_end,
        });
        request_id
    }

    pub(super) fn complete(
        &mut self,
        request_id: u64,
        current_path: &Path,
        current_generation: u64,
        adjacent_path: Option<PathBuf>,
    ) -> SiblingLookupCompletion {
        let Some(active) = self.active.take() else {
            return SiblingLookupCompletion::Stale;
        };
        if active.id != request_id
            || active.document_path != current_path
            || active.document_generation != current_generation
        {
            if active.id != request_id {
                self.active = Some(active);
            }
            return SiblingLookupCompletion::Stale;
        }
        match adjacent_path {
            Some(path) => SiblingLookupCompletion::Found(SiblingNavigationTarget {
                path,
                start_at_end: active.start_at_end,
            }),
            None => SiblingLookupCompletion::Missing,
        }
    }

    pub(super) fn invalidate(&mut self) {
        self.active = None;
    }

    #[cfg(test)]
    fn is_active(&self) -> bool {
        self.active.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_and_cancel_stop_only_the_matching_background_load() {
        let mut load = DocumentLoadController::default();
        let (old, _) = load.begin(LoadPurpose::Normal);
        let old_token = load.cancel_token(old);
        let (current, _) = load.begin_replacement(LoadPurpose::Normal);
        let current_token = load.cancel_token(current);
        assert!(old_token.is_cancelled());
        assert!(!current_token.is_cancelled());
        assert!(load.finish(old).is_none());
        assert!(!current_token.is_cancelled());
        load.cancel();
        assert!(current_token.is_cancelled());
    }

    #[test]
    fn initial_page_resolution_does_not_index_the_document() {
        assert_eq!(InitialPage::LoaderDefault.resolve(3, 10), 3);
        assert_eq!(InitialPage::End.resolve(3, 0), 0);
        assert_eq!(InitialPage::End.resolve(3, 10), 9);
        assert_eq!(InitialPage::Saved(100).resolve(3, 10), 100);
    }

    #[test]
    fn stale_completion_does_not_consume_the_active_request() {
        let mut controller = DocumentLoadController::default();
        let (first_request, _) = controller.begin(LoadPurpose::StartupRestore {
            fallback_root: PathBuf::from("/old"),
        });
        let (second_request, _) = controller.begin(LoadPurpose::Normal);

        assert_ne!(first_request, second_request);
        assert_eq!(controller.finish(first_request), None);
        assert_eq!(
            controller.purpose(second_request),
            Some(&LoadPurpose::Normal)
        );
        assert_eq!(controller.finish(second_request), Some(LoadPurpose::Normal));
        assert!(!controller.has_active_request());
    }

    #[test]
    fn fast_load_and_stale_notifications_do_not_show_loading() {
        let mut controller = DocumentLoadController::default();
        let (first_request, first_revision) = controller.begin(LoadPurpose::Normal);
        let (second_request, second_revision) = controller.begin(LoadPurpose::Normal);

        assert!(!controller.show_loading(first_request, first_revision));
        assert_eq!(controller.finish(first_request), None);
        assert!(controller.show_loading(second_request, second_revision));
        assert!(controller.loading_visible());

        assert_eq!(controller.finish(second_request), Some(LoadPurpose::Normal));
        assert!(!controller.show_loading(second_request, second_revision));
        assert!(!controller.loading_visible());
    }

    #[test]
    fn restarting_loading_delay_rejects_the_previous_revision() {
        let mut controller = DocumentLoadController::default();
        let (request_id, initial_revision) = controller.begin(LoadPurpose::Normal);
        let pending_revision = controller.restart_loading_delay(request_id).unwrap();

        assert!(!controller.show_loading(request_id, initial_revision));
        assert!(controller.show_loading(request_id, pending_revision));
    }

    #[test]
    fn cancel_clears_request_and_invalidates_loading_notification() {
        let mut controller = DocumentLoadController::default();
        let (request_id, revision) = controller.begin(LoadPurpose::Normal);
        assert!(controller.show_loading(request_id, revision));

        controller.cancel();

        assert!(!controller.has_active_request());
        assert!(!controller.loading_visible());
        assert!(!controller.show_loading(request_id, revision));
        assert_eq!(controller.finish(request_id), None);
    }

    #[test]
    fn only_restore_failures_have_a_bookshelf_fallback() {
        assert_eq!(LoadPurpose::Normal.fallback_root(), None);
        assert_eq!(
            LoadPurpose::StartupRestore {
                fallback_root: PathBuf::from("/books"),
            }
            .fallback_root(),
            Some(Path::new("/books"))
        );
    }

    #[test]
    fn favorite_failures_keep_the_exact_path_for_recovery() {
        let path = PathBuf::from("/books/missing.cbz");
        let identity = FavoriteIdentity::FileDocument(path.clone());
        let purpose = LoadPurpose::Favorite {
            identity: identity.clone(),
        };

        assert_eq!(purpose.favorite_target(), Some(&identity));
        assert_eq!(purpose.fallback_root(), None);
        assert_eq!(purpose.history_target(), None);
    }

    #[test]
    fn replacement_loads_are_distinct_without_changing_normal_load_behavior() {
        let mut controller = DocumentLoadController::default();
        let (normal, _) = controller.begin(LoadPurpose::Normal);
        assert!(!controller.is_replacement(normal));

        let (replacement, _) = controller.begin_replacement(LoadPurpose::Normal);
        assert!(!controller.is_replacement(normal));
        assert!(controller.is_replacement(replacement));
    }

    #[test]
    fn sibling_lookup_reports_found_and_missing_without_starting_a_load() {
        let mut controller = SiblingLookupController::default();
        let load = DocumentLoadController::default();
        let source = PathBuf::from("/books/a.rar");
        let found = controller.begin(source.clone(), 7, false);
        assert_eq!(
            controller.complete(found, &source, 7, Some(PathBuf::from("/books/b.rar"))),
            SiblingLookupCompletion::Found(SiblingNavigationTarget {
                path: PathBuf::from("/books/b.rar"),
                start_at_end: false,
            })
        );
        assert!(!controller.is_active());
        assert!(!load.has_active_request());

        let missing = controller.begin(source.clone(), 7, true);
        assert_eq!(
            controller.complete(missing, &source, 7, None),
            SiblingLookupCompletion::Missing
        );
        assert!(!controller.is_active());
        assert!(!load.has_active_request());
    }

    #[test]
    fn newer_sibling_lookup_and_document_identity_reject_stale_results() {
        let mut controller = SiblingLookupController::default();
        let source = PathBuf::from("/books/a.rar");
        let old = controller.begin(source.clone(), 7, false);
        let current = controller.begin(source.clone(), 7, true);

        assert_eq!(
            controller.complete(old, &source, 7, Some(PathBuf::from("/books/old.rar"))),
            SiblingLookupCompletion::Stale
        );
        assert!(controller.is_active());
        assert_eq!(
            controller.complete(current, &source, 8, Some(PathBuf::from("/books/new.rar"))),
            SiblingLookupCompletion::Stale
        );
        assert!(!controller.is_active());

        let changed_path = controller.begin(source, 9, false);
        assert_eq!(
            controller.complete(
                changed_path,
                Path::new("/books/other.rar"),
                9,
                Some(PathBuf::from("/books/b.rar"))
            ),
            SiblingLookupCompletion::Stale
        );
    }
}
