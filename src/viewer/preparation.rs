use super::render::{
    CropAnalysis, analyze_decoded_image_cancellable, analyze_smart_crop_decoded_cancellable,
    decode_rgba,
};
use crate::document::{AssetId, ImageLayout};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, Weak, mpsc};

pub(crate) struct SmartCropPreparationRequest {
    pub(crate) document_generation: u64,
    pub(crate) asset_id: AssetId,
    pub(crate) page_index: usize,
    pub(crate) bytes: gtk::glib::Bytes,
    pub(crate) layout: ImageLayout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SmartCropPreparationResult {
    pub(crate) document_generation: u64,
    pub(crate) asset_id: AssetId,
    pub(crate) page_index: usize,
    pub(crate) success: bool,
}

enum PreparationEntry {
    Queued,
    Running,
    Ready(CropAnalysis),
    Cancelled,
}

#[derive(Default)]
struct SharedState {
    document_generation: Option<u64>,
    targets: HashSet<AssetId>,
    entries: HashMap<AssetId, PreparationEntry>,
    decoded: HashMap<AssetId, Weak<image::DynamicImage>>,
    #[cfg(test)]
    decode_count: usize,
}

#[derive(Clone)]
pub(crate) struct SmartCropPreparationHandle {
    shared: Arc<Mutex<SharedState>>,
}

impl SmartCropPreparationHandle {
    /// Share only pixels that another worker still owns. Never wait for an
    /// in-flight decode or keep full-resolution images alive in this registry.
    pub(crate) fn decode_png_webp(
        &self,
        document_generation: u64,
        asset_id: AssetId,
        bytes: &[u8],
    ) -> Option<Arc<image::DynamicImage>> {
        let format = image::guess_format(bytes).ok()?;
        if !matches!(format, image::ImageFormat::Png | image::ImageFormat::WebP) {
            return None;
        }
        {
            let shared = self.shared.lock().unwrap();
            if shared.document_generation == Some(document_generation)
                && let Some(decoded) = shared.decoded.get(&asset_id).and_then(Weak::upgrade)
            {
                return Some(decoded);
            }
        }
        let decoded = Arc::new(image::load_from_memory_with_format(bytes, format).ok()?);
        let mut shared = self.shared.lock().unwrap();
        if shared.document_generation == Some(document_generation) {
            #[cfg(test)]
            {
                shared.decode_count += 1;
            }
            shared.decoded.retain(|_, image| image.strong_count() > 0);
            shared.decoded.insert(asset_id, Arc::downgrade(&decoded));
        }
        Some(decoded)
    }

    pub(super) fn analyze(
        &self,
        document_generation: u64,
        asset_id: AssetId,
        bytes: &[u8],
        layout: ImageLayout,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Option<CropAnalysis>, ()> {
        if cancelled() {
            return Err(());
        }
        if matches!(
            image::guess_format(bytes),
            Ok(image::ImageFormat::Png | image::ImageFormat::WebP)
        ) {
            let Some(decoded) = self.decode_png_webp(document_generation, asset_id, bytes) else {
                return Ok(None);
            };
            if cancelled() {
                return Err(());
            }
            analyze_decoded_image_cancellable(&decoded, layout, cancelled).map(Some)
        } else {
            let Some(decoded) = decode_rgba(bytes) else {
                return Ok(None);
            };
            if cancelled() {
                return Err(());
            }
            analyze_smart_crop_decoded_cancellable(decoded, layout, cancelled).map(Some)
        }
    }

    #[cfg(test)]
    pub(crate) fn decode_count(&self) -> usize {
        self.shared.lock().unwrap().decode_count
    }

    /// Returns a completed preparation if available. Otherwise any queued or
    /// running duplicate is cancelled before the main-thread fallback starts.
    pub(crate) fn take_ready_or_cancel(
        &self,
        document_generation: u64,
        asset_id: AssetId,
    ) -> Option<CropAnalysis> {
        let mut shared = self.shared.lock().unwrap();
        if shared.document_generation != Some(document_generation) {
            return None;
        }
        match shared.entries.remove(&asset_id) {
            Some(PreparationEntry::Ready(prepared)) => Some(prepared),
            Some(PreparationEntry::Queued | PreparationEntry::Running) => {
                shared.entries.insert(asset_id, PreparationEntry::Cancelled);
                None
            }
            Some(PreparationEntry::Cancelled) | None => None,
        }
    }

    pub(crate) fn take_ready(
        &self,
        document_generation: u64,
        asset_id: AssetId,
    ) -> Option<CropAnalysis> {
        let mut shared = self.shared.lock().unwrap();
        if shared.document_generation != Some(document_generation) {
            return None;
        }
        match shared.entries.remove(&asset_id) {
            Some(PreparationEntry::Ready(prepared)) => Some(prepared),
            Some(entry) => {
                shared.entries.insert(asset_id, entry);
                None
            }
            None => None,
        }
    }

    fn activate_document(&self, document_generation: u64) {
        let mut shared = self.shared.lock().unwrap();
        shared.document_generation = Some(document_generation);
        shared.targets.clear();
        shared.entries.clear();
        shared.decoded.clear();
    }

    fn set_targets(&self, document_generation: u64, targets: HashSet<AssetId>) -> bool {
        let mut shared = self.shared.lock().unwrap();
        if shared.document_generation != Some(document_generation) {
            return false;
        }
        if shared.targets == targets {
            return false;
        }
        shared.targets = targets;
        let SharedState {
            targets, entries, ..
        } = &mut *shared;
        entries.retain(|asset_id, _| targets.contains(asset_id));
        true
    }

    fn queue(&self, document_generation: u64, asset_id: AssetId) -> bool {
        let mut shared = self.shared.lock().unwrap();
        if shared.document_generation != Some(document_generation)
            || !shared.targets.contains(&asset_id)
            || shared.entries.contains_key(&asset_id)
        {
            return false;
        }
        shared.entries.insert(asset_id, PreparationEntry::Queued);
        true
    }

    fn begin(&self, document_generation: u64, asset_id: AssetId) -> bool {
        let mut shared = self.shared.lock().unwrap();
        if shared.document_generation != Some(document_generation)
            || !shared.targets.contains(&asset_id)
        {
            return false;
        }
        let Some(entry) = shared.entries.get_mut(&asset_id) else {
            return false;
        };
        if !matches!(entry, PreparationEntry::Queued) {
            return false;
        }
        *entry = PreparationEntry::Running;
        true
    }

    fn is_running(&self, document_generation: u64, asset_id: AssetId) -> bool {
        let shared = self.shared.lock().unwrap();
        shared.document_generation == Some(document_generation)
            && shared.targets.contains(&asset_id)
            && matches!(
                shared.entries.get(&asset_id),
                Some(PreparationEntry::Running)
            )
    }

    fn publish(&self, document_generation: u64, asset_id: AssetId, prepared: CropAnalysis) -> bool {
        let mut shared = self.shared.lock().unwrap();
        if shared.document_generation != Some(document_generation)
            || !shared.targets.contains(&asset_id)
            || !matches!(
                shared.entries.get(&asset_id),
                Some(PreparationEntry::Running)
            )
        {
            return false;
        }
        shared
            .entries
            .insert(asset_id, PreparationEntry::Ready(prepared));
        true
    }

    fn fail(&self, document_generation: u64, asset_id: AssetId) -> bool {
        let mut shared = self.shared.lock().unwrap();
        if shared.document_generation != Some(document_generation)
            || !matches!(
                shared.entries.get(&asset_id),
                Some(PreparationEntry::Running)
            )
        {
            return false;
        }
        shared.entries.remove(&asset_id);
        true
    }
}

enum Command {
    ActivateDocument(u64),
    SetTargets {
        document_generation: u64,
        targets: HashSet<AssetId>,
    },
    Queue(SmartCropPreparationRequest),
}

pub(crate) struct SmartCropPreparationScheduler {
    commands: mpsc::Sender<Command>,
    handle: SmartCropPreparationHandle,
}

impl SmartCropPreparationScheduler {
    pub(crate) fn new<F>(emit: F) -> Self
    where
        F: FnMut(SmartCropPreparationResult) -> bool + Send + 'static,
    {
        let (commands, receiver) = mpsc::channel();
        let handle = SmartCropPreparationHandle {
            shared: Arc::new(Mutex::new(SharedState::default())),
        };
        let worker_handle = handle.clone();
        std::thread::Builder::new()
            .name("viewer-smart-crop".to_string())
            .spawn(move || PreparationLane::new(receiver, worker_handle, emit).run())
            .expect("failed to start Viewer smart-crop scheduler");
        Self { commands, handle }
    }

    pub(crate) fn handle(&self) -> SmartCropPreparationHandle {
        self.handle.clone()
    }

    pub(crate) fn activate_document(&self, document_generation: u64) {
        self.handle.activate_document(document_generation);
        let _ = self
            .commands
            .send(Command::ActivateDocument(document_generation));
    }

    pub(crate) fn set_targets(
        &self,
        document_generation: u64,
        targets: impl IntoIterator<Item = AssetId>,
    ) {
        let targets = targets.into_iter().collect::<HashSet<_>>();
        if self
            .handle
            .set_targets(document_generation, targets.clone())
        {
            let _ = self.commands.send(Command::SetTargets {
                document_generation,
                targets,
            });
        }
    }

    pub(crate) fn queue(&self, request: SmartCropPreparationRequest) {
        if self
            .handle
            .queue(request.document_generation, request.asset_id)
        {
            let _ = self.commands.send(Command::Queue(request));
        }
    }
}

struct PreparationLane<F> {
    receiver: mpsc::Receiver<Command>,
    handle: SmartCropPreparationHandle,
    emit: F,
    document_generation: Option<u64>,
    targets: HashSet<AssetId>,
    pending: VecDeque<SmartCropPreparationRequest>,
}

impl<F> PreparationLane<F>
where
    F: FnMut(SmartCropPreparationResult) -> bool,
{
    fn new(receiver: mpsc::Receiver<Command>, handle: SmartCropPreparationHandle, emit: F) -> Self {
        Self {
            receiver,
            handle,
            emit,
            document_generation: None,
            targets: HashSet::new(),
            pending: VecDeque::new(),
        }
    }

    fn run(mut self) {
        loop {
            if self.pending.is_empty() {
                let Ok(command) = self.receiver.recv() else {
                    return;
                };
                self.handle_command(command);
            }
            while let Ok(command) = self.receiver.try_recv() {
                self.handle_command(command);
            }
            let Some(request) = self.pending.pop_front() else {
                continue;
            };
            if !self.run_request(request) {
                return;
            }
        }
    }

    fn handle_command(&mut self, command: Command) {
        match command {
            Command::ActivateDocument(document_generation) => {
                self.document_generation = Some(document_generation);
                self.targets.clear();
                self.pending.clear();
            }
            Command::SetTargets {
                document_generation,
                targets,
            } => {
                if self.document_generation == Some(document_generation) {
                    self.targets = targets;
                    self.pending
                        .retain(|request| self.targets.contains(&request.asset_id));
                }
            }
            Command::Queue(request) => {
                if self.document_generation == Some(request.document_generation)
                    && self.targets.contains(&request.asset_id)
                    && !self
                        .pending
                        .iter()
                        .any(|pending| pending.asset_id == request.asset_id)
                {
                    self.pending.push_back(request);
                }
            }
        }
    }

    fn run_request(&mut self, request: SmartCropPreparationRequest) -> bool {
        let token = (request.document_generation, request.asset_id);
        if !self.handle.begin(token.0, token.1) {
            return true;
        }
        let prepared = match self.handle.analyze(
            token.0,
            token.1,
            request.bytes.as_ref(),
            request.layout,
            || !self.handle.is_running(token.0, token.1),
        ) {
            Ok(Some(analysis)) => analysis,
            Ok(None) => return self.emit_failure(request),
            Err(()) => return true,
        };
        if !self.handle.publish(token.0, token.1, prepared) {
            return true;
        }
        (self.emit)(SmartCropPreparationResult {
            document_generation: request.document_generation,
            asset_id: request.asset_id,
            page_index: request.page_index,
            success: true,
        })
    }

    fn emit_failure(&mut self, request: SmartCropPreparationRequest) -> bool {
        if !self
            .handle
            .fail(request.document_generation, request.asset_id)
        {
            return true;
        }
        (self.emit)(SmartCropPreparationResult {
            document_generation: request.document_generation,
            asset_id: request.asset_id,
            page_index: request.page_index,
            success: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::time::Duration;

    fn handle(generation: u64, targets: &[AssetId]) -> SmartCropPreparationHandle {
        let handle = SmartCropPreparationHandle {
            shared: Arc::new(Mutex::new(SharedState::default())),
        };
        handle.activate_document(generation);
        assert!(handle.set_targets(generation, targets.iter().copied().collect()));
        handle
    }

    #[test]
    fn duplicate_queue_is_suppressed_until_completion_or_cancel() {
        let asset = AssetId(2);
        let handle = handle(4, &[asset]);
        assert!(handle.queue(4, asset));
        assert!(!handle.queue(4, asset));
        assert!(handle.begin(4, asset));
        assert!(!handle.queue(4, asset));
        assert!(handle.take_ready_or_cancel(4, asset).is_none());
        assert!(!handle.is_running(4, asset));
    }

    #[test]
    fn target_replacement_and_document_change_cancel_old_work() {
        let first = AssetId(1);
        let second = AssetId(2);
        let handle = handle(4, &[first]);
        assert!(handle.queue(4, first));
        assert!(handle.begin(4, first));
        assert!(handle.set_targets(4, [second].into_iter().collect()));
        assert!(!handle.is_running(4, first));

        handle.activate_document(5);
        assert!(!handle.queue(4, second));
        assert!(handle.take_ready(4, first).is_none());
    }

    #[test]
    fn cancellation_and_replacement_reject_publication_and_ready_is_consumed_once() {
        let asset = AssetId(1);
        let analysis = CropAnalysis {
            layout: ImageLayout::Single,
            crop_result: None,
        };
        let handle = handle(4, &[asset]);
        assert!(handle.queue(4, asset));
        assert!(handle.begin(4, asset));
        assert!(handle.take_ready_or_cancel(4, asset).is_none());
        assert!(!handle.publish(4, asset, analysis));
        assert!(handle.set_targets(4, HashSet::new()));
        assert!(handle.set_targets(4, [asset].into_iter().collect()));
        assert!(handle.queue(4, asset));
        assert!(handle.begin(4, asset));
        assert!(handle.publish(4, asset, analysis));
        assert!(handle.take_ready(3, asset).is_none());
        assert_eq!(handle.take_ready(4, asset).unwrap().crop_result, None);
        assert!(handle.take_ready(4, asset).is_none());
        assert!(handle.queue(4, asset));
        assert!(handle.begin(4, asset));
        assert!(handle.set_targets(4, HashSet::new()));
        assert!(!handle.publish(4, asset, analysis));
        assert!(handle.set_targets(4, [asset].into_iter().collect()));
        assert!(handle.queue(4, asset));
        assert!(handle.begin(4, asset));
        handle.activate_document(5);
        assert!(!handle.publish(4, asset, analysis));
    }

    fn encoded_image(format: image::ImageFormat, value: u8) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            80,
            70,
            image::Rgba([value, 0, 0, 255]),
        ))
        .write_to(&mut output, format)
        .unwrap();
        output.into_inner()
    }

    #[test]
    fn png_webp_share_owned_decodes_across_threads_without_retaining_pixels_in_ready_results() {
        for format in [image::ImageFormat::Png, image::ImageFormat::WebP] {
            let asset = AssetId(1);
            let handle = handle(4, &[asset]);
            let bytes = encoded_image(format, 20);
            let decoded = handle.decode_png_webp(4, asset, &bytes).unwrap();
            let weak = Arc::downgrade(&decoded);
            let other_handle = handle.clone();
            let other_bytes = bytes.clone();
            let other = std::thread::spawn(move || {
                other_handle
                    .decode_png_webp(4, asset, &other_bytes)
                    .unwrap()
            })
            .join()
            .unwrap();
            assert!(Arc::ptr_eq(&decoded, &other));
            assert_eq!(handle.decode_count(), 1);
            assert!(handle.queue(4, asset));
            assert!(handle.begin(4, asset));
            let analysis = handle
                .analyze(4, asset, &bytes, ImageLayout::Single, || false)
                .unwrap()
                .unwrap();
            assert!(handle.publish(4, asset, analysis));
            assert_eq!(handle.decode_count(), 1);
            drop(other);
            drop(decoded);
            assert!(
                weak.upgrade().is_none(),
                "Ready stores analysis, not full-resolution pixels"
            );
            assert!(handle.take_ready(4, asset).is_some());
            let _fresh = handle.decode_png_webp(4, asset, &bytes).unwrap();
            assert_eq!(
                handle.decode_count(),
                2,
                "expired Weak requires a new decode"
            );
        }
    }

    #[test]
    fn document_change_clears_shared_decodes_and_stale_calls_cannot_reuse_or_replace_current_pixels()
     {
        let asset = AssetId(1);
        let handle = handle(4, &[asset]);
        let old_bytes = encoded_image(image::ImageFormat::Png, 20);
        let old = handle.decode_png_webp(4, asset, &old_bytes).unwrap();
        handle.activate_document(5);
        assert!(handle.shared.lock().unwrap().decoded.is_empty());
        let new_bytes = encoded_image(image::ImageFormat::Png, 90);
        let current = handle.decode_png_webp(5, asset, &new_bytes).unwrap();
        assert!(!Arc::ptr_eq(&old, &current));
        assert_eq!(current.as_rgba8().unwrap().get_pixel(0, 0)[0], 90);
        let stale = handle.decode_png_webp(4, asset, &old_bytes).unwrap();
        assert!(!Arc::ptr_eq(&stale, &current));
        assert!(!Arc::ptr_eq(&stale, &old));
        assert_eq!(stale.as_rgba8().unwrap().get_pixel(0, 0)[0], 20);
        let reused = handle.decode_png_webp(5, asset, &new_bytes).unwrap();
        assert!(Arc::ptr_eq(&current, &reused));
    }

    #[test]
    fn worker_publishes_ready_data_for_the_matching_generation_and_asset() {
        let (sender, receiver) = mpsc::channel();
        let scheduler =
            SmartCropPreparationScheduler::new(move |result| sender.send(result).is_ok());
        let asset = AssetId(3);
        let first_target = AssetId(2);
        scheduler.activate_document(7);
        scheduler.set_targets(7, [first_target, asset]);

        let image = image::RgbaImage::from_fn(80, 70, |x, y| {
            if (15..65).contains(&x) && (12..57).contains(&y) {
                image::Rgba([20, 20, 20, 255])
            } else {
                image::Rgba([255, 255, 255, 255])
            }
        });
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut output, image::ImageFormat::Png)
            .unwrap();
        scheduler.queue(SmartCropPreparationRequest {
            document_generation: 7,
            asset_id: asset,
            page_index: 4,
            bytes: gtk::glib::Bytes::from_owned(output.into_inner()),
            layout: ImageLayout::Single,
        });

        let result = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            result,
            SmartCropPreparationResult {
                document_generation: 7,
                asset_id: asset,
                page_index: 4,
                success: true,
            }
        );
        // A ready second-byte-preload target remains available when it moves
        // into the one-display-unit Texture preload window.
        scheduler.set_targets(7, [asset]);
        assert!(scheduler.handle().take_ready(7, asset).is_some());
        assert!(scheduler.handle().take_ready(6, asset).is_none());
    }
}
