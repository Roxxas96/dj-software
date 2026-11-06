//! Stereo echo: a delay of half a beat with feedback.
//!
//! The delay slews toward a new beat length and is read between frames, so a tempo change
//! bends the echoes briefly instead of jumping (a zipper or a click).

/// Longest delay, in seconds.
const MAX_SECONDS: u32 = 2;
const FEEDBACK: f32 = 0.5;
/// Gain of the echoes in the wet output.
const WET_GAIN: f32 = 0.5;
const FLUSH_BELOW: f32 = 1e-20;
/// Time constant with which the delay follows a new beat length.
const SLEW_SECONDS: f64 = 0.1;
/// Fastest delay change, in frames per frame: bends the echoes' pitch by at most 10 %.
const MAX_SLEW: f64 = 0.1;

#[derive(Debug, Clone)]
pub(super) struct Echo {
    sample_rate: f64,
    /// Per-frame factor of the delay's slew toward its target.
    slew: f64,
    lines: [Vec<f32>; 2],
    pos: [usize; 2],
    /// The current delay in frames per channel; NaN after a clear, so the first frame
    /// starts at the target.
    delay: [f64; 2],
}

impl Echo {
    pub(super) fn new(sample_rate: u32) -> Self {
        // One frame more than the longest delay for the read between frames.
        let len = (MAX_SECONDS * sample_rate) as usize + 2;
        let sample_rate = f64::from(sample_rate);
        Self {
            sample_rate,
            slew: 1.0 - (-1.0 / (SLEW_SECONDS * sample_rate)).exp(),
            lines: [vec![0.0; len], vec![0.0; len]],
            pos: [0; 2],
            delay: [f64::NAN; 2],
        }
    }

    pub(super) fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    pub(super) fn clear(&mut self) {
        for line in &mut self.lines {
            line.fill(0.0);
        }
        self.pos = [0; 2];
        self.delay = [f64::NAN; 2];
    }

    #[inline]
    pub(super) fn process(&mut self, ch: usize, x: f32, beat_frames: f64) -> f32 {
        let line = &mut self.lines[ch];
        let len = line.len();
        let target = (beat_frames / 2.0).clamp(1.0, (len - 2) as f64);
        let current = self.delay[ch];
        let delay = if current.is_nan() {
            target
        } else {
            current + ((target - current) * self.slew).clamp(-MAX_SLEW, MAX_SLEW)
        };
        self.delay[ch] = delay;
        let whole = delay.floor();
        let frac = (delay - whole) as f32;
        let pos = self.pos[ch];
        // `whole` is at least 1 and at most len - 2, so both reads are behind `pos`.
        let near = line[(pos + len - whole as usize) % len];
        let far = line[(pos + len - whole as usize - 1) % len];
        let delayed = near + (far - near) * frac;
        let mut fed = x + FEEDBACK * delayed;
        if fed.abs() < FLUSH_BELOW {
            fed = 0.0;
        }
        line[pos] = fed;
        self.pos[ch] = (pos + 1) % len;
        WET_GAIN * delayed
    }
}
