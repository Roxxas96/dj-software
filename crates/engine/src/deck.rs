use std::sync::Arc;

use crate::{DeckCommand, DeckSnapshot, TempoRange, TrackBuffer};

/// Transport state of one deck. Owned by the renderer.
#[derive(Debug)]
pub(crate) struct Deck {
    track: Option<Arc<TrackBuffer>>,
    playing: bool,
    /// Playhead in track frames.
    position: f64,
    tempo_slider: f32,
    tempo_range: TempoRange,
}

impl Default for Deck {
    fn default() -> Self {
        Self {
            track: None,
            playing: false,
            position: 0.0,
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
                self.playing = false;
                self.position = 0.0;
                return self.track.replace(track);
            }
            DeckCommand::Unload => {
                self.playing = false;
                self.position = 0.0;
                return self.track.take();
            }
            DeckCommand::Play => self.playing = self.track.is_some(),
            DeckCommand::Pause => self.playing = false,
            DeckCommand::TogglePlay => self.playing = !self.playing && self.track.is_some(),
            // CDJ cue behavior is not implemented yet.
            DeckCommand::CuePress | DeckCommand::CueRelease => {}
            DeckCommand::SetTempoSlider(value) => self.tempo_slider = value.clamp(0.0, 1.0),
            DeckCommand::SetTempoRange(range) => self.tempo_range = range,
            DeckCommand::Seek(frames) => self.position = frames.clamp(0.0, self.frames() as f64),
        }
        None
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
                self.playing = false;
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
        }
    }
}
