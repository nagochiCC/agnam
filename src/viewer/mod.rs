mod background;
mod cache;
mod page_paintable;
mod preparation;
mod render;
#[cfg(test)]
pub(crate) use render::{ANALYSIS_COUNT, CPU_DECODE_COUNT};
mod session;
mod smart_crop;
mod state;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ThumbnailGenerationSpeed {
    Low,
    #[default]
    Normal,
    High,
}

pub(crate) use background::{
    HoverThumbnailRequest, NearThumbnailRequest, PreloadRequest, ViewerBackgroundResult,
    ViewerBackgroundScheduler,
};
pub(crate) use preparation::{
    SmartCropPreparationHandle, SmartCropPreparationRequest, SmartCropPreparationResult,
    SmartCropPreparationScheduler,
};
pub(crate) use session::ViewerSession;
pub(crate) use state::{ViewMode, snap_to_view};
