//! A playhead that moves smoothly between the engine's snapshots.

use std::time::{Duration, Instant};

use engine::DeckSnapshot;

/// How far past the block that rendered a snapshot the playhead is extrapolated.
pub const MAX_EXTRAPOLATION: Duration = Duration::from_millis(100);

/// Where the playhead is, from the last snapshot seen. A snapshot only changes when the
/// audio thread publishes one, so between blocks the position is advanced by the clock
/// from when its block was rendered.
#[derive(Debug, Clone, Default)]
pub struct Playhead {
    position_frames: f64,
    /// When the snapshot's block began rendering; `None` when unknown, and the snapshot is
    /// then shown as it is.
    rendered_at: Option<Instant>,
    playing: bool,
    moving_freely: bool,
    /// Track frames per second at the current tempo.
    frames_per_second: f64,
}

impl Playhead {
    /// Takes `snapshot`, from the block that began rendering `rendered_at_micros` on
    /// [`engine::clock`] (0: unknown).
    pub fn observe(&mut self, snapshot: &DeckSnapshot, rendered_at_micros: u64) {
        self.position_frames = snapshot.position_frames;
        self.rendered_at =
            (rendered_at_micros != 0).then(|| engine::clock::instant_at(rendered_at_micros));
        self.playing = snapshot.playing;
        self.moving_freely = !snapshot.jog_touched && !snapshot.backspinning;
        self.frames_per_second = snapshot.tempo_ratio * f64::from(snapshot.track_sample_rate);
    }

    /// The position at `now` in track frames, `latency` before what the engine has
    /// rendered: that is what is being heard.
    pub fn position(&self, now: Instant, latency: Duration) -> f64 {
        let Some(rendered_at) = self.rendered_at else {
            return self.position_frames;
        };
        if !self.playing {
            return self.position_frames;
        }
        let elapsed = if self.moving_freely {
            now.saturating_duration_since(rendered_at)
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

    /// A snapshot at `position_frames`, with its block rendered at `rendered_at`.
    fn snapshot(position_frames: f64, playing: bool, rendered_at: Instant) -> (DeckSnapshot, u64) {
        let snapshot = DeckSnapshot {
            loaded: true,
            playing,
            position_frames,
            tempo_ratio: 1.0,
            track_sample_rate: 44_100,
            ..DeckSnapshot::default()
        };
        (snapshot, engine::clock::micros_at(rendered_at))
    }

    fn observe(head: &mut Playhead, (snapshot, rendered_at): (DeckSnapshot, u64)) {
        head.observe(&snapshot, rendered_at);
    }

    /// An instant on a whole microsecond of the engine's clock, so stamps are exact, and
    /// after its epoch (a stamp of 0 means unpublished).
    fn start() -> Instant {
        engine::clock::instant_at(engine::clock::micros_at(Instant::now()).max(1))
    }

    #[test]
    fn playhead_extrapolates_while_playing_and_holds_when_paused() {
        let t0 = start();
        let ms = Duration::from_millis;
        let mut head = Playhead::default();
        observe(&mut head, snapshot(1000.0, true, t0));
        let at = head.position(t0 + ms(10), Duration::ZERO);
        assert!((at - 1441.0).abs() <= 1.0, "{at}");
        let capped = head.position(t0 + ms(500), Duration::ZERO);
        assert!((capped - 5410.0).abs() <= 1.0, "{capped}");

        let mut paused = Playhead::default();
        observe(&mut paused, snapshot(1000.0, false, t0));
        assert_eq!(paused.position(t0 + ms(10), Duration::ZERO), 1000.0);
    }

    #[test]
    fn a_held_jog_or_backspin_shows_the_snapshot_and_latency_is_subtracted() {
        let t0 = start();
        let ms = Duration::from_millis;
        let mut head = Playhead::default();
        let mut touched = snapshot(1000.0, true, t0);
        touched.0.jog_touched = true;
        observe(&mut head, touched);
        assert_eq!(head.position(t0 + ms(50), Duration::ZERO), 1000.0);
        let mut spinning = Playhead::default();
        let mut backspin = snapshot(1000.0, true, t0);
        backspin.0.backspinning = true;
        observe(&mut spinning, backspin);
        assert_eq!(spinning.position(t0 + ms(50), Duration::ZERO), 1000.0);
        let mut free = Playhead::default();
        observe(&mut free, snapshot(1000.0, true, t0));
        let at = free.position(t0, ms(10));
        assert!((at - 559.0).abs() <= 1.0, "{at}");
    }

    #[test]
    fn the_clock_runs_from_when_the_block_was_rendered_not_when_it_was_seen() {
        let t0 = start();
        let ms = Duration::from_millis;
        let mut head = Playhead::default();
        // Seen 40 ms after its block rendered.
        observe(&mut head, snapshot(1000.0, true, t0));
        let at = head.position(t0 + ms(50), Duration::ZERO);
        assert!((at - (1000.0 + 2205.0)).abs() <= 1.0, "{at}");
    }

    #[test]
    fn blocks_and_frames_at_their_own_rates_give_a_smooth_playhead() {
        const RATE: f64 = 44_100.0;
        let block = Duration::from_micros(11_610);
        let frame = Duration::from_micros(8_300);
        // A block becomes visible this long after it began rendering.
        let render = Duration::from_micros(2_500);
        let t0 = start();
        // What the engine has rendered by `t`: the position it extrapolates to.
        let truth = |t: Instant| (t - t0).as_secs_f64() * RATE;
        let mut head = Playhead::default();
        let mut previous = f64::MIN;
        let mut samples = 0;
        let mut t = t0 + render;
        while t < t0 + Duration::from_secs(2) {
            // The newest block visible at `t`.
            let k = ((t - t0 - render).as_micros() / block.as_micros()) as u32;
            let rendered_at = t0 + block * k;
            observe(&mut head, snapshot(truth(rendered_at), true, rendered_at));
            let at = head.position(t, Duration::ZERO);
            assert!(
                at >= previous,
                "went back from {previous} to {at} at sample {samples}"
            );
            let error = (at - truth(t)).abs();
            assert!(
                error <= RATE / 1000.0,
                "{error} frames off at sample {samples}"
            );
            previous = at;
            samples += 1;
            t += frame;
        }
        assert!(samples > 200, "{samples}");
    }
}
