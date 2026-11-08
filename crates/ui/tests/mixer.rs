//! The mixer draws headless, without a GPU.

use egui::{Rect, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use engine::{DeckId, EqMode, StemKnobs};
use ui::layout::{Sections, sections};
use ui::{ControlFrame, Layout};

fn draw(frame: &ControlFrame, stems_active: bool) -> Harness<'_> {
    draw_in(frame, stems_active, Layout::Pro)
}

fn draw_in(frame: &ControlFrame, stems_active: bool, layout: Layout) -> Harness<'_> {
    let mut harness = Harness::builder()
        .with_size([340.0, 300.0])
        .build_ui(move |ui| {
            let rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(170.0, 300.0));
            ui::mixer::strip(
                ui,
                rect,
                DeckId::new(0).unwrap(),
                frame,
                stems_active,
                (0.5, 0.7),
                &sections(layout),
            );
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

#[test]
fn starter_keeps_the_eq() {
    let frame = ControlFrame::default();
    let harness = draw_in(&frame, false, Layout::Starter);
    harness.get_by_label("GAIN");
    harness.get_by_label("LOW");
    harness.get_by_label("FILTER");
}

#[test]
fn without_the_eq_gain_and_filter_stay() {
    let frame = ControlFrame::default();
    let hidden = Sections {
        mixer_eq: false,
        ..sections(Layout::Pro)
    };
    let mut harness = Harness::builder()
        .with_size([340.0, 300.0])
        .build_ui(move |ui| {
            let rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(170.0, 300.0));
            ui::mixer::strip(
                ui,
                rect,
                DeckId::new(0).unwrap(),
                &frame,
                false,
                (0.5, 0.7),
                &hidden,
            );
        });
    harness.run_steps(3);
    harness.get_by_label("GAIN");
    harness.get_by_label("FILTER");
    assert!(harness.query_by_label("LOW").is_none());
}
