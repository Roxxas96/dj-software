//! Keeping a deck's cue point and hotcues and the library in step: wait until the engine
//! has applied the `Load` (which carries the remembered memory), then write every change
//! through.

use engine::{DeckSnapshot, Grid, HOTCUES, HotcueSlot, TrackMemory};
use library::Beatgrid;

/// A change to write to the library, seen in a deck's snapshot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MemoryAction {
    /// Store this cue point.
    SaveCue(f64),
    /// Store this hotcue.
    SaveHotcue(HotcueSlot, f64),
    /// Forget this hotcue.
    DeleteHotcue(HotcueSlot),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    /// The `Load` was sent; the snapshot does not show the new track with its memory yet.
    AwaitLoad,
    /// The library holds `cue` and `hotcues`.
    Tracking {
        cue: f64,
        hotcues: [Option<f64>; HOTCUES],
    },
}

/// The memory state of the library track loaded on one deck.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemorySync {
    frames: usize,
    rate: u32,
    /// The memory as the engine applies it on `Load`.
    expected: TrackMemory,
    state: State,
}

impl MemorySync {
    /// A track of `frames` frames at `rate` Hz was just sent to the deck with
    /// `Load { memory, .. }`.
    pub fn new(frames: usize, rate: u32, memory: TrackMemory) -> Self {
        Self {
            frames,
            rate,
            expected: memory.clamped(frames),
            state: State::AwaitLoad,
        }
    }

    /// Looks at the deck's latest snapshot and returns the changes to save.
    ///
    /// The load counts as applied once the snapshot shows the new track (its length and
    /// rate) with exactly the cue and hotcues the engine applies from the memory
    /// ([`TrackMemory::clamped`]). Until then nothing is saved, so the previous track's
    /// values (or a reset) never overwrite the remembered ones, and a remembered value the
    /// engine clamped is not written back. From then on each change is reported once,
    /// against the last values seen.
    pub fn step(&mut self, snap: &DeckSnapshot) -> Vec<MemoryAction> {
        let mut actions = Vec::new();
        match self.state {
            State::AwaitLoad => {
                let applied = snap.loaded
                    && snap.duration_frames == self.frames
                    && snap.track_sample_rate == self.rate
                    && snap.cue_frames == self.expected.cue
                    && snap.hotcues == self.expected.hotcues;
                if applied {
                    self.state = State::Tracking {
                        cue: self.expected.cue,
                        hotcues: self.expected.hotcues,
                    };
                }
            }
            State::Tracking {
                ref mut cue,
                ref mut hotcues,
            } => {
                if !snap.loaded {
                    return actions;
                }
                if snap.cue_frames != *cue && snap.cue_frames.is_finite() {
                    *cue = snap.cue_frames;
                    actions.push(MemoryAction::SaveCue(*cue));
                }
                for slot in HotcueSlot::ALL {
                    let (last, now) = (&mut hotcues[slot.index()], snap.hotcues[slot.index()]);
                    if now == *last || now.is_some_and(|v| !v.is_finite()) {
                        continue;
                    }
                    *last = now;
                    actions.push(match now {
                        Some(frames) => MemoryAction::SaveHotcue(slot, frames),
                        None => MemoryAction::DeleteHotcue(slot),
                    });
                }
            }
        }
        actions
    }

    /// The decoded track's sample rate.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Whether the snapshot has shown this track loaded (so the deck's playhead is this
    /// track's).
    pub fn load_applied(&self) -> bool {
        self.state != State::AwaitLoad
    }
}

/// The engine grid for a library beatgrid, at the decoded track's `rate`: `None` without a
/// BPM. A missing first beat counts as frame 0, as grid edits do.
pub fn engine_grid(grid: Option<&Beatgrid>, rate: u32) -> Option<Grid> {
    let grid = grid?;
    Grid::from_bpm(grid.bpm?, grid.first_beat_frames.unwrap_or(0.0), rate)
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
            ..DeckSnapshot::default()
        }
    }

    fn with_hotcues(snap: DeckSnapshot, hotcues: [Option<f64>; HOTCUES]) -> DeckSnapshot {
        DeckSnapshot { hotcues, ..snap }
    }

    fn memory(cue: f64, hotcues: [Option<f64>; HOTCUES]) -> TrackMemory {
        TrackMemory {
            cue,
            hotcues,
            grid: None,
        }
    }

    fn slot(n: u8) -> HotcueSlot {
        HotcueSlot::new(n).unwrap()
    }

    #[test]
    fn load_applies_when_snapshot_shows_the_memory() {
        let mut hotcues = [None; HOTCUES];
        hotcues[2] = Some(900.0);
        let mut sync = MemorySync::new(1000, 44_100, memory(500.0, hotcues));
        assert!(!sync.load_applied());
        // The previous track (another length, its own cue) is still on the deck.
        assert_eq!(sync.step(&snap(2000, 700.0)), []);
        assert!(!sync.load_applied());
        // The new track, but not its memory yet (say, a reset cue): never saved.
        assert_eq!(sync.step(&snap(1000, 0.0)), []);
        assert_eq!(sync.step(&with_hotcues(snap(1000, 500.0), [None; 8])), []);
        assert!(!sync.load_applied());
        // Another rate: not this track.
        let other_rate = DeckSnapshot {
            track_sample_rate: 48_000,
            ..with_hotcues(snap(1000, 500.0), hotcues)
        };
        assert_eq!(sync.step(&other_rate), []);
        assert!(!sync.load_applied());
        // The load applied with its memory.
        assert_eq!(sync.step(&with_hotcues(snap(1000, 500.0), hotcues)), []);
        assert!(sync.load_applied());
        assert_eq!(sync.step(&with_hotcues(snap(1000, 500.0), hotcues)), []);
    }

    #[test]
    fn memory_sync_saves_set_and_deleted_hotcues() {
        let mut sync = MemorySync::new(1000, 44_100, TrackMemory::default());
        assert_eq!(sync.step(&snap(1000, 0.0)), []);
        let mut hotcues = [None; HOTCUES];
        hotcues[1] = Some(321.0);
        let set = with_hotcues(snap(1000, 0.0), hotcues);
        assert_eq!(sync.step(&set), [MemoryAction::SaveHotcue(slot(2), 321.0)]);
        assert_eq!(sync.step(&set), [], "reported once");
        let deleted = snap(1000, 0.0);
        assert_eq!(sync.step(&deleted), [MemoryAction::DeleteHotcue(slot(2))]);
        assert_eq!(sync.step(&deleted), [], "reported once");
        // A cue and two hotcues change in the same block.
        let mut both = [None; HOTCUES];
        both[0] = Some(10.0);
        both[7] = Some(20.0);
        assert_eq!(
            sync.step(&with_hotcues(snap(1000, 5.0), both)),
            [
                MemoryAction::SaveCue(5.0),
                MemoryAction::SaveHotcue(slot(1), 10.0),
                MemoryAction::SaveHotcue(slot(8), 20.0),
            ]
        );
    }

    #[test]
    fn clamped_memory_is_not_saved_back() {
        let mut hotcues = [None; HOTCUES];
        hotcues[0] = Some(-5.0);
        hotcues[1] = Some(f64::NAN);
        hotcues[2] = Some(9000.0);
        let mut sync = MemorySync::new(1000, 44_100, memory(5000.0, hotcues));
        // What the engine shows: everything clamped like `TrackMemory::clamped`.
        let mut shown = [None; HOTCUES];
        shown[0] = Some(0.0);
        shown[2] = Some(1000.0);
        let applied = with_hotcues(snap(1000, 1000.0), shown);
        assert_eq!(sync.step(&applied), []);
        assert!(sync.load_applied());
        assert_eq!(sync.step(&applied), []);
    }

    #[test]
    fn writes_cue_changes_once_after_the_load() {
        let mut sync = MemorySync::new(1000, 44_100, memory(500.0, [None; 8]));
        assert_eq!(sync.step(&snap(2000, 700.0)), []);
        assert_eq!(sync.step(&snap(1000, 500.0)), []);
        assert_eq!(sync.step(&snap(1000, 500.0)), []);
        // The DJ sets a new cue: written through once.
        assert_eq!(
            sync.step(&snap(1000, 250.0)),
            [MemoryAction::SaveCue(250.0)]
        );
        assert_eq!(sync.step(&snap(1000, 250.0)), []);
        assert_eq!(sync.step(&snap(1000, 0.0)), [MemoryAction::SaveCue(0.0)]);
    }

    #[test]
    fn without_a_saved_cue_tracks_from_zero() {
        let mut sync = MemorySync::new(1000, 44_100, TrackMemory::default());
        assert_eq!(sync.step(&snap(1000, 0.0)), []);
        assert_eq!(sync.step(&snap(1000, 0.0)), []);
        assert_eq!(sync.step(&snap(1000, 42.0)), [MemoryAction::SaveCue(42.0)]);
    }

    #[test]
    fn nothing_is_saved_while_the_deck_is_empty() {
        let mut sync = MemorySync::new(1000, 44_100, memory(500.0, [None; 8]));
        assert_eq!(sync.step(&snap(1000, 500.0)), []);
        let empty = DeckSnapshot {
            loaded: false,
            ..snap(1000, 0.0)
        };
        assert_eq!(sync.step(&empty), []);
    }

    #[test]
    fn engine_grids_use_the_decoded_rate() {
        let grid = Beatgrid {
            bpm: Some(120.0),
            first_beat_frames: Some(100.0),
            grid_locked: false,
        };
        assert_eq!(
            engine_grid(Some(&grid), 48_000),
            Some(Grid {
                beat_period_frames: 24_000.0,
                first_beat_frames: 100.0,
            })
        );
        let no_first = Beatgrid {
            first_beat_frames: None,
            ..grid
        };
        assert_eq!(
            engine_grid(Some(&no_first), 44_100).map(|g| g.first_beat_frames),
            Some(0.0)
        );
        let no_bpm = Beatgrid { bpm: None, ..grid };
        assert_eq!(engine_grid(Some(&no_bpm), 44_100), None);
        assert_eq!(engine_grid(None, 44_100), None);
    }
}
