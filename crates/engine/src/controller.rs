use std::sync::Arc;

use rtrb::{Consumer, Producer};
use thiserror::Error;

use crate::shared::Shared;
use crate::{Command, DeckId, DeckSnapshot, Meters, TrackBuffer};

/// The command queue to the audio thread is full; the command was not sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("engine command queue is full")]
pub struct CommandQueueFull;

/// Control-side handle: sends commands to the renderer and reads its published state.
pub struct EngineController {
    commands: Producer<Command>,
    garbage: Consumer<Arc<TrackBuffer>>,
    shared: Arc<Shared>,
}

impl EngineController {
    pub(crate) fn new(
        commands: Producer<Command>,
        garbage: Consumer<Arc<TrackBuffer>>,
        shared: Arc<Shared>,
    ) -> Self {
        Self {
            commands,
            garbage,
            shared,
        }
    }

    /// Queues `cmd` for the next render block.
    pub fn send(&mut self, cmd: Command) -> Result<(), CommandQueueFull> {
        // Draining garbage before every push bounds it by the commands in flight, so the
        // renderer's garbage queue (same capacity as the command queue) can never fill up.
        self.collect_garbage();
        self.commands.push(cmd).map_err(|_| CommandQueueFull)
    }

    /// Frees tracks the renderer has replaced. Dropping them here keeps deallocation off
    /// the audio thread.
    pub fn collect_garbage(&mut self) {
        while let Ok(track) = self.garbage.pop() {
            drop(track);
        }
    }

    /// The deck's state as of the last rendered block.
    pub fn deck(&self, id: DeckId) -> DeckSnapshot {
        self.shared.load_deck(id)
    }

    /// Peak levels of the last rendered block.
    pub fn meters(&self) -> Meters {
        self.shared.load_meters()
    }
}
