//! Parameter smoothing.

/// A gain that moves linearly from `current` to `target` across one render block, so
/// parameter changes do not cause zipper noise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Ramp {
    pub(crate) current: f32,
    pub(crate) target: f32,
}

impl Ramp {
    /// A ramp at rest at `value`.
    pub(crate) fn new(value: f32) -> Self {
        Self {
            current: value,
            target: value,
        }
    }

    /// Starts a block of `frames` frames: returns the gains to apply across it and leaves
    /// the ramp at rest at its target.
    pub(crate) fn block(&mut self, frames: usize) -> Segment {
        if frames == 0 {
            return Segment {
                start: self.current,
                step: 0.0,
            };
        }
        let segment = Segment {
            start: self.current,
            step: (self.target - self.current) / frames as f32,
        };
        self.current = self.target;
        segment
    }
}

/// The gains of one ramp across one block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Segment {
    start: f32,
    step: f32,
}

impl Segment {
    /// The gain at `frame`: one step past `current` on the first frame, `target` on the
    /// last. Exactly `current` when the ramp is at rest.
    #[inline]
    pub(crate) fn at(&self, frame: usize) -> f32 {
        self.start + self.step * (frame + 1) as f32
    }

    /// Whether the gain is exactly zero across the whole block.
    pub(crate) fn is_zero(&self) -> bool {
        self.start == 0.0 && self.step == 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramps_linearly_to_target_and_rests() {
        let mut ramp = Ramp::new(1.0);
        ramp.target = 0.0;
        let seg = ramp.block(4);
        assert_eq!(
            [seg.at(0), seg.at(1), seg.at(2), seg.at(3)],
            [0.75, 0.5, 0.25, 0.0]
        );
        assert_eq!(ramp.current, 0.0);
        let seg = ramp.block(4);
        assert_eq!(seg.at(3), 0.0);
    }

    #[test]
    fn at_rest_is_exact() {
        let mut ramp = Ramp::new(0.3);
        let seg = ramp.block(512);
        assert!((0..512).all(|i| seg.at(i) == 0.3));
    }
}
