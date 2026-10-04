use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BoundaryDirection {
    Next,
    Prev,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BoundaryEntryDecision {
    LookupBoundary,
    OpenDirectly,
}

pub(super) fn boundary_entry_decision(
    setting_enabled: bool,
    explicit_file_navigation: bool,
) -> BoundaryEntryDecision {
    if setting_enabled && !explicit_file_navigation {
        BoundaryEntryDecision::LookupBoundary
    } else {
        BoundaryEntryDecision::OpenDirectly
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeldDocument {
    Previous,
    Next,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoundaryLookup {
    request_id: u64,
    current_path: PathBuf,
    direction: BoundaryDirection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DocumentBoundary {
    previous_path: PathBuf,
    next_path: PathBuf,
    held_document: HeldDocument,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BoundaryTarget {
    pub(super) path: PathBuf,
    pub(super) is_held_document: bool,
}

#[derive(Debug, Default)]
pub(super) struct DocumentBoundaryState {
    request_id: u64,
    lookup: Option<BoundaryLookup>,
    boundary: Option<DocumentBoundary>,
    retained_load_request_id: Option<u64>,
}

impl DocumentBoundaryState {
    pub(super) fn begin_lookup(
        &mut self,
        current_path: PathBuf,
        direction: BoundaryDirection,
    ) -> u64 {
        self.request_id = self.request_id.wrapping_add(1);
        self.lookup = Some(BoundaryLookup {
            request_id: self.request_id,
            current_path,
            direction,
        });
        self.boundary = None;
        self.retained_load_request_id = None;
        self.request_id
    }

    pub(super) fn complete_lookup(
        &mut self,
        request_id: u64,
        current_path: &Path,
        adjacent_path: Option<PathBuf>,
    ) -> bool {
        if !self.lookup.as_ref().is_some_and(|lookup| {
            lookup.request_id == request_id && lookup.current_path == current_path
        }) {
            return false;
        }
        let lookup = self.lookup.take().expect("validated boundary lookup");
        let Some(adjacent_path) = adjacent_path else {
            return false;
        };

        self.boundary = Some(match lookup.direction {
            BoundaryDirection::Next => DocumentBoundary {
                previous_path: lookup.current_path,
                next_path: adjacent_path,
                held_document: HeldDocument::Previous,
            },
            BoundaryDirection::Prev => DocumentBoundary {
                previous_path: adjacent_path,
                next_path: lookup.current_path,
                held_document: HeldDocument::Next,
            },
        });
        true
    }

    pub(super) fn target(&self, direction: BoundaryDirection) -> Option<BoundaryTarget> {
        let boundary = self.boundary.as_ref()?;
        let (path, target_document) = match direction {
            BoundaryDirection::Next => (&boundary.next_path, HeldDocument::Next),
            BoundaryDirection::Prev => (&boundary.previous_path, HeldDocument::Previous),
        };
        Some(BoundaryTarget {
            path: path.clone(),
            is_held_document: boundary.held_document == target_document,
        })
    }

    pub(super) fn is_visible(&self) -> bool {
        self.boundary.is_some()
    }

    pub(super) fn previous_name(&self) -> String {
        self.boundary
            .as_ref()
            .map(|boundary| document_display_name(&boundary.previous_path))
            .unwrap_or_default()
    }

    pub(super) fn next_name(&self) -> String {
        self.boundary
            .as_ref()
            .map(|boundary| document_display_name(&boundary.next_path))
            .unwrap_or_default()
    }

    pub(super) fn close(&mut self) {
        self.boundary = None;
        self.retained_load_request_id = None;
    }

    pub(super) fn retain_for_load(&mut self, load_request_id: u64) -> bool {
        if self.boundary.is_none() {
            return false;
        }
        self.cancel_lookup();
        self.retained_load_request_id = Some(load_request_id);
        true
    }

    pub(super) fn finish_retained_load(&mut self, load_request_id: u64) -> bool {
        if self.retained_load_request_id != Some(load_request_id) {
            return false;
        }
        self.close();
        true
    }

    pub(super) fn cancel_lookup(&mut self) {
        self.request_id = self.request_id.wrapping_add(1);
        self.lookup = None;
    }

    pub(super) fn invalidate(&mut self) {
        self.cancel_lookup();
        self.boundary = None;
        self.retained_load_request_id = None;
    }
}

pub(super) fn document_display_name(path: &Path) -> String {
    let name = if crate::archive::is_image_ext(path) {
        path.parent().and_then(Path::file_name)
    } else {
        path.file_stem().or_else(|| path.file_name())
    };

    name.map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Document".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(name: &str) -> PathBuf {
        PathBuf::from("/books").join(name)
    }

    #[test]
    fn forward_and_backward_entry_create_the_same_boundary() {
        let mut forward = DocumentBoundaryState::default();
        let request = forward.begin_lookup(path("A.cbz"), BoundaryDirection::Next);
        assert!(forward.complete_lookup(request, &path("A.cbz"), Some(path("B.cbz"))));

        let mut backward = DocumentBoundaryState::default();
        let request = backward.begin_lookup(path("B.cbz"), BoundaryDirection::Prev);
        assert!(backward.complete_lookup(request, &path("B.cbz"), Some(path("A.cbz"))));

        assert_eq!(forward.previous_name(), "A");
        assert_eq!(forward.next_name(), "B");
        assert_eq!(backward.previous_name(), "A");
        assert_eq!(backward.next_name(), "B");
    }

    #[test]
    fn next_selects_b_and_prev_selects_a_from_either_entry_side() {
        for (current, adjacent, entry_direction) in [
            ("A.cbz", "B.cbz", BoundaryDirection::Next),
            ("B.cbz", "A.cbz", BoundaryDirection::Prev),
        ] {
            let mut state = DocumentBoundaryState::default();
            let request = state.begin_lookup(path(current), entry_direction);
            assert!(state.complete_lookup(request, &path(current), Some(path(adjacent))));

            assert_eq!(
                state.target(BoundaryDirection::Next).unwrap().path,
                path("B.cbz")
            );
            assert_eq!(
                state.target(BoundaryDirection::Prev).unwrap().path,
                path("A.cbz")
            );
        }
    }

    #[test]
    fn missing_adjacent_document_does_not_create_a_boundary() {
        let mut state = DocumentBoundaryState::default();
        let request = state.begin_lookup(path("A.cbz"), BoundaryDirection::Next);

        assert!(!state.complete_lookup(request, &path("A.cbz"), None));
        assert!(!state.is_visible());
    }

    #[test]
    fn stale_lookup_results_are_rejected() {
        let mut state = DocumentBoundaryState::default();
        let stale = state.begin_lookup(path("A.cbz"), BoundaryDirection::Next);
        let current = state.begin_lookup(path("B.cbz"), BoundaryDirection::Next);

        assert!(!state.complete_lookup(stale, &path("A.cbz"), Some(path("B.cbz"))));
        assert!(!state.is_visible());
        assert!(state.complete_lookup(current, &path("B.cbz"), Some(path("C.cbz"))));
    }

    #[test]
    fn invalidation_removes_lookup_and_visible_state() {
        let mut state = DocumentBoundaryState::default();
        let request = state.begin_lookup(path("A.cbz"), BoundaryDirection::Next);
        assert!(state.complete_lookup(request, &path("A.cbz"), Some(path("B.cbz"))));

        state.invalidate();

        assert!(!state.is_visible());
        assert!(state.target(BoundaryDirection::Next).is_none());
        assert!(!state.complete_lookup(request, &path("A.cbz"), Some(path("B.cbz"))));
    }

    #[test]
    fn cancelling_a_lookup_rejects_its_late_result() {
        let mut state = DocumentBoundaryState::default();
        let request = state.begin_lookup(path("A.cbz"), BoundaryDirection::Next);

        state.cancel_lookup();

        assert!(!state.complete_lookup(request, &path("A.cbz"), Some(path("B.cbz"))));
        assert!(!state.is_visible());
    }

    #[test]
    fn held_document_is_identified_without_reloading() {
        let mut forward = DocumentBoundaryState::default();
        let request = forward.begin_lookup(path("A.cbz"), BoundaryDirection::Next);
        forward.complete_lookup(request, &path("A.cbz"), Some(path("B.cbz")));
        assert!(
            forward
                .target(BoundaryDirection::Prev)
                .unwrap()
                .is_held_document
        );
        assert!(
            !forward
                .target(BoundaryDirection::Next)
                .unwrap()
                .is_held_document
        );

        let mut backward = DocumentBoundaryState::default();
        let request = backward.begin_lookup(path("B.cbz"), BoundaryDirection::Prev);
        backward.complete_lookup(request, &path("B.cbz"), Some(path("A.cbz")));
        assert!(
            backward
                .target(BoundaryDirection::Next)
                .unwrap()
                .is_held_document
        );
    }

    #[test]
    fn adjacent_document_load_retains_boundary_until_matching_finish() {
        let mut state = DocumentBoundaryState::default();
        let lookup = state.begin_lookup(path("A.cbz"), BoundaryDirection::Next);
        state.complete_lookup(lookup, &path("A.cbz"), Some(path("B.cbz")));

        assert!(state.retain_for_load(10));
        assert!(state.is_visible());
        assert!(!state.finish_retained_load(9));
        assert!(state.is_visible());
        assert!(state.finish_retained_load(10));
        assert!(!state.is_visible());
    }

    #[test]
    fn stale_load_finish_does_not_close_a_new_boundary() {
        let mut state = DocumentBoundaryState::default();
        let lookup = state.begin_lookup(path("A.cbz"), BoundaryDirection::Next);
        state.complete_lookup(lookup, &path("A.cbz"), Some(path("B.cbz")));
        state.retain_for_load(10);

        let lookup = state.begin_lookup(path("C.cbz"), BoundaryDirection::Next);
        state.complete_lookup(lookup, &path("C.cbz"), Some(path("D.cbz")));
        state.retain_for_load(11);

        assert!(!state.finish_retained_load(10));
        assert!(state.is_visible());
        assert_eq!(
            state.target(BoundaryDirection::Next).unwrap().path,
            path("D.cbz")
        );
    }

    #[test]
    fn held_document_navigation_still_closes_boundary_immediately() {
        let mut state = DocumentBoundaryState::default();
        let lookup = state.begin_lookup(path("A.cbz"), BoundaryDirection::Next);
        state.complete_lookup(lookup, &path("A.cbz"), Some(path("B.cbz")));
        assert!(
            state
                .target(BoundaryDirection::Prev)
                .unwrap()
                .is_held_document
        );

        state.close();

        assert!(!state.is_visible());
    }

    #[test]
    fn normal_load_invalidation_closes_a_retained_boundary() {
        let mut state = DocumentBoundaryState::default();
        let lookup = state.begin_lookup(path("A.cbz"), BoundaryDirection::Next);
        state.complete_lookup(lookup, &path("A.cbz"), Some(path("B.cbz")));
        state.retain_for_load(10);

        state.invalidate();

        assert!(!state.is_visible());
        assert!(!state.finish_retained_load(10));
    }

    #[test]
    fn settings_and_explicit_file_navigation_choose_the_expected_path() {
        assert_eq!(
            boundary_entry_decision(true, false),
            BoundaryEntryDecision::LookupBoundary
        );
        assert_eq!(
            boundary_entry_decision(false, false),
            BoundaryEntryDecision::OpenDirectly
        );
        for setting_enabled in [false, true] {
            assert_eq!(
                boundary_entry_decision(setting_enabled, true),
                BoundaryEntryDecision::OpenDirectly
            );
        }
    }

    #[test]
    fn display_names_strip_archive_extensions_and_use_image_directories() {
        assert_eq!(
            document_display_name(Path::new("/books/Chapter 43.2.cbz")),
            "Chapter 43.2"
        );
        assert_eq!(
            document_display_name(Path::new("/books/Chapter 44.1/001.png")),
            "Chapter 44.1"
        );
    }
}
