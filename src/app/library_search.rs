use crate::bookshelf::search::{SearchIndex, SearchItem, SearchItemId, normalize_search_text};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SearchBuildRequest {
    pub(super) id: u64,
    pub(super) root: PathBuf,
}

#[derive(Debug, Default)]
pub(super) struct LibrarySearchState {
    root: Option<PathBuf>,
    query: String,
    next_request_id: u64,
    active_request: Option<SearchBuildRequest>,
    index: Option<SearchIndex>,
    results: Option<Vec<SearchItemId>>,
    error: Option<String>,
}

impl LibrarySearchState {
    pub(super) fn is_active(&self) -> bool {
        self.root.is_some()
    }

    pub(super) fn query(&self) -> &str {
        &self.query
    }

    pub(super) fn begin(&mut self, root: &Path) -> Option<SearchBuildRequest> {
        if self.root.as_deref() == Some(root) {
            return None;
        }
        self.reset();
        self.root = Some(root.to_path_buf());
        self.next_request_id = self.next_request_id.wrapping_add(1);
        let request = SearchBuildRequest {
            id: self.next_request_id,
            root: root.to_path_buf(),
        };
        self.active_request = Some(request.clone());
        Some(request)
    }

    pub(super) fn reset(&mut self) {
        self.root = None;
        self.query.clear();
        self.active_request = None;
        self.index = None;
        self.results = None;
        self.error = None;
    }

    pub(super) fn set_query(&mut self, query: String) -> bool {
        if !self.is_active() || self.query == query {
            return false;
        }
        self.query = query;
        self.refresh_results();
        true
    }

    pub(super) fn has_meaningful_query(&self) -> bool {
        self.is_active() && !normalize_search_text(&self.query).is_empty()
    }

    pub(super) fn is_building(&self) -> bool {
        self.active_request.is_some()
    }

    pub(super) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub(super) fn results(&self) -> Option<LibrarySearchResults<'_>> {
        self.results
            .as_deref()
            .zip(self.index.as_ref())
            .map(|(ids, index)| LibrarySearchResults { ids, index })
    }

    pub(super) fn apply_build(
        &mut self,
        request_id: u64,
        root: &Path,
        result: Result<SearchIndex, String>,
    ) -> bool {
        if self.root.as_deref() != Some(root)
            || self
                .active_request
                .as_ref()
                .is_none_or(|request| request.id != request_id || request.root != root)
        {
            return false;
        }
        self.active_request = None;
        match result {
            Ok(index) => {
                self.index = Some(index);
                self.error = None;
                self.refresh_results();
            }
            Err(error) => {
                self.index = None;
                self.results = None;
                self.error = Some(error);
            }
        }
        true
    }

    fn refresh_results(&mut self) {
        self.results = if self.has_meaningful_query() {
            self.index.as_ref().map(|index| index.search(&self.query))
        } else {
            None
        };
    }
}

pub(super) struct LibrarySearchResults<'a> {
    ids: &'a [SearchItemId],
    index: &'a SearchIndex,
}

impl LibrarySearchResults<'_> {
    pub(super) fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &SearchItem> {
        self.ids.iter().filter_map(|id| self.index.item(*id))
    }
}

pub(super) fn location_text(parent_relative: &Path) -> String {
    let components = parent_relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(component) => Some(component.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if components.is_empty() {
        "本棚".into()
    } else {
        components.join(" / ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn searchable_index() -> (tempfile::TempDir, SearchIndex) {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("手塚治虫/ブラック・ジャック");
        fs::create_dir_all(&folder).unwrap();
        let mut archive =
            zip::ZipWriter::new(fs::File::create(folder.join("ブラック・ジャック.cbz")).unwrap());
        archive
            .start_file("1.jpg", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"image").unwrap();
        archive.finish().unwrap();
        let index = SearchIndex::build(root.path()).unwrap();
        (root, index)
    }

    #[test]
    fn opening_issues_a_new_request_and_reset_discards_session_state() {
        let mut state = LibrarySearchState::default();
        let first = state.begin(Path::new("/books")).unwrap();
        state.set_query("漫画".into());
        state.apply_build(first.id, &first.root, Ok(searchable_index().1));

        state.reset();
        assert!(!state.is_active());
        assert!(!state.has_meaningful_query());
        assert!(state.results().is_none());
        assert!(state.error().is_none());

        let second = state.begin(Path::new("/books")).unwrap();
        assert_ne!(first.id, second.id);
    }

    #[test]
    fn reset_discards_a_build_error() {
        let mut state = LibrarySearchState::default();
        let request = state.begin(Path::new("/books")).unwrap();
        state.set_query("漫画".into());
        assert!(state.apply_build(request.id, &request.root, Err("failure".into())));
        assert_eq!(state.error(), Some("failure"));

        state.reset();
        assert!(state.error().is_none());
        assert!(state.results().is_none());
    }

    #[test]
    fn reset_reopened_and_wrong_root_build_results_are_stale() {
        let mut state = LibrarySearchState::default();
        let first = state.begin(Path::new("/books")).unwrap();
        state.reset();
        assert!(!state.apply_build(first.id, &first.root, Ok(searchable_index().1)));

        let second = state.begin(Path::new("/books")).unwrap();
        assert!(!state.apply_build(first.id, &first.root, Ok(searchable_index().1)));
        assert!(!state.apply_build(second.id, Path::new("/other"), Ok(searchable_index().1)));
        assert!(state.is_building());
    }

    #[test]
    fn empty_and_normalized_empty_queries_keep_normal_display() {
        let mut state = LibrarySearchState::default();
        state.begin(Path::new("/books"));
        assert!(!state.has_meaningful_query());
        state.set_query("!? ・　".into());
        assert!(!state.has_meaningful_query());
        assert!(state.results().is_none());
    }

    #[test]
    fn beginning_an_active_root_keeps_the_current_session() {
        let mut state = LibrarySearchState::default();
        let request = state.begin(Path::new("/books")).unwrap();
        state.set_query("ブラックジャック".into());

        assert!(state.begin(Path::new("/books")).is_none());
        assert_eq!(state.query(), "ブラックジャック");
        assert!(state.is_building());
        assert_eq!(state.active_request.as_ref(), Some(&request));
    }

    #[test]
    fn completed_index_uses_the_latest_query() {
        let (root, index) = searchable_index();
        let mut state = LibrarySearchState::default();
        let request = state.begin(root.path()).unwrap();
        state.set_query("存在しない".into());
        state.set_query("ブラックジャック".into());
        assert!(state.apply_build(request.id, &request.root, Ok(index)));
        let results = state.results().unwrap();
        assert_eq!(results.iter().count(), 1);
    }

    #[test]
    fn query_updates_reuse_the_index_and_switch_result_handles() {
        let (root, index) = searchable_index();
        let mut state = LibrarySearchState::default();
        let request = state.begin(root.path()).unwrap();
        state.set_query("存在しない".into());
        assert!(state.apply_build(request.id, &request.root, Ok(index)));
        let index_address = state.index.as_ref().unwrap() as *const SearchIndex;

        assert!(state.results().unwrap().is_empty());
        state.set_query("ブラックジャック".into());
        let results = state.results().unwrap();
        assert_eq!(
            state.index.as_ref().unwrap() as *const SearchIndex,
            index_address
        );
        assert!(matches!(results.iter().next(), Some(SearchItem::Folder(_))));
    }

    #[test]
    fn reset_discards_index_and_result_handles() {
        let (root, index) = searchable_index();
        let mut state = LibrarySearchState::default();
        let request = state.begin(root.path()).unwrap();
        state.set_query("ブラックジャック".into());
        assert!(state.apply_build(request.id, &request.root, Ok(index)));

        state.reset();
        assert!(state.index.is_none());
        assert!(state.results.is_none());
    }

    #[test]
    fn location_uses_human_readable_relative_components() {
        assert_eq!(location_text(Path::new("")), "本棚");
        assert_eq!(location_text(Path::new("手塚治虫")), "手塚治虫");
        assert_eq!(
            location_text(Path::new("手塚治虫/ブラック・ジャック")),
            "手塚治虫 / ブラック・ジャック"
        );
    }
}
