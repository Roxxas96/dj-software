//! The top bar: the status line (or the error that stopped the session) on the left; the master meter, the CPU load, the clock and
//! the controller and audio state on the right.

use std::sync::mpsc::Sender;
use std::time::Instant;

use egui::{CornerRadius, Rect, RichText, Sense, Ui, Vec2, pos2};
use engine::EngineMonitor;

use crate::command::UiCommand;
use crate::menu;
use crate::model::ControlFrame;
use crate::theme;

/// One channel's bar of the master meter.
const METER_BAR: Vec2 = Vec2::new(60.0, 5.0);
/// Above this peak the meter turns red.
const METER_HOT: f32 = 0.95;

pub fn show(
    ui: &mut Ui,
    frame: &ControlFrame,
    monitor: Option<&EngineMonitor>,
    commands: &Sender<UiCommand>,
) {
    let p = theme::palette(ui.ctx());
    ui.horizontal(|ui| {
        menu::show(ui, &frame.view, commands);
        ui.separator();
        match &frame.error {
            Some(error) => ui.label(RichText::new(error.as_str()).color(p.error).strong()),
            None => ui.label(frame.status_line(Instant::now())),
        };
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            match &frame.audio {
                Ok(device) => ui.label(device.as_str()),
                Err(why) => ui.label(RichText::new(why.as_str()).color(p.text_dim)),
            };
            ui.separator();
            match &frame.controller {
                Some(port) => ui.label(port.as_str()),
                None => ui.label(RichText::new("no controller").color(p.text_dim)),
            };
            ui.separator();
            ui.label(clock_text());
            ui.separator();
            ui.label(cpu_text(monitor.map(EngineMonitor::dsp_load)));
            ui.separator();
            let peaks = monitor.map_or([0.0; 2], |m| m.meters().master_peak);
            master_meter(ui, peaks);
        });
    });
}

/// `CPU 37%`: the audio thread's share of each block's time; `CPU –` before audio opens.
fn cpu_text(dsp_load: Option<f32>) -> String {
    match dsp_load {
        Some(load) => format!("CPU {:.0}%", load * 100.0),
        None => "CPU –".to_owned(),
    }
}

/// The local time as `HH:MM:SS`.
fn clock_text() -> String {
    jiff::Zoned::now().strftime("%H:%M:%S").to_string()
}

/// Two stacked bars, left over right, filled to each master peak.
fn master_meter(ui: &mut Ui, peaks: [f32; 2]) {
    let p = theme::palette(ui.ctx());
    let gap = 2.0;
    let size = Vec2::new(METER_BAR.x, METER_BAR.y * 2.0 + gap);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    for (row, peak) in peaks.into_iter().enumerate() {
        let top = rect.top() + row as f32 * (METER_BAR.y + gap);
        let bar = Rect::from_min_size(pos2(rect.left(), top), METER_BAR);
        painter.rect_filled(bar, CornerRadius::ZERO, p.background);
        let level = peak.clamp(0.0, 1.0);
        let filled = Rect::from_min_size(bar.min, Vec2::new(bar.width() * level, bar.height()));
        let colour = if peak > METER_HOT {
            p.meter_clip
        } else {
            p.meter_ok
        };
        painter.rect_filled(filled, CornerRadius::ZERO, colour);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_reads_as_a_percentage() {
        assert_eq!(cpu_text(Some(0.374)), "CPU 37%");
        assert_eq!(cpu_text(Some(1.2)), "CPU 120%");
        assert_eq!(cpu_text(None), "CPU –");
    }

    #[test]
    fn the_clock_has_hours_minutes_and_seconds() {
        let text = clock_text();
        assert_eq!(text.len(), 8, "{text}");
        assert_eq!(text.as_bytes()[2], b':');
        assert_eq!(text.as_bytes()[5], b':');
    }
}
