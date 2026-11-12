//! The Interface page: the four layouts as tiles, each a sketch of the sections it shows.

use std::sync::mpsc::Sender;

use egui::{Align2, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use crate::command::{UiCommand, ViewChange};
use crate::layout::{self, LANES_SHARE};
use crate::menu::{FOUR_DECKS_HINT, layout_name, palette_name};
use crate::theme;
use crate::view::{Layout, ViewState, WaveformPalette};

const TILE: Vec2 = Vec2::new(170.0, 130.0);
/// The room under a tile's sketch for its name.
const NAME_HEIGHT: f32 = 22.0;

/// The tiles; while the 4-Decks view is on they cannot be chosen, and say why.
pub fn show(ui: &mut Ui, view: &ViewState, commands: &Sender<UiCommand>) {
    if view.four_decks {
        let dim = theme::palette(ui.ctx()).text_dim;
        ui.label(egui::RichText::new(FOUR_DECKS_HINT).color(dim));
    }
    ui.add_enabled_ui(!view.four_decks, |ui| {
        ui.horizontal_wrapped(|ui| {
            for layout in Layout::ALL {
                if tile(ui, *layout, view.layout == *layout) {
                    let _ = commands.send(UiCommand::View(ViewChange::SetLayout(*layout)));
                }
            }
        });
    });
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.label("Waveform colours");
        for palette in WaveformPalette::ALL {
            let selected = view.waveform_palette == *palette;
            if ui
                .selectable_label(selected, palette_name(*palette))
                .clicked()
            {
                let _ = commands.send(UiCommand::View(ViewChange::SetWaveformPalette(*palette)));
            }
        }
    });
}

/// One tile; whether it was clicked.
fn tile(ui: &mut Ui, layout: Layout, selected: bool) -> bool {
    let name = layout_name(layout);
    let (rect, response) = ui.allocate_exact_size(TILE, Sense::click());
    let response = response.on_disabled_hover_text(FOUR_DECKS_HINT);
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, name));
    let p = theme::palette(ui.ctx());
    let painter = ui.painter();
    let border = if selected {
        Stroke::new(2.0, p.highlight)
    } else if response.hovered() {
        Stroke::new(1.0, p.text_dim)
    } else {
        Stroke::new(1.0, p.lane)
    };
    painter.rect(rect, 4, p.panel, border, StrokeKind::Inside);
    let sketch = Rect::from_min_max(
        rect.min + vec2(8.0, 8.0),
        pos2(rect.max.x - 8.0, rect.max.y - NAME_HEIGHT),
    );
    sketch_of(ui, sketch, layout);
    painter.text(
        pos2(rect.center().x, rect.max.y - NAME_HEIGHT / 2.0),
        Align2::CENTER_CENTER,
        name,
        FontId::proportional(14.0),
        p.text,
    );
    response.clicked()
}

/// The window height the sketches' proportions are taken at: the default window's.
const SKETCH_WINDOW_HEIGHT: f32 = 900.0;

/// The share of the sketch the deck band takes in `layout`.
fn deck_band_share(layout: Layout) -> f32 {
    crate::deck::row_height(&layout::sections(layout)) / SKETCH_WINDOW_HEIGHT
}

/// A small drawing of the window in `layout`: the lanes, the deck panels with what they
/// show, the mixer between them, and the browser.
fn sketch_of(ui: &Ui, area: Rect, layout: Layout) {
    let p = theme::palette(ui.ctx());
    let painter = ui.painter();
    let sections = layout::sections(layout);
    let band = |from: f32, share: f32| {
        Rect::from_min_size(
            pos2(area.min.x, area.min.y + from * area.height()),
            vec2(area.width(), share * area.height() - 2.0),
        )
    };
    painter.rect_filled(
        band(0.0, LANES_SHARE),
        CornerRadius::ZERO,
        p.wave_background,
    );
    let decks_share = deck_band_share(layout);
    let decks = band(LANES_SHARE, decks_share);
    painter.rect_filled(
        band(LANES_SHARE + decks_share, 1.0 - LANES_SHARE - decks_share),
        CornerRadius::ZERO,
        p.background,
    );
    let mixer = decks.width() * 0.16;
    let side = (decks.width() - mixer) / 2.0;
    for (left, accent) in [
        (decks.min.x, p.accent_left),
        (decks.max.x - side, p.accent_right),
    ] {
        let deck = Rect::from_min_size(pos2(left, decks.min.y), vec2(side, decks.height()));
        painter.rect_filled(deck, CornerRadius::ZERO, p.background);
        let unit = deck.height() / 6.0;
        let mut y = deck.min.y + 2.0;
        let mut bar = |height: f32, width: f32, colour| {
            let r = Rect::from_min_size(pos2(deck.min.x + 2.0, y), vec2(width, height));
            painter.rect_filled(r, CornerRadius::ZERO, colour);
            y += height + 2.0;
        };
        if sections.overview {
            bar(unit * 0.6, side - 4.0, accent);
        }
        if sections.jog {
            let centre = pos2(deck.center().x, deck.center().y + unit * 0.4);
            painter.circle_stroke(centre, unit * 1.4, Stroke::new(1.0, p.text_dim));
        }
        if sections.pads {
            let pad = unit * 0.8;
            for i in 0..4 {
                let r = Rect::from_min_size(
                    pos2(
                        deck.min.x + 2.0 + i as f32 * (pad + 2.0),
                        deck.max.y - pad - 2.0,
                    ),
                    Vec2::splat(pad),
                );
                painter.rect_filled(r, CornerRadius::ZERO, p.text_dim);
            }
        }
        if sections.fx {
            let r = Rect::from_min_size(
                pos2(deck.max.x - unit * 2.0 - 2.0, deck.min.y + 2.0),
                vec2(unit * 2.0, unit * 0.6),
            );
            painter.rect_filled(r, CornerRadius::ZERO, p.text_dim);
        }
    }
    let strip = Rect::from_min_size(
        pos2(decks.center().x - mixer / 2.0, decks.min.y),
        vec2(mixer, decks.height()),
    );
    painter.rect_filled(strip, CornerRadius::ZERO, p.lane);
    if sections.mixer_eq {
        for i in 0..3 {
            let c = pos2(strip.center().x, strip.min.y + 5.0 + i as f32 * 7.0);
            painter.circle_filled(c, 2.0, p.text_dim);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sketch_deck_band_follows_the_window() {
        for layout in Layout::ALL {
            let sections = layout::sections(*layout);
            let want = crate::deck::row_height(&sections) / SKETCH_WINDOW_HEIGHT;
            let share = deck_band_share(*layout);
            assert!(
                (share - want).abs() <= 0.02,
                "{layout:?}: {share} against {want}"
            );
        }
    }
}
