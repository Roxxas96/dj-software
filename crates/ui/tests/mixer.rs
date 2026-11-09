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

#[test]
fn a_narrow_strip_keeps_its_name_off_the_headphone_button() {
    // A strip of the 4-Decks mixer at 1024 wide.
    let frame = ControlFrame {
        view: ui::ViewState {
            four_decks: true,
            ..ui::ViewState::default()
        },
        ..ControlFrame::default()
    };
    let rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(82.0, 224.0));
    let mut harness = Harness::builder()
        .with_size([200.0, 300.0])
        .build_ui(move |ui| {
            ui::mixer::strip(
                ui,
                rect,
                DeckId::new(0).unwrap(),
                &frame,
                false,
                (0.5, 0.7),
                &ui::layout::four_deck_sections(),
            );
        });
    harness.run_steps(3);
    let layout = ui::mixer::strip_layout(rect, 5, true);
    let name = harness.get_by_label("DECK 1").rect();
    assert!(
        layout.name.expand(0.5).contains_rect(name),
        "{name:?} {layout:?}"
    );
    assert!(!name.intersects(layout.pfl), "{name:?} {layout:?}");
    assert!(
        layout
            .pfl
            .expand(0.5)
            .contains_rect(harness.get_by_label("PFL").rect())
    );
}
