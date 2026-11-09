//! The 4-Decks view draws headless, without a GPU.

use std::path::PathBuf;
use std::sync::mpsc::channel;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use engine::DeckSnapshot;
use library::TrackRow;
use ui::cover::Covers;
use ui::deck::{DeckPanels, DeckReads};
use ui::layout::four_deck_sections;
use ui::{ControlFrame, DeckInfo};

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
        overview: None,
        detail: None,
        stems: None,
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
            panels.show_reads(ui, &frame, &reads, &mut covers, &sections, &channel().0);
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
