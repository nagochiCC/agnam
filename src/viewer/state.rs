use crate::document::{Page, PagePart};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ViewMode {
    #[default]
    Spread,
    Single,
}

pub(crate) fn snap_to_view(val: usize, view_starts: &[usize]) -> Option<usize> {
    let first = *view_starts.first()?;
    match view_starts.binary_search(&val) {
        Ok(idx) => Some(view_starts[idx]),
        Err(0) => Some(first),
        Err(idx) if idx == view_starts.len() => view_starts.last().copied(),
        Err(idx) => {
            let lower = view_starts[idx - 1];
            let upper = view_starts[idx];
            if val - lower < upper - val {
                Some(lower)
            } else {
                Some(upper)
            }
        }
    }
}

fn build_view_starts(pages: &[Page], view_mode: ViewMode, spread_shift: bool) -> Vec<usize> {
    match view_mode {
        ViewMode::Single => (0..pages.len()).collect(),
        ViewMode::Spread => {
            if pages.is_empty() {
                return Vec::new();
            }

            let first_is_split = pages
                .first()
                .is_some_and(|page| page.part != PagePart::Whole);
            let default_parity = usize::from(!first_is_split);
            let start_parity = if spread_shift {
                1 - default_parity
            } else {
                default_parity
            };

            let mut view_starts = Vec::new();
            if start_parity == 1 {
                view_starts.push(0);
            }

            let mut index = start_parity;
            while index < pages.len() {
                view_starts.push(index);
                index += 2;
            }
            view_starts
        }
    }
}

pub(super) struct ViewState {
    current_index: usize,
    view_mode: ViewMode,
    view_starts: Vec<usize>,
    spread_shift: bool,
    progressive: bool,
    available_page_count: usize,
}

impl ViewState {
    pub(super) fn new() -> Self {
        Self {
            current_index: 0,
            view_mode: ViewMode::default(),
            view_starts: Vec::new(),
            spread_shift: false,
            progressive: false,
            available_page_count: 0,
        }
    }

    pub(super) fn with_view_mode(view_mode: ViewMode) -> Self {
        Self {
            view_mode,
            ..Self::new()
        }
    }

    pub(super) fn current_index(&self) -> usize {
        self.current_index
    }

    pub(super) fn view_mode(&self) -> ViewMode {
        self.view_mode
    }

    pub(super) fn view_starts(&self) -> &[usize] {
        &self.view_starts
    }

    pub(super) fn reset_for_document(&mut self, pages: &[Page], initial_index: usize) {
        self.spread_shift = false;
        self.progressive = false;
        self.rebuild_view_starts(pages);
        self.current_index =
            snap_to_view(initial_index, &self.view_starts).unwrap_or(initial_index);
    }

    pub(super) fn reset_for_progressive_document(&mut self, pages: &[Page], initial_index: usize) {
        self.spread_shift = false;
        self.progressive = true;
        self.rebuild_view_starts(pages);
        self.current_index =
            snap_to_view(initial_index, &self.view_starts).unwrap_or(initial_index);
    }

    pub(super) fn extend_progressive_document(&mut self, pages: &[Page]) {
        debug_assert!(self.progressive);
        self.rebuild_view_starts(pages);
    }

    pub(super) fn finish_progressive_document(&mut self, pages: &[Page]) {
        self.progressive = false;
        self.rebuild_view_starts(pages);
    }

    pub(super) fn available_page_count(&self, total_page_count: usize) -> usize {
        self.available_page_count.min(total_page_count)
    }

    fn rebuild_view_starts(&mut self, pages: &[Page]) {
        self.available_page_count = progressive_available_page_count(
            pages,
            self.view_mode,
            self.spread_shift,
            self.progressive,
        );
        self.view_starts = build_view_starts(
            &pages[..self.available_page_count],
            self.view_mode,
            self.spread_shift,
        );
    }

    pub(super) fn is_full_spread(&self, pages: &[Page]) -> bool {
        self.is_full_spread_at(self.current_index, pages)
    }

    pub(super) fn is_full_spread_at(&self, index: usize, pages: &[Page]) -> bool {
        let pages = &pages[..self.available_page_count(pages.len())];
        if self.view_mode != ViewMode::Spread {
            return false;
        }
        if index + 1 >= pages.len() {
            return false;
        }
        self.view_starts.binary_search(&(index + 1)).is_err()
    }

    pub(super) fn at_document_end(&self, pages: &[Page]) -> bool {
        !self.progressive
            && self.available_page_count(pages.len()) == pages.len()
            && self.view_starts.last().copied() == Some(self.current_index)
    }

    fn display_unit_indices(&self, view_idx: usize, pages: &[Page]) -> Vec<usize> {
        let Some(&start_idx) = self.view_starts.get(view_idx) else {
            return Vec::new();
        };
        let end_idx = self
            .view_starts
            .get(view_idx + 1)
            .copied()
            .unwrap_or(pages.len())
            .min(pages.len());
        (start_idx..end_idx).collect()
    }

    pub(super) fn texture_preload_indices(&self, pages: &[Page]) -> Vec<usize> {
        let pages = &pages[..self.available_page_count(pages.len())];
        let Ok(view_idx) = self.view_starts.binary_search(&self.current_index) else {
            return Vec::new();
        };

        self.display_unit_indices(view_idx + 1, pages)
    }

    pub(super) fn byte_preload_indices(&self, pages: &[Page]) -> Vec<usize> {
        let pages = &pages[..self.available_page_count(pages.len())];
        let Ok(view_idx) = self.view_starts.binary_search(&self.current_index) else {
            return Vec::new();
        };

        let mut indices = self.display_unit_indices(view_idx + 1, pages);
        indices.extend(self.display_unit_indices(view_idx + 2, pages));
        indices
    }

    pub(super) fn texture_retention_indices(&self, pages: &[Page]) -> Vec<usize> {
        let pages = &pages[..self.available_page_count(pages.len())];
        let Ok(view_idx) = self.view_starts.binary_search(&self.current_index) else {
            return Vec::new();
        };

        let mut indices = Vec::new();
        if let Some(previous_view_idx) = view_idx.checked_sub(1) {
            indices.extend(self.display_unit_indices(previous_view_idx, pages));
        }
        indices.extend(self.display_unit_indices(view_idx, pages));
        indices.extend(self.display_unit_indices(view_idx + 1, pages));
        indices
    }

    pub(super) fn next_page(&mut self, pages: &[Page]) -> bool {
        let pages = &pages[..self.available_page_count(pages.len())];
        if pages.is_empty() {
            return false;
        }
        let active_last_idx = if self.is_full_spread(pages) {
            self.current_index + 1
        } else {
            self.current_index
        };
        if active_last_idx >= pages.len() - 1 {
            return false;
        }

        let Ok(view_idx) = self.view_starts.binary_search(&self.current_index) else {
            return false;
        };
        let Some(&next_idx) = self.view_starts.get(view_idx + 1) else {
            return false;
        };
        self.current_index = next_idx;
        true
    }

    pub(super) fn next_single_page(&mut self, pages: &[Page]) -> bool {
        if self.current_index + 1 < self.available_page_count(pages.len()) {
            self.current_index += 1;
            if self.view_mode == ViewMode::Spread {
                self.spread_shift = !self.spread_shift;
                self.rebuild_view_starts(pages);
            }
            true
        } else {
            false
        }
    }

    pub(super) fn prev_page(&mut self) -> bool {
        let Ok(view_idx) = self.view_starts.binary_search(&self.current_index) else {
            return false;
        };
        let Some(prev_view_idx) = view_idx.checked_sub(1) else {
            return false;
        };
        let Some(&prev_idx) = self.view_starts.get(prev_view_idx) else {
            return false;
        };
        self.current_index = prev_idx;
        true
    }

    pub(super) fn prev_single_page(&mut self, pages: &[Page]) -> bool {
        if self.current_index > 0 {
            self.current_index -= 1;
            if self.view_mode == ViewMode::Spread {
                self.spread_shift = !self.spread_shift;
                self.rebuild_view_starts(pages);
            }
            true
        } else {
            false
        }
    }

    pub(super) fn set_view_mode(&mut self, view_mode: ViewMode, pages: &[Page]) -> bool {
        self.view_mode = view_mode;
        self.rebuild_view_starts(pages);

        let Some(target_idx) = snap_to_view(self.current_index, &self.view_starts) else {
            return false;
        };
        self.current_index = target_idx;
        true
    }

    pub(super) fn set_page(&mut self, val: f64, pages: &[Page]) -> bool {
        let target_val = val.round() as usize;
        let Some(snapped) = snap_to_view(target_val, &self.view_starts) else {
            return false;
        };
        if snapped < pages.len() && self.current_index != snapped {
            self.current_index = snapped;
            return true;
        }
        false
    }

    pub(super) fn set_progressive_page(&mut self, val: f64, pages: &[Page]) -> bool {
        debug_assert!(self.progressive);
        self.available_page_count = pages.len();
        self.view_starts = build_view_starts(pages, self.view_mode, self.spread_shift);
        self.set_page(val, pages)
    }

    pub(super) fn progressive_view_for_page(
        &self,
        page_index: usize,
        pages: &[Page],
    ) -> Option<(usize, Option<usize>)> {
        debug_assert!(self.progressive);
        let view_starts = build_view_starts(pages, self.view_mode, self.spread_shift);
        let first_page = snap_to_view(page_index, &view_starts)?;
        let end = view_starts
            .iter()
            .copied()
            .find(|&start| start > first_page)
            .unwrap_or(pages.len());
        let second_page = (first_page + 1 < end).then_some(first_page + 1);
        Some((first_page, second_page))
    }
}

fn progressive_available_page_count(
    pages: &[Page],
    view_mode: ViewMode,
    spread_shift: bool,
    progressive: bool,
) -> usize {
    if !progressive || view_mode == ViewMode::Single || pages.is_empty() {
        return pages.len();
    }

    let view_starts = build_view_starts(pages, view_mode, spread_shift);
    let Some(&last_start) = view_starts.last() else {
        return 0;
    };
    let incomplete_last_view = pages.len() - last_start == 1;
    let stable_cover = last_start == 0 && pages[0].part == PagePart::Whole;
    if incomplete_last_view && !stable_cover {
        last_start
    } else {
        pages.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::AssetId;

    fn pages(parts: &[PagePart]) -> Vec<Page> {
        parts
            .iter()
            .enumerate()
            .map(|(index, &part)| Page {
                asset_id: AssetId(index),
                part,
            })
            .collect()
    }

    fn repeated_pages(count: usize, first_part: PagePart) -> Vec<Page> {
        let mut parts = vec![PagePart::Whole; count];
        if let Some(first) = parts.first_mut() {
            *first = first_part;
        }
        if first_part == PagePart::Right
            && let Some(second) = parts.get_mut(1)
        {
            *second = PagePart::Left;
        }
        pages(&parts)
    }

    fn state_with_pages(
        count: usize,
        first_part: PagePart,
        view_mode: ViewMode,
    ) -> (ViewState, Vec<Page>) {
        let mut state = ViewState::new();
        let pages = repeated_pages(count, first_part);
        state.view_mode = view_mode;
        state.rebuild_view_starts(&pages);
        (state, pages)
    }

    #[test]
    fn snap_handles_empty_exact_and_outside_values() {
        let view_starts = [0, 3, 7];
        assert_eq!(snap_to_view(0, &[]), None);
        assert_eq!(snap_to_view(3, &view_starts), Some(3));
        assert_eq!(snap_to_view(0, &view_starts), Some(0));
        assert_eq!(snap_to_view(100, &view_starts), Some(7));
    }

    #[test]
    fn snap_chooses_nearest_and_prefers_upper_on_ties() {
        let view_starts = [0, 3, 7, 12];
        assert_eq!(snap_to_view(1, &view_starts), Some(0));
        assert_eq!(snap_to_view(2, &view_starts), Some(3));
        assert_eq!(snap_to_view(5, &view_starts), Some(7));
        assert_eq!(snap_to_view(10, &view_starts), Some(12));
    }

    #[test]
    fn builds_no_view_starts_for_empty_pages() {
        assert!(build_view_starts(&[], ViewMode::Single, false).is_empty());
        assert!(build_view_starts(&[], ViewMode::Spread, false).is_empty());
        assert!(build_view_starts(&[], ViewMode::Spread, true).is_empty());
    }

    #[test]
    fn builds_every_page_start_in_single_mode() {
        let pages = repeated_pages(5, PagePart::Right);
        assert_eq!(
            build_view_starts(&pages, ViewMode::Single, false),
            vec![0, 1, 2, 3, 4]
        );
        assert_eq!(
            build_view_starts(&pages, ViewMode::Single, true),
            vec![0, 1, 2, 3, 4]
        );
    }

    #[test]
    fn builds_spread_starts_for_whole_first_page() {
        let pages = repeated_pages(5, PagePart::Whole);
        assert_eq!(
            build_view_starts(&pages, ViewMode::Spread, false),
            vec![0, 1, 3]
        );
        assert_eq!(
            build_view_starts(&pages, ViewMode::Spread, true),
            vec![0, 2, 4]
        );
    }

    #[test]
    fn builds_spread_starts_for_split_first_page() {
        let pages = repeated_pages(6, PagePart::Right);
        assert_eq!(
            build_view_starts(&pages, ViewMode::Spread, false),
            vec![0, 2, 4]
        );
        assert_eq!(
            build_view_starts(&pages, ViewMode::Spread, true),
            vec![0, 1, 3, 5]
        );
    }

    #[test]
    fn rebuilds_view_starts_after_spread_shift_changes() {
        let (mut state, pages) = state_with_pages(5, PagePart::Whole, ViewMode::Spread);
        assert_eq!(state.view_starts, vec![0, 1, 3]);

        state.spread_shift = true;
        state.rebuild_view_starts(&pages);
        assert_eq!(state.view_starts, vec![0, 2, 4]);
    }

    #[test]
    fn identifies_only_complete_spreads() {
        let (state, pages) = state_with_pages(5, PagePart::Whole, ViewMode::Spread);
        assert!(!state.is_full_spread_at(0, &pages));
        assert!(state.is_full_spread_at(1, &pages));
        assert!(state.is_full_spread_at(3, &pages));
        assert!(!state.is_full_spread_at(4, &pages));

        let (single, pages) = state_with_pages(5, PagePart::Whole, ViewMode::Single);
        assert!(!single.is_full_spread_at(1, &pages));
    }

    #[test]
    fn document_end_follows_single_display_units() {
        let (mut state, pages) = state_with_pages(3, PagePart::Whole, ViewMode::Single);
        state.current_index = 1;
        assert!(!state.at_document_end(&pages));

        state.current_index = 2;
        assert!(state.at_document_end(&pages));
    }

    #[test]
    fn document_end_follows_spread_display_units() {
        let (mut state, pages) = state_with_pages(65, PagePart::Whole, ViewMode::Spread);
        assert_eq!(state.view_starts.last(), Some(&63));
        state.current_index = 61;
        assert!(!state.at_document_end(&pages));

        state.current_index = 63;
        assert!(state.is_full_spread(&pages));
        assert!(state.at_document_end(&pages));
    }

    #[test]
    fn document_end_uses_shifted_and_split_image_display_units() {
        let (mut state, pages) = state_with_pages(6, PagePart::Right, ViewMode::Spread);
        assert_eq!(state.view_starts, [0, 2, 4]);
        state.current_index = 2;
        assert!(!state.at_document_end(&pages));
        state.current_index = 4;
        assert!(state.at_document_end(&pages));

        state.spread_shift = true;
        state.rebuild_view_starts(&pages);
        assert_eq!(state.view_starts, [0, 1, 3, 5]);
        state.current_index = 3;
        assert!(!state.at_document_end(&pages));
        state.current_index = 5;
        assert!(state.at_document_end(&pages));
    }

    #[test]
    fn single_preload_and_retention_use_display_units() {
        let (mut state, pages) = state_with_pages(6, PagePart::Whole, ViewMode::Single);
        state.current_index = 3;

        assert_eq!(state.texture_preload_indices(&pages), vec![4]);
        assert_eq!(state.byte_preload_indices(&pages), vec![4, 5]);
        assert_eq!(state.texture_retention_indices(&pages), vec![2, 3, 4]);
    }

    #[test]
    fn spread_preloads_and_retains_complete_display_units() {
        let (mut spread, spread_pages) = state_with_pages(8, PagePart::Whole, ViewMode::Spread);
        spread.current_index = 1;

        assert_eq!(spread.texture_preload_indices(&spread_pages), vec![3, 4]);
        assert_eq!(spread.byte_preload_indices(&spread_pages), vec![3, 4, 5, 6]);
        assert_eq!(
            spread.texture_retention_indices(&spread_pages),
            vec![0, 1, 2, 3, 4]
        );

        spread.current_index = 3;
        assert_eq!(spread.texture_preload_indices(&spread_pages), vec![5, 6]);
        assert_eq!(spread.byte_preload_indices(&spread_pages), vec![5, 6, 7]);
        assert_eq!(
            spread.texture_retention_indices(&spread_pages),
            vec![1, 2, 3, 4, 5, 6]
        );
    }

    #[test]
    fn preload_handles_invalid_and_boundary_display_units() {
        let (mut spread, spread_pages) = state_with_pages(5, PagePart::Whole, ViewMode::Spread);
        assert_eq!(spread.texture_preload_indices(&spread_pages), vec![1, 2]);
        assert_eq!(spread.byte_preload_indices(&spread_pages), vec![1, 2, 3, 4]);

        spread.current_index = 3;
        assert!(spread.texture_preload_indices(&spread_pages).is_empty());
        assert!(spread.byte_preload_indices(&spread_pages).is_empty());
        assert_eq!(
            spread.texture_retention_indices(&spread_pages),
            vec![1, 2, 3, 4]
        );

        let (mut single, single_pages) = state_with_pages(5, PagePart::Whole, ViewMode::Single);
        single.current_index = 10;
        assert!(single.texture_preload_indices(&single_pages).is_empty());
        assert!(single.byte_preload_indices(&single_pages).is_empty());
        assert!(single.texture_retention_indices(&single_pages).is_empty());
    }

    #[test]
    fn moves_between_spread_starts_and_stops_at_boundaries() {
        let (mut state, pages) = state_with_pages(5, PagePart::Whole, ViewMode::Spread);
        assert!(state.next_page(&pages));
        assert_eq!(state.current_index, 1);
        assert!(state.next_page(&pages));
        assert_eq!(state.current_index, 3);
        assert!(!state.next_page(&pages));
        assert_eq!(state.current_index, 3);

        assert!(state.prev_page());
        assert_eq!(state.current_index, 1);
        assert!(state.prev_page());
        assert_eq!(state.current_index, 0);
        assert!(!state.prev_page());
    }

    #[test]
    fn moves_between_single_page_starts() {
        let (mut state, pages) = state_with_pages(3, PagePart::Whole, ViewMode::Single);

        assert!(state.next_page(&pages));
        assert_eq!(state.current_index, 1);
        assert!(state.next_page(&pages));
        assert_eq!(state.current_index, 2);
        assert!(!state.next_page(&pages));
        assert!(state.prev_page());
        assert_eq!(state.current_index, 1);
    }

    #[test]
    fn keeps_the_last_page_as_an_isolated_spread_view() {
        let (mut state, pages) = state_with_pages(5, PagePart::Whole, ViewMode::Spread);
        state.spread_shift = true;
        state.rebuild_view_starts(&pages);
        state.current_index = 2;

        assert_eq!(state.view_starts, vec![0, 2, 4]);
        assert!(state.next_page(&pages));
        assert_eq!(state.current_index, 4);
        assert!(!state.is_full_spread(&pages));
        assert!(!state.next_page(&pages));
        assert!(state.prev_page());
        assert_eq!(state.current_index, 2);
    }

    #[test]
    fn progressive_spread_exposes_only_complete_trailing_view_units() {
        let mut state = ViewState::with_view_mode(ViewMode::Spread);
        let one_page = repeated_pages(1, PagePart::Whole);
        state.reset_for_progressive_document(&one_page, 0);
        assert_eq!(state.available_page_count(one_page.len()), 1);
        assert_eq!(state.view_starts, [0]);

        let two_pages = repeated_pages(2, PagePart::Whole);
        state.extend_progressive_document(&two_pages);
        assert_eq!(state.available_page_count(two_pages.len()), 1);
        assert_eq!(state.view_starts, [0]);
        assert!(!state.next_page(&two_pages));

        let three_pages = repeated_pages(3, PagePart::Whole);
        state.extend_progressive_document(&three_pages);
        assert_eq!(state.available_page_count(three_pages.len()), 3);
        assert_eq!(state.view_starts, [0, 1]);
        assert!(state.next_page(&three_pages));
        assert_eq!(state.current_index, 1);
    }

    #[test]
    fn progressive_finish_keeps_current_position_and_releases_the_final_page() {
        let mut state = ViewState::with_view_mode(ViewMode::Spread);
        let three_pages = repeated_pages(3, PagePart::Whole);
        state.reset_for_progressive_document(&three_pages, 0);
        assert!(state.next_page(&three_pages));
        assert_eq!(state.current_index, 1);

        let four_pages = repeated_pages(4, PagePart::Whole);
        state.extend_progressive_document(&four_pages);
        assert_eq!(state.available_page_count(four_pages.len()), 3);
        state.finish_progressive_document(&four_pages);

        assert_eq!(state.current_index, 1);
        assert_eq!(state.available_page_count(four_pages.len()), 4);
        assert_eq!(state.view_starts, [0, 1, 3]);
    }

    #[test]
    fn progressive_finish_preserves_spread_shift() {
        let pages = repeated_pages(5, PagePart::Whole);
        let mut state = ViewState::with_view_mode(ViewMode::Spread);
        state.reset_for_progressive_document(&pages, 0);
        assert!(state.next_page(&pages));
        assert!(state.next_single_page(&pages));
        assert_eq!(state.current_index, 2);
        assert!(state.spread_shift);
        assert_eq!(state.view_starts, [0, 2]);

        state.finish_progressive_document(&pages);

        assert_eq!(state.current_index, 2);
        assert!(state.spread_shift);
        assert_eq!(state.view_starts, [0, 2, 4]);
    }

    #[test]
    fn explicit_progressive_target_can_reveal_the_incomplete_trailing_view() {
        let pages = repeated_pages(2, PagePart::Whole);
        let mut state = ViewState::with_view_mode(ViewMode::Spread);
        state.reset_for_progressive_document(&pages, 0);
        assert_eq!(state.available_page_count(pages.len()), 1);
        assert!(!state.next_page(&pages));

        assert!(state.set_progressive_page(1.0, &pages));
        assert_eq!(state.current_index(), 1);
        assert_eq!(state.available_page_count(pages.len()), 2);
    }

    #[test]
    fn progressive_physical_page_mapping_uses_the_full_snapped_view() {
        let pages = repeated_pages(3, PagePart::Whole);
        let mut state = ViewState::with_view_mode(ViewMode::Spread);
        state.reset_for_progressive_document(&pages, 0);

        assert_eq!(state.progressive_view_for_page(0, &pages), Some((0, None)));
        assert_eq!(
            state.progressive_view_for_page(1, &pages),
            Some((1, Some(2)))
        );
        assert_eq!(
            state.progressive_view_for_page(2, &pages),
            Some((1, Some(2)))
        );
    }

    #[test]
    fn single_mode_page_moves_keep_spread_shift() {
        let (mut state, pages) = state_with_pages(5, PagePart::Whole, ViewMode::Single);
        state.current_index = 1;
        state.spread_shift = true;
        let initial_view_starts = state.view_starts.clone();

        assert!(state.next_single_page(&pages));
        assert_eq!(state.current_index, 2);
        assert!(state.spread_shift);
        assert_eq!(state.view_starts, initial_view_starts);

        assert!(state.prev_single_page(&pages));
        assert_eq!(state.current_index, 1);
        assert!(state.spread_shift);
        assert_eq!(state.view_starts, initial_view_starts);
    }

    #[test]
    fn spread_mode_page_moves_flip_spread_shift_and_rebuild_starts() {
        let (mut state, pages) = state_with_pages(5, PagePart::Whole, ViewMode::Spread);
        state.current_index = 1;

        assert!(state.next_single_page(&pages));
        assert_eq!(state.current_index, 2);
        assert!(state.spread_shift);
        assert_eq!(state.view_starts, vec![0, 2, 4]);

        assert!(state.prev_single_page(&pages));
        assert_eq!(state.current_index, 1);
        assert!(!state.spread_shift);
        assert_eq!(state.view_starts, vec![0, 1, 3]);
    }

    #[test]
    fn failed_single_page_moves_leave_state_unchanged() {
        let (mut state, pages) = state_with_pages(2, PagePart::Whole, ViewMode::Spread);
        let initial_view_starts = state.view_starts.clone();
        assert!(!state.prev_single_page(&pages));
        assert!(!state.spread_shift);
        assert_eq!(state.view_starts, initial_view_starts);

        state.current_index = 1;
        assert!(!state.next_single_page(&pages));
        assert!(!state.spread_shift);
        assert_eq!(state.view_starts, initial_view_starts);
    }

    #[test]
    fn toggling_view_mode_rebuilds_and_snaps_current_page() {
        let (mut state, pages) = state_with_pages(5, PagePart::Whole, ViewMode::Single);
        state.current_index = 2;

        assert!(state.set_view_mode(ViewMode::Spread, &pages));
        assert_eq!(state.view_mode, ViewMode::Spread);
        assert_eq!(state.view_starts, vec![0, 1, 3]);
        assert_eq!(state.current_index, 3);

        assert!(state.set_view_mode(ViewMode::Single, &pages));
        assert_eq!(state.view_mode, ViewMode::Single);
        assert_eq!(state.current_index, 3);
    }

    #[test]
    fn setting_view_mode_applies_the_requested_mode() {
        let (mut state, pages) = state_with_pages(5, PagePart::Whole, ViewMode::Single);
        state.current_index = 2;

        assert!(state.set_view_mode(ViewMode::Spread, &pages));
        assert_eq!(state.view_mode, ViewMode::Spread);
        assert_eq!(state.current_index, 3);

        assert!(state.set_view_mode(ViewMode::Single, &pages));
        assert_eq!(state.view_mode, ViewMode::Single);
        assert_eq!(state.current_index, 3);
    }

    #[test]
    fn toggling_empty_state_changes_mode_but_has_no_target() {
        let mut state = ViewState::new();
        assert!(!state.set_view_mode(ViewMode::Spread, &[]));
        assert_eq!(state.view_mode, ViewMode::Spread);
        assert!(state.view_starts.is_empty());
    }

    #[test]
    fn set_page_rounds_then_snaps_and_reports_only_changes() {
        let (mut state, pages) = state_with_pages(5, PagePart::Whole, ViewMode::Spread);
        assert!(state.set_page(2.0, &pages));
        assert_eq!(state.current_index, 3);
        assert!(!state.set_page(2.0, &pages));
        assert!(!state.set_page(100.0, &pages));
        assert!(state.set_page(0.49, &pages));
        assert_eq!(state.current_index, 0);
    }
}
