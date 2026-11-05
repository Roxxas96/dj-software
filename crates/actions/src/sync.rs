//! Beat sync: which deck to follow and the tempo that matches it.

use engine::{DeckId, DeckSnapshot, TempoRange};

use crate::{ControlState, Side};

/// The tempo ranges SYNC may widen to, narrowest first.
const RANGES: [TempoRange; 4] = [
    TempoRange::Six,
    TempoRange::Ten,
    TempoRange::Sixteen,
    TempoRange::Hundred,
];

/// What SYNC does to a deck: follow `master` at `slider` within `range`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SyncPlan {
    pub master: DeckId,
    pub slider: f32,
    pub range: TempoRange,
    /// The deck's beats per master beat: 2^k, so 0.5 or 2.0 for half or double time.
    pub beat_ratio: f64,
}

/// The track's own tempo in BPM, or `None` without a usable grid.
fn bpm(snap: &DeckSnapshot) -> Option<f64> {
    let grid = snap.grid?;
    let bpm = 60.0 * f64::from(snap.track_sample_rate) / grid.beat_period_frames;
    (bpm.is_finite() && bpm > 0.0).then_some(bpm)
}

/// The deck `deck` should follow: a loaded, playing deck with a grid, the active deck of
/// the other side first, otherwise the lowest deck number.
fn choose_master(deck: DeckId, state: &ControlState, snaps: &[DeckSnapshot; 4]) -> Option<DeckId> {
    let usable = |id: DeckId| {
        let snap = &snaps[id.index()];
        id != deck && snap.loaded && snap.playing && bpm(snap).is_some()
    };
    let other = match Side::of(deck) {
        Side::Left => Side::Right,
        Side::Right => Side::Left,
    };
    let active = state.side(other).active_deck;
    if usable(active) {
        return Some(active);
    }
    DeckId::ALL.into_iter().find(|&id| usable(id))
}

/// The plan to sync `deck` to another playing deck, or `None` when there is no master,
/// `deck` has no grid or the needed tempo is out of reach (more than 100 %).
pub(crate) fn plan_sync(
    deck: DeckId,
    state: &ControlState,
    snaps: &[DeckSnapshot; 4],
) -> Option<SyncPlan> {
    let own = bpm(&snaps[deck.index()])?;
    let master = choose_master(deck, state, snaps)?;
    let master_snap = &snaps[master.index()];
    let master_bpm = bpm(master_snap)? * master_snap.tempo_ratio;
    if !(master_bpm.is_finite() && master_bpm > 0.0) {
        return None;
    }

    // Half or double time when that lands closer to the deck's own tempo.
    let (target, beat_ratio) = [0.5, 1.0, 2.0]
        .into_iter()
        .map(|k| (master_bpm * k, k))
        .min_by(|a, b| {
            let distance = |t: f64| (t / own).ln().abs();
            distance(a.0).total_cmp(&distance(b.0))
        })?;
    let ratio = target / own;
    if !ratio.is_finite() {
        return None;
    }

    let needed = (ratio - 1.0).abs();
    let current = state.deck(deck).tempo_range;
    // A little slack so a ratio at the edge of a range (1.10 in ±10 %) does not widen it.
    let fits = |r: TempoRange| needed <= r.fraction() + 1e-9;
    let range = if fits(current) {
        current
    } else {
        RANGES.into_iter().find(|&r| fits(r))?
    };
    let slider = 0.5 + (ratio - 1.0) / (2.0 * range.fraction());
    Some(SyncPlan {
        master,
        slider: slider.clamp(0.0, 1.0) as f32,
        range,
        beat_ratio,
    })
}

#[cfg(test)]
mod tests {
    use engine::Grid;

    use super::*;
    use crate::Side;

    fn snap(bpm: f64, ratio: f64) -> DeckSnapshot {
        DeckSnapshot {
            loaded: true,
            playing: true,
            track_sample_rate: 44_100,
            tempo_ratio: ratio,
            grid: Grid::from_bpm(bpm, 0.0, 44_100),
            ..Default::default()
        }
    }

    fn plan(own: f64, master: f64, master_ratio: f64) -> Option<SyncPlan> {
        let mut snaps: [DeckSnapshot; 4] = Default::default();
        snaps[0] = snap(own, 1.0);
        snaps[1] = snap(master, master_ratio);
        plan_sync(DeckId::ALL[0], &ControlState::default(), &snaps)
    }

    fn deck(i: usize) -> DeckId {
        DeckId::ALL[i]
    }

    /// Plans for deck 1 (index 0) with these decks playing at the given BPM.
    fn plan_with(playing: &[(usize, f64)], active_right: usize) -> Option<SyncPlan> {
        let mut snaps: [DeckSnapshot; 4] = Default::default();
        snaps[0] = snap(128.0, 1.0);
        snaps[0].playing = false;
        for &(i, bpm) in playing {
            snaps[i] = snap(bpm, 1.0);
        }
        let mut state = ControlState::default();
        state.side_mut(Side::Right).active_deck = deck(active_right);
        plan_sync(deck(0), &state, &snaps)
    }

    #[test]
    fn the_other_sides_playing_deck_is_the_master() {
        let plan = plan(128.0, 125.0, 1.0).unwrap();
        assert_eq!(plan.master, deck(1));
        assert_eq!(plan.beat_ratio, 1.0);
    }

    #[test]
    fn the_other_sides_active_deck_wins_at_equal_tempo() {
        // Same BPM everywhere: only the choice of master tells the plans apart.
        let plan = plan_with(&[(1, 125.0), (3, 125.0)], 3).unwrap();
        assert_eq!(plan.master, deck(3));
        let plan = plan_with(&[(1, 125.0), (3, 125.0)], 1).unwrap();
        assert_eq!(plan.master, deck(1));
    }

    #[test]
    fn the_lowest_deck_follows_when_the_active_one_is_unusable() {
        // The right side's active deck (index 1) does not play; decks 3 and 4 do.
        let plan = plan_with(&[(2, 125.0), (3, 125.0)], 1).unwrap();
        assert_eq!(plan.master, deck(2));
    }

    #[test]
    fn half_time_picks_half_the_master_tempo() {
        let plan = plan(87.0, 174.0, 1.0).unwrap();
        assert_eq!(plan.master, deck(1));
        assert_eq!(plan.beat_ratio, 0.5);
        assert_eq!(plan.slider, 0.5);
    }

    #[test]
    fn beat_ratio_is_the_power_of_two_picked() {
        assert_eq!(plan(87.0, 174.0, 1.0).unwrap().beat_ratio, 0.5);
        assert_eq!(plan(174.0, 87.0, 1.0).unwrap().beat_ratio, 2.0);
        assert_eq!(plan(128.0, 125.0, 1.0).unwrap().beat_ratio, 1.0);
    }

    #[test]
    fn the_master_tempo_slider_counts() {
        let plan = plan(100.0, 100.0, 1.05).unwrap();
        assert_eq!(plan.range, TempoRange::Ten);
        assert!((plan.slider - (0.5 + 0.05 / 0.2)).abs() < 1e-6);
    }

    #[test]
    fn a_ratio_on_the_edge_of_the_range_keeps_it() {
        // 110 / 100 is 1.1 up to rounding: still ±10 %, slider at the end.
        let plan = plan(100.0, 110.0, 1.0).unwrap();
        assert_eq!(plan.range, TempoRange::Ten);
        assert!((plan.slider - 1.0).abs() < 1e-6);
        assert_eq!(
            self::plan(100.0, 110.0001, 1.0).unwrap().range,
            TempoRange::Sixteen
        );
    }

    #[test]
    fn a_master_without_a_usable_tempo_gives_no_plan() {
        assert_eq!(plan(128.0, 125.0, 0.0), None);
        assert_eq!(plan(128.0, 125.0, f64::NAN), None);
    }
}
