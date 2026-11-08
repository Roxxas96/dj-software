use std::sync::Arc;
use std::time::Instant;

use rtrb::{Consumer, Producer, PushError};

use crate::deck::Deck;
use crate::mixer::Mixer;
use crate::sampler::Sampler;
use crate::shared::Shared;
use crate::stems::Garbage;
use crate::{Command, DeckId, EngineConfig, EqMode, MasterCommand, Meters, StemKnobs};

/// Audio-thread handle: renders output blocks. [`render`](Self::render) never allocates,
/// frees, locks or blocks.
pub struct EngineRenderer {
    config: EngineConfig,
    commands: Consumer<Command>,
    garbage: Producer<Garbage>,
    shared: Arc<Shared>,
    decks: [Deck; DeckId::ALL.len()],
    mixer: Mixer,
    sampler: Sampler,
    eq_mode: EqMode,
    stem_knobs: StemKnobs,
}

impl EngineRenderer {
    pub(crate) fn new(
        config: EngineConfig,
        commands: Consumer<Command>,
        garbage: Producer<Garbage>,
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
            eq_mode: EqMode::default(),
            stem_knobs: StemKnobs::default(),
        };
        renderer.publish(crate::clock::micros_at(Instant::now()));
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
        let started = Instant::now();
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
            for (id, deck) in DeckId::ALL.into_iter().zip(&self.decks) {
                self.mixer
                    .set_beat_frames(id, deck.beat_frames(self.config.sample_rate));
            }
            self.update_stems(master.len() / 2);
            let inputs = self.mixer.inputs(master.len());
            for (deck, input) in self.decks.iter_mut().zip(inputs) {
                deck.render_add(input, self.config.sample_rate);
            }
            let sampler = self
                .sampler
                .render(master.len() / 2, self.config.sample_rate);
            meters.merge(&self.mixer.process(master, headphones, sampler));
        }

        self.publish(crate::clock::micros_at(started));
        self.shared.store_meters(&meters);
        let block_seconds = (samples / 2) as f64 / f64::from(self.config.sample_rate);
        if block_seconds > 0.0 {
            let load = started.elapsed().as_secs_f64() / block_seconds;
            self.shared.store_dsp_load(load as f32);
        }
    }

    fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::Deck(id, cmd) => {
                for old in self.decks[id.index()].apply(cmd).into_iter().flatten() {
                    self.discard(old);
                }
                self.drop_syncs_to_stopped_master(id);
            }
            Command::Master(MasterCommand::EqMode(mode)) => self.eq_mode = mode,
            Command::Master(MasterCommand::StemKnobs(knobs)) => self.stem_knobs = knobs,
            Command::PhaseSync {
                deck,
                master,
                beat_ratio,
            } => self.phase_sync(deck, master, beat_ratio),
            Command::Channel(id, cmd) => self.mixer.apply_channel(id, cmd),
            Command::Master(cmd) => self.mixer.apply_master(cmd),
            Command::Sampler(cmd) => {
                if let Some(old) = self.sampler.apply(cmd) {
                    self.discard(Garbage::Track(old));
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

    /// Before a chunk renders: sets each deck's stem gains from its channel's EQ knobs (in
    /// the knob layout) and bypasses the EQ bands of the channels whose stems take the
    /// knobs. A change of either ramps over a chunk, which is the changeover's crossfade;
    /// under keylock the bands change once the stem gains come out of the stretcher.
    fn update_stems(&mut self, frames: usize) {
        for id in DeckId::ALL {
            let knobs = self.mixer.eq_positions(id);
            let deck = &mut self.decks[id.index()];
            let (active, released) = deck.update_stems(self.eq_mode, self.stem_knobs, knobs);
            // The deck's gains are heard after its stretcher's latency: so are the bands'.
            self.mixer
                .set_stems_active(id, active, deck.stem_latency(), frames);
            if let Some(stems) = released {
                self.discard(Garbage::Stems(stems));
            }
        }
    }

    /// Hands a replaced track or stem set back to the controller, which frees it off the
    /// audio thread.
    fn discard(&mut self, garbage: Garbage) {
        let result = self.garbage.push(garbage);
        debug_assert!(result.is_ok(), "garbage queue full");
        if let Err(PushError::Full(garbage)) = result {
            // Unreachable while `EngineController::send` collects garbage first. Leaking
            // is still better than freeing on the audio thread.
            std::mem::forget(garbage);
        }
    }

    /// Publishes the decks' state, stamped `rendered_at_micros` on [`crate::clock`].
    fn publish(&self, rendered_at_micros: u64) {
        for (id, deck) in DeckId::ALL.into_iter().zip(&self.decks) {
            self.shared.store_deck(
                id,
                &deck.snapshot(),
                &self.mixer.channel_snapshot(id),
                rendered_at_micros,
            );
        }
        self.shared.store_sampler(&self.sampler.snapshot());
    }
}
