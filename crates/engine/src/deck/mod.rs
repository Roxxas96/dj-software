use std::sync::Arc;

mod backspin;
mod beat_jump;
mod hotcue;
mod jog;
mod keylock;
mod looping;
mod roll;
mod slip;

use crate::dsp::smooth::{Ramp, Segment};
use crate::stems::Garbage;
use crate::{
    DeckCommand, DeckId, DeckSnapshot, EqMode, Grid, HOTCUES, StemSet, TempoRange, TrackBuffer,
    knob_law,
};

/// What a command hands back for the renderer to discard: at most a track and a stem set.
pub(crate) type Discarded = [Option<Garbage>; 2];

const NOTHING: Discarded = [None, None];

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
    /// Keylock is on. Kept across loads.
    keylock: bool,
    /// The time-stretcher keylock plays through; `None` for a deck built without one,
    /// which then always plays resampled.
    stretcher: Option<keylock::Stretcher>,
    /// A held loop roll or slice and its roll loop, which the playhead wraps in instead of
    /// the normal loop.
    roll: Option<roll::Roll>,
    /// How many `Load`s were applied.
    load_seq: u64,
    /// A running backspin.
    backspin: Option<backspin::Backspin>,
    /// Output frames this deck has rendered: the deck's clock.
    clock: u64,
    /// The output sample rate, to turn times into clock frames.
    output_rate: u32,
    /// The clock when the jog was last touched.
    touched_at: Option<u64>,
    /// The loaded track's stems.
    stems: Option<Arc<StemSet>>,
    /// The stems were dropped: they stay until the gains have ramped back to the mix.
    stems_leaving: bool,
    /// The stem pads, in [`Stem::ALL`](crate::Stem::ALL) order. All on after a load.
    stem_on: [bool; 4],
    /// Per stem gain, from the knob law and the pads; all 1.0 plays the mix itself.
    stem_gain: [Ramp; 4],
    /// The EQ knobs control the stems.
    stems_active: bool,
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
            keylock: false,
            stretcher: None,
            roll: None,
            load_seq: 0,
            backspin: None,
            clock: 0,
            output_rate: 44_100,
            touched_at: None,
            stems: None,
            stems_leaving: false,
            stem_on: [true; 4],
            stem_gain: [Ramp::new(1.0); 4],
            stems_active: false,
        }
    }
}

impl Deck {
    /// A deck with a keylock stretcher for `output_rate` and blocks of up to
    /// `max_block_frames`. Allocates: call it off the audio thread.
    pub(crate) fn new(output_rate: u32, max_block_frames: usize) -> Self {
        Self {
            stretcher: Some(keylock::Stretcher::new(output_rate, max_block_frames)),
            output_rate,
            ..Self::default()
        }
    }

    /// Applies `cmd` and returns the track and stems it replaced or ignored, if any. The
    /// caller must not drop them on the audio thread.
    #[must_use]
    pub(crate) fn apply(&mut self, cmd: DeckCommand) -> Discarded {
        match cmd {
            DeckCommand::Load { track, memory } => {
                self.reset_transport();
                let memory = memory.clamped(track.frames());
                self.cue = memory.cue;
                self.hotcues = memory.hotcues;
                self.grid = memory.grid;
                self.load_seq = self.load_seq.wrapping_add(1);
                let stems = self.reset_stems();
                return [self.track.replace(track).map(Garbage::Track), stems];
            }
            DeckCommand::Unload => {
                self.reset_transport();
                let stems = self.reset_stems();
                return [self.track.take().map(Garbage::Track), stems];
            }
            DeckCommand::SetStems(stems) => return [self.set_stems(stems), None],
            DeckCommand::StemOn(stem, on) => self.stem_on[stem.index()] = on,
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
            // A held roll or slice owns the playhead: the normal loop's buttons wait.
            DeckCommand::LoopIn
            | DeckCommand::LoopOut
            | DeckCommand::AutoLoop
            | DeckCommand::AutoLoopBeats(_)
            | DeckCommand::LoopExit
            | DeckCommand::LoopHalve
            | DeckCommand::LoopDouble
            | DeckCommand::ReloopExit
                if self.roll.is_some() => {}
            DeckCommand::LoopIn => self.loop_in_press(),
            DeckCommand::LoopOut => self.loop_out_press(),
            DeckCommand::AutoLoop => self.auto_loop(),
            DeckCommand::AutoLoopBeats(beats) if beats.is_finite() && beats > 0.0 => {
                self.auto_loop_beats(beats);
            }
            DeckCommand::AutoLoopBeats(_) => {}
            DeckCommand::LoopExit => self.exit_loop(),
            DeckCommand::LoopHalve => self.loop_resize(0.5),
            DeckCommand::LoopDouble => self.loop_resize(2.0),
            DeckCommand::ReloopExit => self.reloop_exit(),
            DeckCommand::BeatJump { beats } => self.beat_jump(beats),
            DeckCommand::RollPress { beats } => self.roll_press(beats),
            DeckCommand::RollRelease { beats } => self.roll_release(beats),
            DeckCommand::SlicePress(slice) => self.slice_press(slice),
            DeckCommand::SliceRelease(slice) => self.slice_release(slice),
            DeckCommand::SetVinyl(on) => self.set_vinyl(on),
            DeckCommand::SetSlip(on) => self.set_slip(on),
            DeckCommand::SetKeylock(on) => self.set_keylock(on),
            DeckCommand::JogTouch(touched) => self.jog_touch(touched),
            DeckCommand::Jog { turns, surface } if turns.is_finite() => self.jog(turns, surface),
            DeckCommand::JogSearch { turns } if turns.is_finite() => self.jog_search(turns),
            DeckCommand::Backspin { seconds } if seconds.is_finite() => {
                self.backspin_start(seconds)
            }
            DeckCommand::SetTempoSlider(_)
            | DeckCommand::Seek(_)
            | DeckCommand::SetCue(_)
            | DeckCommand::SetGrid(_)
            | DeckCommand::SetHotcue(..)
            | DeckCommand::Jog { .. }
            | DeckCommand::JogSearch { .. }
            | DeckCommand::Backspin { .. } => {}
        }
        NOTHING
    }

    /// Drops the stems at once, with every pad on and the gains at the mix: for a load,
    /// where the sound jumps anyway.
    fn reset_stems(&mut self) -> Option<Garbage> {
        self.stems_leaving = false;
        self.stem_on = [true; 4];
        self.stem_gain = [Ramp::new(1.0); 4];
        self.stems_active = false;
        self.stems.take().map(Garbage::Stems)
    }

    /// Takes `stems` for the loaded track, or drops the current ones (`None`), and returns
    /// the set to discard. Stems that are dropped keep playing until the gains have ramped
    /// back to the mix ([`Deck::update_stems`] hands them back then).
    fn set_stems(&mut self, stems: Option<Arc<StemSet>>) -> Option<Garbage> {
        match stems {
            None => {
                self.stems_leaving = self.stems.is_some();
                None
            }
            Some(set) if self.track.is_none() || set.frames() != self.frames() => {
                Some(Garbage::Stems(set))
            }
            Some(set) => {
                self.stems_leaving = false;
                self.stems.replace(set).map(Garbage::Stems)
            }
        }
    }

    /// Before a block: sets the stem gains the block ramps to, from the EQ knob positions
    /// `[low, mid, high]` under `mode`, and returns whether the knobs control the stems,
    /// with a dropped set to discard once its gains are back at the mix. In
    /// [`EqMode::Eq`] the gains are the pads alone.
    pub(crate) fn update_stems(
        &mut self,
        mode: EqMode,
        [low, mid, high]: [f32; 3],
    ) -> (bool, Option<Arc<StemSet>>) {
        let released = if self.stems_leaving && self.stem_gain.iter().all(|g| g.current == 1.0) {
            self.stems_leaving = false;
            self.stems.take()
        } else {
            None
        };
        let present = self.stems.is_some() && !self.stems_leaving;
        self.stems_active = present && mode == EqMode::Stems;
        let law = if self.stems_active {
            knob_law(low, mid, high)
        } else {
            [1.0; 4]
        };
        for ((gain, law), on) in self.stem_gain.iter_mut().zip(law).zip(self.stem_on) {
            gain.target = if present && !on { 0.0 } else { law };
        }
        (self.stems_active, released)
    }

    /// Starts the stem gains' ramp over `reads` source reads. `None` plays the mix itself:
    /// no stems, or every gain at rest at 1.
    fn stem_segments(&mut self, reads: usize) -> Option<[Segment; 4]> {
        let segments = self.stem_gain.each_mut().map(|gain| gain.block(reads));
        let mix = self.stems.is_none() || segments.iter().all(|s| s.rests_at(1.0));
        (!mix).then_some(segments)
    }

    /// The stem gains held where they are, for reads outside the block's ramp.
    fn stem_held(&self) -> Option<[Segment; 4]> {
        let mix = self.stems.is_none() || self.stem_gain.iter().all(|g| g.current == 1.0);
        (!mix).then(|| self.stem_gain.map(|mut gain| gain.block(0)))
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
        self.touched_at = None;
        self.scratch = None;
        self.bend = 0.0;
        self.pending_sync = None;
        self.ghost = None;
        self.roll = None;
        self.backspin = None;
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
        self.backspin = None;
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
    /// fraction as `target_beats`. A running slip ghost moves by the same amount (clamped to
    /// the track, like a beat jump), so the return stays in phase. A move that lands outside
    /// the active loop exits the loop, like any deliberate jump; during a roll the check is
    /// on the ghost, which the loop holds then.
    pub(crate) fn align_phase(&mut self, target_beats: f64) {
        let Some(grid) = self.grid else {
            return;
        };
        let diff = target_beats - grid.beats_at(self.position);
        let delta = (diff - diff.round()) * grid.beat_period_frames;
        if !delta.is_finite() {
            return;
        }
        let end = self.frames() as f64;
        // The ghost's place in its loop is read before the loop can exit.
        let ghost = self.ghost_frames().map(|g| (g + delta).clamp(0.0, end));
        self.position = (self.position + delta).clamp(0.0, end);
        if let (Some(ghost), Some(slot)) = (ghost, self.ghost.as_mut()) {
            slot.position = ghost;
        }
        // During a roll the normal loop holds the ghost, not the playhead.
        let held = match ghost {
            Some(ghost) if self.roll.is_some() => ghost,
            _ => self.position,
        };
        self.exit_loop_if_outside(held);
    }

    fn frames(&self) -> usize {
        self.track.as_ref().map_or(0, |t| t.frames())
    }

    /// One beat of the grid in output frames at `output_rate`, at the current tempo; 0
    /// without a loaded track and grid, or when stopped at tempo 0.
    pub(crate) fn beat_frames(&self, output_rate: u32) -> f64 {
        let (Some(grid), Some(track)) = (self.grid, &self.track) else {
            return 0.0;
        };
        let ratio = self.tempo_ratio();
        if ratio <= 0.0 {
            return 0.0;
        }
        grid.beat_period_frames * f64::from(output_rate) / f64::from(track.sample_rate()) / ratio
    }

    fn tempo_ratio(&self) -> f64 {
        let offset = (f64::from(self.tempo_slider) - 0.5) * 2.0 * self.tempo_range.fraction();
        (1.0 + offset).max(0.0)
    }

    /// The bounds the playhead wraps in: a held roll's or slice's roll loop, else the
    /// active normal loop.
    fn play_bounds(&self) -> Option<(f64, f64)> {
        if self.roll.is_some() {
            self.roll_bounds()
        } else {
            self.active_loop()
        }
    }

    /// Adds this deck's output to the interleaved stereo `out` and advances the playhead.
    pub(crate) fn render_add(&mut self, out: &mut [f32], output_sample_rate: u32) {
        self.clock = self.clock.wrapping_add((out.len() / 2) as u64);
        // Keylock stretches a deck playing on its own; scratching, a paused deck and the
        // original tempo play resampled.
        if self.scratch.is_none()
            && self.backspin.is_none()
            && self.playing
            && self.tempo_ratio() > 0.0
            && self.stretches()
            // A clone of the deck's own track: it is never the last one, so never freed here.
            && let Some(track) = self.track.clone()
        {
            // The same for the stems: never the last reference either.
            let stems = self.stems.clone();
            let step = f64::from(track.sample_rate()) / f64::from(output_sample_rate);
            let bounds = self.play_bounds();
            self.render_stretched(
                out,
                &track,
                stems.as_deref(),
                output_sample_rate,
                step,
                bounds,
            );
            return;
        }
        self.keylock_bypassed();
        // One read per output frame: the stem gains ramp across the block.
        let gains = self.stem_segments(out.len() / 2);
        let Some(track) = self.track.as_deref() else {
            return;
        };
        let end = track.frames() as f64;
        let frames = out.as_chunks_mut::<2>().0;
        let rate =
            self.tempo_ratio() * f64::from(track.sample_rate()) / f64::from(output_sample_rate);
        // A backspin plays first; once it ends, the rest of the block plays forward.
        let (frames, done) = if self.backspin.is_some() {
            let done = self.render_backspin(frames, rate, output_sample_rate, gains);
            (&mut frames[done..], done)
        } else {
            (frames, 0)
        };
        let Some(track) = self.track.as_deref() else {
            return;
        };
        let source = Source::new(track, self.stems.as_deref(), gains);
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
            for (i, frame) in frames.iter_mut().enumerate() {
                // The nudge bend keeps fading under the hand, as the ghost hears it.
                if ghost_moves {
                    if let Some(ghost) = &mut self.ghost {
                        slip::advance(ghost, ghost_bounds, rate * (1.0 + self.bend), end);
                    }
                    self.bend *= decay;
                }
                let speed = (pending / lag).clamp(-jog::SCRATCH_MAX_SPEED, jog::SCRATCH_MAX_SPEED);
                if speed.abs() >= jog::SCRATCH_STILL_SPEED {
                    add_frame(frame, source, self.position, done + i);
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
        let bounds = self.play_bounds();

        for (i, frame) in frames.iter_mut().enumerate() {
            let step = rate * (1.0 + self.bend);
            let Some(sound) = read_frame(source, &mut self.position, bounds, step, done + i) else {
                self.pause();
                return;
            };
            frame[0] += sound[0];
            frame[1] += sound[1];
            if ghost_moves && let Some(ghost) = &mut self.ghost {
                slip::advance(ghost, ghost_bounds, step, end);
            }
            self.bend *= decay;
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
            keylock: self.keylock,
            ghost_frames: self.ghost_frames(),
            roll_active: self.roll.is_some(),
            backspinning: self.backspin.is_some(),
            slicer_block_beat: self.slicer_block_beat(),
            load_seq: self.load_seq,
            stems: self.stems.is_some() && !self.stems_leaving,
            stems_active: self.stems_active,
        }
    }
}

/// What a deck plays: its track, or the gain-weighted sum of its stems.
#[derive(Clone, Copy)]
struct Source<'a> {
    track: &'a TrackBuffer,
    /// The stems and their gains across the block's reads; `None` plays the track.
    stems: Option<(&'a StemSet, [Segment; 4])>,
}

impl<'a> Source<'a> {
    fn new(
        track: &'a TrackBuffer,
        stems: Option<&'a StemSet>,
        gains: Option<[Segment; 4]>,
    ) -> Self {
        Self {
            track,
            stems: stems.zip(gains),
        }
    }

    fn mix(track: &'a TrackBuffer) -> Self {
        Self { track, stems: None }
    }

    fn frames(&self) -> usize {
        self.track.frames()
    }

    /// Frame `index` as heard at read `read` of the block.
    #[inline]
    fn frame(&self, index: usize, read: usize) -> [f32; 2] {
        match self.stems {
            None => self.track.frame(index),
            Some((stems, gains)) => stems.mixed(self.track, index, gains.map(|g| g.at(read))),
        }
    }
}

/// The playing source reader, shared by the normal path and keylock: wraps `position` in
/// `bounds`, returns the source's sound there (at read `read` of the block, for the stem
/// gains) and moves it on by `step`. At the track's end it holds `position` there and
/// returns `None`.
fn read_frame(
    source: Source<'_>,
    position: &mut f64,
    bounds: Option<(f64, f64)>,
    step: f64,
    read: usize,
) -> Option<[f32; 2]> {
    let end = source.frames() as f64;
    *position = looping::wrap_position(*position, bounds, step);
    if *position >= end {
        *position = end;
        return None;
    }
    let mut sound = [0.0; 2];
    add_frame(&mut sound, source, *position, read);
    *position += step;
    Some(sound)
}

/// Adds the source's sound at `position` (in `[0, frames]`), at read `read` of the block,
/// to `frame`, interpolating linearly between the frames around it.
fn add_frame(frame: &mut [f32; 2], source: Source<'_>, position: f64, read: usize) {
    let last = source.frames() - 1;
    let index = (position as usize).min(last);
    let frac = (position - index as f64) as f32;
    let a = source.frame(index, read);
    // At the last frame there is nothing to interpolate towards; frac is then weighted
    // against the same frame.
    let b = source.frame((index + 1).min(last), read);
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
            .iter()
            .all(Option::is_none)
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
            .iter()
            .all(Option::is_none)
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
    fn auto_loop_beats_loops_exactly_that_size() {
        let track = ramp_track(400_000);
        let grid = Grid::from_bpm(180.0, 0.0, SR);
        let mut deck = deck_at(&track, grid, 20_000.0);
        let _ = deck.apply(DeckCommand::AutoLoopBeats(1.0));
        let (start, end) = (deck.loop_in.unwrap(), deck.loop_out.unwrap());
        assert!(deck.loop_active);
        assert_eq!(start, 14_700.0);
        assert!((end - start - 14_700.0).abs() < 1e-9);
        assert_eq!(deck.loop_beats, 1.0);
        // An active loop is left alone, and bad sizes are ignored.
        let _ = deck.apply(DeckCommand::AutoLoopBeats(4.0));
        assert_eq!(deck.loop_beats, 1.0);
        let _ = deck.apply(DeckCommand::LoopExit);
        for bad in [f64::NAN, f64::INFINITY, 0.0, -2.0] {
            let _ = deck.apply(DeckCommand::AutoLoopBeats(bad));
            assert!(!deck.loop_active, "{bad}");
        }
        let _ = deck.apply(DeckCommand::AutoLoopBeats(4.0));
        assert_eq!(deck.loop_beats, 4.0);
        assert!(deck.loop_active);
    }

    #[test]
    fn auto_loop_beats_needs_a_grid_and_waits_for_a_roll() {
        let track = ramp_track(400_000);
        let mut deck = deck_at(&track, None, 20_000.0);
        let _ = deck.apply(DeckCommand::AutoLoopBeats(1.0));
        assert!(!deck.loop_active);
        assert_eq!(deck.loop_beats, looping::DEFAULT_LOOP_BEATS);
        let grid = Grid::from_bpm(180.0, 0.0, SR);
        let mut deck = deck_at(&track, grid, 20_000.0);
        let _ = deck.apply(DeckCommand::RollPress { beats: 1.0 });
        let _ = deck.apply(DeckCommand::AutoLoopBeats(1.0));
        let _ = deck.apply(DeckCommand::LoopExit);
        assert!(!deck.loop_active);
        assert_eq!(deck.loop_beats, looping::DEFAULT_LOOP_BEATS);
    }

    #[test]
    fn loop_exit_only_exits() {
        let track = ramp_track(400_000);
        let grid = Grid::from_bpm(180.0, 0.0, SR);
        let mut deck = deck_at(&track, grid, 20_000.0);
        let _ = deck.apply(DeckCommand::LoopExit);
        assert!(!deck.loop_active);
        let _ = deck.apply(DeckCommand::AutoLoop);
        assert!(deck.loop_active);
        let _ = deck.apply(DeckCommand::LoopExit);
        assert!(!deck.loop_active);
        // Unlike ReloopExit, a second exit never reloops.
        let _ = deck.apply(DeckCommand::LoopExit);
        assert!(!deck.loop_active);
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
