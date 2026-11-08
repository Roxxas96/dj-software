//! The mixer draws headless, without a GPU.

use egui::{Rect, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use engine::{DeckId, EqMode, StemKnobs};
use ui::ControlFrame;

fn draw(frame: &ControlFrame, stems_active: bool) -> Harness<'_> {
    let mut harness = Harness::builder()
        .with_size([340.0, 300.0])
        .build_ui(move |ui| {
            let rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(170.0, 300.0));
            ui::mixer::strip(ui, rect, DeckId::new(0).unwrap(), frame, stems_active, 0.5);
        });
    harness.run_steps(3);
    harness
}

#[test]
fn mixer_shows_knob_labels() {
    let frame = ControlFrame {
        eq_mode: EqMode::Stems,
        stem_knobs: StemKnobs::Vdj,
        ..ControlFrame::default()
    };
    let harness = draw(&frame, true);
    harness.get_by_label("KICK");
    harness.get_by_label("HIHAT");
    harness.get_by_label("GAIN");
    let plain = draw(&frame, false);
    plain.get_by_label("LOW");
}

#[test]
fn a_pending_pickup_leaves_the_labels_in_place() {
    let mut frame = ControlFrame::default();
    frame.ghosts[0][5] = Some(0.2);
    frame.ghosts[0][1] = Some(0.9);
    draw(&frame, false).get_by_label("FILTER");
}
