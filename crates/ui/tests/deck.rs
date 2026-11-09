//! The deck panel draws headless, without a GPU.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::channel;

use actions::PadMode;
use analysis::Waveform;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use engine::{ChannelSnapshot, DeckId, DeckSnapshot, Grid, SamplerSnapshot};
use library::TrackRow;
use ui::cover::Covers;
use ui::deck::{DeckPanels, DeckReads, DeckView};
use ui::layout::sections;
use ui::{ControlFrame, DeckInfo, JogDisplay, Layout, TimeMode, UiCommand, ViewChange};

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
        .build_ui(move |ui| ui::deck::panel(ui, view, None, &sections, &channel().0));
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
    // The time cycles on a click; a deck starts on the time remaining.
    harness.get_by_label("−4:31.9");
    harness.get_by_label("0:57.1");
    harness.get_by_label("pads");
}

#[test]
fn the_time_shows_its_mode() {
    let (snapshot, mut frame) = loaded();
    // The mode's time, then the other of elapsed and remaining beside it.
    for (mode, labels) in [
        (TimeMode::Remaining, ["−4:31.9", "0:57.1"]),
        (TimeMode::Elapsed, ["0:57.1", "−4:31.9"]),
        (TimeMode::Total, ["5:29.0", "0:57.1"]),
    ] {
        frame.view.time_mode[0] = mode;
        let view = view(&snapshot, &frame);
        let harness = draw(&view);
        for label in labels {
            harness.get_by_label(label);
        }
    }
}

#[test]
fn the_beat_counter_shows_when_asked() {
    let (snapshot, mut frame) = loaded();
    assert!(view(&snapshot, &frame).beat_counter.is_none());
    frame.view.beat_counter = true;
    // 57.1 s at 126 BPM is beat 119 from the first: bar 30, beat 4.
    let view = view(&snapshot, &frame);
    assert_eq!(view.beat_counter.as_deref(), Some("30.4"));
    draw(&view).get_by_label("30.4");
}

#[test]
fn info_jog_shows_the_bpm() {
    let (snapshot, mut frame) = loaded();
    let plain = view(&snapshot, &frame);
    assert_eq!(draw(&plain).query_all_by_label("126.00").count(), 1);
    frame.view.jog_display = JogDisplay::Info;
    let info = view(&snapshot, &frame);
    // In the header and in the jog.
    assert_eq!(draw(&info).query_all_by_label("126.00").count(), 2);
}

#[test]
fn clicking_the_time_cycles_it() {
    let (snapshot, mut frame) = loaded();
    frame.view.time_mode[0] = TimeMode::Elapsed;
    let mut reads = DeckReads::default();
    reads.decks[0].0 = snapshot;
    let (commands, received) = channel();
    let mut panels = DeckPanels::default();
    let mut covers = Covers::default();
    let sections = sections(Layout::Pro);
    let mut harness = Harness::builder()
        .with_size([1440.0, 400.0])
        .build_ui(move |ui| {
            panels.show_reads(ui, &frame, &reads, &mut covers, &sections, &commands);
        });
    harness.run_steps(3);
    harness.get_by_label_contains("0:5").click();
    harness.run_steps(2);
    let deck = DeckId::new(0).unwrap();
    assert_eq!(
        received.try_recv().ok(),
        Some(UiCommand::View(ViewChange::CycleTime(deck)))
    );
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
