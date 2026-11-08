use std::sync::Arc;

use crate::shared::Shared;
use crate::{ChannelSnapshot, DeckId, DeckSnapshot, Meters, SamplerSnapshot};

/// Read-only, cloneable handle on the state the renderer publishes, for other threads
/// (such as the UI). It cannot send commands.
#[derive(Debug, Clone)]
pub struct EngineMonitor {
    shared: Arc<Shared>,
    sample_rate: u32,
}

impl EngineMonitor {
    pub(crate) fn new(shared: Arc<Shared>, sample_rate: u32) -> Self {
        Self {
            shared,
            sample_rate,
        }
    }

    /// The deck's state as of the last rendered block (for display: its fields may mix two
    /// consecutive blocks).
    pub fn deck(&self, id: DeckId) -> DeckSnapshot {
        self.shared.load_deck(id)
    }

    /// [`EngineMonitor::deck`], with when its block began rendering in microseconds on
    /// [`crate::clock`]. Both are from one block unless the renderer kept publishing while
    /// they were read (then, as for [`EngineMonitor::deck`], they may mix two).
    pub fn deck_rendered_at(&self, id: DeckId) -> (DeckSnapshot, u64) {
        self.shared.load_deck_rendered_at(id)
    }

    /// The deck's state from one rendered block, every field together, or `None` if the
    /// renderer kept publishing while it was read (try again later). Use it when acting on
    /// several fields together, such as telling which track the cue points belong to;
    /// [`EngineMonitor::deck`] is enough for display.
    pub fn deck_consistent(&self, id: DeckId) -> Option<DeckSnapshot> {
        self.shared.load_deck_consistent(id)
    }

    /// The channel's published mixer state as of the last rendered block (for display).
    pub fn channel(&self, id: DeckId) -> ChannelSnapshot {
        self.shared.load_channel(id)
    }

    /// Peak levels of the last rendered block.
    pub fn meters(&self) -> Meters {
        self.shared.load_meters()
    }

    /// Which sampler slots were playing at the last rendered block (for display).
    pub fn sampler(&self) -> SamplerSnapshot {
        self.shared.load_sampler()
    }

    /// The last block's render time divided by the block's duration: 0 to 1 in time, above 1
    /// when the block took longer than it plays.
    pub fn dsp_load(&self) -> f32 {
        self.shared.load_dsp_load()
    }

    /// The engine's output sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}
