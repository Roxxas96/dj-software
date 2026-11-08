//! Real-time audio engine core: mixing, transport and timing.
//!
//! [`engine`] returns a pair of handles: the [`EngineController`] stays on the control side
//! (UI, MIDI) and sends [`Command`]s, while the [`EngineRenderer`] lives on the audio thread
//! and renders output blocks without allocating, freeing or locking.

mod command;
mod controller;
mod deck;
mod dsp;
mod ids;
mod memory;
pub mod mixer;
mod monitor;
mod renderer;
mod sampler;
mod shared;
mod stems;
mod track;

pub use command::{
    ChannelCommand, Command, DeckCommand, JogSurface, MasterCommand, SamplerCommand, TempoRange,
    XfaderAssign,
};
pub use controller::{CommandQueueFull, EngineController};
pub use dsp::fx::EffectKind;
#[doc(hidden)]
pub use dsp::fx::{EffectForTests, effect_for_tests};
pub use ids::DeckId;
pub use memory::{Grid, HOTCUES, HotcueSlot, TrackMemory};
pub use monitor::EngineMonitor;
pub use renderer::EngineRenderer;
pub use sampler::{SAMPLER_SLOTS, SamplerSnapshot};
pub use shared::{ChannelSnapshot, DeckSnapshot, Meters};
pub use stems::{
    DEFAULT_KICKS_HZ, EqMode, Stem, StemError, StemKnobs, StemSet, knob_law, pad_gains,
};
pub use track::{TrackBuffer, TrackError};

/// Capacity of the command queue (controller to renderer).
const COMMAND_QUEUE_CAPACITY: usize = 256;
/// Capacity of the garbage queue (renderer to controller): a command hands back at most
/// three items (a `Load` returns the old track, its stems and a set waiting to replace
/// them), and each deck may hand back one set of dropped stems later, once their gains
/// have ramped out.
const GARBAGE_QUEUE_CAPACITY: usize = 3 * COMMAND_QUEUE_CAPACITY + DeckId::ALL.len();

/// Output format the renderer is driven with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineConfig {
    /// Output sample rate in Hz.
    pub sample_rate: u32,
    /// Largest block, in frames, that a single [`EngineRenderer::render`] call may receive.
    pub max_block_frames: usize,
}

/// Creates a connected controller/renderer pair.
///
/// # Panics
///
/// If `config.sample_rate` or `config.max_block_frames` is zero.
pub fn engine(config: EngineConfig) -> (EngineController, EngineRenderer) {
    assert!(config.sample_rate > 0, "sample rate must be positive");
    assert!(
        config.max_block_frames > 0,
        "max block size must be positive"
    );

    let (command_tx, command_rx) = rtrb::RingBuffer::new(COMMAND_QUEUE_CAPACITY);
    let (garbage_tx, garbage_rx) = rtrb::RingBuffer::new(GARBAGE_QUEUE_CAPACITY);
    let renderer = EngineRenderer::new(config, command_rx, garbage_tx);
    let controller = EngineController::new(
        command_tx,
        garbage_rx,
        EngineMonitor::new(renderer.shared(), config.sample_rate),
    );
    (controller, renderer)
}
