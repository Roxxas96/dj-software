//! Real-time audio engine core: mixing, transport and timing.
//!
//! [`engine`] returns a pair of handles: the [`EngineController`] stays on the control side
//! (UI, MIDI) and sends [`Command`]s, while the [`EngineRenderer`] lives on the audio thread
//! and renders output blocks without allocating, freeing or locking.

mod command;
mod controller;
mod deck;
mod ids;
mod renderer;
mod shared;
mod track;

pub use command::{ChannelCommand, Command, DeckCommand, MasterCommand, TempoRange};
pub use controller::{CommandQueueFull, EngineController};
pub use ids::DeckId;
pub use renderer::EngineRenderer;
pub use shared::DeckSnapshot;
pub use track::{TrackBuffer, TrackError};

/// Capacity of the command queue (controller to renderer).
const COMMAND_QUEUE_CAPACITY: usize = 256;
/// Capacity of the garbage queue (renderer to controller).
const GARBAGE_QUEUE_CAPACITY: usize = 256;

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
    let controller = EngineController::new(command_tx, garbage_rx, renderer.shared());
    (controller, renderer)
}
