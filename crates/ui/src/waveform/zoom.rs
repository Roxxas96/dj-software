//! The waveform zoom: how many beats a lane shows, and how a scroll, a pinch or the zoom
//! buttons turn into zoom steps. The window only asks for steps; the session owns the
//! zoom, applies the steps and saves it.

/// How much one step zooms: in divides the beats shown by this, out multiplies them.
pub const ZOOM_STEP: f64 = 1.25;
/// How far a scroll moves, in points, for one zoom step.
const SCROLL_PER_STEP: f32 = 40.0;

/// The beats a lane shows after `steps` zoom steps from `beats` (positive steps zoom in),
/// kept within `range` (fewest beats, most beats).
pub fn zoomed(beats: f64, steps: i32, range: (f64, f64)) -> f64 {
    (beats / ZOOM_STEP.powi(steps)).clamp(range.0, range.1)
}

/// Turns scrolls and pinches over the waveforms into whole zoom steps, keeping what is
/// left of a step for the next one (a trackpad scrolls a few points at a time).
#[derive(Debug, Default)]
pub struct ZoomGesture {
    /// Scroll not yet turned into a step, in points; up (zoom in) is positive.
    pending: f32,
}

impl ZoomGesture {
    /// The zoom steps a frame's `scroll` (points, up is positive) and `pinch` (the
    /// frame's zoom factor, above 1 to zoom in) add up to.
    pub fn steps(&mut self, scroll: f32, pinch: f32) -> i32 {
        if scroll.is_finite() {
            self.pending += scroll;
        }
        if pinch.is_finite() && pinch > 0.0 && pinch != 1.0 {
            self.pending += pinch.ln() / (ZOOM_STEP as f32).ln() * SCROLL_PER_STEP;
        }
        let steps = (self.pending / SCROLL_PER_STEP).trunc();
        self.pending -= steps * SCROLL_PER_STEP;
        steps as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zooming_divides_the_beats_and_stays_in_range() {
        let range = (4.0, 64.0);
        assert_eq!(zoomed(16.0, 0, range), 16.0);
        assert!((zoomed(16.0, 1, range) - 12.8).abs() < 1e-9);
        assert!((zoomed(16.0, -1, range) - 20.0).abs() < 1e-9);
        assert_eq!(zoomed(16.0, 100, range), 4.0);
        assert_eq!(zoomed(16.0, -100, range), 64.0);
        assert_eq!(zoomed(16.0, -100, (2.0, 128.0)), 128.0);
    }

    #[test]
    fn small_scrolls_add_up_to_a_step() {
        let mut gesture = ZoomGesture::default();
        assert_eq!(gesture.steps(15.0, 1.0), 0);
        assert_eq!(gesture.steps(15.0, 1.0), 0);
        assert_eq!(gesture.steps(15.0, 1.0), 1);
        assert_eq!(gesture.steps(-100.0, 1.0), -2);
        // One step's worth of pinch zooms in by one step.
        let mut gesture = ZoomGesture::default();
        assert_eq!(gesture.steps(0.0, ZOOM_STEP as f32 * 1.001), 1);
        assert_eq!(gesture.steps(f32::NAN, f32::NAN), 0);
    }
}
