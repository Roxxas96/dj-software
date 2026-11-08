//! The waveform lanes draw headless, without a GPU.

use std::sync::Arc;

use analysis::Waveform;
use egui::{Mesh, Sense};
use egui_kittest::Harness;
use engine::{DeckId, DeckSnapshot, Grid};
use ui::waveform::Lane;

fn draw(snapshot: DeckSnapshot, waveform: &Waveform) {
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
    loaded.hotcues[0] = Some(435_000.0);
    loaded.hotcues[2] = Some(450_000.0);
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
