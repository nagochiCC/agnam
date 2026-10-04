use super::scheduler::{ThumbnailSourceKind, ThumbnailTextureKey};
use crate::bookshelf::thumbnail::ThumbnailData;
use std::path::PathBuf;

#[derive(Debug, Default)]
pub(in crate::app) struct LibraryThumbnailBatch {
    generation: u64,
    batch_id: u64,
    quiet_revision: u64,
    pending: Vec<(ThumbnailTextureKey, ThumbnailData)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) struct ThumbnailBatchTimers {
    pub(in crate::app) generation: u64,
    pub(in crate::app) batch_id: u64,
    pub(in crate::app) quiet_revision: u64,
    pub(in crate::app) starts_batch: bool,
}

impl LibraryThumbnailBatch {
    pub(in crate::app) fn begin_generation(&mut self, generation: u64) {
        self.generation = generation;
        self.batch_id = 0;
        self.quiet_revision = 0;
        self.pending.clear();
    }

    /// Queues a prepared thumbnail and returns tokens for the quiet and maximum timers.
    #[cfg(test)]
    pub(in crate::app) fn push(
        &mut self,
        generation: u64,
        source: PathBuf,
        thumbnail: ThumbnailData,
    ) -> Option<ThumbnailBatchTimers> {
        self.push_with_kind(
            generation,
            source,
            ThumbnailSourceKind::BookCover,
            thumbnail,
        )
    }

    pub(in crate::app) fn push_with_kind(
        &mut self,
        generation: u64,
        source: PathBuf,
        kind: ThumbnailSourceKind,
        thumbnail: ThumbnailData,
    ) -> Option<ThumbnailBatchTimers> {
        if generation != self.generation {
            return None;
        }
        let starts_batch = self.pending.is_empty();
        if starts_batch {
            self.batch_id = self.batch_id.wrapping_add(1);
        }
        self.quiet_revision = self.quiet_revision.wrapping_add(1);
        self.pending
            .push((ThumbnailTextureKey::new(source, kind), thumbnail));
        Some(ThumbnailBatchTimers {
            generation,
            batch_id: self.batch_id,
            quiet_revision: self.quiet_revision,
            starts_batch,
        })
    }

    /// Takes a batch for a valid quiet timeout (`Some(revision)`) or its maximum
    /// latency timeout (`None`). Stale timer tokens leave the current batch intact.
    pub(in crate::app) fn take_for_timeout(
        &mut self,
        generation: u64,
        batch_id: u64,
        quiet_revision: Option<u64>,
    ) -> Vec<(ThumbnailTextureKey, ThumbnailData)> {
        if generation != self.generation
            || batch_id != self.batch_id
            || self.pending.is_empty()
            || quiet_revision.is_some_and(|revision| revision != self.quiet_revision)
        {
            return Vec::new();
        }
        std::mem::take(&mut self.pending)
    }
}
