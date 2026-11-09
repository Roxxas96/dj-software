//! The sideview: the sampler bank beside the browser, 8 slots in a 2x4 grid. Display only.

use egui::{CornerRadius, Label, Rect, RichText, Ui, pos2, vec2};
use engine::{EngineMonitor, SAMPLER_SLOTS, SamplerSnapshot};

use crate::ControlFrame;
use crate::theme;

const COLUMNS: usize = 2;
const GAP: f32 = 6.0;
const HEIGHT: f32 = 56.0;

/// Draws the sampler slots into `ui`'s width: lit when a sample is loaded, brighter while
/// it plays. `monitor` is `None` until the audio device opens.
pub fn show(ui: &mut Ui, frame: &ControlFrame, monitor: Option<&EngineMonitor>) {
    let p = theme::palette(ui.ctx());
    let playing = monitor.map(EngineMonitor::sampler).unwrap_or_default();
    ui.label(RichText::new("SAMPLER").color(p.text_dim));
    ui.add_space(GAP);
    let area = ui.available_rect_before_wrap();
    let width = ((area.width() - GAP * (COLUMNS as f32 - 1.0)) / COLUMNS as f32).max(0.0);
    for slot in 0..SAMPLER_SLOTS {
        let at = pos2(
            area.min.x + (slot % COLUMNS) as f32 * (width + GAP),
            area.min.y + (slot / COLUMNS) as f32 * (HEIGHT + GAP),
        );
        let cell = Rect::from_min_size(at, vec2(width, HEIGHT));
        let (fill, text) = colours(&p, frame.samples_loaded[slot], &playing, slot);
        ui.painter().rect_filled(cell, CornerRadius::same(3), fill);
        ui.put(
            cell,
            Label::new(RichText::new((slot + 1).to_string()).size(18.0).color(text)),
        );
    }
}

fn colours(
    p: &theme::Palette,
    loaded: bool,
    playing: &SamplerSnapshot,
    slot: usize,
) -> (egui::Color32, egui::Color32) {
    if playing.playing[slot] {
        (p.play_lit, p.on_accent)
    } else if loaded {
        (p.highlight, p.text)
    } else {
        (p.panel, p.text_dim)
    }
}
