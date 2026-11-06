use std::sync::Arc;

use rtrb::{Consumer, Producer, PushError};

use crate::deck::Deck;
use crate::mixer::Mixer;
use crate::sampler::Sampler;
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
    sampler: Sampler,
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
            // The decks' keylock stretchers allocate: built here, before the audio thread.
            decks: std::array::from_fn(|_| Deck::new(config.sample_rate, config.max_block_frames)),
            mixer: Mixer::new(config.sample_rate, config.max_block_frames),
            sampler: Sampler::new(config.max_block_frames),
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
            self.resolve_pending_syncs();
            let inputs = self.mixer.inputs(master.len());
            for (deck, input) in self.decks.iter_mut().zip(inputs) {
                deck.render_add(input, self.config.sample_rate);
            }
            let sampler = self
                .sampler
                .render(master.len() / 2, self.config.sample_rate);
            meters.merge(&self.mixer.process(master, headphones, sampler));
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
                self.drop_syncs_to_stopped_master(id);
            }
            Command::PhaseSync {
                deck,
                master,
                beat_ratio,
            } => self.phase_sync(deck, master, beat_ratio),
            Command::Channel(id, cmd) => self.mixer.apply_channel(id, cmd),
            Command::Master(cmd) => self.mixer.apply_master(cmd),
            Command::Sampler(cmd) => {
                if let Some(old) = self.sampler.apply(cmd) {
                    self.discard(old);
                }
            }
        }
    }

    fn phase_sync(&mut self, deck: DeckId, master: DeckId, beat_ratio: f64) {
        if deck == master || !beat_ratio.is_finite() || beat_ratio <= 0.0 {
            return;
        }
        let Some((grid, position)) = self.decks[master.index()].phase_source() else {
            return;
        };
        self.decks[deck.index()].phase_sync(
            master,
            beat_ratio,
            grid.beats_at(position) * beat_ratio,
        );
    }

    /// Drops the pending syncs that follow `master` once it is no longer playing. Checked
    /// after each command so a stop followed by a play in the same batch still counts.
    fn drop_syncs_to_stopped_master(&mut self, master: DeckId) {
        if self.decks[master.index()].phase_source().is_some() {
            return;
        }
        for deck in &mut self.decks {
            if deck.pending_sync().is_some_and(|(id, _)| id == master) {
                deck.drop_pending_sync();
            }
        }
    }

    /// Before a chunk renders: aligns the decks that started playing (not just previewing)
    /// with a pending sync
    /// against their master's position now, and drops the syncs whose master cannot be
    /// followed any more.
    fn resolve_pending_syncs(&mut self) {
        for id in DeckId::ALL {
            let Some((master, ratio)) = self.decks[id.index()].pending_sync() else {
                continue;
            };
            let Some((grid, position)) = self.decks[master.index()].phase_source() else {
                self.decks[id.index()].drop_pending_sync();
                continue;
            };
            let deck = &mut self.decks[id.index()];
            // A preview from a held CUE or pad snaps back on release: wait for real play.
            if deck.plays_for_real() {
                deck.drop_pending_sync();
                deck.align_phase(grid.beats_at(position) * ratio);
            }
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
        self.shared.store_sampler(&self.sampler.snapshot());
    }
}
