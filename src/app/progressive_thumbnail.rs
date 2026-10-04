use crate::document::{AssetId, ImageLayout, ImageSource};
use gtk::glib;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(super) struct ProgressiveThumbnailRequest {
    pub(super) document_path: std::path::PathBuf,
    pub(super) asset_id: AssetId,
    pub(super) first_page: usize,
    pub(super) layout: ImageLayout,
    pub(super) source: ImageSource,
    pub(super) temp_dir: Option<Arc<tempfile::TempDir>>,
    pub(super) preview_generation: u64,
}

impl ProgressiveThumbnailRequest {
    pub(super) fn load_bytes(&self) -> Option<glib::Bytes> {
        let _temp_lifetime_guard = &self.temp_dir;
        match &self.source {
            ImageSource::Memory(bytes) => Some(bytes.clone()),
            ImageSource::File(_) if self.temp_dir.is_some() => {
                crate::archive::image_loader::load_image_bytes(&self.source)
            }
            ImageSource::File(_) => None,
            ImageSource::ArchiveEntry { .. } => None,
        }
    }
}

#[derive(Debug)]
pub(super) struct ProgressiveThumbnailJob {
    pub(super) session_generation: u64,
    pub(super) request: ProgressiveThumbnailRequest,
}

#[derive(Debug, Default)]
pub(super) struct ProgressiveThumbnailController {
    session_generation: u64,
    in_flight: Option<AssetId>,
    pending: VecDeque<ProgressiveThumbnailRequest>,
    attempted: HashSet<AssetId>,
    successful: HashSet<AssetId>,
}

impl ProgressiveThumbnailController {
    pub(super) fn reset(&mut self) {
        self.session_generation = self.session_generation.wrapping_add(1);
        self.in_flight = None;
        self.pending.clear();
        self.attempted.clear();
        self.successful.clear();
    }

    pub(super) fn request(
        &mut self,
        requests: impl IntoIterator<Item = ProgressiveThumbnailRequest>,
    ) -> Option<ProgressiveThumbnailJob> {
        self.pending.clear();
        for request in requests {
            if self.attempted.contains(&request.asset_id)
                || self.successful.contains(&request.asset_id)
                || self
                    .pending
                    .iter()
                    .any(|pending| pending.asset_id == request.asset_id)
            {
                continue;
            }
            self.pending.push_back(request);
        }
        self.start_next()
    }

    pub(super) fn clear_pending(&mut self) {
        self.pending.clear();
    }

    pub(super) fn accepts_completion(&self, session_generation: u64, asset_id: AssetId) -> bool {
        self.session_generation == session_generation && self.in_flight == Some(asset_id)
    }

    pub(super) fn complete(
        &mut self,
        session_generation: u64,
        asset_id: AssetId,
        cached_successfully: bool,
    ) -> Option<ProgressiveThumbnailJob> {
        if !self.accepts_completion(session_generation, asset_id) {
            return None;
        }
        self.in_flight = None;
        if cached_successfully {
            self.successful.insert(asset_id);
        }
        self.start_next()
    }

    pub(super) fn finish(&mut self) -> HashSet<AssetId> {
        let successful = std::mem::take(&mut self.successful);
        self.reset();
        successful
    }

    fn start_next(&mut self) -> Option<ProgressiveThumbnailJob> {
        if self.in_flight.is_some() {
            return None;
        }
        let request = self.pending.pop_front()?;
        self.attempted.insert(request.asset_id);
        self.in_flight = Some(request.asset_id);
        Some(ProgressiveThumbnailJob {
            session_generation: self.session_generation,
            request,
        })
    }
}

pub(super) fn nearest_ready_physical<F>(
    target: usize,
    available_count: usize,
    mut is_ready: F,
) -> Option<usize>
where
    F: FnMut(usize) -> bool,
{
    if available_count == 0 {
        return None;
    }
    (0..available_count)
        .filter(|&index| is_ready(index))
        .min_by_key(|&index| (target.abs_diff(index), index))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(asset_id: usize) -> ProgressiveThumbnailRequest {
        ProgressiveThumbnailRequest {
            document_path: "book.cbr".into(),
            asset_id: AssetId(asset_id),
            first_page: asset_id,
            layout: ImageLayout::Single,
            source: ImageSource::Memory(glib::Bytes::from_static(b"image")),
            temp_dir: None,
            preview_generation: 3,
        }
    }

    #[test]
    fn one_job_runs_and_only_the_latest_pending_demand_is_kept() {
        let mut controller = ProgressiveThumbnailController::default();
        let first = controller.request([request(0), request(1)]).unwrap();
        assert_eq!(first.request.asset_id, AssetId(0));
        assert!(controller.request([request(2)]).is_none());
        assert!(controller.request([request(3)]).is_none());

        let next = controller
            .complete(first.session_generation, AssetId(0), true)
            .unwrap();
        assert_eq!(next.request.asset_id, AssetId(3));
        assert!(
            controller
                .complete(next.session_generation, AssetId(3), true)
                .is_none()
        );
    }

    #[test]
    fn duplicate_and_failed_assets_are_not_started_again_in_the_same_session() {
        let mut controller = ProgressiveThumbnailController::default();
        let job = controller.request([request(4), request(4)]).unwrap();
        assert!(controller.request([request(4)]).is_none());
        assert!(
            controller
                .complete(job.session_generation, AssetId(4), false)
                .is_none()
        );
        assert!(controller.request([request(4)]).is_none());
        assert!(controller.finish().is_empty());
    }

    #[test]
    fn reset_rejects_stale_completion_and_clears_pending_work() {
        let mut controller = ProgressiveThumbnailController::default();
        let stale = controller.request([request(1), request(2)]).unwrap();
        controller.reset();

        assert!(!controller.accepts_completion(stale.session_generation, AssetId(1)));
        assert!(
            controller
                .complete(stale.session_generation, AssetId(1), true)
                .is_none()
        );
        let current = controller.request([request(1)]).unwrap();
        assert_ne!(current.session_generation, stale.session_generation);
    }

    #[test]
    fn finish_returns_only_successfully_cached_assets() {
        let mut controller = ProgressiveThumbnailController::default();
        let success = controller.request([request(1)]).unwrap();
        controller.complete(success.session_generation, AssetId(1), true);
        let failure = controller.request([request(2)]).unwrap();
        controller.complete(failure.session_generation, AssetId(2), false);

        assert_eq!(controller.finish(), HashSet::from([AssetId(1)]));
    }

    #[test]
    fn nearest_ready_uses_physical_distance_and_prefers_lower_on_ties() {
        assert_eq!(nearest_ready_physical(7, 0, |_| true), None);
        assert_eq!(
            nearest_ready_physical(7, 10, |index| [2, 5, 9].contains(&index)),
            Some(5)
        );
        assert_eq!(
            nearest_ready_physical(7, 10, |index| [6, 8].contains(&index)),
            Some(6)
        );
    }

    #[test]
    fn temp_file_bytes_are_loaded_while_the_request_keeps_storage_alive() {
        let temp_dir = Arc::new(tempfile::tempdir().unwrap());
        let temp_path = temp_dir.path().to_path_buf();
        let image_path = temp_path.join("image.bin");
        std::fs::write(&image_path, b"image").unwrap();
        let request = ProgressiveThumbnailRequest {
            document_path: "book.cbr".into(),
            asset_id: AssetId(0),
            first_page: 0,
            layout: ImageLayout::Single,
            source: ImageSource::File(image_path),
            temp_dir: Some(temp_dir.clone()),
            preview_generation: 3,
        };

        drop(temp_dir);
        assert!(temp_path.exists());
        assert_eq!(request.load_bytes().unwrap().as_ref(), b"image");
        drop(request);
        assert!(!temp_path.exists());
    }
}
