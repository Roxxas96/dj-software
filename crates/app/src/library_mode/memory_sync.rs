//! Keeping a deck's cue point and hotcues and the library in step: wait until the engine
//! has applied the `Load` (which carries the remembered memory, and advances the deck's
//! `load_seq`), then write every change through.

use engine::{DeckSnapshot, Grid, HOTCUES, Hotcue, HotcueSlot, TrackMemory};
use library::Beatgrid;

/// A change to write to the library, seen in a deck's snapshot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MemoryAction {
    /// Store this cue point.
    SaveCue(f64),
    /// Store this hotcue.
    SaveHotcue(HotcueSlot, Hotcue),
    /// Forget this hotcue.
    DeleteHotcue(HotcueSlot),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    /// The `Load` was sent; the snapshot does not show it applied yet.
    AwaitLoad,
    /// The library holds `cue` and `hotcues`.
    Tracking {
        cue: f64,
        hotcues: [Option<Hotcue>; HOTCUES],
    },
}

/// The memory state of the library track loaded on one deck.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemorySync {
    rate: u32,
    /// The deck's `load_seq` before this track's `Load`: a greater one shows it applied.
    after: u64,
    /// The memory as the engine applies it on `Load`.
    expected: TrackMemory,
    state: State,
    /// Hotcue changes are saved (off when the hotcues could not be read at load, Ruling
    /// R19: the empty-looking pads must not overwrite the stored ones).
    save_hotcues: bool,
    /// Per slot, a hotcue sent to the deck because another deck holding the same track
    /// changed it (Ruling R18), not shown yet: the value the deck showed before it. Until
    /// the snapshot shows the sent value, that old value is not a change to save.
    pushed: [Option<Option<Hotcue>>; HOTCUES],
}

impl MemorySync {
    /// A track of `frames` frames at `rate` Hz was just sent to the deck with
    /// `Load { memory, .. }`; the deck's snapshot shows the load applied once its
    /// `load_seq` is greater than `after`.
    pub fn new(frames: usize, rate: u32, memory: TrackMemory, after: u64) -> Self {
        Self {
            rate,
            after,
            expected: memory.clamped(frames),
            state: State::AwaitLoad,
            save_hotcues: true,
            pushed: [None; HOTCUES],
        }
    }

    /// The same, never saving hotcue changes (the cue point still is).
    pub fn without_hotcue_saves(self) -> Self {
        Self {
            save_hotcues: false,
            ..self
        }
    }

    /// Whether the deck should be sent hotcue `slot` at `value`, which another deck holding
    /// the same track just saved: the load has applied and the deck holds another value.
    pub fn wants_hotcue(&self, slot: HotcueSlot, value: Option<Hotcue>) -> bool {
        match &self.state {
            State::AwaitLoad => false,
            State::Tracking { hotcues, .. } => hotcues[slot.index()] != value,
        }
    }

    /// Hotcue `slot` was sent to the deck at `value` (see [`Self::wants_hotcue`]): the
    /// library already holds it, so it is not saved again when the snapshot shows it.
    pub fn pushed_hotcue(&mut self, slot: HotcueSlot, value: Option<Hotcue>) {
        let State::Tracking { hotcues, .. } = &mut self.state else {
            return;
        };
        let i = slot.index();
        // A push on top of one not shown yet: the deck may still show the first old value.
        self.pushed[i] = Some(self.pushed[i].unwrap_or(hotcues[i]));
        hotcues[i] = value;
    }

    /// Whether `snap` shows this track's `Load` applied.
    pub fn shows_load(&self, snap: &DeckSnapshot) -> bool {
        snap.load_seq > self.after
    }

    /// Looks at the deck's latest snapshot and returns the changes to save.
    ///
    /// The load counts as applied once the snapshot's `load_seq` passes the one before the
    /// `Load` (Ruling R12). Until then nothing is saved, so the previous track's values (or
    /// a reset) never overwrite the remembered ones. From then on the snapshot is compared
    /// with the memory as the engine applied it ([`TrackMemory::clamped`]), so a value the
    /// engine clamped is not written back but a change made in the very block of the load
    /// is; each change is reported once, against the last values seen.
    pub fn step(&mut self, snap: &DeckSnapshot) -> Vec<MemoryAction> {
        let mut actions = Vec::new();
        if self.state == State::AwaitLoad {
            if !self.shows_load(snap) {
                return actions;
            }
            self.state = State::Tracking {
                cue: self.expected.cue,
                hotcues: self.expected.hotcues,
            };
        }
        let State::Tracking { cue, hotcues } = &mut self.state else {
            return actions;
        };
        if !snap.loaded {
            return actions;
        }
        if snap.cue_frames != *cue && snap.cue_frames.is_finite() {
            *cue = snap.cue_frames;
            actions.push(MemoryAction::SaveCue(*cue));
        }
        if !self.save_hotcues {
            return actions;
        }
        for slot in HotcueSlot::ALL {
            let (last, now) = (&mut hotcues[slot.index()], snap.hotcues[slot.index()]);
            if let Some(before) = self.pushed[slot.index()] {
                if now == *last {
                    // The pushed value shows.
                    self.pushed[slot.index()] = None;
                    continue;
                }
                if now == before {
                    continue; // Not applied yet.
                }
                // Changed on this deck since: a change like any other.
                self.pushed[slot.index()] = None;
            }
            if now == *last || now.is_some_and(|h| !h.frames.is_finite()) {
                continue;
            }
            *last = now;
            actions.push(match now {
                Some(hotcue) => MemoryAction::SaveHotcue(slot, hotcue),
                None => MemoryAction::DeleteHotcue(slot),
            });
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

    /// A snapshot after `load_seq` loads, of a 1000-frame track with cue `cue`.
    fn snap(load_seq: u64, cue: f64) -> DeckSnapshot {
        DeckSnapshot {
            loaded: true,
            duration_frames: 1000,
            track_sample_rate: 44_100,
            cue_frames: cue,
            load_seq,
            ..DeckSnapshot::default()
        }
    }

    fn with_hotcues(snap: DeckSnapshot, hotcues: [Option<Hotcue>; HOTCUES]) -> DeckSnapshot {
        DeckSnapshot { hotcues, ..snap }
    }

    fn memory(cue: f64, hotcues: [Option<Hotcue>; HOTCUES]) -> TrackMemory {
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
        hotcues[2] = Some(Hotcue::point(900.0));
        // The deck had applied 3 loads when this one was sent.
        let mut sync = MemorySync::new(1000, 44_100, memory(500.0, hotcues), 3);
        assert!(!sync.load_applied());
        // The previous track, with its own cue and hotcues, or reset: never saved.
        let mut previous = [None; HOTCUES];
        previous[0] = Some(Hotcue::point(1.0));
        assert_eq!(sync.step(&with_hotcues(snap(3, 700.0), previous)), []);
        assert_eq!(sync.step(&snap(3, 0.0)), []);
        assert!(!sync.load_applied());
        // The load applied with its memory: nothing to save.
        assert_eq!(sync.step(&with_hotcues(snap(4, 500.0), hotcues)), []);
        assert!(sync.load_applied());
        assert_eq!(sync.step(&with_hotcues(snap(4, 500.0), hotcues)), []);
    }

    #[test]
    fn a_change_in_the_block_of_the_load_is_saved() {
        let mut hotcues = [None; HOTCUES];
        hotcues[2] = Some(Hotcue::point(900.0));
        let mut sync = MemorySync::new(1000, 44_100, memory(500.0, hotcues), 0);
        // The first snapshot showing the load already has hotcue 2 set and 3 deleted.
        let mut shown = [None; HOTCUES];
        shown[1] = Some(Hotcue::point(42.0));
        assert_eq!(
            sync.step(&with_hotcues(snap(1, 500.0), shown)),
            [
                MemoryAction::SaveHotcue(slot(2), Hotcue::point(42.0)),
                MemoryAction::DeleteHotcue(slot(3)),
            ]
        );
        assert!(sync.load_applied());
        assert_eq!(sync.step(&with_hotcues(snap(1, 500.0), shown)), []);
    }

    #[test]
    fn memory_sync_saves_set_and_deleted_hotcues() {
        let mut sync = MemorySync::new(1000, 44_100, TrackMemory::default(), 0);
        assert_eq!(sync.step(&snap(1, 0.0)), []);
        let mut hotcues = [None; HOTCUES];
        hotcues[1] = Some(Hotcue::point(321.0));
        let set = with_hotcues(snap(1, 0.0), hotcues);
        assert_eq!(
            sync.step(&set),
            [MemoryAction::SaveHotcue(slot(2), Hotcue::point(321.0))]
        );
        assert_eq!(sync.step(&set), [], "reported once");
        let deleted = snap(1, 0.0);
        assert_eq!(sync.step(&deleted), [MemoryAction::DeleteHotcue(slot(2))]);
        assert_eq!(sync.step(&deleted), [], "reported once");
        // A cue and two hotcues change in the same block.
        let mut both = [None; HOTCUES];
        both[0] = Some(Hotcue::point(10.0));
        both[7] = Some(Hotcue::point(20.0));
        assert_eq!(
            sync.step(&with_hotcues(snap(1, 5.0), both)),
            [
                MemoryAction::SaveCue(5.0),
                MemoryAction::SaveHotcue(slot(1), Hotcue::point(10.0)),
                MemoryAction::SaveHotcue(slot(8), Hotcue::point(20.0)),
            ]
        );
    }

    #[test]
    fn clamped_memory_is_not_saved_back() {
        let mut hotcues = [None; HOTCUES];
        hotcues[0] = Some(Hotcue::point(-5.0));
        hotcues[1] = Some(Hotcue::point(f64::NAN));
        hotcues[2] = Some(Hotcue::point(9000.0));
        let mut sync = MemorySync::new(1000, 44_100, memory(5000.0, hotcues), 0);
        // What the engine shows: everything clamped like `TrackMemory::clamped`.
        let mut shown = [None; HOTCUES];
        shown[0] = Some(Hotcue::point(0.0));
        shown[2] = Some(Hotcue::point(1000.0));
        let applied = with_hotcues(snap(1, 1000.0), shown);
        assert_eq!(sync.step(&applied), []);
        assert!(sync.load_applied());
        assert_eq!(sync.step(&applied), []);
    }

    #[test]
    fn writes_cue_changes_once_after_the_load() {
        let mut sync = MemorySync::new(1000, 44_100, memory(500.0, [None; 8]), 0);
        assert_eq!(sync.step(&snap(0, 700.0)), []);
        assert_eq!(sync.step(&snap(1, 500.0)), []);
        assert_eq!(sync.step(&snap(1, 500.0)), []);
        // The DJ sets a new cue: written through once.
        assert_eq!(sync.step(&snap(1, 250.0)), [MemoryAction::SaveCue(250.0)]);
        assert_eq!(sync.step(&snap(1, 250.0)), []);
        assert_eq!(sync.step(&snap(1, 0.0)), [MemoryAction::SaveCue(0.0)]);
    }

    #[test]
    fn without_a_saved_cue_tracks_from_zero() {
        let mut sync = MemorySync::new(1000, 44_100, TrackMemory::default(), 0);
        assert_eq!(sync.step(&snap(1, 0.0)), []);
        assert_eq!(sync.step(&snap(1, 0.0)), []);
        assert_eq!(sync.step(&snap(1, 42.0)), [MemoryAction::SaveCue(42.0)]);
    }

    #[test]
    fn nothing_is_saved_while_the_deck_is_empty() {
        let mut sync = MemorySync::new(1000, 44_100, memory(500.0, [None; 8]), 0);
        assert_eq!(sync.step(&snap(1, 500.0)), []);
        let empty = DeckSnapshot {
            loaded: false,
            ..snap(1, 0.0)
        };
        assert_eq!(sync.step(&empty), []);
    }

    #[test]
    fn without_hotcue_saves_only_the_cue_is_saved() {
        let mut sync =
            MemorySync::new(1000, 44_100, TrackMemory::default(), 0).without_hotcue_saves();
        assert_eq!(sync.step(&snap(1, 0.0)), []);
        let mut hotcues = [None; HOTCUES];
        hotcues[0] = Some(Hotcue::point(10.0));
        assert_eq!(
            sync.step(&with_hotcues(snap(1, 5.0), hotcues)),
            [MemoryAction::SaveCue(5.0)]
        );
        assert_eq!(
            sync.step(&snap(1, 5.0)),
            [],
            "a deletion is not saved either"
        );
    }

    #[test]
    fn a_pushed_hotcue_is_not_saved_back() {
        let mut sync = MemorySync::new(1000, 44_100, TrackMemory::default(), 0);
        assert!(
            !sync.wants_hotcue(slot(1), Some(Hotcue::point(10.0))),
            "before the load applies"
        );
        assert_eq!(sync.step(&snap(1, 0.0)), []);
        assert!(sync.wants_hotcue(slot(1), Some(Hotcue::point(10.0))));
        sync.pushed_hotcue(slot(1), Some(Hotcue::point(10.0)));
        assert!(!sync.wants_hotcue(slot(1), Some(Hotcue::point(10.0))));
        // Not applied yet: the old (empty) pad is no deletion.
        assert_eq!(sync.step(&snap(1, 0.0)), []);
        let mut hotcues = [None; HOTCUES];
        hotcues[0] = Some(Hotcue::point(10.0));
        // Applied: nothing to save.
        assert_eq!(sync.step(&with_hotcues(snap(1, 0.0), hotcues)), []);
        // From then on, the deck's own changes are saved, back to empty too.
        assert_eq!(
            sync.step(&snap(1, 0.0)),
            [MemoryAction::DeleteHotcue(slot(1))]
        );

        // A change on the deck before the push shows is saved.
        sync.pushed_hotcue(slot(2), Some(Hotcue::point(20.0)));
        let mut mine = [None; HOTCUES];
        mine[1] = Some(Hotcue::point(30.0));
        assert_eq!(
            sync.step(&with_hotcues(snap(1, 0.0), mine)),
            [MemoryAction::SaveHotcue(slot(2), Hotcue::point(30.0))]
        );
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
