//! The coloured bars one waveform column is made of, in each display style.

use egui::Color32;
use engine::DeckId;

use crate::theme::Palette;
use crate::view::WaveformStyle;

/// How opaque a deck's waveform is when two share a lane.
pub const OVERLAY_ALPHA: f32 = 0.55;
/// How opaque the Frequency style's mid layer is over the low one.
pub const MID_ALPHA: f32 = 0.75;
/// How opaque the Frequency style's high layer is over the other two.
pub const HIGH_ALPHA: f32 = 0.65;
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

/// What one column shows of the `buckets` under it: the peak and each band at their
/// highest, so a kick's bucket does not hide the mids and highs beside it. `None` when
/// there are no buckets.
pub fn loudest(buckets: &[[u8; 4]]) -> Option<[u8; 4]> {
    let (first, rest) = buckets.split_first()?;
    let mut out = *first;
    for bucket in rest {
        for (level, &other) in out.iter_mut().zip(bucket) {
            *level = (*level).max(other);
        }
    }
    Some(out)
}

/// The Frequency style's `[low, mid, high]` layer colours for `bands`: the low layer
/// opaque, the mid and high ones see-through so the layers behind them show.
pub fn layers(bands: [Color32; 3]) -> [Color32; 3] {
    let [low, mid, high] = bands;
    [
        low,
        mid.gamma_multiply(MID_ALPHA),
        high.gamma_multiply(HIGH_ALPHA),
    ]
}

/// The bars of one column for `bucket`, `[peak, low, mid, high]`, of `deck`'s waveform;
/// `bands` are the `[low, mid, high]` colours ([`super::colour::band_colours`]).
pub fn column(
    style: WaveformStyle,
    bucket: [u8; 4],
    deck: DeckId,
    palette: &Palette,
    bands: [Color32; 3],
) -> Vec<Bar> {
    let (bars, count) = bars(style, bucket, deck, palette, bands);
    bars[..count].to_vec()
}

/// [`column`] without the allocation: the bars and how many of them are used.
pub(super) fn bars(
    style: WaveformStyle,
    bucket: [u8; 4],
    deck: DeckId,
    palette: &Palette,
    bands: [Color32; 3],
) -> ([Bar; MAX_BARS], usize) {
    let none = Bar {
        colour: Color32::TRANSPARENT,
        from: 0.5,
        to: 0.5,
    };
    let mut out = [none; MAX_BARS];
    let [peak, low, mid, high] = bucket;
    let [low_colour, mid_colour, high_colour] = bands;
    match style {
        // Three layers over the same centre, the low band at the back; the front two
        // let it show through.
        WaveformStyle::Frequency => {
            let [low_colour, mid_colour, high_colour] = layers(bands);
            let mut count = 0;
            for (level, colour) in [(low, low_colour), (mid, mid_colour), (high, high_colour)] {
                if level == 0 {
                    continue;
                }
                out[count] = mirrored(colour, 0.5, 0.5, level);
                count += 1;
            }
            (out, count)
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
                (high, high_colour, third / 2.0),
                (mid, mid_colour, 0.5),
                (low, low_colour, 1.0 - third / 2.0),
            ] {
                if level == 0 {
                    continue;
                }
                out[count] = mirrored(anchor, centre, third / 2.0, level);
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
    use crate::view::WaveformPalette;
    use crate::waveform::colour::band_colours;

    fn rgb_bands() -> [Color32; 3] {
        band_colours(WaveformPalette::Rgb, &Palette::DARK)
    }

    fn deck(index: usize) -> DeckId {
        DeckId::new(index).unwrap()
    }

    #[test]
    fn bands_split_the_lane_in_three() {
        let p = Palette::DARK;
        let bass = column(
            WaveformStyle::Bands,
            [255, 255, 0, 0],
            deck(0),
            &p,
            rgb_bands(),
        );
        assert!(!bass.is_empty());
        assert!(
            bass.iter().all(|b| b.from >= 0.66 && b.to <= 1.0),
            "{bass:?}"
        );
        let highs = column(
            WaveformStyle::Bands,
            [255, 0, 0, 255],
            deck(0),
            &p,
            rgb_bands(),
        );
        assert!(!highs.is_empty());
        assert!(
            highs.iter().all(|b| b.from >= 0.0 && b.to <= 0.34),
            "{highs:?}"
        );
    }

    #[test]
    fn deck_colour_uses_the_accent() {
        let p = Palette::DARK;
        let bars = column(
            WaveformStyle::DeckColour,
            [200, 10, 10, 10],
            deck(0),
            &p,
            rgb_bands(),
        );
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].colour, p.accent_left);
        let bars = column(
            WaveformStyle::DeckColour,
            [200, 10, 10, 10],
            deck(1),
            &p,
            rgb_bands(),
        );
        assert_eq!(bars[0].colour, p.accent_right);
    }

    #[test]
    fn frequency_draws_three_layers_back_to_front() {
        let p = Palette::DARK;
        let [low, mid, high] = layers(rgb_bands());
        let bars = column(
            WaveformStyle::Frequency,
            [255, 200, 100, 50],
            deck(0),
            &p,
            rgb_bands(),
        );
        assert_eq!(bars.len(), 3);
        for (bar, (colour, level)) in bars.iter().zip([
            (low, 200.0 / 255.0),
            (mid, 100.0 / 255.0),
            (high, 50.0 / 255.0),
        ]) {
            assert_eq!(bar.colour, colour);
            assert!((bar.from - (0.5 - 0.5 * level)).abs() < 1e-6, "{bar:?}");
            assert!((bar.to - (0.5 + 0.5 * level)).abs() < 1e-6, "{bar:?}");
        }
    }

    #[test]
    fn frequency_layers_blend() {
        // Low is opaque; mid and high let the layers behind them show through.
        let p = Palette::DARK;
        let bars = column(
            WaveformStyle::Frequency,
            [255, 200, 100, 50],
            deck(0),
            &p,
            rgb_bands(),
        );
        let alphas: Vec<u8> = bars.iter().map(|b| b.colour.a()).collect();
        assert_eq!(alphas[0], 255, "{bars:?}");
        assert!(alphas[1] < 255 && alphas[2] < 255, "{bars:?}");
        // A silent mid leaves the high its own alpha.
        let bars = column(
            WaveformStyle::Frequency,
            [255, 200, 0, 50],
            deck(0),
            &p,
            rgb_bands(),
        );
        assert_eq!(bars[1].colour.a(), alphas[2], "{bars:?}");
    }

    #[test]
    fn frequency_skips_silent_bands() {
        let p = Palette::DARK;
        let bars = column(
            WaveformStyle::Frequency,
            [255, 255, 0, 80],
            deck(0),
            &p,
            rgb_bands(),
        );
        assert_eq!(bars.len(), 2);
        let bars = column(
            WaveformStyle::Frequency,
            [255, 0, 80, 0],
            deck(0),
            &p,
            rgb_bands(),
        );
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].colour, layers(rgb_bands())[1]);
    }

    #[test]
    fn bands_use_the_palette() {
        let p = Palette::DARK;
        let bands = band_colours(WaveformPalette::OrangeYellowCyan, &p);
        let bars = column(
            WaveformStyle::Bands,
            [255, 255, 255, 255],
            deck(0),
            &p,
            bands,
        );
        let colours: Vec<_> = bars.iter().map(|b| b.colour).collect();
        assert_eq!(colours, [bands[2], bands[1], bands[0]]);
    }

    #[test]
    fn overlaid_is_the_accent_at_reduced_alpha() {
        let p = Palette::DARK;
        let bars = column(
            WaveformStyle::Overlaid,
            [255, 1, 1, 1],
            deck(0),
            &p,
            rgb_bands(),
        );
        assert_eq!(bars[0].colour, p.accent_left.gamma_multiply(OVERLAY_ALPHA));
    }
}
