//! Open Jockey's colours: a dark and a light palette in the VirtualDJ style, our own
//! values. The root stores the scheme's [`Palette`] in the context each frame
//! ([`apply`]); every draw reads it back with [`palette`].

use actions::Side;
use egui::{Color32, Context, Id, Theme};
use engine::DeckId;

use crate::view::ColourScheme;

/// Every colour the window draws with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub background: Color32,
    pub panel: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    /// The left side's decks (1 and 3).
    pub accent_left: Color32,
    /// The right side's decks (2 and 4).
    pub accent_right: Color32,
    /// A bar's first beat on a waveform.
    pub bar_tick: Color32,
    /// Every other beat on a waveform.
    pub beat_tick: Color32,
    /// Behind a waveform lane.
    pub lane: Color32,
    /// The line at a lane's centre: what is being heard.
    pub playhead: Color32,
    /// The cue point's triangle.
    pub cue: Color32,
    /// A hotcue's flag.
    pub hotcue: Color32,
    /// An active loop, over the waveform (translucent green, premultiplied).
    pub loop_band: Color32,
    /// An error that stopped the session.
    pub error: Color32,
    /// A browser row under the cursor of the pane that has focus.
    pub highlight: Color32,
    /// A browser row under the cursor of the pane without focus.
    pub highlight_dim: Color32,
    /// A meter's level below the clip point.
    pub meter_ok: Color32,
    /// A meter's level above the clip point.
    pub meter_clip: Color32,
    /// A lit play button.
    pub play_lit: Color32,
    /// The music note of a track without a cover.
    pub note: Color32,
    /// Text on a lit (accent-coloured) button.
    pub on_accent: Color32,
}

impl Palette {
    pub const DARK: Self = Self {
        background: Color32::from_rgb(0x1e, 0x1f, 0x22),
        panel: Color32::from_rgb(0x2a, 0x2b, 0x2f),
        text: Color32::from_rgb(0xd6, 0xd6, 0xd6),
        text_dim: Color32::from_rgb(0x8a, 0x8c, 0x91),
        accent_left: Color32::from_rgb(0x2f, 0xa6, 0xd9),
        accent_right: Color32::from_rgb(0xe2, 0x38, 0x4b),
        bar_tick: Color32::from_rgb(0xff, 0x3b, 0x3b),
        beat_tick: Color32::from_rgb(0x5b, 0x8f, 0xd6),
        lane: Color32::from_rgb(0x12, 0x13, 0x15),
        playhead: Color32::WHITE,
        cue: Color32::from_rgb(0xff, 0xb3, 0x2e),
        hotcue: Color32::from_rgb(0x3d, 0xd6, 0x8c),
        loop_band: Color32::from_rgba_premultiplied(0x13, 0x30, 0x1a, 0x40),
        error: Color32::from_rgb(0xff, 0x5c, 0x5c),
        highlight: Color32::from_rgb(0x2a, 0x5f, 0x8a),
        highlight_dim: Color32::from_rgb(0x38, 0x3b, 0x42),
        meter_ok: Color32::from_rgb(0x4c, 0xc2, 0x6a),
        meter_clip: Color32::from_rgb(0xff, 0x3b, 0x3b),
        play_lit: Color32::from_rgb(0x4c, 0xc2, 0x6a),
        note: Color32::from_gray(0x55),
        on_accent: Color32::BLACK,
    };

    /// Light grey, VirtualDJ's other look. The accents, ticks and waveform colours keep
    /// their hues.
    pub const LIGHT: Self = Self {
        background: Color32::from_rgb(0xe6, 0xe6, 0xe6),
        panel: Color32::from_rgb(0xf4, 0xf4, 0xf4),
        text: Color32::from_rgb(0x1e, 0x1f, 0x22),
        text_dim: Color32::from_rgb(0x5f, 0x61, 0x66),
        lane: Color32::from_rgb(0xd8, 0xd8, 0xd8),
        // Darker than the dark scheme's, to read on the light lane.
        playhead: Color32::from_rgb(0x1e, 0x1f, 0x22),
        cue: Color32::from_rgb(0xe0, 0x8a, 0x00),
        hotcue: Color32::from_rgb(0x1f, 0xa8, 0x66),
        loop_band: Color32::from_rgba_premultiplied(0x00, 0x40, 0x10, 0x40),
        highlight: Color32::from_rgb(0x9c, 0xc6, 0xe8),
        highlight_dim: Color32::from_rgb(0xcf, 0xd2, 0xd8),
        note: Color32::from_gray(0xa0),
        ..Self::DARK
    };

    /// The palette of `scheme`.
    pub const fn of(scheme: ColourScheme) -> Self {
        match scheme {
            ColourScheme::Dark => Self::DARK,
            ColourScheme::Light => Self::LIGHT,
        }
    }

    /// The accent of `deck`'s side.
    pub fn deck_accent(&self, deck: DeckId) -> Color32 {
        match Side::of(deck) {
            Side::Left => self.accent_left,
            Side::Right => self.accent_right,
        }
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::DARK
    }
}

fn id() -> Id {
    Id::NULL.with("open-jockey-palette")
}

/// The palette [`apply`] stored in `ctx`; the dark one before any.
pub fn palette(ctx: &Context) -> Palette {
    ctx.data(|data| data.get_temp(id())).unwrap_or_default()
}

/// Makes `ctx` dark or light, with the palette of `scheme`, and stores that palette for
/// [`palette`].
pub fn apply(ctx: &Context, scheme: ColourScheme) {
    let palette = Palette::of(scheme);
    let before: Option<Palette> = ctx.data(|data| data.get_temp(id()));
    if before == Some(palette) {
        return;
    }
    ctx.data_mut(|data| data.insert_temp(id(), palette));
    let theme = match scheme {
        ColourScheme::Dark => Theme::Dark,
        ColourScheme::Light => Theme::Light,
    };
    ctx.set_theme(theme);
    ctx.style_mut_of(theme, |style| {
        let visuals = &mut style.visuals;
        visuals.panel_fill = palette.background;
        visuals.window_fill = palette.panel;
        visuals.extreme_bg_color = palette.background;
        visuals.faint_bg_color = palette.panel;
        visuals.override_text_color = Some(palette.text);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palettes_differ_and_keep_accents() {
        assert_ne!(Palette::DARK.background, Palette::LIGHT.background);
        assert_eq!(Palette::DARK.accent_left, Palette::LIGHT.accent_left);
    }
}
