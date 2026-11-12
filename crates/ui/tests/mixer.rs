//! The mixer draws headless, without a GPU.

use egui::epaint::{CircleShape, Shape};
use egui::{Rect, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use engine::{DeckId, EqMode, StemKnobs};
use ui::deck::DeckReads;
use ui::layout::{Sections, sections};
use ui::mixer::{Mixer, mixer_layout};
use ui::{ControlFrame, Layout};

/// The two-deck mixer's area in the tests.
const MIXER: Rect = Rect {
    min: pos2(0.0, 0.0),
    max: pos2(360.0, 300.0),
};

/// Draws the two-deck mixer in [`MIXER`] with `sections`.
fn draw_with(frame: ControlFrame, stems_active: bool, sections: Sections) -> Harness<'static> {
    let mut reads = DeckReads::default();
    for (snapshot, _) in &mut reads.decks {
        snapshot.stems_active = stems_active;
    }
    let mut mixer = Mixer::default();
    let mut harness = Harness::builder()
        .with_size([380.0, 320.0])
        .build_ui(move |ui| {
            mixer.show(ui, MIXER, &frame, &reads, &sections);
        });
    harness.run_steps(3);
    harness
}

fn draw(frame: &ControlFrame, stems_active: bool) -> Harness<'static> {
    draw_with(frame.clone(), stems_active, sections(Layout::Pro))
}

#[test]
fn mixer_shows_knob_labels() {
    let frame = ControlFrame {
        eq_mode: EqMode::Stems,
        stem_knobs: StemKnobs::Vdj,
        ..ControlFrame::default()
    };
    let harness = draw(&frame, true);
    harness.get_all_by_label("KICK").next().unwrap();
    harness.get_all_by_label("HIHAT").next().unwrap();
    assert_eq!(harness.get_all_by_label("GAIN").count(), 2);
    let plain = draw(&frame, false);
    plain.get_all_by_label("LOW").next().unwrap();
}

#[test]
fn a_pending_pickup_leaves_the_labels_in_place() {
    let mut frame = ControlFrame::default();
    frame.ghosts[0][5] = Some(0.2);
    frame.ghosts[0][1] = Some(0.9);
    assert_eq!(draw(&frame, false).get_all_by_label("FILTER").count(), 2);
}

#[test]
fn starter_keeps_the_eq() {
    let harness = draw_with(ControlFrame::default(), false, sections(Layout::Starter));
    for label in ["GAIN", "LOW", "FILTER"] {
        assert_eq!(harness.get_all_by_label(label).count(), 2, "{label}");
    }
}

#[test]
fn without_the_eq_gain_and_filter_stay() {
    let hidden = Sections {
        mixer_eq: false,
        ..sections(Layout::Pro)
    };
    let harness = draw_with(ControlFrame::default(), false, hidden);
    assert_eq!(harness.get_all_by_label("GAIN").count(), 2);
    assert_eq!(harness.get_all_by_label("FILTER").count(), 2);
    assert!(harness.query_by_label("LOW").is_none());
}

/// Every circle `shapes` draw, nested ones included.
fn circles(shapes: impl IntoIterator<Item = Shape>, out: &mut Vec<CircleShape>) {
    for shape in shapes {
        match shape {
            Shape::Circle(circle) => out.push(circle),
            Shape::Vec(inner) => circles(inner, out),
            _ => {}
        }
    }
}

#[test]
fn ghosts_still_draw() {
    // The left side's deck 1 waits for its gain to come back to the middle.
    let mut frame = ControlFrame::default();
    frame.ghosts[0][0] = Some(0.5);
    let harness = draw(&frame, false);
    let cue = ui::theme::palette(&harness.ctx).cue;
    let layout = mixer_layout(MIXER, true, true);
    let gain = layout.sides[0].knobs[0];
    // At 0.5 the marker sits straight above the knob, just outside its rim.
    let at = gain - egui::vec2(0.0, layout.knob_radius + 3.0);
    let mut drawn = Vec::new();
    circles(
        harness.output().shapes.iter().map(|c| c.shape.clone()),
        &mut drawn,
    );
    let hollow = |c: &CircleShape| c.fill == egui::Color32::TRANSPARENT && c.stroke.color == cue;
    assert!(
        drawn
            .iter()
            .any(|c| hollow(c) && c.center.distance(at) < 1.0),
        "no hollow marker at {at:?} by the gain at {gain:?}"
    );
    // Without a ghost there is none.
    let plain = draw(&ControlFrame::default(), false);
    let mut drawn = Vec::new();
    circles(
        plain.output().shapes.iter().map(|c| c.shape.clone()),
        &mut drawn,
    );
    assert!(!drawn.iter().any(hollow));
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
