use std::sync::Arc;

use rtrb::{Consumer, Producer, PushError};

use crate::deck::Deck;
use crate::shared::Shared;
use crate::{Command, DeckId, EngineConfig, TrackBuffer};

/// Audio-thread handle: renders output blocks. [`render`](Self::render) never allocates,
/// frees, locks or blocks.
pub struct EngineRenderer {
    config: EngineConfig,
    commands: Consumer<Command>,
    garbage: Producer<Arc<TrackBuffer>>,
    shared: Arc<Shared>,
    decks: [Deck; DeckId::ALL.len()],
}

impl EngineRenderer {
    pub(crate) fn new(
        config: EngineConfig,
        commands: Consumer<Command>,
        garbage: Producer<Arc<TrackBuffer>>,
    ) -> Self {
        let renderer = Self {
            config,
            commands,
            garbage,
            shared: Arc::default(),
            decks: Default::default(),
        };
        renderer.publish();
        renderer
    }

    pub(crate) fn shared(&self) -> Arc<Shared> {
        Arc::clone(&self.shared)
    }

    pub fn max_block_frames(&self) -> usize {
        self.config.max_block_frames
    }

    /// Applies all queued commands, then overwrites both interleaved stereo buffers with
    /// the next `master.len() / 2` frames.
    pub fn render(&mut self, master: &mut [f32], headphones: &mut [f32]) {
        debug_assert_eq!(master.len(), headphones.len());
        debug_assert!(master.len().is_multiple_of(2));
        debug_assert!(master.len() / 2 <= self.config.max_block_frames);

        while let Ok(cmd) = self.commands.pop() {
            self.apply(cmd);
        }

        master.fill(0.0);
        headphones.fill(0.0);
        for deck in &mut self.decks {
            deck.render_add(master, self.config.sample_rate);
        }

        self.publish();
    }

    fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::Deck(id, cmd) => {
                if let Some(old) = self.decks[id.index()].apply(cmd) {
                    self.discard(old);
                }
            }
            Command::Channel(_, cmd) => match cmd {},
            Command::Master(cmd) => match cmd {},
        }
    }

    /// Hands a replaced track back to the controller, which frees it off the audio thread.
    fn discard(&mut self, track: Arc<TrackBuffer>) {
        let result = self.garbage.push(track);
        debug_assert!(result.is_ok(), "garbage queue full");
        if let Err(PushError::Full(track)) = result {
            // Unreachable while `EngineController::send` collects garbage first. Leaking
            // is still better than freeing on the audio thread.
            std::mem::forget(track);
        }
    }

    fn publish(&self) {
        for (id, deck) in DeckId::ALL.into_iter().zip(&self.decks) {
            self.shared.store_deck(id, &deck.snapshot());
        }
    }
}
