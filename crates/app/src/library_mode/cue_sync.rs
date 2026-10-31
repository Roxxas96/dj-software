//! Keeping a deck's cue point and the library in step: restore the remembered cue once the
//! engine has applied a `Load`, then write every change through.

use engine::DeckSnapshot;

/// What to do after looking at a deck's snapshot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CueAction {
    Nothing,
    /// Send `DeckCommand::SetCue` with this value.
    Restore(f64),
    /// Store this cue point in the library.
    Save(f64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    /// The `Load` was sent; the snapshot does not show the new track yet.
    AwaitLoad,
    /// `SetCue(expected)` was sent; the snapshot still shows the reset cue (0).
    AwaitCue { expected: f64 },
    /// The cue in the library is `last`.
    Tracking { last: f64 },
}

/// The cue state of the library track loaded on one deck.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CueSync {
    frames: usize,
    rate: u32,
    saved: Option<f64>,
    state: State,
}

impl CueSync {
    /// A track of `frames` frames at `rate` Hz was just sent to the deck with `Load`;
    /// `saved` is its remembered cue point.
    pub fn new(frames: usize, rate: u32, saved: Option<f64>) -> Self {
        Self {
            frames,
            rate,
            saved,
            state: State::AwaitLoad,
        }
    }

    /// Looks at the deck's latest snapshot.
    ///
    /// The engine resets the cue to 0 when it applies a `Load`, so the remembered cue is
    /// restored only once the snapshot shows the new track (its length and rate, cue 0).
    /// Until the restored cue shows, nothing is saved, so the reset 0 never overwrites
    /// the remembered cue. (Engine commands are applied in order, so a `SetCue` sent
    /// after the `Load` can never be undone by it, even if the snapshot check matched
    /// the previous track by chance.)
    pub fn step(&mut self, snap: &DeckSnapshot) -> CueAction {
        match self.state {
            State::AwaitLoad => {
                let loaded = snap.loaded
                    && snap.duration_frames == self.frames
                    && snap.track_sample_rate == self.rate
                    && snap.cue_frames == 0.0;
                if !loaded {
                    return CueAction::Nothing;
                }
                match self.saved.filter(|c| c.is_finite() && *c > 0.0) {
                    Some(cue) => {
                        self.state = State::AwaitCue {
                            expected: cue.min(self.frames as f64),
                        };
                        CueAction::Restore(cue)
                    }
                    None => {
                        self.state = State::Tracking { last: 0.0 };
                        CueAction::Nothing
                    }
                }
            }
            State::AwaitCue { expected } => {
                if snap.cue_frames == 0.0 {
                    return CueAction::Nothing;
                }
                self.state = State::Tracking {
                    last: snap.cue_frames,
                };
                if snap.cue_frames == expected {
                    CueAction::Nothing
                } else {
                    CueAction::Save(snap.cue_frames)
                }
            }
            State::Tracking { last } => {
                if !snap.loaded || snap.cue_frames == last || !snap.cue_frames.is_finite() {
                    return CueAction::Nothing;
                }
                self.state = State::Tracking {
                    last: snap.cue_frames,
                };
                CueAction::Save(snap.cue_frames)
            }
        }
    }

    /// The `SetCue` of [`CueAction::Restore`] could not be sent: try again next time.
    pub fn restore_failed(&mut self) {
        self.state = State::AwaitLoad;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(frames: usize, cue: f64) -> DeckSnapshot {
        DeckSnapshot {
            loaded: true,
            playing: false,
            position_frames: 0.0,
            duration_frames: frames,
            track_sample_rate: 44_100,
            tempo_ratio: 1.0,
            cue_frames: cue,
            previewing: false,
        }
    }

    #[test]
    fn restores_after_the_load_shows_then_writes_changes() {
        let mut sync = CueSync::new(1000, 44_100, Some(500.0));
        // The previous track (another length, its own cue) is still on the deck.
        assert_eq!(sync.step(&snap(2000, 700.0)), CueAction::Nothing);
        // The load applied: the engine reset the cue.
        assert_eq!(sync.step(&snap(1000, 0.0)), CueAction::Restore(500.0));
        // The SetCue is not applied yet: the reset 0 is never saved.
        assert_eq!(sync.step(&snap(1000, 0.0)), CueAction::Nothing);
        assert_eq!(sync.step(&snap(1000, 500.0)), CueAction::Nothing);
        assert_eq!(sync.step(&snap(1000, 500.0)), CueAction::Nothing);
        // The DJ sets a new cue: written through once.
        assert_eq!(sync.step(&snap(1000, 250.0)), CueAction::Save(250.0));
        assert_eq!(sync.step(&snap(1000, 250.0)), CueAction::Nothing);
        assert_eq!(sync.step(&snap(1000, 0.0)), CueAction::Save(0.0));
    }

    #[test]
    fn without_a_saved_cue_tracks_from_zero() {
        let mut sync = CueSync::new(1000, 44_100, None);
        assert_eq!(sync.step(&snap(1000, 0.0)), CueAction::Nothing);
        assert_eq!(sync.step(&snap(1000, 0.0)), CueAction::Nothing);
        assert_eq!(sync.step(&snap(1000, 42.0)), CueAction::Save(42.0));
    }

    #[test]
    fn a_saved_cue_past_the_end_is_clamped_like_the_engine() {
        let mut sync = CueSync::new(1000, 44_100, Some(5000.0));
        assert_eq!(sync.step(&snap(1000, 0.0)), CueAction::Restore(5000.0));
        // The engine clamps to the length: not a change to save.
        assert_eq!(sync.step(&snap(1000, 1000.0)), CueAction::Nothing);
    }

    #[test]
    fn a_failed_restore_is_retried() {
        let mut sync = CueSync::new(1000, 44_100, Some(500.0));
        assert_eq!(sync.step(&snap(1000, 0.0)), CueAction::Restore(500.0));
        sync.restore_failed();
        assert_eq!(sync.step(&snap(1000, 0.0)), CueAction::Restore(500.0));
    }
}
