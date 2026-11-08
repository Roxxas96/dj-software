//! A playhead that moves smoothly between the engine's snapshots.

use std::time::{Duration, Instant};

use engine::DeckSnapshot;

/// How far past the last change in a snapshot the playhead is extrapolated.
pub const MAX_EXTRAPOLATION: Duration = Duration::from_millis(100);

/// Where the playhead is, from the snapshots seen. A snapshot only changes when the audio
/// thread publishes one, so between changes the position is advanced by the clock.
#[derive(Debug, Clone, Default)]
pub struct Playhead {
    position_frames: f64,
    /// When the position, play state or load last changed.
    changed_at: Option<Instant>,
    load_seq: u64,
    playing: bool,
    moving_freely: bool,
    /// Track frames per second at the current tempo.
    frames_per_second: f64,
}

impl Playhead {
    /// Takes `snapshot`, seen at `now`.
    pub fn observe(&mut self, snapshot: &DeckSnapshot, now: Instant) {
        if self.changed_at.is_none()
            || snapshot.position_frames != self.position_frames
            || snapshot.playing != self.playing
            || snapshot.load_seq != self.load_seq
        {
            self.changed_at = Some(now);
        }
        self.position_frames = snapshot.position_frames;
        self.playing = snapshot.playing;
        self.load_seq = snapshot.load_seq;
        self.moving_freely = !snapshot.jog_touched && !snapshot.backspinning;
        self.frames_per_second = snapshot.tempo_ratio * f64::from(snapshot.track_sample_rate);
    }

    /// The position at `now` in track frames, `latency` before what the engine has
    /// rendered: that is what is being heard.
    pub fn position(&self, now: Instant, latency: Duration) -> f64 {
        let Some(changed_at) = self.changed_at else {
            return self.position_frames;
        };
        if !self.playing {
            return self.position_frames;
        }
        let elapsed = if self.moving_freely {
            now.saturating_duration_since(changed_at)
                .min(MAX_EXTRAPOLATION)
        } else {
            Duration::ZERO
        };
        self.position_frames
            + self.frames_per_second * (elapsed.as_secs_f64() - latency.as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(position_frames: f64, playing: bool) -> DeckSnapshot {
        DeckSnapshot {
            loaded: true,
            playing,
            position_frames,
            tempo_ratio: 1.0,
            track_sample_rate: 44_100,
            ..DeckSnapshot::default()
        }
    }

    #[test]
    fn playhead_extrapolates_while_playing_and_holds_when_paused() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut head = Playhead::default();
        head.observe(&snapshot(1000.0, true), t0);
        let at = head.position(t0 + ms(10), Duration::ZERO);
        assert!((at - 1441.0).abs() <= 1.0, "{at}");
        let capped = head.position(t0 + ms(500), Duration::ZERO);
        assert!((capped - 5410.0).abs() <= 1.0, "{capped}");

        let mut paused = Playhead::default();
        paused.observe(&snapshot(1000.0, false), t0);
        assert_eq!(paused.position(t0 + ms(10), Duration::ZERO), 1000.0);
    }

    #[test]
    fn a_held_jog_or_backspin_shows_the_snapshot_and_latency_is_subtracted() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut head = Playhead::default();
        let mut touched = snapshot(1000.0, true);
        touched.jog_touched = true;
        head.observe(&touched, t0);
        assert_eq!(head.position(t0 + ms(50), Duration::ZERO), 1000.0);
        let mut free = Playhead::default();
        free.observe(&snapshot(1000.0, true), t0);
        let at = free.position(t0, ms(10));
        assert!((at - 559.0).abs() <= 1.0, "{at}");
    }

    #[test]
    fn an_unchanged_snapshot_does_not_restart_the_clock() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut head = Playhead::default();
        head.observe(&snapshot(1000.0, true), t0);
        head.observe(&snapshot(1000.0, true), t0 + ms(40));
        let at = head.position(t0 + ms(50), Duration::ZERO);
        assert!((at - (1000.0 + 2205.0)).abs() <= 1.0, "{at}");
    }
}
