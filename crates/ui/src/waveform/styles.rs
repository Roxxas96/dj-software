//! The coloured bars one waveform column is made of, in each display style.

use egui::Color32;
use engine::DeckId;
use engine::mixer::{eq_gain, trim_gain};

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

/// How much each band of a deck's waveform is scaled by: its EQ (or stem) knob times the
/// gain, as the engine applies them, so a killed band disappears and a boosted one grows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandScale {
    pub low: f32,
    pub mid: f32,
    pub high: f32,
}

impl BandScale {
    /// Every band as recorded.
    pub const UNITY: BandScale = BandScale {
        low: 1.0,
        mid: 1.0,
        high: 1.0,
    };

    /// The scale of the largest band, which a one-colour style scales its peak by.
    pub fn loudest(self) -> f32 {
        self.low.max(self.mid).max(self.high)
    }

    /// `level` scaled by `factor`, kept inside the lane (255).
    pub fn apply(level: u8, factor: f32) -> u8 {
        (f32::from(level) * factor).round().clamp(0.0, 255.0) as u8
    }
}

/// The scale of a deck's bands from its knobs, each 0 to 1: the `trim` gain times each EQ
/// (or stem) position, `eq_high` the top knob, `eq_low` the bottom one.
pub fn band_scale(trim: f32, eq_high: f32, eq_mid: f32, eq_low: f32) -> BandScale {
    let gain = trim_gain(trim);
    BandScale {
        low: eq_gain(eq_low) * gain,
        mid: eq_gain(eq_mid) * gain,
        high: eq_gain(eq_high) * gain,
    }
}

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
/// `bands` are the `[low, mid, high]` colours ([`super::colour::band_colours`]) and `scale`
/// the deck's knobs ([`band_scale`]); a bar is `scale` times as tall, up to the lane's.
pub fn column(
    style: WaveformStyle,
    bucket: [u8; 4],
    deck: DeckId,
    palette: &Palette,
    bands: [Color32; 3],
    scale: BandScale,
) -> Vec<Bar> {
    let (bars, count) = bars(style, bucket, deck, palette, bands, scale);
    bars[..count].to_vec()
}

/// [`column`] without the allocation: the bars and how many of them are used.
pub(super) fn bars(
    style: WaveformStyle,
    bucket: [u8; 4],
    deck: DeckId,
    palette: &Palette,
    bands: [Color32; 3],
    scale: BandScale,
) -> ([Bar; MAX_BARS], usize) {
    let none = Bar {
        colour: Color32::TRANSPARENT,
        from: 0.5,
        to: 0.5,
    };
    let mut out = [none; MAX_BARS];
    let [peak, low, mid, high] = bucket;
    let bucket_bands = [low, mid, high];
    let low = BandScale::apply(low, scale.low);
    let mid = BandScale::apply(mid, scale.mid);
    let high = BandScale::apply(high, scale.high);
    let peak = one_colour_level(peak, [low, mid, high], bucket_bands, scale);
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

/// The level of a one-colour style's bar: `peak` as far down as the loudest band has gone,
/// the scaled `bands` over the raw `raw` ones. With no raw band to compare, the largest scale.
fn one_colour_level(peak: u8, scaled: [u8; 3], raw: [u8; 3], scale: BandScale) -> u8 {
    let scaled_max = scaled.into_iter().max().unwrap_or(0);
    let raw_max = raw.into_iter().max().unwrap_or(0);
    if raw_max == 0 {
        return BandScale::apply(peak, scale.loudest());
    }
    let level = f32::from(peak) * f32::from(scaled_max) / f32::from(raw_max);
    level.round().clamp(0.0, 255.0) as u8
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
            BandScale::UNITY,
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
            BandScale::UNITY,
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
            BandScale::UNITY,
        );
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].colour, p.accent_left);
        let bars = column(
            WaveformStyle::DeckColour,
            [200, 10, 10, 10],
            deck(1),
            &p,
            rgb_bands(),
            BandScale::UNITY,
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
            BandScale::UNITY,
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
            BandScale::UNITY,
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
            BandScale::UNITY,
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
            BandScale::UNITY,
        );
        assert_eq!(bars.len(), 2);
        let bars = column(
            WaveformStyle::Frequency,
            [255, 0, 80, 0],
            deck(0),
            &p,
            rgb_bands(),
            BandScale::UNITY,
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
            BandScale::UNITY,
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
            BandScale::UNITY,
        );
        assert_eq!(bars[0].colour, p.accent_left.gamma_multiply(OVERLAY_ALPHA));
    }

    /// The knob position where `trim_gain` is a half (about -6 dB).
    const HALF_GAIN_KNOB: f32 = 0.5 - 6.0206 / 24.0;

    fn reach(bars: &[Bar]) -> Vec<f32> {
        bars.iter().map(|b| b.to - b.from).collect()
    }

    #[test]
    fn a_killed_band_disappears() {
        let p = Palette::DARK;
        let scale = band_scale(0.5, 0.5, 0.0, 0.5);
        let bars = column(
            WaveformStyle::Frequency,
            [255, 200, 200, 200],
            deck(0),
            &p,
            rgb_bands(),
            scale,
        );
        // Low and high stay; the mid is gone.
        assert_eq!(bars.len(), 2, "{bars:?}");
        let bars = column(
            WaveformStyle::Bands,
            [255, 200, 200, 200],
            deck(0),
            &p,
            rgb_bands(),
            scale,
        );
        assert_eq!(bars.len(), 2, "{bars:?}");
    }

    #[test]
    fn gain_scales_every_band() {
        let p = Palette::DARK;
        let half = band_scale(HALF_GAIN_KNOB, 0.5, 0.5, 0.5);
        for style in [WaveformStyle::Frequency, WaveformStyle::Bands] {
            let full = column(
                style,
                [200, 200, 200, 200],
                deck(0),
                &p,
                rgb_bands(),
                BandScale::UNITY,
            );
            let halved = column(style, [200, 200, 200, 200], deck(0), &p, rgb_bands(), half);
            assert_eq!(full.len(), halved.len());
            for (f, h) in reach(&full).iter().zip(reach(&halved)) {
                assert!((h - f / 2.0).abs() < 0.01, "{style:?} {f} {h}");
            }
        }
    }

    #[test]
    fn boosted_bands_stay_inside_the_lane() {
        let p = Palette::DARK;
        let boost = band_scale(1.0, 1.0, 1.0, 1.0);
        for style in [
            WaveformStyle::Frequency,
            WaveformStyle::DeckColour,
            WaveformStyle::Bands,
            WaveformStyle::Overlaid,
        ] {
            let bars = column(style, [255, 255, 255, 255], deck(0), &p, rgb_bands(), boost);
            assert!(!bars.is_empty());
            for bar in bars {
                assert!(bar.from >= 0.0 && bar.to <= 1.0, "{style:?} {bar:?}");
            }
        }
    }

    #[test]
    fn a_one_colour_style_shrinks_with_any_killed_band() {
        let p = Palette::DARK;
        // Each bucket is heavy in one band; that band's knob is killed.
        let cases = [
            ([255, 200, 10, 10], band_scale(0.5, 0.5, 0.5, 0.0)),
            ([255, 10, 200, 10], band_scale(0.5, 0.5, 0.0, 0.5)),
            ([255, 10, 10, 200], band_scale(0.5, 0.0, 0.5, 0.5)),
        ];
        for style in [WaveformStyle::DeckColour, WaveformStyle::Overlaid] {
            for (bucket, scale) in cases {
                let bars = column(style, bucket, deck(0), &p, rgb_bands(), scale);
                let reach = reach(&bars).first().copied().unwrap_or(0.0);
                assert!(reach < 0.2, "{style:?} {bucket:?} {reach}");
                let flat = column(style, bucket, deck(0), &p, rgb_bands(), BandScale::UNITY);
                assert!((reach_of(&flat) - 1.0).abs() < 1e-6, "{flat:?}");
            }
        }
        // All bands killed: nothing shows.
        let bars = column(
            WaveformStyle::DeckColour,
            [255, 200, 200, 200],
            deck(0),
            &p,
            rgb_bands(),
            band_scale(0.5, 0.0, 0.0, 0.0),
        );
        assert!(bars.is_empty() || reach(&bars)[0] == 0.0, "{bars:?}");
        // A bass-only boost still shows the bar in full.
        let bars = column(
            WaveformStyle::Overlaid,
            [255, 200, 200, 200],
            deck(0),
            &p,
            rgb_bands(),
            band_scale(0.5, 0.0, 0.0, 0.5),
        );
        assert!((reach(&bars)[0] - 1.0).abs() < 1e-6, "{bars:?}");
    }

    fn reach_of(bars: &[Bar]) -> f32 {
        reach(bars)[0]
    }
}
