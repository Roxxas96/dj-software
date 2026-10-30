use std::sync::Arc;

use crate::{DeckCommand, DeckSnapshot, TempoRange, TrackBuffer};

/// Transport state of one deck. Owned by the renderer.
#[derive(Debug)]
pub(crate) struct Deck {
    track: Option<Arc<TrackBuffer>>,
    playing: bool,
    /// Playhead in track frames.
    position: f64,
    /// Cue point in track frames.
    cue: f64,
    /// Cue is held down and playing back from the cue point.
    preview: bool,
    /// Play was pressed during preview, so releasing cue keeps playing.
    play_latched: bool,
    tempo_slider: f32,
    tempo_range: TempoRange,
}

impl Default for Deck {
    fn default() -> Self {
        Self {
            track: None,
            playing: false,
            position: 0.0,
            cue: 0.0,
            preview: false,
            play_latched: false,
            tempo_slider: 0.5,
            tempo_range: TempoRange::default(),
        }
    }
}

impl Deck {
    /// Applies `cmd` and returns the track it replaced, if any. The caller must not drop
    /// that track on the audio thread.
    #[must_use]
    pub(crate) fn apply(&mut self, cmd: DeckCommand) -> Option<Arc<TrackBuffer>> {
        match cmd {
            DeckCommand::Load(track) => {
                self.reset_transport();
                return self.track.replace(track);
            }
            DeckCommand::Unload => {
                self.reset_transport();
                return self.track.take();
            }
            DeckCommand::Play => self.play(),
            DeckCommand::Pause => self.pause(),
            DeckCommand::TogglePlay => {
                if self.playing {
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
                self.position = frames.clamp(0.0, self.frames() as f64);
            }
            DeckCommand::SetTempoSlider(_) | DeckCommand::Seek(_) => {}
        }
        None
    }

    fn reset_transport(&mut self) {
        self.playing = false;
        self.preview = false;
        self.play_latched = false;
        self.position = 0.0;
        self.cue = 0.0;
    }

    fn play(&mut self) {
        if self.track.is_none() {
            return;
        }
        self.playing = true;
        // Play pressed during cue preview: keep playing after cue is released.
        self.play_latched = self.preview;
    }

    fn pause(&mut self) {
        self.playing = false;
        self.preview = false;
        self.play_latched = false;
    }

    fn at_cue(&self) -> bool {
        (self.position - self.cue).abs() < 1.0
    }

    fn cue_press(&mut self) {
        if self.track.is_none() {
            return;
        }
        if self.playing {
            if !self.preview {
                self.position = self.cue;
                self.playing = false;
            }
        } else if self.at_cue() {
            self.preview = true;
            self.play_latched = false;
            self.playing = true;
        } else {
            self.cue = self.position;
        }
    }

    fn cue_release(&mut self) {
        if !self.preview {
            return;
        }
        self.preview = false;
        if !self.play_latched {
            self.position = self.cue;
            self.playing = false;
        }
        self.play_latched = false;
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
        if !self.playing {
            return;
        }
        let frames = track.frames();
        let end = frames as f64;
        let rate =
            self.tempo_ratio() * f64::from(track.sample_rate()) / f64::from(output_sample_rate);

        for frame in out.as_chunks_mut::<2>().0 {
            if self.position >= end {
                self.position = end;
                self.pause();
                return;
            }
            let index = self.position as usize;
            let frac = (self.position - index as f64) as f32;
            let a = track.frame(index);
            // At the last frame there is nothing to interpolate towards; frac is then
            // weighted against the same frame.
            let b = track.frame((index + 1).min(frames - 1));
            frame[0] += a[0] + (b[0] - a[0]) * frac;
            frame[1] += a[1] + (b[1] - a[1]) * frac;
            self.position += rate;
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
        }
    }
}
