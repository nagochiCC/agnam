use crate::archive::{ProgressiveArchiveCancelToken, ProgressiveArchiveImage};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PendingPageMove {
    Next,
    NextSingle,
    PhysicalImage(usize),
}

#[derive(Debug)]
pub(super) enum ProgressiveArrival {
    Stale,
    Buffered,
    Contiguous(Vec<ProgressiveArchiveImage>),
}

#[derive(Debug)]
struct ActiveProgressiveArchive {
    request_id: u64,
    path: PathBuf,
    next_physical_index: usize,
    buffered: BTreeMap<usize, ProgressiveArchiveImage>,
    ready: Vec<ProgressiveArchiveImage>,
    initial_index: usize,
    required_initial_pages: usize,
    total_physical_images: Option<usize>,
    displayed: bool,
    pending_move: Option<PendingPageMove>,
    cancel_token: ProgressiveArchiveCancelToken,
    temp_dir: Option<Arc<tempfile::TempDir>>,
}

#[derive(Debug, Default)]
pub(super) struct ProgressiveArchiveState {
    active: Option<ActiveProgressiveArchive>,
}

impl ProgressiveArchiveState {
    pub(super) fn begin(
        &mut self,
        request_id: u64,
        path: PathBuf,
        initial_index: usize,
        required_initial_pages: usize,
    ) -> ProgressiveArchiveCancelToken {
        if let Some(active) = self.active.take() {
            active.cancel_token.cancel();
        }
        let cancel_token = ProgressiveArchiveCancelToken::default();
        self.active = Some(ActiveProgressiveArchive {
            request_id,
            path,
            next_physical_index: 0,
            buffered: BTreeMap::new(),
            ready: Vec::new(),
            initial_index,
            required_initial_pages,
            total_physical_images: None,
            displayed: false,
            pending_move: None,
            cancel_token: cancel_token.clone(),
            temp_dir: None,
        });
        cancel_token
    }

    pub(super) fn accept(
        &mut self,
        request_id: u64,
        path: &Path,
        image: ProgressiveArchiveImage,
    ) -> ProgressiveArrival {
        let Some(active) = self
            .active
            .as_mut()
            .filter(|active| active.request_id == request_id && active.path == path)
        else {
            return ProgressiveArrival::Stale;
        };

        if let Some(temp_dir) = image.backing.temp_dir() {
            active.temp_dir = Some(temp_dir);
        }

        if image.total_physical_images == 0
            || image.physical_index >= image.total_physical_images
            || active
                .total_physical_images
                .is_some_and(|total| total != image.total_physical_images)
        {
            return ProgressiveArrival::Buffered;
        }
        active.total_physical_images = Some(image.total_physical_images);

        if image.physical_index < active.next_physical_index
            || active.buffered.contains_key(&image.physical_index)
        {
            return ProgressiveArrival::Buffered;
        }
        active.buffered.insert(image.physical_index, image);

        let mut contiguous = Vec::new();
        while let Some(image) = active.buffered.remove(&active.next_physical_index) {
            contiguous.push(image);
            active.next_physical_index += 1;
        }
        if active.displayed {
            return if contiguous.is_empty() {
                ProgressiveArrival::Buffered
            } else {
                ProgressiveArrival::Contiguous(contiguous)
            };
        }

        active.ready.extend(contiguous);
        let ready_page_count = active
            .ready
            .iter()
            .map(|image| match image.layout {
                crate::document::ImageLayout::Single => 1,
                crate::document::ImageLayout::Spread => 2,
            })
            .sum::<usize>();
        if ready_page_count < active.required_initial_pages {
            ProgressiveArrival::Buffered
        } else {
            ProgressiveArrival::Contiguous(std::mem::take(&mut active.ready))
        }
    }

    pub(super) fn mark_displayed(&mut self, request_id: u64) -> bool {
        let Some(active) = self
            .active
            .as_mut()
            .filter(|active| active.request_id == request_id)
        else {
            return false;
        };
        active.displayed = true;
        true
    }

    pub(super) fn initial_index(&self, request_id: u64) -> Option<usize> {
        self.active
            .as_ref()
            .filter(|active| active.request_id == request_id)
            .map(|active| active.initial_index)
    }

    pub(super) fn is_displayed(&self) -> bool {
        self.active.as_ref().is_some_and(|active| active.displayed)
    }

    pub(super) fn is_displayed_for(&self, request_id: u64) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.request_id == request_id && active.displayed)
    }

    pub(super) fn temp_dir(&self, request_id: u64) -> Option<Arc<tempfile::TempDir>> {
        self.active
            .as_ref()
            .filter(|active| active.request_id == request_id)
            .and_then(|active| active.temp_dir.clone())
    }

    pub(super) fn request_move(&mut self, movement: PendingPageMove) -> bool {
        let Some(active) = self.active.as_mut().filter(|active| active.displayed) else {
            return false;
        };
        if active.pending_move.is_some() {
            return false;
        }
        active.pending_move = Some(movement);
        true
    }

    pub(super) fn pending_move(&self) -> Option<PendingPageMove> {
        self.active.as_ref()?.pending_move
    }

    pub(super) fn total_physical_images(&self) -> Option<usize> {
        self.active
            .as_ref()
            .filter(|active| active.displayed)
            .and_then(|active| active.total_physical_images)
    }

    pub(super) fn pending_physical_target(&self) -> Option<usize> {
        match self.pending_move()? {
            PendingPageMove::PhysicalImage(target) => Some(target),
            PendingPageMove::Next | PendingPageMove::NextSingle => None,
        }
    }

    pub(super) fn resolve_pending_move(&mut self, moved: bool) -> Option<PendingPageMove> {
        if !moved {
            return None;
        }
        self.active.as_mut()?.pending_move.take()
    }

    pub(super) fn is_waiting_for_page(&self) -> bool {
        self.pending_move().is_some()
    }

    pub(super) fn finish(&mut self, request_id: u64) -> bool {
        if self
            .active
            .as_ref()
            .is_none_or(|active| active.request_id != request_id)
        {
            return false;
        }
        self.active.take().is_some_and(|active| active.displayed)
    }

    pub(super) fn cancel(&mut self) -> bool {
        self.active.take().is_some_and(|active| {
            active.cancel_token.cancel();
            active.displayed
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::ImageLayout;
    use gtk::glib;

    fn image(index: usize) -> ProgressiveArchiveImage {
        ProgressiveArchiveImage {
            physical_index: index,
            total_physical_images: 8,
            backing: crate::archive::SequentialImageBacking::memory(glib::Bytes::from_owned(vec![
                index as u8,
            ])),
            layout: ImageLayout::Single,
        }
    }

    #[test]
    fn out_of_order_arrivals_extend_only_the_contiguous_prefix() {
        let path = PathBuf::from("book.rar");
        let mut state = ProgressiveArchiveState::default();
        state.begin(7, path.clone(), 0, 1);

        for (index, expected, contiguous_end) in [
            (0, vec![0], 0),
            (1, vec![1], 1),
            (3, vec![], 1),
            (4, vec![], 1),
        ] {
            let arrival = state.accept(7, &path, image(index));
            let actual = match arrival {
                ProgressiveArrival::Contiguous(images) => images
                    .into_iter()
                    .map(|image| image.physical_index)
                    .collect(),
                ProgressiveArrival::Buffered => Vec::new(),
                ProgressiveArrival::Stale => panic!("current arrival must be accepted"),
            };
            assert_eq!(actual, expected);
            assert_eq!(
                state
                    .active
                    .as_ref()
                    .unwrap()
                    .next_physical_index
                    .saturating_sub(1),
                contiguous_end
            );
        }

        let ProgressiveArrival::Contiguous(images) = state.accept(7, &path, image(2)) else {
            panic!("filling the gap must release the buffered suffix");
        };
        assert_eq!(
            images
                .into_iter()
                .map(|image| image.physical_index)
                .collect::<Vec<_>>(),
            [2, 3, 4]
        );
        assert_eq!(state.active.as_ref().unwrap().next_physical_index, 5);
    }

    #[test]
    fn stale_request_and_path_are_rejected() {
        let path = PathBuf::from("current.rar");
        let mut state = ProgressiveArchiveState::default();
        state.begin(9, path.clone(), 0, 1);

        assert!(matches!(
            state.accept(8, &path, image(0)),
            ProgressiveArrival::Stale
        ));
        assert!(matches!(
            state.accept(9, Path::new("other.rar"), image(0)),
            ProgressiveArrival::Stale
        ));
        assert!(matches!(
            state.accept(9, &path, image(0)),
            ProgressiveArrival::Contiguous(_)
        ));
        assert!(!state.finish(8));
        assert!(!state.finish(9));
        assert!(matches!(
            state.accept(9, &path, image(1)),
            ProgressiveArrival::Stale
        ));
    }

    #[test]
    fn pending_move_is_not_stacked_and_resolves_once() {
        let mut state = ProgressiveArchiveState::default();
        state.begin(3, PathBuf::from("book.rar"), 0, 1);
        assert!(state.mark_displayed(3));

        assert!(state.request_move(PendingPageMove::Next));
        assert!(!state.request_move(PendingPageMove::NextSingle));
        assert_eq!(state.pending_move(), Some(PendingPageMove::Next));
        assert_eq!(state.resolve_pending_move(false), None);
        assert_eq!(
            state.resolve_pending_move(true),
            Some(PendingPageMove::Next)
        );
        assert_eq!(state.resolve_pending_move(true), None);
    }

    #[test]
    fn physical_total_and_arbitrary_pending_target_are_managed_together() {
        let path = PathBuf::from("book.rar");
        let mut state = ProgressiveArchiveState::default();
        state.begin(5, path.clone(), 0, 1);

        assert!(matches!(
            state.accept(5, &path, image(0)),
            ProgressiveArrival::Contiguous(_)
        ));
        assert!(state.mark_displayed(5));
        assert_eq!(state.total_physical_images(), Some(8));
        assert!(state.request_move(PendingPageMove::PhysicalImage(6)));
        assert_eq!(state.pending_physical_target(), Some(6));
        assert!(!state.request_move(PendingPageMove::PhysicalImage(7)));
        assert_eq!(
            state.resolve_pending_move(true),
            Some(PendingPageMove::PhysicalImage(6))
        );
        assert_eq!(state.pending_physical_target(), None);
    }

    #[test]
    fn inconsistent_or_out_of_range_total_metadata_is_ignored() {
        let path = PathBuf::from("book.rar");
        let mut state = ProgressiveArchiveState::default();
        state.begin(6, path.clone(), 0, 1);
        assert!(matches!(
            state.accept(6, &path, image(0)),
            ProgressiveArrival::Contiguous(_)
        ));

        let mut inconsistent = image(1);
        inconsistent.total_physical_images = 7;
        assert!(matches!(
            state.accept(6, &path, inconsistent),
            ProgressiveArrival::Buffered
        ));
        let mut out_of_range = image(8);
        out_of_range.total_physical_images = 8;
        assert!(matches!(
            state.accept(6, &path, out_of_range),
            ProgressiveArrival::Buffered
        ));
        assert_eq!(state.active.as_ref().unwrap().next_physical_index, 1);
    }

    #[test]
    fn saved_position_waits_for_enough_initial_pages() {
        let path = PathBuf::from("book.rar");
        let mut state = ProgressiveArchiveState::default();
        state.begin(4, path.clone(), 2, 4);

        for index in 0..3 {
            assert!(matches!(
                state.accept(4, &path, image(index)),
                ProgressiveArrival::Buffered
            ));
        }
        let ProgressiveArrival::Contiguous(images) = state.accept(4, &path, image(3)) else {
            panic!("saved position should start when its complete view can be built");
        };
        assert_eq!(images.len(), 4);
        assert_eq!(state.initial_index(4), Some(2));
    }

    #[test]
    fn cancellation_marks_the_active_request_token() {
        let mut state = ProgressiveArchiveState::default();
        let token = state.begin(1, PathBuf::from("book.rar"), 0, 1);
        assert!(state.mark_displayed(1));
        assert!(state.request_move(PendingPageMove::PhysicalImage(7)));

        assert!(!token.is_cancelled());
        assert!(state.cancel());
        assert!(token.is_cancelled());
        assert_eq!(state.pending_physical_target(), None);
    }

    #[test]
    fn stale_finish_does_not_clear_the_current_request_target() {
        let mut state = ProgressiveArchiveState::default();
        state.begin(2, PathBuf::from("current.rar"), 0, 1);
        assert!(state.mark_displayed(2));
        assert!(state.request_move(PendingPageMove::PhysicalImage(5)));

        assert!(!state.finish(1));
        assert_eq!(state.pending_physical_target(), Some(5));
        assert!(state.finish(2));
        assert_eq!(state.pending_physical_target(), None);
        assert_eq!(state.total_physical_images(), None);
    }

    #[test]
    fn beginning_a_new_request_cancels_only_the_previous_token() {
        let mut state = ProgressiveArchiveState::default();
        let previous = state.begin(1, PathBuf::from("previous.rar"), 0, 1);
        let current = state.begin(2, PathBuf::from("current.7z"), 0, 1);

        assert!(previous.is_cancelled());
        assert!(!current.is_cancelled());
    }

    #[test]
    fn spill_updates_ready_and_queued_backings_without_retaining_memory() {
        let path = PathBuf::from("book.rar");
        let mut state = ProgressiveArchiveState::default();
        state.begin(7, path.clone(), 0, 3);

        let first = image(0);
        let first_backing = first.backing.clone();
        assert!(matches!(
            state.accept(7, &path, first),
            ProgressiveArrival::Buffered
        ));

        let temp_dir = std::sync::Arc::new(tempfile::tempdir().unwrap());
        let first_path = crate::archive::sequential_image_path(temp_dir.path(), 0);
        std::fs::write(&first_path, [0]).unwrap();
        first_backing.replace_with_file(first_path.clone(), temp_dir.clone());
        let second = image(1);
        let second_path = crate::archive::sequential_image_path(temp_dir.path(), 1);
        std::fs::write(&second_path, [1]).unwrap();
        second
            .backing
            .replace_with_file(second_path, temp_dir.clone());

        assert!(matches!(
            state.accept(7, &path, second),
            ProgressiveArrival::Buffered
        ));
        assert!(matches!(
            first_backing.source(),
            crate::document::ImageSource::File(path) if path == first_path
        ));
        assert!(
            state
                .active
                .as_ref()
                .unwrap()
                .ready
                .iter()
                .all(|image| matches!(
                    image.backing.source(),
                    crate::document::ImageSource::File(_)
                ))
        );
        assert!(std::sync::Arc::ptr_eq(
            &state.temp_dir(7).unwrap(),
            &temp_dir
        ));
    }

    #[test]
    fn stale_spilled_arrival_does_not_attach_temp_storage_to_the_current_request() {
        let path = PathBuf::from("current.rar");
        let mut state = ProgressiveArchiveState::default();
        state.begin(9, path.clone(), 0, 1);

        let temp_dir = std::sync::Arc::new(tempfile::tempdir().unwrap());
        let temp_path = temp_dir.path().to_path_buf();
        let stale = image(0);
        let image_path = crate::archive::sequential_image_path(&temp_path, 0);
        std::fs::write(&image_path, [0]).unwrap();
        stale
            .backing
            .replace_with_file(image_path, temp_dir.clone());

        assert!(matches!(
            state.accept(8, &path, stale),
            ProgressiveArrival::Stale
        ));
        assert!(state.temp_dir(9).is_none());
        drop(temp_dir);
        assert!(!temp_path.exists());
    }
}
