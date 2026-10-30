use std::sync::Arc;

use rtrb::{Consumer, Producer, PushError};

use crate::deck::Deck;
use crate::mixer::Mixer;
use crate::shared::Shared;
use crate::{Command, DeckId, EngineConfig, Meters, TrackBuffer};

/// Audio-thread handle: renders output blocks. [`render`](Self::render) never allocates,
/// frees, locks or blocks.
pub struct EngineRenderer {
    config: EngineConfig,
    commands: Consumer<Command>,
    garbage: Producer<Arc<TrackBuffer>>,
    shared: Arc<Shared>,
    decks: [Deck; DeckId::ALL.len()],
    mixer: Mixer,
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
            mixer: Mixer::new(config.max_block_frames),
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
    /// the next frames.
    ///
    /// Any buffer lengths work and never panic: blocks longer than
    /// [`max_block_frames`](Self::max_block_frames) are rendered in chunks, and when the
    /// lengths differ (or a trailing half frame is left over) only the whole frames both
    /// buffers share are rendered; the rest of either buffer is zero-filled.
    pub fn render(&mut self, master: &mut [f32], headphones: &mut [f32]) {
        while let Ok(cmd) = self.commands.pop() {
            self.apply(cmd);
        }

        let samples = master.len().min(headphones.len()) / 2 * 2;
        let (master, master_rest) = master.split_at_mut(samples);
        let (headphones, headphones_rest) = headphones.split_at_mut(samples);
        master_rest.fill(0.0);
        headphones_rest.fill(0.0);

        let chunk = self.config.max_block_frames * 2;
        let mut meters = Meters::default();
        for (master, headphones) in master.chunks_mut(chunk).zip(headphones.chunks_mut(chunk)) {
            let inputs = self.mixer.inputs(master.len());
            for (deck, input) in self.decks.iter_mut().zip(inputs) {
                deck.render_add(input, self.config.sample_rate);
            }
            meters.merge(&self.mixer.process(master, headphones));
        }

        self.publish();
        self.shared.store_meters(&meters);
    }

    fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::Deck(id, cmd) => {
                if let Some(old) = self.decks[id.index()].apply(cmd) {
                    self.discard(old);
                }
            }
            Command::Channel(id, cmd) => self.mixer.apply_channel(id, cmd),
            Command::Master(cmd) => self.mixer.apply_master(cmd),
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
