//! The beat grid's ticks inside a stretch of a track.

use engine::Grid;

/// A beat on the grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tick {
    /// Where it falls, in track frames.
    pub frame: f64,
    /// It is the first beat of a bar.
    pub bar: bool,
}

/// Every beat of `grid` from `from_frame` to `to_frame`, both included. The grid
/// extends before its first beat too; every fourth beat from that one is a bar. None
/// without a grid.
pub fn ticks(grid: Option<Grid>, from_frame: f64, to_frame: f64) -> Vec<Tick> {
    let Some(grid) = grid else {
        return Vec::new();
    };
    let period = grid.beat_period_frames;
    if !(period.is_finite() && period > 0.0) || !from_frame.is_finite() || !to_frame.is_finite() {
        return Vec::new();
    }
    let first = ((from_frame - grid.first_beat_frames) / period).ceil() as i64;
    let last = ((to_frame - grid.first_beat_frames) / period).floor() as i64;
    (first..=last)
        .map(|k| Tick {
            frame: grid.first_beat_frames + k as f64 * period,
            bar: k.rem_euclid(4) == 0,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(first: f64) -> Option<Grid> {
        Some(Grid {
            beat_period_frames: 100.0,
            first_beat_frames: first,
        })
    }

    #[test]
    fn ticks_mark_every_fourth_beat_red() {
        let found = ticks(grid(50.0), 0.0, 1000.0);
        let frames: Vec<f64> = found.iter().map(|t| t.frame).collect();
        let expected: Vec<f64> = (0..10).map(|k| 50.0 + 100.0 * f64::from(k)).collect();
        assert_eq!(frames, expected);
        let bars: Vec<f64> = found.iter().filter(|t| t.bar).map(|t| t.frame).collect();
        assert_eq!(bars, [50.0, 450.0, 850.0]);
    }

    #[test]
    fn ticks_before_the_first_beat_count_backwards() {
        let found = ticks(grid(450.0), 0.0, 200.0);
        assert_eq!(
            found[0],
            Tick {
                frame: 50.0,
                bar: true
            }
        );
        assert!(!found[1].bar);
    }

    #[test]
    fn no_grid_no_ticks() {
        assert!(ticks(None, 0.0, 1000.0).is_empty());
    }
}
