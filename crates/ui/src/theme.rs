//! Open Jockey's colours: a dark and a light palette in the VirtualDJ style, our own
//! values. The root stores the scheme's [`Palette`] in the context each frame
//! ([`apply`]); every draw reads it back with [`palette`].

use std::f32::consts::FRAC_PI_2;

use actions::Side;
use egui::epaint::TextShape;
use egui::{Color32, Context, FontId, Id, Rect, Role, Sense, Stroke, Theme, Ui, WidgetInfo, pos2};
use engine::DeckId;

use crate::view::ColourScheme;

/// The space between neighbouring sections (the rows, the deck panels and the mixer), in
/// points: the background shows through it, as the sections have no outline.
pub const SECTION_GAP: f32 = 2.0;
/// The width of the strip an [`edge_label`] is drawn in, in points.
pub const EDGE_LABEL_WIDTH: f32 = 14.0;
/// An edge label's text size.
const EDGE_LABEL_FONT: f32 = 10.0;

/// Every colour the window draws with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub background: Color32,
    pub panel: Color32,
    /// Every other row of a list: a shade between [`Self::background`] and [`Self::panel`].
    pub row_alt: Color32,
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
    /// The colour of a waveform's high frequencies.
    pub wave_high: Color32,
}

impl Palette {
    pub const DARK: Self = Self {
        background: Color32::from_rgb(0x1e, 0x1f, 0x22),
        panel: Color32::from_rgb(0x2a, 0x2b, 0x2f),
        row_alt: Color32::from_rgb(0x24, 0x25, 0x28),
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
        wave_high: Color32::WHITE,
    };

    /// Light grey, VirtualDJ's other look. The accents, ticks and the waveforms' bass and
    /// mids keep their hues; the highs and the error text are darker, to read on light grey.
    pub const LIGHT: Self = Self {
        background: Color32::from_rgb(0xe6, 0xe6, 0xe6),
        panel: Color32::from_rgb(0xf4, 0xf4, 0xf4),
        row_alt: Color32::from_rgb(0xed, 0xed, 0xed),
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
        error: Color32::from_rgb(0xc4, 0x1e, 0x1e),
        wave_high: Color32::from_rgb(0x3e, 0x4c, 0x5e),
        ..Self::DARK
    };

    /// A hotcue's saved colour as it shows on this palette: the colour itself, darkened when
    /// it is too pale to read on a light background, or [`Self::hotcue`] without one.
    pub fn mark(&self, colour: Option<Color32>) -> Color32 {
        let Some(colour) = colour else {
            return self.hotcue;
        };
        if luminance(self.background) > 0.5 && luminance(colour) > PALE {
            let dim = |c: u8| (f32::from(c) * DARKEN) as u8;
            Color32::from_rgb(dim(colour.r()), dim(colour.g()), dim(colour.b()))
        } else {
            colour
        }
    }

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

/// A colour's brightness, 0.0 to 1.0.
pub(crate) fn luminance(colour: Color32) -> f32 {
    (0.299 * f32::from(colour.r()) + 0.587 * f32::from(colour.g()) + 0.114 * f32::from(colour.b()))
        / 255.0
}

/// The WCAG contrast ratio of `a` on `b`.
pub(crate) fn contrast(a: Color32, b: Color32) -> f32 {
    let luminance = |c: Color32| {
        let channel = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    };
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// White or black, whichever reads better on `fill`: at least 4.5:1 on any colour.
pub(crate) fn text_on(fill: Color32) -> Color32 {
    if contrast(Color32::WHITE, fill) >= contrast(Color32::BLACK, fill) {
        Color32::WHITE
    } else {
        Color32::BLACK
    }
}

/// The brightness above which a hotcue colour is too pale for a light background.
const PALE: f32 = 0.8;
/// How much of a pale colour is kept when it is darkened.
const DARKEN: f32 = 0.6;

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
        // Sections and idle widgets have no outline: fills on the background set them apart.
        visuals.widgets.noninteractive.bg_stroke = Stroke::NONE;
        visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    });
}

/// The dim line a separator draws in the menus, the settings window and the top bar.
pub fn separator_stroke(palette: &Palette) -> Stroke {
    Stroke::new(1.0, palette.text_dim.gamma_multiply(0.4))
}

/// Gives `ui`'s separators back their line ([`apply`] takes it away from the panels).
pub fn separators(ui: &mut Ui) {
    let stroke = separator_stroke(&palette(ui.ctx()));
    ui.visuals_mut().widgets.noninteractive.bg_stroke = stroke;
}

/// Draws `text` turned a quarter anticlockwise, reading upwards as VirtualDJ's section
/// names do, centred in `strip` (a strip [`EDGE_LABEL_WIDTH`] wide at a section's edge).
/// Screen readers get it as a label.
pub fn edge_label(ui: &Ui, strip: Rect, text: &str) {
    let p = palette(ui.ctx());
    let painter = ui.painter();
    let galley = painter.layout_no_wrap(
        text.to_owned(),
        FontId::proportional(EDGE_LABEL_FONT),
        p.text_dim,
    );
    let size = galley.size();
    // Turned about its top left corner: the text runs up from there, its lines to the right.
    let at = pos2(
        strip.center().x - size.y / 2.0,
        strip.center().y + size.x / 2.0,
    );
    painter.add(TextShape::new(at, galley, p.text_dim).with_angle(-FRAC_PI_2));
    let response = ui.interact(
        strip,
        ui.scope_id().with(("edge label", text)),
        Sense::hover(),
    );
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, text));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_text_and_highs_read_on_light_grey() {
        let light = Palette::LIGHT;
        for surface in [light.panel, light.background] {
            assert!(contrast(light.error, surface) >= 4.5, "{surface:?}");
        }
        assert!(contrast(light.wave_high, light.lane) >= 4.5);
        // The dark scheme keeps its own.
        assert_eq!(Palette::DARK.wave_high, Color32::WHITE);
    }

    #[test]
    fn light_scheme_keeps_section_shades() {
        let light = Palette::LIGHT;
        let (panel, background) = (luminance(light.panel), luminance(light.background));
        assert!(
            (panel - background) * 255.0 >= 8.0,
            "{panel} against {background}"
        );
        let alt = luminance(light.row_alt);
        assert!(alt > background && alt < panel, "{alt}");
    }

    #[test]
    fn no_strokes_on_panels_and_idle_widgets() {
        for (scheme, theme) in [
            (ColourScheme::Dark, Theme::Dark),
            (ColourScheme::Light, Theme::Light),
        ] {
            let ctx = Context::default();
            apply(&ctx, scheme);
            let widgets = ctx.style_of(theme).visuals.widgets.clone();
            assert_eq!(
                widgets.noninteractive.bg_stroke,
                egui::Stroke::NONE,
                "{scheme:?}"
            );
            assert_eq!(widgets.inactive.bg_stroke, egui::Stroke::NONE, "{scheme:?}");
        }
    }

    #[test]
    fn palettes_differ_and_keep_accents() {
        assert_ne!(Palette::DARK.background, Palette::LIGHT.background);
        assert_eq!(Palette::DARK.accent_left, Palette::LIGHT.accent_left);
    }
}
