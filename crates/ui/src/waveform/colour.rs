//! The colour of a waveform bucket: bass is red-orange, mids pink, highs white, and the
//! peak sets how bright.

use egui::Color32;

use crate::theme;

/// The colour of a bucket that is all low frequencies.
pub const LOW: [f32; 3] = [255.0, 58.0, 31.0]; // #ff3a1f
/// The colour of a bucket that is all mid frequencies.
pub const MID: [f32; 3] = [255.0, 154.0, 213.0]; // #ff9ad5
/// The colour of a bucket that is all high frequencies.
pub const HIGH: [f32; 3] = [255.0, 255.0, 255.0]; // #ffffff
/// How bright the quietest bucket still is, as a share of its colour.
pub const MIN_BRIGHTNESS: f32 = 0.35;

/// The colour of `bucket`, `[peak, low, mid, high]`: the three bands' share of their sum
/// blends the anchors, and the peak scales the brightness, never below
/// [`MIN_BRIGHTNESS`]. A bucket without bands is [`theme::TEXT_DIM`].
pub fn colour(bucket: [u8; 4]) -> Color32 {
    let [peak, low, mid, high] = bucket.map(f32::from);
    let sum = low + mid + high;
    if sum == 0.0 {
        return theme::TEXT_DIM;
    }
    let brightness = MIN_BRIGHTNESS + (1.0 - MIN_BRIGHTNESS) * peak / 255.0;
    let channel = |i: usize| {
        let blended = (low * LOW[i] + mid * MID[i] + high * HIGH[i]) / sum;
        (blended * brightness).round().clamp(0.0, 255.0) as u8
    };
    Color32::from_rgb(channel(0), channel(1), channel(2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_is_warm_for_bass_and_light_for_highs() {
        let bass = colour([255, 255, 0, 0]);
        assert!(bass.r() > 200 && bass.g() < 120, "{bass:?}");
        let highs = colour([255, 0, 0, 255]);
        assert!(
            highs.r() > 230 && highs.g() > 230 && highs.b() > 230,
            "{highs:?}"
        );
        assert_eq!(colour([0, 0, 0, 0]), theme::TEXT_DIM);
    }

    #[test]
    fn a_quiet_bucket_keeps_a_third_of_its_brightness() {
        let quiet = colour([0, 0, 0, 255]);
        assert_eq!(quiet.r(), 89); // 255 * 0.35
    }
}
