use std::sync::Arc;

mod hotcue;
mod jog;
mod looping;
mod slip;

use crate::{DeckCommand, DeckId, DeckSnapshot, Grid, HOTCUES, TempoRange, TrackBuffer};

/// Transport state of one deck. Owned by the renderer.
#[derive(Debug)]
pub(crate) struct Deck {
    track: Option<Arc<TrackBuffer>>,
    playing: bool,
    /// Playhead in track frames.
    position: f64,
    /// Cue point in track frames.
    cue: f64,
    /// A cue or hotcue is held down and playing back from its point, and which one.
    preview: Option<hotcue::PreviewSource>,
    /// Play was pressed during preview, so releasing the held button keeps playing.
    play_latched: bool,
    tempo_slider: f32,
    tempo_range: TempoRange,
    hotcues: [Option<f64>; HOTCUES],
    grid: Option<Grid>,
    loop_in: Option<f64>,
    loop_out: Option<f64>,
    loop_active: bool,
    /// Length in beats of the next auto loop.
    loop_beats: f64,
    /// Touching the jog wheel's top scratches. Kept across loads.
    vinyl: bool,
    jog_touched: bool,
    /// While scratching, the track frames left between the playhead and the platter's
    /// position (the scratch target).
    scratch: Option<f64>,
    /// Nudge speed bend, as a fraction of normal speed; fades to 0 while playing.
    bend: f64,
    /// A phase sync waiting for this deck to play: the master and the beat ratio.
    pending_sync: Option<(DeckId, f64)>,
    /// Scratches, loops and hotcue jumps return to where the track would be. Kept across
    /// loads.
    slip: bool,
    /// Where the track would be without the slipping action, while one lasts.
    ghost: Option<slip::Ghost>,
    /// How many `Load`s were applied.
    load_seq: u64,
}

impl Default for Deck {
    fn default() -> Self {
        Self {
            track: None,
            playing: false,
            position: 0.0,
            cue: 0.0,
            preview: None,
            play_latched: false,
            tempo_slider: 0.5,
            tempo_range: TempoRange::default(),
            hotcues: [None; HOTCUES],
            grid: None,
            loop_in: None,
            loop_out: None,
            loop_active: false,
            loop_beats: looping::DEFAULT_LOOP_BEATS,
            vinyl: true,
            jog_touched: false,
            scratch: None,
            bend: 0.0,
            pending_sync: None,
            slip: false,
            ghost: None,
            load_seq: 0,
        }
    }
}

impl Deck {
    /// Applies `cmd` and returns the track it replaced, if any. The caller must not drop
    /// that track on the audio thread.
    #[must_use]
    pub(crate) fn apply(&mut self, cmd: DeckCommand) -> Option<Arc<TrackBuffer>> {
        match cmd {
            DeckCommand::Load { track, memory } => {
                self.reset_transport();
                let memory = memory.clamped(track.frames());
                self.cue = memory.cue;
                self.hotcues = memory.hotcues;
                self.grid = memory.grid;
                self.load_seq = self.load_seq.wrapping_add(1);
                return self.track.replace(track);
            }
            DeckCommand::Unload => {
                self.reset_transport();
                return self.track.take();
            }
            DeckCommand::Play => self.play(),
            DeckCommand::Pause => self.pause(),
            DeckCommand::TogglePlay => {
                // During cue preview, play latches instead of pausing (hold CUE, press PLAY).
                if self.playing && self.preview.is_none() {
                    self.pause();
                } else {
                    self.play();
                }
            }
            DeckCommand::CuePress => self.cue_press(),
            DeckCommand::CueRelease => self.cue_release(),
            // Non-finite values would poison the playhead or tempo forever: drop them.
            DeckCommand::SetTempoSlider(value) if value.is_finite() => {
                self.tempo_slider = value.clamp(0.0, 1.0);
            }
            DeckCommand::SetTempoRange(range) => self.tempo_range = range,
            DeckCommand::Seek(frames) if frames.is_finite() => {
                self.slip_drop();
                self.position = frames.clamp(0.0, self.frames() as f64);
                self.exit_loop_if_outside(self.position);
                self.pending_sync = None;
            }
            DeckCommand::SetCue(frames) if frames.is_finite() => {
                self.cue = frames.clamp(0.0, self.frames() as f64);
            }
            DeckCommand::SetGrid(None) => {
                self.grid = None;
                self.pending_sync = None;
            }
            // A grid without a usable period would break every beat calculation: ignore it.
            DeckCommand::SetGrid(Some(grid)) if grid.is_valid() => {
                self.grid = Some(grid);
                self.pending_sync = None;
            }
            DeckCommand::HotcuePress(slot) => self.hotcue_press(slot),
            DeckCommand::HotcueRelease(slot) => self.hotcue_release(slot),
            DeckCommand::HotcueDelete(slot) => self.hotcue_set(slot, None),
            DeckCommand::SetHotcue(slot, None) => self.hotcue_set(slot, None),
            DeckCommand::SetHotcue(slot, Some(frames)) if frames.is_finite() => {
                self.hotcue_set(slot, Some(frames));
            }
            DeckCommand::LoopIn => self.loop_in_press(),
            DeckCommand::LoopOut => self.loop_out_press(),
            DeckCommand::AutoLoop => self.auto_loop(),
            DeckCommand::LoopHalve => self.loop_resize(0.5),
            DeckCommand::LoopDouble => self.loop_resize(2.0),
            DeckCommand::ReloopExit => self.reloop_exit(),
            DeckCommand::SetVinyl(on) => self.set_vinyl(on),
            DeckCommand::SetSlip(on) => self.set_slip(on),
            DeckCommand::JogTouch(touched) => self.jog_touch(touched),
            DeckCommand::Jog { turns, surface } if turns.is_finite() => self.jog(turns, surface),
            DeckCommand::JogSearch { turns } if turns.is_finite() => self.jog_search(turns),
            DeckCommand::SetTempoSlider(_)
            | DeckCommand::Seek(_)
            | DeckCommand::SetCue(_)
            | DeckCommand::SetGrid(_)
            | DeckCommand::SetHotcue(..)
            | DeckCommand::Jog { .. }
            | DeckCommand::JogSearch { .. } => {}
        }
        None
    }

    fn reset_transport(&mut self) {
        self.playing = false;
        self.preview = None;
        self.play_latched = false;
        self.position = 0.0;
        self.cue = 0.0;
        self.hotcues = [None; HOTCUES];
        self.grid = None;
        self.loop_in = None;
        self.loop_out = None;
        self.loop_active = false;
        self.jog_touched = false;
        self.scratch = None;
        self.bend = 0.0;
        self.pending_sync = None;
        self.ghost = None;
    }

    fn play(&mut self) {
        if self.track.is_none() {
            return;
        }
        self.playing = true;
        // Play pressed during cue preview: keep playing after cue is released.
        self.play_latched = self.preview.is_some();
    }

    fn pause(&mut self) {
        self.playing = false;
        self.preview = None;
        self.play_latched = false;
        // A bend only makes sense against the motion it was given for.
        self.bend = 0.0;
    }

    fn at_cue(&self) -> bool {
        (self.position - self.cue).abs() < 1.0
    }

    fn cue_press(&mut self) {
        if self.track.is_none() {
            return;
        }
        if self.playing {
            if self.preview.is_none() {
                self.slip_drop();
                self.position = self.cue;
                self.exit_loop_if_outside(self.cue);
                self.playing = false;
            }
        } else if self.at_cue() {
            self.preview = Some(hotcue::PreviewSource::Cue);
            self.play_latched = false;
            self.playing = true;
        } else {
            self.cue = self.position;
        }
    }

    fn cue_release(&mut self) {
        if self.preview == Some(hotcue::PreviewSource::Cue) {
            self.end_preview(self.cue);
        }
    }

    /// Ends a preview: returns to `point` and pauses, unless play was latched during the hold.
    fn end_preview(&mut self, point: f64) {
        self.preview = None;
        if !self.play_latched {
            self.slip_drop();
            self.position = point;
            self.exit_loop_if_outside(point);
            self.playing = false;
        }
        self.play_latched = false;
    }

    /// The grid and playhead of a loaded, playing deck with a grid: what other decks sync to.
    pub(crate) fn phase_source(&self) -> Option<(Grid, f64)> {
        if self.track.is_some() && self.playing {
            self.grid.map(|grid| (grid, self.position))
        } else {
            None
        }
    }

    /// Playing for real: not just previewing from a held CUE or pad (unless play latched
    /// during the hold). A preview snaps back on release, so a phase sync waits for this.
    pub(crate) fn plays_for_real(&self) -> bool {
        self.playing && (self.preview.is_none() || self.play_latched)
    }

    pub(crate) fn pending_sync(&self) -> Option<(DeckId, f64)> {
        self.pending_sync
    }

    pub(crate) fn drop_pending_sync(&mut self) {
        self.pending_sync = None;
    }

    /// Starts a phase sync to a master at `master_beats` (its beat position times `ratio`).
    /// A deck playing for real aligns now; a paused or previewing one remembers `master` and
    /// `ratio` until it plays.
    /// Does nothing without a loaded track and a grid.
    pub(crate) fn phase_sync(&mut self, master: DeckId, ratio: f64, master_beats: f64) {
        if self.track.is_none() || self.grid.is_none() {
            return;
        }
        if self.plays_for_real() {
            self.pending_sync = None;
            self.align_phase(master_beats);
        } else {
            self.pending_sync = Some((master, ratio));
        }
    }

    /// Moves the playhead by at most half a beat so that its beat position has the same
    /// fraction as `target_beats`. A move that lands outside the active loop exits the loop,
    /// like any deliberate jump.
    pub(crate) fn align_phase(&mut self, target_beats: f64) {
        let Some(grid) = self.grid else {
            return;
        };
        let diff = target_beats - grid.beats_at(self.position);
        let beats = diff - diff.round();
        if !beats.is_finite() {
            return;
        }
        self.position =
            (self.position + beats * grid.beat_period_frames).clamp(0.0, self.frames() as f64);
        self.exit_loop_if_outside(self.position);
    }

    fn frames(&self) -> usize {
        self.track.as_ref().map_or(0, |t| t.frames())
    }

    fn tempo_ratio(&self) -> f64 {
        let offset = (f64::from(self.tempo_slider) - 0.5) * 2.0 * self.tempo_range.fraction();
        (1.0 + offset).max(0.0)
    }

    /// Adds this deck's output to the interleaved stereo `out` and advances the playhead.
    pub(crate) fn render_add(&mut self, out: &mut [f32], output_sample_rate: u32) {
        let Some(track) = self.track.as_deref() else {
            return;
        };
        let end = track.frames() as f64;
        let frames = out.as_chunks_mut::<2>().0;
        let rate =
            self.tempo_ratio() * f64::from(track.sample_rate()) / f64::from(output_sample_rate);
        let decay = (-1.0 / (jog::NUDGE_DECAY_SECONDS * f64::from(output_sample_rate))).exp();
        // The ghost moves at normal speed whenever the deck plays for real.
        let ghost_moves = self.plays_for_real();
        let ghost_bounds = self.ghost_bounds();

        if let Some(mut pending) = self.scratch {
            // An empty track has no frame to scratch over.
            if track.frames() == 0 {
                return;
            }
            // Only the platter moves the record, playing or paused; it never wraps a loop
            // and holds at the ends instead of pausing. A record held still is silent.
            let lag = jog::SCRATCH_LAG_SECONDS * f64::from(output_sample_rate);
            for frame in frames {
                // The nudge bend keeps fading under the hand, as the ghost hears it.
                if ghost_moves {
                    if let Some(ghost) = &mut self.ghost {
                        slip::advance(ghost, ghost_bounds, rate * (1.0 + self.bend), end);
                    }
                    self.bend *= decay;
                }
                let speed = (pending / lag).clamp(-jog::SCRATCH_MAX_SPEED, jog::SCRATCH_MAX_SPEED);
                if speed.abs() >= jog::SCRATCH_STILL_SPEED {
                    add_frame(frame, track, self.position);
                }
                let next = (self.position + speed).clamp(0.0, end);
                pending -= next - self.position;
                self.position = next;
            }
            self.scratch = Some(pending);
            return;
        }

        if !self.playing {
            return;
        }
        // Ratio 0 holds the playhead; output silence rather than a repeated sample (DC).
        if self.tempo_ratio() <= 0.0 {
            return;
        }
        let bounds = self.active_loop();

        for frame in frames {
            let step = rate * (1.0 + self.bend);
            self.position = looping::wrap_position(self.position, bounds, step);
            if self.position >= end {
                self.position = end;
                self.pause();
                return;
            }
            add_frame(frame, track, self.position);
            if ghost_moves && let Some(ghost) = &mut self.ghost {
                slip::advance(ghost, ghost_bounds, step, end);
            }
            self.bend *= decay;
            self.position += step;
        }
    }

    pub(crate) fn snapshot(&self) -> DeckSnapshot {
        DeckSnapshot {
            loaded: self.track.is_some(),
            playing: self.playing,
            position_frames: self.position,
            duration_frames: self.frames(),
            track_sample_rate: self.track.as_ref().map_or(0, |t| t.sample_rate()),
            tempo_ratio: self.tempo_ratio(),
            cue_frames: self.cue,
            previewing: self.preview.is_some(),
            grid: self.grid,
            hotcues: self.hotcues,
            loop_in: self.loop_in,
            loop_out: self.loop_out,
            loop_active: self.loop_active,
            loop_beats: self.loop_beats,
            vinyl: self.vinyl,
            jog_touched: self.jog_touched,
            slip: self.slip,
            ghost_frames: self.ghost_frames(),
            load_seq: self.load_seq,
        }
    }
}

/// Adds the track's sound at `position` (in `[0, frames]`) to `frame`, interpolating
/// linearly between the frames around it.
fn add_frame(frame: &mut [f32; 2], track: &TrackBuffer, position: f64) {
    let last = track.frames() - 1;
    let index = (position as usize).min(last);
    let frac = (position - index as f64) as f32;
    let a = track.frame(index);
    // At the last frame there is nothing to interpolate towards; frac is then weighted
    // against the same frame.
    let b = track.frame((index + 1).min(last));
    frame[0] += a[0] + (b[0] - a[0]) * frac;
    frame[1] += a[1] + (b[1] - a[1]) * frac;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Grid, JogSurface, TrackMemory};

    const SR: u32 = 44_100;

    /// A stereo track whose left and right channels both ramp as `i / frames`.
    fn ramp_track(frames: usize) -> Arc<TrackBuffer> {
        let samples = (0..frames)
            .flat_map(|i| {
                let v = i as f32 / frames as f32;
                [v, v]
            })
            .collect();
        Arc::new(TrackBuffer::new(SR, samples).unwrap())
    }

    fn playing_deck(track: &Arc<TrackBuffer>) -> Deck {
        let mut deck = Deck::default();
        assert!(
            deck.apply(DeckCommand::Load {
                track: Arc::clone(track),
                memory: TrackMemory::default(),
            })
            .is_none()
        );
        let _ = deck.apply(DeckCommand::Play);
        deck
    }

    #[test]
    fn renders_track_samples_exactly() {
        let track = ramp_track(4096);
        let mut deck = playing_deck(&track);
        let mut out = vec![0.0; 2000];
        deck.render_add(&mut out, SR);
        for (i, frame) in out.as_chunks::<2>().0.iter().enumerate() {
            assert_eq!(*frame, track.frame(i));
        }
        assert_eq!(deck.position, 1000.0);
    }

    #[test]
    fn render_adds_onto_existing_output() {
        let track = Arc::new(TrackBuffer::new(SR, vec![0.25; 200]).unwrap());
        let mut deck = playing_deck(&track);
        let mut out = vec![0.25; 100];
        deck.render_add(&mut out, SR);
        assert!(out.iter().all(|&s| s == 0.5));
    }

    #[test]
    fn interpolates_between_frames() {
        let track = ramp_track(4096);
        let mut deck = playing_deck(&track);
        let _ = deck.apply(DeckCommand::SetTempoRange(TempoRange::Hundred));
        let _ = deck.apply(DeckCommand::SetTempoSlider(0.25));
        assert_eq!(deck.tempo_ratio(), 0.5);
        let mut out = vec![0.0; 8];
        deck.render_add(&mut out, SR);
        let (f0, f1) = (track.frame(0)[0], track.frame(1)[0]);
        assert_eq!(out[..2], [f0, f0]);
        assert_eq!(out[2..4], [(f0 + f1) / 2.0; 2]);
        assert_eq!(out[4..6], [f1, f1]);
    }

    #[test]
    fn zero_tempo_ratio_renders_silence_and_holds_position() {
        let track = ramp_track(4096);
        let mut deck = playing_deck(&track);
        let _ = deck.apply(DeckCommand::Seek(100.0));
        let _ = deck.apply(DeckCommand::SetTempoRange(TempoRange::Hundred));
        let _ = deck.apply(DeckCommand::SetTempoSlider(0.0));
        let mut out = vec![0.0; 400];
        deck.render_add(&mut out, SR);
        assert!(out.iter().all(|&s| s == 0.0));
        assert_eq!(deck.position, 100.0);
        assert!(deck.playing);
    }

    #[test]
    fn stops_at_end_of_track() {
        let track = Arc::new(TrackBuffer::new(SR, vec![0.5; 200]).unwrap());
        let mut deck = playing_deck(&track);
        let mut out = vec![0.0; 400];
        deck.render_add(&mut out, SR);
        assert!(out[..200].iter().all(|&s| s == 0.5));
        assert!(out[200..].iter().all(|&s| s == 0.0));
        assert!(!deck.playing);
        assert_eq!(deck.position, 100.0);
    }

    /// Loads `track` with an optional grid and plays from `position`.
    fn deck_at(track: &Arc<TrackBuffer>, grid: Option<Grid>, position: f64) -> Deck {
        let mut deck = Deck::default();
        let memory = TrackMemory {
            grid,
            ..TrackMemory::default()
        };
        assert!(
            deck.apply(DeckCommand::Load {
                track: Arc::clone(track),
                memory,
            })
            .is_none()
        );
        let _ = deck.apply(DeckCommand::Seek(position));
        let _ = deck.apply(DeckCommand::Play);
        deck
    }

    #[test]
    fn loop_wraps_sample_exactly() {
        let track = ramp_track(4096);
        let mut deck = deck_at(&track, None, 1000.0);
        let _ = deck.apply(DeckCommand::LoopIn);
        let _ = deck.apply(DeckCommand::Seek(1500.0));
        let _ = deck.apply(DeckCommand::LoopOut);
        let _ = deck.apply(DeckCommand::Seek(1400.0));
        assert!(deck.loop_active);
        let mut out = vec![0.0; 2 * 700];
        deck.render_add(&mut out, SR);
        for (i, frame) in out.as_chunks::<2>().0.iter().enumerate() {
            // The playhead reaches loop-out after 100 frames and continues from loop-in.
            let index = 1400 + i;
            let index = if index >= 1500 {
                1000 + (index - 1000) % 500
            } else {
                index
            };
            assert_eq!(*frame, track.frame(index), "frame {i}");
        }
        assert_eq!(out[200..202], track.frame(1000));
    }

    #[test]
    fn tiny_loop_stays_inside() {
        let track = ramp_track(40_000);
        let grid = Grid::from_bpm(180.0, 0.0, SR);
        let mut deck = deck_at(&track, grid, 20_000.0);
        for _ in 0..7 {
            let _ = deck.apply(DeckCommand::LoopHalve);
        }
        let _ = deck.apply(DeckCommand::AutoLoop);
        let (start, end) = (deck.loop_in.unwrap(), deck.loop_out.unwrap());
        assert_eq!(start, 14_700.0);
        assert!((end - start - 14_700.0 / 32.0).abs() < 1e-9);
        let low = track.frame(start as usize)[0];
        let high = track.frame(end.ceil() as usize)[0];
        let mut out = vec![0.0; 2 * 8192];
        deck.render_add(&mut out, SR);
        assert!(out.iter().all(|s| (low..=high).contains(s)));
        assert!((start..end).contains(&deck.position));
    }

    #[test]
    fn scratch_backwards_renders_reversed_samples() {
        let track = ramp_track(40_000);
        let mut deck = playing_deck(&track);
        let _ = deck.apply(DeckCommand::Seek(20_000.0));
        let _ = deck.apply(DeckCommand::JogTouch(true));
        let _ = deck.apply(DeckCommand::Jog {
            turns: -0.1,
            surface: JogSurface::Top,
        });
        let mut out = vec![0.0; 2 * 1024];
        deck.render_add(&mut out, SR);
        let frames = out.as_chunks::<2>().0;
        assert_eq!(frames[0], track.frame(20_000));
        assert!(frames.windows(2).all(|w| w[1][0] < w[0][0]));
        // The first frames move at the full scratch speed, interpolated at whole frames.
        assert_eq!(frames[1], track.frame(19_992));
        assert!(deck.position < 20_000.0 - 1023.0 * 2.0);
    }

    #[test]
    fn record_held_still_is_silent() {
        let track = ramp_track(40_000);
        let mut deck = playing_deck(&track);
        let _ = deck.apply(DeckCommand::Seek(20_000.0));
        let _ = deck.apply(DeckCommand::JogTouch(true));
        let mut out = vec![0.0; 2 * 1024];
        deck.render_add(&mut out, SR);
        assert!(out.iter().all(|&s| s == 0.0));
        // After a scratch has settled too.
        let _ = deck.apply(DeckCommand::Jog {
            turns: 0.01,
            surface: JogSurface::Top,
        });
        let mut out = vec![0.0; 2 * 8192];
        deck.render_add(&mut out, SR);
        assert!(out[..2].iter().all(|&s| s != 0.0));
        let settled = &out[2 * 6000..];
        assert!(settled.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn empty_track_renders_nothing_even_when_scratched() {
        let track = Arc::new(TrackBuffer::new(SR, Vec::new()).unwrap());
        let mut deck = playing_deck(&track);
        let _ = deck.apply(DeckCommand::JogTouch(true));
        let _ = deck.apply(DeckCommand::Jog {
            turns: 1.0,
            surface: JogSurface::Top,
        });
        let mut out = vec![0.0; 2 * 64];
        deck.render_add(&mut out, SR);
        assert!(out.iter().all(|&s| s == 0.0));
        assert_eq!(deck.position, 0.0);
    }
}
