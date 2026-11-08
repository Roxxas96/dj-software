//! The deck panel draws headless, without a GPU.

use std::path::PathBuf;
use std::sync::Arc;

use actions::PadMode;
use analysis::Waveform;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use engine::{ChannelSnapshot, DeckId, DeckSnapshot, Grid, SamplerSnapshot};
use library::TrackRow;
use ui::deck::DeckView;
use ui::layout::sections;
use ui::{ControlFrame, DeckInfo, Layout};

/// A 5:29 track at 126 BPM on deck 1, 57.1 s in.
fn loaded() -> (DeckSnapshot, ControlFrame) {
    let snapshot = DeckSnapshot {
        loaded: true,
        playing: true,
        position_frames: 2_518_110.0,
        duration_frames: 14_508_900,
        track_sample_rate: 44_100,
        grid: Grid::from_bpm(126.0, 0.0, 44_100),
        load_seq: 1,
        ..DeckSnapshot::default()
    };
    let mut frame = ControlFrame::default();
    frame.decks[0] = Some(DeckInfo {
        load_seq: 1,
        row: TrackRow {
            id: 1,
            title: Some("Just Us".into()),
            artist: Some("Someone".into()),
            bpm: Some(126.0),
            key: Some(21),
            duration_frames: Some(14_508_900),
            sample_rate: Some(44_100),
            has_cues: false,
            missing: false,
        },
        path: PathBuf::from("/music/just-us.mp3"),
        overview: None,
        detail: None,
        stems: None,
    });
    (snapshot, frame)
}

fn draw(view: &DeckView) -> Harness<'_> {
    draw_in(view, Layout::Pro)
}

fn draw_in(view: &DeckView, layout: Layout) -> Harness<'_> {
    let sections = sections(layout);
    let mut harness = Harness::builder()
        .with_size([560.0, 340.0])
        .build_ui(move |ui| ui::deck::panel(ui, view, None, &sections));
    harness.run_steps(3);
    harness
}

fn view(snapshot: &DeckSnapshot, frame: &ControlFrame) -> DeckView {
    DeckView::new(
        DeckId::new(0).unwrap(),
        snapshot,
        &ChannelSnapshot::default(),
        frame,
        &SamplerSnapshot::default(),
    )
}

#[test]
fn deck_panel_shows_title_and_bpm() {
    let (snapshot, frame) = loaded();
    let view = view(&snapshot, &frame);
    let harness = draw(&view);
    harness.get_by_label("Just Us");
    harness.get_by_label("126.00");
    harness.get_by_label("0:57.1");
    harness.get_by_label("−4:31.9");
    harness.get_by_label("pads");
}

#[test]
fn deck_panel_draws_the_overview_stems_and_jump_pads() {
    let (snapshot, mut frame) = loaded();
    let info = frame.decks[0].as_mut().unwrap();
    info.stems = Some("stems 40%".into());
    info.overview = Some(Arc::new(Waveform {
        frames_per_bucket: 1024,
        buckets: (0..14_169)
            .map(|i| [(i % 256) as u8, 200, 80, 30])
            .collect(),
    }));
    frame.control.sides[0].pad_mode = PadMode::BeatJump;
    let view = view(&snapshot, &frame);
    let harness = draw(&view);
    harness.get_by_label("stems 40%");
    harness.get_by_label("BEAT JUMP");
    harness.get_by_label("−4");
    harness.get_by_label("+32");
}

#[test]
fn starter_hides_the_pads() {
    let (snapshot, frame) = loaded();
    let view = view(&snapshot, &frame);
    draw_in(&view, Layout::Pro).get_by_label("pads");
    let starter = draw_in(&view, Layout::Starter);
    assert!(starter.query_by_label("pads").is_none());
    starter.get_by_label("LOOP");
    let performance = draw_in(&view, Layout::Performance);
    performance.get_by_label("pads");
}
