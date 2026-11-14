//! The 4-Decks view draws headless, without a GPU.

use std::path::PathBuf;
use std::sync::mpsc::channel;

use actions::{Action, Input};
use egui::epaint::{CircleShape, Shape};
use egui::{Rect, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use engine::{ChannelSnapshot, DeckId, DeckSnapshot, SamplerSnapshot};
use library::TrackRow;
use ui::controls::{HeldButtons, Sink};
use ui::cover::Covers;
use ui::deck::{DeckPanels, DeckReads, DeckView};
use ui::layout::four_deck_sections;
use ui::{ControlFrame, DeckInfo, UiCommand};

fn four_decks() -> ControlFrame {
    let mut frame = ControlFrame::default();
    frame.view.four_decks = true;
    frame
}

fn draw(frame: &ControlFrame) -> Harness<'_> {
    let mut screen = ui::Screen::default();
    let mut harness = Harness::builder()
        .with_size([1440.0, 900.0])
        .build_ui(move |ui| ui::root(ui, frame, None, &mut screen));
    harness.run_steps(3);
    harness
}

fn info(title: &str) -> DeckInfo {
    DeckInfo {
        load_seq: 1,
        row: TrackRow {
            id: 1,
            title: Some(title.into()),
            artist: None,
            bpm: None,
            key: None,
            duration_frames: None,
            sample_rate: None,
            has_cues: false,
            missing: false,
        },
        path: PathBuf::from(format!("/music/{title}.mp3")),
        cover_file: None,
        overview: None,
        detail: None,
        stems: None,
        hotcue_marks: Default::default(),
    }
}

#[test]
fn four_decks_show_every_deck() {
    let frame = four_decks();
    let harness = draw(&frame);
    // A panel's tag; "4" is also the default loop size, so there may be more than one.
    for tag in ["1", "2", "3", "4"] {
        assert!(
            !harness
                .query_all_by_label(tag)
                .collect::<Vec<_>>()
                .is_empty()
        );
    }
    for strip in ["DECK 1", "DECK 2", "DECK 3", "DECK 4"] {
        harness.get_by_label(strip);
    }
}

#[test]
fn four_deck_panels_follow_their_decks() {
    let mut frame = four_decks();
    frame.decks[0] = Some(info("One"));
    frame.decks[2] = Some(info("Three"));
    // The engine plays each track: the snapshots carry the frames' load_seq.
    let mut reads = DeckReads::default();
    for deck in [0, 2] {
        reads.decks[deck].0 = DeckSnapshot {
            loaded: true,
            track_sample_rate: 44_100,
            duration_frames: 441_000,
            load_seq: 1,
            ..DeckSnapshot::default()
        };
    }
    let mut panels = DeckPanels::default();
    let mut covers = Covers::default();
    let sections = four_deck_sections();
    let mut harness = Harness::builder()
        .with_size([1440.0, 400.0])
        .build_ui(move |ui| {
            panels.show_reads(
                ui,
                &frame,
                &reads,
                &mut covers,
                &sections,
                &mut Sink::new(&channel().0, &mut HeldButtons::default()),
            );
        });
    harness.run_steps(3);
    harness.get_by_label("One");
    harness.get_by_label("Three");
    // Each title sits in its own deck's panel: left column (1 over 3), by height.
    let top = |label| harness.get_by_label(label).rect();
    let (one, three) = (top("One"), top("Three"));
    assert!(one.top() < three.top(), "{one:?} {three:?}");
    assert!(one.left() < 720.0 && three.left() < 720.0);
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
fn four_deck_knobs_have_letter_captions_and_grow() {
    let frame = four_decks();
    let harness = draw(&frame);
    let p = ui::theme::palette(&harness.ctx);
    // A knob is a disc of the background with a dim rim.
    let mut drawn = Vec::new();
    circles(
        harness.output().shapes.iter().map(|c| c.shape.clone()),
        &mut drawn,
    );
    let knobs: Vec<CircleShape> = drawn
        .into_iter()
        .filter(|c| c.fill == p.background && c.stroke.color == p.text_dim)
        .collect();
    // Five knobs on each of the four strips, then the headphone level and mix.
    assert_eq!(knobs.len(), 22, "{knobs:?}");
    let knobs = &knobs[..20];
    let radius = knobs[0].radius;
    assert!(radius >= 9.0, "the knobs' radius is {radius}");
    for letter in ["G", "H", "M", "B", "F"] {
        let captions: Vec<_> = harness.get_all_by_label(letter).map(|n| n.rect()).collect();
        assert_eq!(captions.len(), 4, "{letter}: {captions:?}");
        for caption in captions {
            // Beside a knob: level with it, left or right of its rim.
            let beside = knobs.iter().any(|k| {
                let circle = Rect::from_center_size(k.center, Vec2::splat(2.0 * k.radius));
                let level = caption.y_range().intersects(circle.y_range());
                let aside = caption.right() <= circle.left() + 0.5
                    || caption.left() >= circle.right() - 0.5;
                level && aside && (caption.center().x - k.center.x).abs() < 3.0 * k.radius
            });
            assert!(beside, "{letter} at {caption:?} is not beside a knob");
        }
    }
    for word in ["GAIN", "HIGH", "MID", "LOW", "FILTER"] {
        assert!(harness.query_by_label(word).is_none(), "{word}");
    }
}

#[test]
fn the_compact_panels_buttons_send_their_actions() {
    // Deck 4: a right deck, mirrored.
    let deck = DeckId::new(3).unwrap();
    // A loop size no tag or pad shows.
    let snapshot = DeckSnapshot {
        loop_beats: 0.5,
        ..DeckSnapshot::default()
    };
    let view = DeckView::new(
        deck,
        &snapshot,
        &ChannelSnapshot::default(),
        &four_decks(),
        &SamplerSnapshot::default(),
    );
    let (commands, received) = channel();
    let sections = four_deck_sections();
    let mut held = HeldButtons::default();
    let mut harness = Harness::builder()
        .with_size([480.0, 120.0])
        .build_ui(move |ui| {
            let mut sink = Sink::new(&commands, &mut held);
            ui::deck::panel(ui, &view, None, &sections, &mut sink);
        });
    harness.run_steps(3);
    for (label, action) in [
        ("CUE", Action::Cue(deck)),
        ("KEYLOCK", Action::KeylockToggle(deck)),
        ("1/2", Action::AutoLoop(deck)),
        ("OUT", Action::LoopOut(deck)),
    ] {
        let at = harness.get_by_label(label).rect().center();
        harness.event(egui::Event::PointerMoved(at));
        harness.run_steps(1);
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            harness.run_steps(2);
        }
        let sent: Vec<UiCommand> = received.try_iter().collect();
        assert_eq!(
            sent,
            [
                UiCommand::Action(action, Input::Press),
                UiCommand::Action(action, Input::Release)
            ],
            "{label}"
        );
    }
}
