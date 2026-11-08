//! Open Jockey's colours: a dark palette in the VirtualDJ style, our own values.

use actions::Side;
use egui::{Color32, Context, Theme};
use engine::DeckId;

pub const BACKGROUND: Color32 = Color32::from_rgb(0x1e, 0x1f, 0x22);
pub const PANEL: Color32 = Color32::from_rgb(0x2a, 0x2b, 0x2f);
pub const TEXT: Color32 = Color32::from_rgb(0xd6, 0xd6, 0xd6);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x8a, 0x8c, 0x91);
/// The left side's decks (1 and 3).
pub const DECK_LEFT: Color32 = Color32::from_rgb(0x2f, 0xa6, 0xd9);
/// The right side's decks (2 and 4).
pub const DECK_RIGHT: Color32 = Color32::from_rgb(0xe2, 0x38, 0x4b);
/// A bar's first beat on a waveform.
pub const BAR_TICK: Color32 = Color32::from_rgb(0xff, 0x3b, 0x3b);
/// Every other beat on a waveform.
pub const BEAT_TICK: Color32 = Color32::from_rgb(0x5b, 0x8f, 0xd6);
/// Behind a waveform lane.
pub const LANE: Color32 = Color32::from_rgb(0x12, 0x13, 0x15);
/// The line at a lane's centre: what is being heard.
pub const PLAYHEAD: Color32 = Color32::WHITE;
/// The cue point's triangle.
pub const CUE: Color32 = Color32::from_rgb(0xff, 0xb3, 0x2e);
/// A hotcue's flag.
pub const HOTCUE: Color32 = Color32::from_rgb(0x3d, 0xd6, 0x8c);
/// An active loop, over the waveform (translucent green, premultiplied).
pub const LOOP_BAND: Color32 = Color32::from_rgba_premultiplied(0x13, 0x30, 0x1a, 0x40);

/// An error that stopped the session.
pub const ERROR: Color32 = Color32::from_rgb(0xff, 0x5c, 0x5c);

/// A browser row under the cursor of the pane that has focus.
pub const HIGHLIGHT: Color32 = Color32::from_rgb(0x2a, 0x5f, 0x8a);
/// A browser row under the cursor of the pane without focus.
pub const HIGHLIGHT_DIM: Color32 = Color32::from_rgb(0x38, 0x3b, 0x42);

/// The accent of `deck`'s side: [`DECK_LEFT`] or [`DECK_RIGHT`].
pub fn deck_accent(deck: DeckId) -> Color32 {
    match Side::of(deck) {
        Side::Left => DECK_LEFT,
        Side::Right => DECK_RIGHT,
    }
}

/// Makes `ctx` dark, with this palette.
pub fn apply(ctx: &Context) {
    ctx.set_theme(Theme::Dark);
    ctx.style_mut_of(Theme::Dark, |style| {
        let visuals = &mut style.visuals;
        visuals.panel_fill = BACKGROUND;
        visuals.window_fill = PANEL;
        visuals.extreme_bg_color = BACKGROUND;
        visuals.faint_bg_color = PANEL;
        visuals.override_text_color = Some(TEXT);
    });
}
