//! Reverb in the Freeverb topology: 8 parallel damped combs into 4 series all-passes per
//! channel. The tunings are Freeverb's public-domain delay lengths.

const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALL_PASSES: [usize; 4] = [556, 441, 341, 225];
/// Added to the right channel's delay lengths.
const STEREO_SPREAD: usize = 23;
const TUNING_RATE: f64 = 44_100.0;

const ROOM: f32 = 0.8;
const DAMPING: f32 = 0.5;
const COMB_FEEDBACK: f32 = ROOM * 0.28 + 0.7;
const COMB_DAMP: f32 = DAMPING * 0.4;
const ALL_PASS_FEEDBACK: f32 = 0.5;
const INPUT_GAIN: f32 = 0.015;
const WET_GAIN: f32 = 1.5;
const FLUSH_BELOW: f32 = 1e-20;

#[derive(Debug, Clone)]
struct Comb {
    buf: Vec<f32>,
    pos: usize,
    store: f32,
}

impl Comb {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len],
            pos: 0,
            store: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let out = self.buf[self.pos];
        self.store = out * (1.0 - COMB_DAMP) + self.store * COMB_DAMP;
        self.buf[self.pos] = x + self.store * COMB_FEEDBACK;
        self.pos = (self.pos + 1) % self.buf.len();
        out
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
        self.store = 0.0;
        self.pos = 0;
    }
}

#[derive(Debug, Clone)]
struct AllPass {
    buf: Vec<f32>,
    pos: usize,
}

impl AllPass {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len],
            pos: 0,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let delayed = self.buf[self.pos];
        self.buf[self.pos] = x + delayed * ALL_PASS_FEEDBACK;
        self.pos = (self.pos + 1) % self.buf.len();
        delayed - x
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }
}

#[derive(Debug, Clone)]
struct Side {
    combs: Vec<Comb>,
    all_passes: Vec<AllPass>,
}

#[derive(Debug, Clone)]
pub(super) struct Reverb {
    sides: [Side; 2],
}

impl Reverb {
    pub(super) fn new(sample_rate: u32) -> Self {
        let scale = f64::from(sample_rate) / TUNING_RATE;
        let len = |n: usize| ((n as f64 * scale).round() as usize).max(1);
        let side = |spread: usize| Side {
            combs: COMBS.iter().map(|&n| Comb::new(len(n + spread))).collect(),
            all_passes: ALL_PASSES
                .iter()
                .map(|&n| AllPass::new(len(n + spread)))
                .collect(),
        };
        Self {
            sides: [side(0), side(STEREO_SPREAD)],
        }
    }

    pub(super) fn clear(&mut self) {
        for side in &mut self.sides {
            side.combs.iter_mut().for_each(Comb::clear);
            side.all_passes.iter_mut().for_each(AllPass::clear);
        }
    }

    #[inline]
    pub(super) fn process(&mut self, ch: usize, x: f32) -> f32 {
        let side = &mut self.sides[ch];
        let input = x * INPUT_GAIN;
        let mut y: f32 = side.combs.iter_mut().map(|c| c.process(input)).sum();
        for ap in &mut side.all_passes {
            y = ap.process(y);
        }
        y * WET_GAIN
    }

    pub(super) fn flush_denormals(&mut self) {
        for side in &mut self.sides {
            for comb in &mut side.combs {
                if comb.store.abs() < FLUSH_BELOW {
                    comb.store = 0.0;
                }
                for v in &mut comb.buf {
                    if v.abs() < FLUSH_BELOW {
                        *v = 0.0;
                    }
                }
            }
            for ap in &mut side.all_passes {
                for v in &mut ap.buf {
                    if v.abs() < FLUSH_BELOW {
                        *v = 0.0;
                    }
                }
            }
        }
    }
}
