//! The 4-Decks view draws headless, without a GPU.

use std::path::PathBuf;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use engine::{ChannelSnapshot, DeckId, DeckSnapshot, SamplerSnapshot};
use library::TrackRow;
use ui::deck::DeckView;
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
    frame.decks[2] = Some(info("Three"));
    frame.decks[0] = Some(info("One"));
    // The engine plays deck 3's track, and deck 1 has nothing loaded.
    let snapshot = DeckSnapshot {
        loaded: true,
        track_sample_rate: 44_100,
        duration_frames: 441_000,
        load_seq: 1,
        ..DeckSnapshot::default()
    };
    let view = DeckView::new(
        DeckId::new(2).unwrap(),
        &snapshot,
        &ChannelSnapshot::default(),
        &frame,
        &SamplerSnapshot::default(),
    );
    let sections = four_deck_sections();
    let mut harness = Harness::builder()
        .with_size([560.0, 200.0])
        .build_ui(move |ui| ui::deck::panel(ui, &view, None, &sections));
    harness.run_steps(3);
    harness.get_by_label("3");
    harness.get_by_label("Three");
    assert!(harness.query_by_label("One").is_none());
}
