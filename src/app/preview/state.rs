use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) struct PreviewTarget {
    hover_key: usize,
    pub(in crate::app) first_page: usize,
    pub(in crate::app) second_page: Option<usize>,
}

impl PreviewTarget {
    pub(in crate::app) fn new(first_page: usize, second_page: Option<usize>) -> Self {
        Self {
            hover_key: first_page,
            first_page,
            second_page,
        }
    }

    pub(in crate::app) fn for_physical(
        physical_index: usize,
        first_page: usize,
        second_page: Option<usize>,
    ) -> Self {
        Self {
            hover_key: physical_index,
            first_page,
            second_page,
        }
    }

    pub(in crate::app) fn unavailable_physical(physical_index: usize) -> Self {
        Self::for_physical(physical_index, usize::MAX, None)
    }

    pub(in crate::app) fn navigation_index(self) -> usize {
        self.hover_key
    }

    pub(super) fn contains(self, page_index: usize) -> bool {
        self.first_page == page_index || self.second_page == Some(page_index)
    }
}

pub(in crate::app) fn nearest_ready_view<F>(
    target: usize,
    view_starts: &[usize],
    mut is_ready: F,
) -> Option<usize>
where
    F: FnMut(usize) -> bool,
{
    let insertion = view_starts.partition_point(|&index| index < target);
    let mut lower = insertion.checked_sub(1);
    let mut upper = insertion;

    while lower.is_some() || upper < view_starts.len() {
        let lower_candidate = lower.map(|index| view_starts[index]);
        let upper_candidate = view_starts.get(upper).copied();
        let use_lower = match (lower_candidate, upper_candidate) {
            (Some(lower), Some(upper)) => target.abs_diff(lower) <= target.abs_diff(upper),
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        let candidate = if use_lower {
            let candidate = lower_candidate.unwrap();
            lower = lower.and_then(|index| index.checked_sub(1));
            candidate
        } else {
            let candidate = upper_candidate.unwrap();
            upper += 1;
            candidate
        };
        if is_ready(candidate) {
            return Some(candidate);
        }
    }
    None
}

pub(super) fn preview_pages_ready<F>(target: PreviewTarget, mut contains: F) -> bool
where
    F: FnMut(usize) -> bool,
{
    contains(target.first_page) && target.second_page.is_none_or(contains)
}

fn locked_preview_target(
    current_raw: Option<PreviewTarget>,
    current_displayed: Option<PreviewTarget>,
    new_raw: PreviewTarget,
    nearest_ready: Option<PreviewTarget>,
) -> Option<PreviewTarget> {
    if current_raw == Some(new_raw) {
        current_displayed
    } else {
        nearest_ready
    }
}

pub(super) struct HoverUpdate {
    pub(super) selected_target: Option<PreviewTarget>,
    pub(super) hover_revision: u64,
}

pub(super) struct PreviewState {
    generation: u64,
    cancel: Arc<AtomicU64>,
    raw_hover_target: Option<PreviewTarget>,
    displayed_preview_target: Option<PreviewTarget>,
    hover_revision: u64,
    pointer: Option<(i32, i32, i32)>,
}

impl PreviewState {
    pub(super) fn new() -> Self {
        Self {
            generation: 0,
            cancel: Arc::new(AtomicU64::new(0)),
            raw_hover_target: None,
            displayed_preview_target: None,
            hover_revision: 0,
            pointer: None,
        }
    }

    pub(super) fn reset_for_document(&mut self) {
        self.advance_generation();
        self.clear_hover(true);
    }

    pub(super) fn generation_context(&self) -> (u64, Arc<AtomicU64>) {
        (self.generation, self.cancel.clone())
    }

    pub(super) fn accepts_generation(&self, generation: u64) -> bool {
        generation == self.generation
    }

    pub(super) fn raw_hover_target(&self) -> Option<PreviewTarget> {
        self.raw_hover_target
    }

    pub(super) fn displayed_preview_target(&self) -> Option<PreviewTarget> {
        self.displayed_preview_target
    }

    pub(super) fn placeholder_target(&self) -> Option<PreviewTarget> {
        self.displayed_preview_target
            .is_none()
            .then_some(self.raw_hover_target)
            .flatten()
    }

    pub(super) fn pointer(&self) -> Option<(i32, i32, i32)> {
        self.pointer
    }

    pub(super) fn update_hover(
        &mut self,
        raw_target: PreviewTarget,
        nearest_ready: Option<PreviewTarget>,
        pointer: (i32, i32, i32),
    ) -> Option<HoverUpdate> {
        self.pointer = Some(pointer);
        if self.raw_hover_target == Some(raw_target) {
            return None;
        }

        let selected_target = locked_preview_target(
            self.raw_hover_target,
            self.displayed_preview_target,
            raw_target,
            nearest_ready,
        );
        self.raw_hover_target = Some(raw_target);
        self.hover_revision = self.hover_revision.wrapping_add(1);
        Some(HoverUpdate {
            selected_target,
            hover_revision: self.hover_revision,
        })
    }

    pub(super) fn mark_preview_displayed(&mut self, target: PreviewTarget) {
        self.displayed_preview_target = Some(target);
    }

    pub(super) fn mark_preview_unavailable(&mut self) {
        self.displayed_preview_target = None;
    }

    pub(super) fn reset_hover_selection_for_view_change(&mut self) {
        self.clear_hover(false);
    }

    pub(super) fn previous_preview_hold_expired(&mut self, hover_revision: u64) -> Option<bool> {
        if hover_revision != self.hover_revision || self.raw_hover_target.is_none() {
            return None;
        }
        let is_spread = self
            .raw_hover_target
            .is_some_and(|target| target.second_page.is_some());
        self.displayed_preview_target = None;
        Some(is_spread)
    }

    pub(super) fn hide(&mut self) {
        self.clear_hover(true);
    }

    pub(super) fn shutdown(&mut self) {
        self.advance_generation();
        self.clear_hover(false);
    }

    fn advance_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.cancel.store(self.generation, Ordering::Relaxed);
    }

    fn clear_hover(&mut self, clear_pointer: bool) {
        self.raw_hover_target = None;
        self.displayed_preview_target = None;
        self.hover_revision = self.hover_revision.wrapping_add(1);
        if clear_pointer {
            self.pointer = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Document, ImageLayout, ImageSource};
    use crate::viewer::{ViewMode, ViewerSession};

    #[test]
    fn nearest_ready_preview_prefers_the_closest_index() {
        let view_starts: Vec<_> = (0..200).collect();
        let ready = [100, 125, 150, 175];

        assert_eq!(
            nearest_ready_view(137, &view_starts, |index| ready.contains(&index)),
            Some(125)
        );
    }

    #[test]
    fn progressive_targets_keep_physical_hover_identity_separate_from_pages() {
        let first = PreviewTarget::for_physical(4, 7, Some(8));
        let second = PreviewTarget::for_physical(5, 7, Some(8));

        assert_ne!(first, second);
        assert_eq!(first.navigation_index(), 4);
        assert_ne!(first.navigation_index(), first.first_page);
        assert!(first.contains(7));
        assert!(first.contains(8));
    }

    #[test]
    fn normal_target_uses_its_first_page_as_navigation_identity() {
        let target = PreviewTarget::new(7, Some(8));

        assert_eq!(target.navigation_index(), 7);
    }

    #[test]
    fn only_a_placeholder_hover_can_promote_its_raw_target_when_ready() {
        let mut state = PreviewState::new();
        let raw = PreviewTarget::new(7, None);
        state.update_hover(raw, None, (10, 100, 20));
        assert_eq!(state.placeholder_target(), Some(raw));

        state.mark_preview_displayed(PreviewTarget::new(5, None));
        assert_eq!(state.placeholder_target(), None);
    }

    #[test]
    fn nearest_ready_preview_prefers_the_lower_index_on_a_tie() {
        let view_starts: Vec<_> = (0..200).collect();
        let ready = [125, 149];

        assert_eq!(
            nearest_ready_view(137, &view_starts, |index| ready.contains(&index)),
            Some(125)
        );
    }

    #[test]
    fn hover_selection_stays_locked_until_the_raw_target_changes() {
        let raw = PreviewTarget::new(137, None);
        let displayed = PreviewTarget::new(125, None);
        let mut state = PreviewState::new();

        let first = state
            .update_hover(raw, Some(displayed), (10, 100, 20))
            .unwrap();
        assert_eq!(first.selected_target, Some(displayed));
        state.mark_preview_displayed(displayed);

        assert!(state.update_hover(raw, Some(raw), (11, 100, 20)).is_none());
        assert_eq!(state.displayed_preview_target(), Some(displayed));
        assert_eq!(state.pointer(), Some((11, 100, 20)));

        let changed = state
            .update_hover(PreviewTarget::new(138, None), Some(raw), (12, 100, 20))
            .unwrap();
        assert_eq!(changed.selected_target, Some(raw));
    }

    #[test]
    fn spread_ready_uses_all_pages_from_the_current_logical_target() {
        let ready = [125, 126];
        let before_shift = PreviewTarget::new(124, Some(125));
        let after_shift = PreviewTarget::new(125, Some(126));

        assert!(!preview_pages_ready(before_shift, |index| ready.contains(&index)));
        assert!(preview_pages_ready(after_shift, |index| ready.contains(&index)));
        assert!(!preview_pages_ready(after_shift, |index| index == 125));
    }

    #[test]
    fn spread_alignment_rebuilds_ready_candidates_from_the_existing_pages() {
        let mut document = Document::new("book".into(), None);
        for layout in [
            ImageLayout::Spread,
            ImageLayout::Single,
            ImageLayout::Single,
        ] {
            document.add_asset(
                ImageSource::Memory(gtk::glib::Bytes::from_static(b"image")),
                layout,
            );
        }
        let ready = [1, 2];
        let mut viewer = ViewerSession::new(ViewMode::Spread);
        viewer.replace_document(document, 0);

        let ready_before_shift = nearest_ready_view(1, viewer.view_starts(), |index| {
            let target =
                PreviewTarget::new(index, viewer.is_full_spread_at(index).then_some(index + 1));
            preview_pages_ready(target, |page| ready.contains(&page))
        });
        assert_eq!(ready_before_shift, None);

        assert!(viewer.next_single_page());
        let ready_after_shift = nearest_ready_view(1, viewer.view_starts(), |index| {
            let target =
                PreviewTarget::new(index, viewer.is_full_spread_at(index).then_some(index + 1));
            preview_pages_ready(target, |page| ready.contains(&page))
        });
        assert_eq!(ready_after_shift, Some(1));
    }

    #[test]
    fn hover_revision_rejects_stale_hold_expiration() {
        let mut state = PreviewState::new();
        let first = state
            .update_hover(
                PreviewTarget::new(10, None),
                Some(PreviewTarget::new(8, None)),
                (10, 100, 20),
            )
            .unwrap();
        state.mark_preview_displayed(PreviewTarget::new(8, None));
        let second = state
            .update_hover(PreviewTarget::new(20, None), None, (20, 100, 20))
            .unwrap();

        assert!(
            state
                .previous_preview_hold_expired(first.hover_revision)
                .is_none()
        );
        assert_eq!(
            state.displayed_preview_target(),
            Some(PreviewTarget::new(8, None))
        );
        assert_eq!(
            state.previous_preview_hold_expired(second.hover_revision),
            Some(false)
        );
        assert_eq!(state.displayed_preview_target(), None);
    }

    #[test]
    fn reset_clears_hover_and_cancels_the_previous_generation() {
        let mut state = PreviewState::new();
        let (generation, cancel) = state.generation_context();
        let target = PreviewTarget::new(10, None);
        state.update_hover(target, Some(target), (10, 100, 20));
        state.mark_preview_displayed(target);

        state.reset_for_document();

        let (next_generation, _) = state.generation_context();
        assert_eq!(next_generation, generation.wrapping_add(1));
        assert_eq!(cancel.load(Ordering::Relaxed), next_generation);
        assert!(!state.accepts_generation(generation));
        assert!(state.accepts_generation(next_generation));
        assert_eq!(state.raw_hover_target(), None);
        assert_eq!(state.displayed_preview_target(), None);
        assert_eq!(state.pointer(), None);
    }
}
