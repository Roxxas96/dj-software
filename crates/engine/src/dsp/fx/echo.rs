//! Stereo echo: a delay of half a beat with feedback.

/// Longest delay, in seconds.
const MAX_SECONDS: u32 = 2;
const FEEDBACK: f32 = 0.5;
/// Gain of the echoes in the wet output.
const WET_GAIN: f32 = 0.5;
const FLUSH_BELOW: f32 = 1e-20;

#[derive(Debug, Clone)]
pub(super) struct Echo {
    sample_rate: f64,
    lines: [Vec<f32>; 2],
    pos: [usize; 2],
}

impl Echo {
    pub(super) fn new(sample_rate: u32) -> Self {
        let len = (MAX_SECONDS * sample_rate) as usize + 1;
        Self {
            sample_rate: f64::from(sample_rate),
            lines: [vec![0.0; len], vec![0.0; len]],
            pos: [0; 2],
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
    }

    #[inline]
    pub(super) fn process(&mut self, ch: usize, x: f32, beat_frames: f64) -> f32 {
        let line = &mut self.lines[ch];
        let len = line.len();
        let delay = ((beat_frames / 2.0).round() as usize).clamp(1, len - 1);
        let pos = self.pos[ch];
        let delayed = line[(pos + len - delay) % len];
        let mut fed = x + FEEDBACK * delayed;
        if fed.abs() < FLUSH_BELOW {
            fed = 0.0;
        }
        line[pos] = fed;
        self.pos[ch] = (pos + 1) % len;
        WET_GAIN * delayed
    }
}
