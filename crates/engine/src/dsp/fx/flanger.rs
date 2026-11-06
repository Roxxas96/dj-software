//! Flanger: a short delay swept by a triangle LFO, mixed with the input.

const MIN_SECONDS: f64 = 0.001;
const MAX_SECONDS: f64 = 0.007;
/// Delay line capacity, in seconds.
const CAPACITY_SECONDS: f64 = 0.020;
const FEEDBACK: f32 = 0.3;
/// LFO period in beats.
const PERIOD_BEATS: f64 = 4.0;

#[derive(Debug, Clone)]
pub(super) struct Flanger {
    sample_rate: f64,
    lines: [Vec<f32>; 2],
    pos: usize,
    /// LFO phase in `[0, 1)`, advanced once per frame (on channel 1).
    phase: f64,
}

impl Flanger {
    pub(super) fn new(sample_rate: u32) -> Self {
        let len = (CAPACITY_SECONDS * f64::from(sample_rate)).ceil() as usize + 2;
        Self {
            sample_rate: f64::from(sample_rate),
            lines: [vec![0.0; len], vec![0.0; len]],
            pos: 0,
            phase: 0.0,
        }
    }

    pub(super) fn clear(&mut self) {
        for line in &mut self.lines {
            line.fill(0.0);
        }
        self.pos = 0;
        self.phase = 0.0;
    }

    #[inline]
    pub(super) fn process(&mut self, ch: usize, x: f32, beat_frames: f64) -> f32 {
        let tri = 1.0 - (2.0 * self.phase - 1.0).abs();
        let delay = (MIN_SECONDS + (MAX_SECONDS - MIN_SECONDS) * tri) * self.sample_rate;
        let line = &mut self.lines[ch];
        let len = line.len();
        let whole = delay.floor();
        let frac = (delay - whole) as f32;
        let i0 = (self.pos + len - whole as usize) % len;
        let i1 = (i0 + len - 1) % len;
        let delayed = line[i0] + (line[i1] - line[i0]) * frac;
        let mut fed = x + FEEDBACK * delayed;
        if fed.abs() < 1e-20 {
            fed = 0.0;
        }
        line[self.pos] = fed;
        if ch == 1 {
            self.pos = (self.pos + 1) % len;
            self.phase = (self.phase + 1.0 / (PERIOD_BEATS * beat_frames)).fract();
        }
        0.5 * (x + delayed)
    }
}
