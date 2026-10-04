use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::bookshelf::thumbnail::{ThumbnailData, fit_thumbnail_data};

/// Each Cover scope owns its own generation clock. A worker only reads it at
/// safe phase boundaries; an active codec call cannot be interrupted.
#[derive(Debug, Clone)]
pub(super) struct CoverCancel {
    current: Arc<AtomicU64>,
    generation: u64,
}

impl CoverCancel {
    pub(super) fn new(current: Arc<AtomicU64>, generation: u64) -> Self {
        Self {
            current,
            generation,
        }
    }

    pub(super) fn cancelled(&self) -> bool {
        self.current.load(Ordering::Acquire) != self.generation
    }
}

pub(super) fn fit_generated_cover(
    result: Result<Option<ThumbnailData>, String>,
    width: u32,
    height: u32,
    cancel: &CoverCancel,
) -> Result<Option<ThumbnailData>, String> {
    if cancel.cancelled() {
        return Ok(None);
    }
    let Some(thumbnail) = result? else {
        return Ok(None);
    };
    if cancel.cancelled() {
        return Ok(None);
    }
    let fitted = fit_thumbnail_data(thumbnail, width, height).map_err(|error| error.to_string())?;
    if cancel.cancelled() {
        Ok(None)
    } else {
        Ok(Some(fitted))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    #[test]
    fn cancellation_is_visible_across_worker_phase_boundary() {
        let current = Arc::new(AtomicU64::new(1));
        let token = CoverCancel::new(current.clone(), 1);
        let barrier = Arc::new(Barrier::new(2));
        let worker_barrier = barrier.clone();
        let worker = std::thread::spawn(move || {
            assert!(!token.cancelled());
            worker_barrier.wait();
            worker_barrier.wait();
            token.cancelled()
        });
        barrier.wait();
        current.store(2, Ordering::Release);
        barrier.wait();
        assert!(worker.join().unwrap());
        assert!(!CoverCancel::new(current, 2).cancelled());
    }
}
