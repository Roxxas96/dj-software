use std::sync::Arc;

use rtrb::{Consumer, Producer};
use thiserror::Error;

use crate::shared::Shared;
use crate::stems::Garbage;
use crate::{ChannelSnapshot, Command, DeckId, DeckSnapshot, Meters, SamplerSnapshot};

/// The command queue to the audio thread is full; the command was not sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("engine command queue is full")]
pub struct CommandQueueFull;

/// Control-side handle: sends commands to the renderer and reads its published state.
pub struct EngineController {
    commands: Producer<Command>,
    garbage: Consumer<Garbage>,
    shared: Arc<Shared>,
}

impl EngineController {
    pub(crate) fn new(
        commands: Producer<Command>,
        garbage: Consumer<Garbage>,
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
        // renderer's garbage queue (three times the command queue: a command returns at
        // most three items) can never fill up.
        self.collect_garbage();
        self.commands.push(cmd).map_err(|_| CommandQueueFull)
    }

    /// Queues every command in `cmds`, or none of them when the queue has fewer free slots
    /// than `cmds.len()`.
    pub fn send_all(&mut self, cmds: &[Command]) -> Result<(), CommandQueueFull> {
        self.collect_garbage();
        if self.commands.slots() < cmds.len() {
            return Err(CommandQueueFull);
        }
        for cmd in cmds {
            self.commands
                .push(cmd.clone())
                .map_err(|_| CommandQueueFull)?;
        }
        Ok(())
    }

    /// Frees the tracks and stems the renderer has replaced. Dropping them here keeps
    /// deallocation off the audio thread.
    pub fn collect_garbage(&mut self) {
        while let Ok(garbage) = self.garbage.pop() {
            drop(garbage);
        }
    }

    /// The deck's state as of the last rendered block (for display: its fields may mix two
    /// consecutive blocks).
    pub fn deck(&self, id: DeckId) -> DeckSnapshot {
        self.shared.load_deck(id)
    }

    /// The deck's state from one rendered block, every field together, or `None` if the
    /// renderer kept publishing while it was read (try again later). Use it when acting on
    /// several fields together, such as telling which track the cue points belong to;
    /// [`EngineController::deck`] is enough for display.
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
}
