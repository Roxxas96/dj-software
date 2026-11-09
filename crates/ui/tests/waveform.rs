//! The waveform lanes draw headless, without a GPU.

use std::sync::Arc;

use analysis::Waveform;
use egui::{Mesh, Sense};
use egui_kittest::Harness;
use engine::{DeckId, DeckSnapshot, Grid, Hotcue};
use ui::waveform::Lane;
use ui::{ControlFrame, WaveformPosition, WaveformStyle};

fn draw(snapshot: DeckSnapshot, waveform: &Waveform) {
    draw_styled(snapshot, waveform, WaveformStyle::Frequency, false);
}

fn draw_styled(snapshot: DeckSnapshot, waveform: &Waveform, style: WaveformStyle, overlay: bool) {
    let mut mesh = Arc::new(Mesh::default());
    let mut harness = Harness::builder()
        .with_size([1440.0, 200.0])
        .build_ui(|ui| {
            let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
            let lane = Lane {
                deck: DeckId::new(0).unwrap(),
                snapshot: &snapshot,
                waveform: Some(waveform),
                position_frames: 441_000.0,
                beats: 16.0,
                style,
                overlay,
                marks: None,
            };
            lane.draw(&ui.painter_at(rect), rect, &mut mesh);
        });
    harness.run_steps(3);
    drop(harness);
    assert!(!mesh.vertices.is_empty());
}

#[test]
fn lane_draws_with_and_without_a_grid() {
    let waveform = Waveform {
        frames_per_bucket: 128,
        buckets: (0..10_000)
            .map(|i| [(i % 256) as u8, 200, 80, 30])
            .collect(),
    };
    let mut loaded = DeckSnapshot {
        loaded: true,
        playing: true,
        position_frames: 441_000.0,
        duration_frames: 1_280_000,
        track_sample_rate: 44_100,
        cue_frames: 430_000.0,
        loop_in: Some(440_000.0),
        loop_out: Some(452_000.0),
        loop_active: true,
        ..DeckSnapshot::default()
    };
    loaded.hotcues[0] = Some(Hotcue::point(435_000.0));
    loaded.hotcues[2] = Some(Hotcue::point(450_000.0));
    draw(loaded, &waveform);
    let gridded = DeckSnapshot {
        grid: Some(Grid {
            beat_period_frames: 22_050.0,
            first_beat_frames: 1_000.0,
        }),
        ..loaded
    };
    draw(gridded, &waveform);
}

#[test]
fn every_style_draws() {
    let waveform = Waveform {
        frames_per_bucket: 128,
        buckets: (0..10_000)
            .map(|i| [(i % 256) as u8, 200, 80, 30])
            .collect(),
    };
    let loaded = DeckSnapshot {
        loaded: true,
        playing: true,
        position_frames: 441_000.0,
        duration_frames: 1_280_000,
        track_sample_rate: 44_100,
        ..DeckSnapshot::default()
    };
    for style in [
        WaveformStyle::Frequency,
        WaveformStyle::DeckColour,
        WaveformStyle::Bands,
        WaveformStyle::Overlaid,
    ] {
        draw_styled(loaded, &waveform, style, false);
        draw_styled(loaded, &waveform, style, true);
        for position in [WaveformPosition::Top, WaveformPosition::Middle] {
            for overview in [true, false] {
                let mut frame = ControlFrame::default();
                frame.view.waveform_style = style;
                frame.view.waveform_position = position;
                frame.view.overview = overview;
                let mut screen = ui::Screen::default();
                let mut harness = Harness::builder()
                    .with_size([1440.0, 900.0])
                    .build_ui(|ui| ui::root(ui, &frame, None, &mut screen));
                harness.run_steps(3);
            }
        }
    }
}
