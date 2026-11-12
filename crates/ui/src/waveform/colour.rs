//! The colours of a waveform's three bands: one per frequency band, from a palette the
//! view picks.

use egui::Color32;

use crate::theme::Palette;
use crate::view::WaveformPalette;

/// The `[low, mid, high]` colours of `palette`. Blue / orange / white ends in the scheme's
/// own high colour (white on the dark scheme, dark on the light one), so it stays visible.
pub fn band_colours(palette: WaveformPalette, scheme: &Palette) -> [Color32; 3] {
    let hex = |rgb: u32| Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8);
    match palette {
        WaveformPalette::Rgb => [hex(0xE8322A), hex(0x38C24A), hex(0x3D8BFF)],
        WaveformPalette::BlueOrangeWhite => [hex(0x1E64FF), hex(0xFFA21E), scheme.wave_high],
        WaveformPalette::OrangeYellowCyan => [hex(0xFF4D1A), hex(0xFFD23D), hex(0x5EE6FF)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(hex: u32) -> Color32 {
        Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
    }

    #[test]
    fn rgb_is_the_default_and_matches_the_spec() {
        assert_eq!(
            band_colours(WaveformPalette::default(), &Palette::DARK),
            [rgb(0xE8322A), rgb(0x38C24A), rgb(0x3D8BFF)]
        );
    }

    #[test]
    fn blue_orange_white_highs_follow_the_scheme() {
        for scheme in [Palette::DARK, Palette::LIGHT] {
            let [low, mid, high] = band_colours(WaveformPalette::BlueOrangeWhite, &scheme);
            assert_eq!(low, rgb(0x1E64FF));
            assert_eq!(mid, rgb(0xFFA21E));
            assert_eq!(high, scheme.wave_high);
        }
    }

    #[test]
    fn orange_yellow_cyan_values() {
        assert_eq!(
            band_colours(WaveformPalette::OrangeYellowCyan, &Palette::DARK),
            [rgb(0xFF4D1A), rgb(0xFFD23D), rgb(0x5EE6FF)]
        );
    }
}
