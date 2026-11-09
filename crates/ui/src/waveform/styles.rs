//! The coloured bars one waveform column is made of, in each display style.

use egui::Color32;
use engine::DeckId;

use super::colour::{self, colour};
use crate::theme::Palette;
use crate::view::WaveformStyle;

/// How opaque a deck's waveform is when two share a lane.
pub const OVERLAY_ALPHA: f32 = 0.55;
/// The most bars a column has: one per band.
pub const MAX_BARS: usize = 3;

/// One coloured bar of a column, `from` and `to` as fractions of the lane's height: 0 is
/// the top, 1 the bottom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bar {
    pub colour: Color32,
    pub from: f32,
    pub to: f32,
}

/// The bars of one column for `bucket`, `[peak, low, mid, high]`, of `deck`'s waveform.
pub fn column(style: WaveformStyle, bucket: [u8; 4], deck: DeckId, palette: &Palette) -> Vec<Bar> {
    let (bars, count) = bars(style, bucket, deck, palette);
    bars[..count].to_vec()
}

/// [`column`] without the allocation: the bars and how many of them are used.
pub(super) fn bars(
    style: WaveformStyle,
    bucket: [u8; 4],
    deck: DeckId,
    palette: &Palette,
) -> ([Bar; MAX_BARS], usize) {
    let none = Bar {
        colour: Color32::TRANSPARENT,
        from: 0.5,
        to: 0.5,
    };
    let mut out = [none; MAX_BARS];
    let [peak, low, mid, high] = bucket;
    match style {
        WaveformStyle::Frequency => {
            out[0] = mirrored(colour(bucket, palette.text_dim), 0.5, 0.5, peak);
            (out, 1)
        }
        WaveformStyle::DeckColour => {
            out[0] = mirrored(palette.deck_accent(deck), 0.5, 0.5, peak);
            (out, 1)
        }
        WaveformStyle::Overlaid => {
            let accent = palette.deck_accent(deck).gamma_multiply(OVERLAY_ALPHA);
            out[0] = mirrored(accent, 0.5, 0.5, peak);
            (out, 1)
        }
        WaveformStyle::Bands => {
            let third = 1.0 / 3.0;
            let mut count = 0;
            for (level, anchor, centre) in [
                (high, colour::HIGH, third / 2.0),
                (mid, colour::MID, 0.5),
                (low, colour::LOW, 1.0 - third / 2.0),
            ] {
                if level == 0 {
                    continue;
                }
                let [r, g, b] = anchor.map(|c| c as u8);
                out[count] = mirrored(Color32::from_rgb(r, g, b), centre, third / 2.0, level);
                count += 1;
            }
            (out, count)
        }
    }
}

/// A bar mirrored around `centre`, reaching `level / 255` of `half` each way.
fn mirrored(colour: Color32, centre: f32, half: f32, level: u8) -> Bar {
    let reach = half * f32::from(level) / 255.0;
    Bar {
        colour,
        from: centre - reach,
        to: centre + reach,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deck(index: usize) -> DeckId {
        DeckId::new(index).unwrap()
    }

    #[test]
    fn bands_split_the_lane_in_three() {
        let p = Palette::DARK;
        let bass = column(WaveformStyle::Bands, [255, 255, 0, 0], deck(0), &p);
        assert!(!bass.is_empty());
        assert!(
            bass.iter().all(|b| b.from >= 0.66 && b.to <= 1.0),
            "{bass:?}"
        );
        let highs = column(WaveformStyle::Bands, [255, 0, 0, 255], deck(0), &p);
        assert!(!highs.is_empty());
        assert!(
            highs.iter().all(|b| b.from >= 0.0 && b.to <= 0.34),
            "{highs:?}"
        );
    }

    #[test]
    fn deck_colour_uses_the_accent() {
        let p = Palette::DARK;
        let bars = column(WaveformStyle::DeckColour, [200, 10, 10, 10], deck(0), &p);
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].colour, p.accent_left);
        let bars = column(WaveformStyle::DeckColour, [200, 10, 10, 10], deck(1), &p);
        assert_eq!(bars[0].colour, p.accent_right);
    }

    #[test]
    fn frequency_matches_plan_9() {
        let p = Palette::DARK;
        let bucket = [220, 200, 80, 30];
        let bars = column(WaveformStyle::Frequency, bucket, deck(0), &p);
        assert_eq!(bars[0].colour, colour(bucket, p.text_dim));
        assert!((bars[0].from + bars[0].to - 1.0).abs() < 1e-6);
    }

    #[test]
    fn overlaid_is_the_accent_at_reduced_alpha() {
        let p = Palette::DARK;
        let bars = column(WaveformStyle::Overlaid, [255, 1, 1, 1], deck(0), &p);
        assert_eq!(bars[0].colour, p.accent_left.gamma_multiply(OVERLAY_ALPHA));
    }
}
