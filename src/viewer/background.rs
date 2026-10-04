use super::ThumbnailGenerationSpeed;
use crate::archive::image_loader::ThumbnailImageLoader;
use crate::document::{AssetId, ImageAsset, ImageSource, Page};
use crate::thumbnail::worker::{
    DemandThumbnailStep, NearThumbnailStep, ThumbnailResult, ThumbnailStep, ThumbnailWorker,
};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::AtomicU64,
    mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
};
use std::time::{Duration, Instant};

const LOW_THUMBNAIL_MIN_REST: Duration = Duration::from_millis(100);

fn rest_duration(speed: ThumbnailGenerationSpeed, work_duration: Duration) -> Duration {
    match speed {
        ThumbnailGenerationSpeed::High => Duration::ZERO,
        ThumbnailGenerationSpeed::Normal => work_duration / 2,
        ThumbnailGenerationSpeed::Low => {
            work_duration.saturating_mul(2).max(LOW_THUMBNAIL_MIN_REST)
        }
    }
}

pub(crate) struct PreloadRequest {
    pub(crate) asset_id: AssetId,
    pub(crate) page_index: usize,
    pub(crate) source: ImageSource,
}

pub(crate) struct NearThumbnailRequest {
    pub(crate) asset_id: AssetId,
    pub(crate) bytes: Option<gtk::glib::Bytes>,
}

pub(crate) struct HoverThumbnailRequest {
    pub(crate) asset_id: AssetId,
    pub(crate) bytes: Option<gtk::glib::Bytes>,
}

pub(crate) enum ViewerBackgroundResult {
    Preload {
        document_generation: u64,
        asset_id: AssetId,
        page_index: usize,
        bytes: gtk::glib::Bytes,
    },
    Thumbnail(ThumbnailResult),
}

struct ThumbnailRequest {
    document_path: PathBuf,
    document_generation: u64,
    generation: u64,
    cancel: Arc<AtomicU64>,
    assets: Vec<ImageAsset>,
    pages: Vec<Page>,
    already_generated: HashSet<AssetId>,
}

enum Command {
    ActivateDocument(u64),
    ReplacePreloads {
        document_generation: u64,
        thumbnail_generation: Option<u64>,
        requests: Vec<PreloadRequest>,
        near: Vec<NearThumbnailRequest>,
    },
    ReplaceThumbnails(ThumbnailRequest),
    ReplaceHoverThumbnails {
        token: ThumbnailToken,
        requests: Vec<HoverThumbnailRequest>,
    },
    StopDistributedWarmup(ThumbnailToken),
    CancelThumbnails {
        document_generation: u64,
    },
    SetThumbnailGenerationSpeed(ThumbnailGenerationSpeed),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ThumbnailToken {
    document_generation: u64,
    generation: u64,
}

struct NearThumbnailCandidate {
    token: ThumbnailToken,
    asset_id: AssetId,
    bytes: Option<gtk::glib::Bytes>,
}

struct HoverThumbnailCandidate {
    token: ThumbnailToken,
    asset_id: AssetId,
    bytes: Option<gtk::glib::Bytes>,
}

enum NextJob {
    Preload {
        document_generation: u64,
        request: PreloadRequest,
    },
    NearThumbnail {
        token: ThumbnailToken,
        asset_id: AssetId,
        bytes: gtk::glib::Bytes,
    },
    HoverThumbnail {
        token: ThumbnailToken,
        asset_id: AssetId,
        bytes: Option<gtk::glib::Bytes>,
    },
    DistributedThumbnail(ThumbnailToken),
}

#[derive(Default)]
struct SchedulingController {
    document_generation: Option<u64>,
    preloads: VecDeque<PreloadRequest>,
    hover: VecDeque<HoverThumbnailCandidate>,
    near: VecDeque<NearThumbnailCandidate>,
    thumbnail: Option<ThumbnailToken>,
    distributed_pending: bool,
}

impl SchedulingController {
    fn activate_document(&mut self, document_generation: u64) -> bool {
        if self.document_generation == Some(document_generation) {
            return false;
        }
        self.document_generation = Some(document_generation);
        self.preloads.clear();
        self.hover.clear();
        self.near.clear();
        self.thumbnail = None;
        self.distributed_pending = false;
        true
    }

    fn replace_preloads(
        &mut self,
        document_generation: u64,
        thumbnail_generation: Option<u64>,
        requests: Vec<PreloadRequest>,
        near: Vec<NearThumbnailRequest>,
    ) -> bool {
        if self.document_generation != Some(document_generation) {
            return false;
        }

        let mut queued = HashSet::new();
        self.preloads = requests
            .into_iter()
            .filter(|request| queued.insert(request.asset_id))
            .collect();

        let token = thumbnail_generation.map(|generation| ThumbnailToken {
            document_generation,
            generation,
        });
        let token = token.filter(|token| self.thumbnail.is_none_or(|current| current == *token));
        let mut queued = HashSet::new();
        self.near = token.map_or_else(VecDeque::new, |token| {
            near.into_iter()
                .filter(|candidate| queued.insert(candidate.asset_id))
                .map(|candidate| NearThumbnailCandidate {
                    token,
                    asset_id: candidate.asset_id,
                    bytes: candidate.bytes,
                })
                .collect()
        });
        true
    }

    fn replace_thumbnail(&mut self, token: ThumbnailToken) -> bool {
        if self.document_generation != Some(token.document_generation) {
            return false;
        }
        self.thumbnail = Some(token);
        self.distributed_pending = true;
        self.hover.clear();
        self.near.retain(|candidate| candidate.token == token);
        true
    }

    fn replace_hover_thumbnails(
        &mut self,
        token: ThumbnailToken,
        requests: Vec<HoverThumbnailRequest>,
    ) -> bool {
        if self.thumbnail != Some(token) {
            return false;
        }
        let mut queued = HashSet::new();
        self.hover = requests
            .into_iter()
            .filter(|request| queued.insert(request.asset_id))
            .map(|request| HoverThumbnailCandidate {
                token,
                asset_id: request.asset_id,
                bytes: request.bytes,
            })
            .collect();
        true
    }

    fn cancel_thumbnail(&mut self, document_generation: u64) -> bool {
        if self.document_generation != Some(document_generation) {
            return false;
        }
        self.thumbnail = None;
        self.distributed_pending = false;
        self.hover.clear();
        self.near.clear();
        true
    }

    fn complete_preload(
        &mut self,
        document_generation: u64,
        asset_id: AssetId,
        bytes: &gtk::glib::Bytes,
    ) {
        if self.document_generation != Some(document_generation) {
            return;
        }
        self.preloads.retain(|request| request.asset_id != asset_id);
        for candidate in &mut self.hover {
            if candidate.token.document_generation == document_generation
                && candidate.asset_id == asset_id
            {
                candidate.bytes = Some(bytes.clone());
            }
        }
        for candidate in &mut self.near {
            if candidate.token.document_generation == document_generation
                && candidate.asset_id == asset_id
            {
                candidate.bytes = Some(bytes.clone());
            }
        }
    }

    fn fail_preload(&mut self, document_generation: u64, asset_id: AssetId) {
        if self.document_generation != Some(document_generation)
            || self
                .preloads
                .iter()
                .any(|request| request.asset_id == asset_id)
        {
            return;
        }
        self.near.retain(|candidate| candidate.asset_id != asset_id);
    }

    fn next_job(&mut self) -> Option<NextJob> {
        if let Some(request) = self.preloads.pop_front() {
            return Some(NextJob::Preload {
                document_generation: self.document_generation?,
                request,
            });
        }

        let token = self.thumbnail?;
        if let Some(candidate) = self.hover.pop_front() {
            return Some(NextJob::HoverThumbnail {
                token: candidate.token,
                asset_id: candidate.asset_id,
                bytes: candidate.bytes,
            });
        }
        if let Some(index) = self
            .near
            .iter()
            .position(|candidate| candidate.token == token && candidate.bytes.is_some())
        {
            let candidate = self.near.remove(index)?;
            return Some(NextJob::NearThumbnail {
                token,
                asset_id: candidate.asset_id,
                bytes: candidate.bytes?,
            });
        }
        self.distributed_pending
            .then_some(NextJob::DistributedThumbnail(token))
    }

    fn has_runnable_jobs(&self) -> bool {
        !self.preloads.is_empty()
            || self.thumbnail.is_some_and(|token| {
                self.distributed_pending
                    || self.hover.iter().any(|candidate| candidate.token == token)
                    || self
                        .near
                        .iter()
                        .any(|candidate| candidate.token == token && candidate.bytes.is_some())
            })
    }

    fn finish_distributed(&mut self, token: ThumbnailToken) {
        if self.thumbnail == Some(token) {
            self.distributed_pending = false;
        }
    }
}

/// Owns the single execution lane for normal Viewer background image work.
/// Commands only contain immutable job inputs; GTK/Viewer state remains on the
/// main thread and validates every result before accepting it.
pub(crate) struct ViewerBackgroundScheduler {
    commands: Sender<Command>,
}

impl ViewerBackgroundScheduler {
    pub(crate) fn new<F>(
        thumbnail_generation_speed: ThumbnailGenerationSpeed,
        preparation: super::preparation::SmartCropPreparationHandle,
        emit: F,
    ) -> Self
    where
        F: FnMut(ViewerBackgroundResult) -> bool + Send + 'static,
    {
        let (commands, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("viewer-background".to_string())
            .spawn(move || {
                let mut lane = BackgroundLane::new(receiver, thumbnail_generation_speed, emit);
                lane.preparation = Some(preparation);
                lane.run();
            })
            .expect("failed to start Viewer background scheduler");
        Self { commands }
    }

    pub(crate) fn activate_document(&self, document_generation: u64) {
        let _ = self
            .commands
            .send(Command::ActivateDocument(document_generation));
    }

    pub(crate) fn replace_preloads(
        &self,
        document_generation: u64,
        thumbnail_generation: Option<u64>,
        requests: Vec<PreloadRequest>,
        near: Vec<NearThumbnailRequest>,
    ) {
        let _ = self.commands.send(Command::ReplacePreloads {
            document_generation,
            thumbnail_generation,
            requests,
            near,
        });
    }

    pub(crate) fn replace_thumbnails(
        &self,
        document_path: PathBuf,
        document_generation: u64,
        generation: u64,
        cancel: Arc<AtomicU64>,
        assets: Vec<ImageAsset>,
        pages: Vec<Page>,
        already_generated: HashSet<AssetId>,
    ) {
        let _ = self
            .commands
            .send(Command::ReplaceThumbnails(ThumbnailRequest {
                document_path,
                document_generation,
                generation,
                cancel,
                assets,
                pages,
                already_generated,
            }));
    }

    pub(crate) fn cancel_thumbnails(&self, document_generation: u64) {
        let _ = self.commands.send(Command::CancelThumbnails {
            document_generation,
        });
    }

    pub(crate) fn replace_hover_thumbnails(
        &self,
        document_generation: u64,
        generation: u64,
        requests: Vec<HoverThumbnailRequest>,
    ) {
        let _ = self.commands.send(Command::ReplaceHoverThumbnails {
            token: ThumbnailToken {
                document_generation,
                generation,
            },
            requests,
        });
    }

    pub(crate) fn stop_distributed_warmup(&self, document_generation: u64, generation: u64) {
        let _ = self
            .commands
            .send(Command::StopDistributedWarmup(ThumbnailToken {
                document_generation,
                generation,
            }));
    }

    pub(crate) fn set_thumbnail_generation_speed(&self, speed: ThumbnailGenerationSpeed) {
        let _ = self
            .commands
            .send(Command::SetThumbnailGenerationSpeed(speed));
    }
}

struct BackgroundLane<F> {
    receiver: Receiver<Command>,
    emit: F,
    controller: SchedulingController,
    thumbnail_worker: Option<(ThumbnailToken, ThumbnailWorker)>,
    preload_loader: ThumbnailImageLoader,
    thumbnail_generation_speed: ThumbnailGenerationSpeed,
    preparation: Option<super::preparation::SmartCropPreparationHandle>,
}

impl<F> BackgroundLane<F>
where
    F: FnMut(ViewerBackgroundResult) -> bool,
{
    fn new(
        receiver: Receiver<Command>,
        thumbnail_generation_speed: ThumbnailGenerationSpeed,
        emit: F,
    ) -> Self {
        Self {
            receiver,
            emit,
            controller: SchedulingController::default(),
            thumbnail_worker: None,
            preload_loader: ThumbnailImageLoader::new(),
            thumbnail_generation_speed,
            preparation: None,
        }
    }

    fn run(mut self) {
        loop {
            if !self.controller.has_runnable_jobs() {
                let Ok(command) = self.receiver.recv() else {
                    return;
                };
                self.handle_command(command);
            }
            if !self.drain_commands() {
                return;
            }

            let Some(job) = self.controller.next_job() else {
                continue;
            };
            let step = match job {
                NextJob::Preload {
                    document_generation,
                    request,
                } => BackgroundStep::Continue(self.run_preload(document_generation, request)),
                NextJob::NearThumbnail {
                    token,
                    asset_id,
                    bytes,
                } => BackgroundStep::Continue(self.run_near_thumbnail(token, asset_id, bytes)),
                NextJob::HoverThumbnail {
                    token,
                    asset_id,
                    bytes,
                } => BackgroundStep::Continue(self.run_hover_thumbnail(
                    token,
                    asset_id,
                    bytes.as_ref(),
                )),
                NextJob::DistributedThumbnail(token) => self.run_distributed_thumbnail(token),
            };
            match step {
                BackgroundStep::Continue(true) => {}
                BackgroundStep::Continue(false) | BackgroundStep::OutputClosed => return,
                BackgroundStep::Pace(work_duration) => {
                    let rest = rest_duration(self.thumbnail_generation_speed, work_duration);
                    if !self.wait_for_pacing(rest) {
                        return;
                    }
                }
            }
        }
    }

    fn drain_commands(&mut self) -> bool {
        loop {
            match self.receiver.try_recv() {
                Ok(command) => self.handle_command(command),
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
    }

    fn handle_command(&mut self, command: Command) {
        match command {
            Command::ActivateDocument(document_generation) => {
                if self.controller.activate_document(document_generation) {
                    self.thumbnail_worker = None;
                    self.preload_loader = ThumbnailImageLoader::new();
                }
            }
            Command::ReplacePreloads {
                document_generation,
                thumbnail_generation,
                requests,
                near,
            } => {
                self.controller.replace_preloads(
                    document_generation,
                    thumbnail_generation,
                    requests,
                    near,
                );
            }
            Command::ReplaceThumbnails(request) => {
                let token = ThumbnailToken {
                    document_generation: request.document_generation,
                    generation: request.generation,
                };
                if self.controller.replace_thumbnail(token) {
                    self.thumbnail_worker = Some((
                        token,
                        ThumbnailWorker::new_skipping(
                            request.assets,
                            request.pages,
                            request.document_generation,
                            request.generation,
                            request.cancel,
                            request.already_generated,
                        )
                        .with_disk_cache(&request.document_path)
                        .with_preparation(self.preparation.clone()),
                    ));
                }
            }
            Command::ReplaceHoverThumbnails { token, requests } => {
                self.controller.replace_hover_thumbnails(token, requests);
            }
            Command::StopDistributedWarmup(token) => {
                self.controller.finish_distributed(token);
            }
            Command::CancelThumbnails {
                document_generation,
            } => {
                if self.controller.cancel_thumbnail(document_generation) {
                    self.thumbnail_worker = None;
                }
            }
            Command::SetThumbnailGenerationSpeed(speed) => {
                self.thumbnail_generation_speed = speed;
            }
        }
    }

    fn wait_for_pacing(&mut self, duration: Duration) -> bool {
        if duration.is_zero() {
            return true;
        }
        match self.receiver.recv_timeout(duration) {
            Ok(command) => {
                self.handle_command(command);
                self.drain_commands()
            }
            Err(RecvTimeoutError::Timeout) => true,
            Err(RecvTimeoutError::Disconnected) => false,
        }
    }

    fn run_preload(&mut self, document_generation: u64, request: PreloadRequest) -> bool {
        let bytes = self.preload_loader.load_image_bytes(&request.source);
        if !self.drain_commands() {
            return false;
        }
        let Some(bytes) = bytes else {
            self.controller
                .fail_preload(document_generation, request.asset_id);
            return true;
        };
        self.controller
            .complete_preload(document_generation, request.asset_id, &bytes);
        (self.emit)(ViewerBackgroundResult::Preload {
            document_generation,
            asset_id: request.asset_id,
            page_index: request.page_index,
            bytes,
        })
    }

    fn run_near_thumbnail(
        &mut self,
        token: ThumbnailToken,
        asset_id: AssetId,
        bytes: gtk::glib::Bytes,
    ) -> bool {
        let Some((worker_token, worker)) = self.thumbnail_worker.as_mut() else {
            return true;
        };
        if *worker_token != token {
            return true;
        }

        worker.run_near(asset_id, &bytes, &mut |result| {
            (self.emit)(ViewerBackgroundResult::Thumbnail(result))
        }) != NearThumbnailStep::OutputClosed
    }

    fn run_hover_thumbnail(
        &mut self,
        token: ThumbnailToken,
        asset_id: AssetId,
        bytes: Option<&gtk::glib::Bytes>,
    ) -> bool {
        let Some((worker_token, worker)) = self.thumbnail_worker.as_mut() else {
            return true;
        };
        if *worker_token != token {
            return true;
        }

        worker.run_demand(asset_id, bytes, &mut |result| {
            (self.emit)(ViewerBackgroundResult::Thumbnail(result))
        }) != DemandThumbnailStep::OutputClosed
    }

    fn run_distributed_thumbnail(&mut self, token: ThumbnailToken) -> BackgroundStep {
        let Some((worker_token, worker)) = self.thumbnail_worker.as_mut() else {
            self.controller.finish_distributed(token);
            return BackgroundStep::Continue(true);
        };
        if *worker_token != token {
            self.controller.finish_distributed(token);
            return BackgroundStep::Continue(true);
        }

        let started = Instant::now();
        match worker.run_next(&mut |result| (self.emit)(ViewerBackgroundResult::Thumbnail(result)))
        {
            ThumbnailStep::More => BackgroundStep::Pace(started.elapsed()),
            ThumbnailStep::Finished => {
                self.controller.finish_distributed(token);
                BackgroundStep::Continue(true)
            }
            ThumbnailStep::OutputClosed => BackgroundStep::OutputClosed,
        }
    }
}

enum BackgroundStep {
    Continue(bool),
    Pace(Duration),
    OutputClosed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn bytes(asset: usize) -> gtk::glib::Bytes {
        gtk::glib::Bytes::from_owned(vec![asset as u8])
    }

    fn preload(asset: usize) -> PreloadRequest {
        PreloadRequest {
            asset_id: AssetId(asset),
            page_index: asset,
            source: ImageSource::Memory(bytes(asset)),
        }
    }

    fn near(asset: usize, ready: bool) -> NearThumbnailRequest {
        NearThumbnailRequest {
            asset_id: AssetId(asset),
            bytes: ready.then(|| bytes(asset)),
        }
    }

    fn hover(asset: usize) -> HoverThumbnailRequest {
        HoverThumbnailRequest {
            asset_id: AssetId(asset),
            bytes: Some(bytes(asset)),
        }
    }

    fn thumbnail(document_generation: u64, generation: u64) -> ThumbnailToken {
        ThumbnailToken {
            document_generation,
            generation,
        }
    }

    #[test]
    fn byte_preload_emits_loaded_bytes_without_cpu_preparation() {
        let (_commands, receiver) = mpsc::channel();
        let outputs = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&outputs);
        let mut lane =
            BackgroundLane::new(receiver, ThumbnailGenerationSpeed::High, move |result| {
                captured.lock().unwrap().push(result);
                true
            });
        lane.controller.activate_document(4);
        let bytes = gtk::glib::Bytes::from_static(b"compressed image bytes");
        let request = PreloadRequest {
            asset_id: AssetId(2),
            page_index: 2,
            source: ImageSource::Memory(bytes.clone()),
        };

        assert!(lane.run_preload(4, request));
        let result = outputs.lock().unwrap().pop().unwrap();
        let ViewerBackgroundResult::Preload {
            document_generation,
            asset_id,
            page_index,
            bytes: emitted_bytes,
        } = result
        else {
            panic!("preload should emit a preload result")
        };
        assert_eq!(document_generation, 4);
        assert_eq!(asset_id, AssetId(2));
        assert_eq!(page_index, 2);
        assert_eq!(emitted_bytes, bytes);
    }

    #[test]
    fn distributed_rest_duration_matches_each_speed() {
        let work = Duration::from_millis(12);

        assert_eq!(
            rest_duration(ThumbnailGenerationSpeed::High, work),
            Duration::ZERO
        );
        assert_eq!(
            rest_duration(ThumbnailGenerationSpeed::Normal, work),
            Duration::from_millis(6)
        );
        assert_eq!(
            rest_duration(ThumbnailGenerationSpeed::Low, work),
            LOW_THUMBNAIL_MIN_REST
        );
        assert_eq!(
            rest_duration(ThumbnailGenerationSpeed::Low, Duration::from_millis(80)),
            Duration::from_millis(160)
        );
    }

    #[test]
    fn speed_change_interrupts_pacing_without_losing_distributed_work() {
        let (commands, receiver) = mpsc::channel();
        let mut lane = BackgroundLane::new(receiver, ThumbnailGenerationSpeed::Low, |_| true);
        lane.controller.activate_document(4);
        lane.controller.replace_thumbnail(thumbnail(4, 8));
        commands
            .send(Command::SetThumbnailGenerationSpeed(
                ThumbnailGenerationSpeed::High,
            ))
            .unwrap();

        assert!(lane.wait_for_pacing(Duration::from_secs(60)));
        assert_eq!(
            lane.thumbnail_generation_speed,
            ThumbnailGenerationSpeed::High
        );
        assert!(matches!(
            lane.controller.next_job(),
            Some(NextJob::DistributedThumbnail(_))
        ));
    }

    #[test]
    fn thumbnail_cancel_interrupts_pacing_and_drops_distributed_work() {
        let (commands, receiver) = mpsc::channel();
        let mut lane = BackgroundLane::new(receiver, ThumbnailGenerationSpeed::Low, |_| true);
        lane.controller.activate_document(4);
        lane.controller.replace_thumbnail(thumbnail(4, 8));
        commands
            .send(Command::CancelThumbnails {
                document_generation: 4,
            })
            .unwrap();

        assert!(lane.wait_for_pacing(Duration::from_secs(60)));
        assert!(lane.controller.next_job().is_none());
    }

    #[test]
    fn document_change_interrupts_pacing_and_drops_old_work() {
        let (commands, receiver) = mpsc::channel();
        let mut lane = BackgroundLane::new(receiver, ThumbnailGenerationSpeed::Normal, |_| true);
        lane.controller.activate_document(4);
        lane.controller.replace_thumbnail(thumbnail(4, 8));
        commands.send(Command::ActivateDocument(5)).unwrap();

        assert!(lane.wait_for_pacing(Duration::from_secs(60)));
        assert!(lane.controller.next_job().is_none());
        assert_eq!(lane.controller.document_generation, Some(5));
    }

    #[test]
    fn preload_is_selected_before_pending_thumbnail() {
        let mut controller = SchedulingController::default();
        controller.activate_document(4);
        assert!(controller.replace_thumbnail(thumbnail(4, 8)));
        assert!(controller.replace_preloads(4, Some(8), vec![preload(2)], vec![near(2, false)]));

        assert!(matches!(
            controller.next_job(),
            Some(NextJob::Preload {
                request: PreloadRequest {
                    asset_id: AssetId(2),
                    ..
                },
                ..
            })
        ));
        assert!(matches!(
            controller.next_job(),
            Some(NextJob::DistributedThumbnail(ThumbnailToken {
                generation: 8,
                ..
            }))
        ));
    }

    #[test]
    fn all_preloads_finish_before_near_thumbnail_and_near_precedes_distributed() {
        let mut controller = SchedulingController::default();
        controller.activate_document(4);
        controller.replace_thumbnail(thumbnail(4, 8));
        controller.replace_preloads(
            4,
            Some(8),
            vec![preload(1), preload(2)],
            vec![near(1, false), near(2, false)],
        );

        assert!(matches!(
            controller.next_job(),
            Some(NextJob::Preload { .. })
        ));
        controller.complete_preload(4, AssetId(1), &bytes(1));
        assert!(matches!(
            controller.next_job(),
            Some(NextJob::Preload { .. })
        ));
        controller.complete_preload(4, AssetId(2), &bytes(2));
        assert!(matches!(
            controller.next_job(),
            Some(NextJob::NearThumbnail {
                asset_id: AssetId(1),
                ..
            })
        ));
        assert!(matches!(
            controller.next_job(),
            Some(NextJob::NearThumbnail {
                asset_id: AssetId(2),
                ..
            })
        ));
        assert!(matches!(
            controller.next_job(),
            Some(NextJob::DistributedThumbnail(_))
        ));
    }

    #[test]
    fn hover_demand_is_selected_after_preload_and_before_near_and_distributed() {
        let mut controller = SchedulingController::default();
        controller.activate_document(4);
        controller.replace_thumbnail(thumbnail(4, 8));
        controller.replace_preloads(4, Some(8), vec![preload(1)], vec![near(2, true)]);
        controller.replace_hover_thumbnails(thumbnail(4, 8), vec![hover(3)]);

        assert!(matches!(
            controller.next_job(),
            Some(NextJob::Preload { .. })
        ));
        assert!(matches!(
            controller.next_job(),
            Some(NextJob::HoverThumbnail {
                asset_id: AssetId(3),
                ..
            })
        ));
        assert!(matches!(
            controller.next_job(),
            Some(NextJob::NearThumbnail {
                asset_id: AssetId(2),
                ..
            })
        ));
        assert!(matches!(
            controller.next_job(),
            Some(NextJob::DistributedThumbnail(_))
        ));
    }

    #[test]
    fn latest_hover_demand_replaces_old_unstarted_assets_and_deduplicates_spreads() {
        let mut controller = SchedulingController::default();
        controller.activate_document(4);
        controller.replace_thumbnail(thumbnail(4, 8));
        controller.replace_hover_thumbnails(thumbnail(4, 8), vec![hover(1), hover(2), hover(2)]);

        controller.replace_hover_thumbnails(thumbnail(4, 8), vec![hover(7), hover(7), hover(8)]);

        assert!(matches!(
            controller.next_job(),
            Some(NextJob::HoverThumbnail {
                asset_id: AssetId(7),
                ..
            })
        ));
        assert!(matches!(
            controller.next_job(),
            Some(NextJob::HoverThumbnail {
                asset_id: AssetId(8),
                ..
            })
        ));
        assert!(matches!(
            controller.next_job(),
            Some(NextJob::DistributedThumbnail(_))
        ));
    }

    #[test]
    fn stopping_distributed_warmup_keeps_hover_and_near_jobs_available() {
        let mut controller = SchedulingController::default();
        controller.activate_document(4);
        controller.replace_thumbnail(thumbnail(4, 8));
        controller.replace_hover_thumbnails(thumbnail(4, 8), vec![hover(3)]);
        controller.replace_preloads(4, Some(8), Vec::new(), vec![near(2, true)]);

        controller.finish_distributed(thumbnail(4, 8));

        assert!(matches!(
            controller.next_job(),
            Some(NextJob::HoverThumbnail { .. })
        ));
        assert!(matches!(
            controller.next_job(),
            Some(NextJob::NearThumbnail { .. })
        ));
        assert!(controller.next_job().is_none());
    }

    #[test]
    fn progressive_transition_starts_normal_worker_before_saturated_hover_demand() {
        let mut controller = SchedulingController::default();
        controller.activate_document(4);

        assert!(!controller.replace_hover_thumbnails(thumbnail(4, 8), vec![hover(3)]));
        assert!(controller.replace_thumbnail(thumbnail(4, 8)));
        controller.finish_distributed(thumbnail(4, 8));
        assert!(controller.replace_hover_thumbnails(thumbnail(4, 8), vec![hover(3)]));

        assert!(matches!(
            controller.next_job(),
            Some(NextJob::HoverThumbnail {
                asset_id: AssetId(3),
                ..
            })
        ));
        assert!(controller.next_job().is_none());
    }

    #[test]
    fn newly_queued_preload_interrupts_and_then_resumes_thumbnail() {
        let mut controller = SchedulingController::default();
        controller.activate_document(4);
        controller.replace_thumbnail(thumbnail(4, 8));
        controller.replace_preloads(4, Some(8), Vec::new(), vec![near(1, true)]);

        assert!(matches!(
            controller.next_job(),
            Some(NextJob::NearThumbnail { .. })
        ));
        controller.replace_preloads(4, Some(8), vec![preload(3)], vec![near(3, false)]);
        assert!(matches!(
            controller.next_job(),
            Some(NextJob::Preload { .. })
        ));
        controller.complete_preload(4, AssetId(3), &bytes(3));
        assert!(matches!(
            controller.next_job(),
            Some(NextJob::NearThumbnail {
                asset_id: AssetId(3),
                ..
            })
        ));
        assert!(matches!(
            controller.next_job(),
            Some(NextJob::DistributedThumbnail(_))
        ));
    }

    #[test]
    fn latest_range_replaces_old_unstarted_near_candidates() {
        let mut controller = SchedulingController::default();
        controller.activate_document(4);
        controller.replace_thumbnail(thumbnail(4, 8));
        controller.replace_preloads(4, Some(8), Vec::new(), vec![near(1, true), near(2, true)]);

        controller.replace_preloads(4, Some(8), Vec::new(), vec![near(7, true)]);

        assert!(matches!(
            controller.next_job(),
            Some(NextJob::NearThumbnail {
                asset_id: AssetId(7),
                ..
            })
        ));
        assert!(matches!(
            controller.next_job(),
            Some(NextJob::DistributedThumbnail(_))
        ));
    }

    #[test]
    fn stale_document_jobs_are_rejected_and_document_change_clears_pending_work() {
        let mut controller = SchedulingController::default();
        controller.activate_document(4);
        controller.replace_thumbnail(thumbnail(4, 8));
        controller.replace_preloads(4, Some(8), vec![preload(1)], vec![near(1, true)]);

        controller.activate_document(5);

        assert!(controller.next_job().is_none());
        assert!(!controller.replace_preloads(4, Some(8), vec![preload(3)], vec![near(3, true)]));
        assert!(!controller.replace_thumbnail(thumbnail(4, 9)));
        assert!(controller.next_job().is_none());
    }

    #[test]
    fn stale_thumbnail_generation_near_is_not_selected() {
        let mut controller = SchedulingController::default();
        controller.activate_document(4);
        controller.replace_thumbnail(thumbnail(4, 9));

        controller.replace_preloads(4, Some(8), Vec::new(), vec![near(1, true)]);

        assert!(matches!(
            controller.next_job(),
            Some(NextJob::DistributedThumbnail(ThumbnailToken {
                generation: 9,
                ..
            }))
        ));
    }

    #[test]
    fn stale_thumbnail_generation_hover_demand_is_rejected() {
        let mut controller = SchedulingController::default();
        controller.activate_document(4);
        controller.replace_thumbnail(thumbnail(4, 9));

        assert!(!controller.replace_hover_thumbnails(thumbnail(4, 8), vec![hover(1)]));
        assert!(matches!(
            controller.next_job(),
            Some(NextJob::DistributedThumbnail(ThumbnailToken {
                generation: 9,
                ..
            }))
        ));
    }

    #[test]
    fn stale_thumbnail_completion_does_not_remove_current_generation() {
        let mut controller = SchedulingController::default();
        controller.activate_document(4);
        controller.replace_thumbnail(thumbnail(4, 9));

        controller.finish_distributed(thumbnail(4, 8));

        assert!(matches!(
            controller.next_job(),
            Some(NextJob::DistributedThumbnail(ThumbnailToken {
                generation: 9,
                ..
            }))
        ));
    }

    #[test]
    fn near_thumbnail_remains_available_after_distributed_scan_finishes() {
        let mut controller = SchedulingController::default();
        controller.activate_document(4);
        controller.replace_thumbnail(thumbnail(4, 9));
        controller.finish_distributed(thumbnail(4, 9));

        controller.replace_preloads(4, Some(9), Vec::new(), vec![near(3, true)]);

        assert!(matches!(
            controller.next_job(),
            Some(NextJob::NearThumbnail {
                asset_id: AssetId(3),
                ..
            })
        ));
        assert!(controller.next_job().is_none());
    }

    #[test]
    fn duplicate_preload_assets_and_near_candidates_are_queued_once() {
        let mut controller = SchedulingController::default();
        controller.activate_document(4);
        controller.replace_thumbnail(thumbnail(4, 8));
        controller.replace_preloads(
            4,
            Some(8),
            vec![preload(2), preload(2), preload(3)],
            vec![near(2, true), near(2, true), near(3, true)],
        );

        assert!(matches!(
            controller.next_job(),
            Some(NextJob::Preload {
                request: PreloadRequest {
                    asset_id: AssetId(2),
                    ..
                },
                ..
            })
        ));
        assert!(matches!(
            controller.next_job(),
            Some(NextJob::Preload {
                request: PreloadRequest {
                    asset_id: AssetId(3),
                    ..
                },
                ..
            })
        ));
        controller.complete_preload(4, AssetId(2), &bytes(2));
        controller.complete_preload(4, AssetId(3), &bytes(3));
        assert!(matches!(
            controller.next_job(),
            Some(NextJob::NearThumbnail {
                asset_id: AssetId(2),
                ..
            })
        ));
        assert!(matches!(
            controller.next_job(),
            Some(NextJob::NearThumbnail {
                asset_id: AssetId(3),
                ..
            })
        ));
        assert!(matches!(
            controller.next_job(),
            Some(NextJob::DistributedThumbnail(_))
        ));
    }

    #[test]
    fn thumbnail_cancel_drops_pending_near_without_stopping_preload() {
        let mut controller = SchedulingController::default();
        controller.activate_document(4);
        controller.replace_thumbnail(thumbnail(4, 8));
        controller.replace_preloads(4, Some(8), vec![preload(2)], vec![near(2, true)]);

        controller.cancel_thumbnail(4);

        assert!(matches!(
            controller.next_job(),
            Some(NextJob::Preload { .. })
        ));
        assert!(controller.next_job().is_none());
    }
}
