use super::scheduler::{ThumbnailDemandEvaluation, ThumbnailTextureKey};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) struct InitialRevealTimeout {
    pub(in crate::app) generation: u64,
    pub(in crate::app) revision: u64,
}

#[derive(Debug, PartialEq, Eq)]
enum InitialRevealPhase {
    Revealed,
    PendingRender,
    AwaitingDemand,
    AwaitingLookups(HashSet<ThumbnailTextureKey>),
}

#[derive(Debug)]
pub(in crate::app) struct LibraryInitialReveal {
    generation: u64,
    revision: u64,
    phase: InitialRevealPhase,
}

impl Default for LibraryInitialReveal {
    fn default() -> Self {
        Self {
            generation: 0,
            revision: 0,
            phase: InitialRevealPhase::Revealed,
        }
    }
}

impl LibraryInitialReveal {
    pub(in crate::app) fn begin_directory(&mut self, generation: u64) {
        self.generation = generation;
        self.revision = self.revision.wrapping_add(1);
        self.phase = InitialRevealPhase::PendingRender;
    }

    pub(in crate::app) fn generation(&self) -> u64 {
        self.generation
    }

    pub(in crate::app) fn begin_render(&mut self) -> Option<InitialRevealTimeout> {
        if self.phase != InitialRevealPhase::PendingRender {
            return None;
        }
        self.revision = self.revision.wrapping_add(1);
        self.phase = InitialRevealPhase::AwaitingDemand;
        Some(InitialRevealTimeout {
            generation: self.generation,
            revision: self.revision,
        })
    }

    /// Captures only the first allocated visible demand after directory render.
    /// Later viewport changes and scrolling never hide or restart the library.
    pub(in crate::app) fn begin_demand(&mut self, evaluation: &ThumbnailDemandEvaluation) -> bool {
        if self.phase != InitialRevealPhase::AwaitingDemand {
            return false;
        }
        let ThumbnailDemandEvaluation::Ready(demands) = evaluation else {
            return false;
        };
        let remaining = demands
            .iter()
            .map(|demand| demand.key())
            .collect::<HashSet<_>>();
        if remaining.is_empty() {
            self.phase = InitialRevealPhase::Revealed;
            true
        } else {
            self.phase = InitialRevealPhase::AwaitingLookups(remaining);
            false
        }
    }

    /// A cache miss completes its lookup just like a hit. Source generation is
    /// deliberately not part of the initial reveal condition.
    pub(in crate::app) fn complete_lookup(
        &mut self,
        generation: u64,
        key: &ThumbnailTextureKey,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        let InitialRevealPhase::AwaitingLookups(remaining) = &mut self.phase else {
            return false;
        };
        remaining.remove(key);
        if remaining.is_empty() {
            self.phase = InitialRevealPhase::Revealed;
            true
        } else {
            false
        }
    }

    pub(in crate::app) fn timeout(&mut self, timeout: InitialRevealTimeout) -> bool {
        if timeout.generation != self.generation
            || timeout.revision != self.revision
            || matches!(self.phase, InitialRevealPhase::Revealed)
        {
            return false;
        }
        self.phase = InitialRevealPhase::Revealed;
        true
    }
}
