//! Every layout's deck panels and mixer fit the row the window gives them, headless: at
//! the smallest window and the default one, with loaded decks whose headers show a key,
//! the beat counter, a remix and the stems.

use std::path::PathBuf;
use std::sync::mpsc::channel;

use egui::{Rect, Vec2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT, Queryable};
use engine::{DeckSnapshot, Grid};
use library::TrackRow;
use ui::cover::Covers;
use ui::deck::{DeckPanels, DeckReads};
use ui::layout::{drawn_sizes, shown_sections};
use ui::mixer::Mixer;
use ui::{ControlFrame, DeckInfo, Layout};

const WINDOWS: [Vec2; 2] = [vec2(1024.0, 700.0), vec2(1440.0, 900.0)];
/// The deck panels' share of the row's width (two decks, then four).
const PANEL_SHARE: f32 = 0.38;
const FOUR_PANEL_SHARE: f32 = 0.34;
/// The harness's margin around what it draws.
const MARGIN: f32 = 8.0;

fn info(deck: usize) -> DeckInfo {
    DeckInfo {
        load_seq: 1,
        row: TrackRow {
            id: deck as i64 + 1,
            title: Some(format!("Track Number {} (Extended Mix)", deck + 1)),
            artist: Some("Someone".into()),
            bpm: Some(126.0),
            key: Some(21),
            duration_frames: Some(14_508_900),
            sample_rate: Some(44_100),
            has_cues: true,
            missing: false,
        },
        path: PathBuf::from(format!("/music/track-{deck}.mp3")),
        overview: None,
        detail: None,
        stems: Some("stems 40%".into()),
        hotcue_marks: Default::default(),
    }
}

/// Every deck loaded and playing, with a grid so the beat counter shows.
fn loaded(layout: Layout, four_decks: bool) -> (ControlFrame, DeckReads) {
    let mut frame = ControlFrame::default();
    frame.view.layout = layout;
    frame.view.four_decks = four_decks;
    frame.view.beat_counter = true;
    let mut reads = DeckReads::default();
    for deck in 0..4 {
        frame.decks[deck] = Some(info(deck));
        reads.decks[deck].0 = DeckSnapshot {
            loaded: true,
            playing: true,
            position_frames: 2_518_110.0,
            duration_frames: 14_508_900,
            track_sample_rate: 44_100,
            tempo_ratio: 1.0,
            grid: Grid::from_bpm(126.0, 0.0, 44_100),
            load_seq: 1,
            ..DeckSnapshot::default()
        };
    }
    (frame, reads)
}

/// Draws the deck panels' row as the window sizes it for `window`; returns the harness
/// and the row's size.
fn draw_row(frame: ControlFrame, reads: DeckReads, window: Vec2) -> (Harness<'static>, Vec2) {
    let sections = shown_sections(&frame.view);
    let sizes = drawn_sizes(frame.view.sizes, &sections, window);
    let row = vec2(window.x, (sizes.decks * window.y).floor());
    let mut panels = DeckPanels::default();
    let mut covers = Covers::default();
    let mut mixer = Mixer::default();
    // The harness keeps an 8-point margin around what it draws.
    let mut harness = Harness::builder()
        .with_size(row + Vec2::splat(2.0 * MARGIN))
        .build_ui(move |ui| {
            let space = panels.show_reads(ui, &frame, &reads, &mut covers, &sections, &channel().0);
            mixer.show(ui, space, &frame, None, &sections);
        });
    harness.run_steps(3);
    (harness, row)
}

/// The panels' and the mixer's areas in a row of `row`.
fn slots(row: Vec2, four_decks: bool) -> Vec<Rect> {
    let share = if four_decks {
        FOUR_PANEL_SHARE
    } else {
        PANEL_SHARE
    };
    let width = row.x * share;
    let min = pos2(MARGIN, MARGIN);
    let columns = [
        Rect::from_min_size(min, vec2(width, row.y)),
        Rect::from_min_size(min + vec2(row.x - width, 0.0), vec2(width, row.y)),
    ];
    let mut slots: Vec<Rect> = if four_decks {
        columns
            .iter()
            .flat_map(|c| {
                let (top, bottom) = c.split_top_bottom_at_fraction(0.5);
                [top, bottom]
            })
            .collect()
    } else {
        columns.to_vec()
    };
    slots.push(Rect::from_min_max(
        min + vec2(width, 0.0),
        min + vec2(row.x - width, row.y),
    ));
    slots
}

/// Every control in each panel lies inside its own panel, and the ones `wanted` are there,
/// once a panel.
fn check(layout: Layout, four_decks: bool, wanted: &[&str]) {
    for window in WINDOWS {
        let (frame, reads) = loaded(layout, four_decks);
        let (harness, row) = draw_row(frame, reads, window);
        let slots = slots(row, four_decks);
        let panels = slots.len() - 1;
        let at = format!("{layout:?} four_decks={four_decks} at {window:?}");
        let leaves = harness.query_all(By::new().predicate(|n| n.children().next().is_none()));
        for node in leaves {
            let rect = node.rect();
            let name = node.accesskit_node().label().unwrap_or_default();
            let slot = slots
                .iter()
                .find(|s| s.contains(rect.center()))
                .unwrap_or_else(|| panic!("{name:?} {rect:?} is outside the row, {at}"));
            assert!(
                slot.expand(0.5).contains_rect(rect),
                "{name:?} {rect:?} leaves its area {slot:?}, {at}"
            );
        }
        // The header keeps to the cover's height: nothing in it is cut off.
        for label in HEADER {
            for slot in &slots[..panels] {
                let header = Rect::from_min_size(slot.min, vec2(slot.width(), HEADER_BOTTOM));
                let found: Vec<Rect> = harness.get_all_by_label(label).map(|n| n.rect()).collect();
                assert!(
                    found.iter().any(|r| header.expand(0.5).contains_rect(*r)),
                    "{label:?} is not in the header {header:?}: {found:?}, {at}"
                );
            }
        }
        for label in wanted {
            let found: Vec<Rect> = harness.get_all_by_label(label).map(|n| n.rect()).collect();
            for slot in &slots[..panels] {
                assert!(
                    found.iter().any(|r| slot.contains_rect(*r)),
                    "no {label:?} in {slot:?}: {found:?}, {at}"
                );
            }
        }
    }
}

/// The header's text: the key, the beat counter, both times and the stems.
const HEADER: [&str; 5] = ["8A", "30.4", "0:57.1", "\u{2212}4:31.9", "stems 40%"];
/// The header's bottom below a panel's top: the margin and the cover.
const HEADER_BOTTOM: f32 = 6.0 + 64.0;
const TRANSPORT: [&str; 3] = ["CUE", "PLAY", "SYNC"];
const LOOP: [&str; 3] = ["4", "IN", "OUT"];
const PADS: [&str; 9] = ["pad mode", "1", "2", "3", "4", "5", "6", "7", "8"];

#[test]
fn starter_fits() {
    check(
        Layout::Starter,
        false,
        &[&TRANSPORT[..], &LOOP[..]].concat(),
    );
}

#[test]
fn essentials_fits() {
    check(
        Layout::Essentials,
        false,
        &[&TRANSPORT[..], &LOOP[..], &PADS[..], &["FX 1"]].concat(),
    );
}

#[test]
fn pro_fits() {
    check(
        Layout::Pro,
        false,
        &[&TRANSPORT[..], &LOOP[..], &PADS[..], &["FX 1"]].concat(),
    );
}

#[test]
fn performance_fits() {
    check(
        Layout::Performance,
        false,
        &[&TRANSPORT[..], &LOOP[..], &PADS[..], &["FX 1"]].concat(),
    );
}

#[test]
fn four_decks_fit() {
    for layout in Layout::ALL {
        check(*layout, true, &[&TRANSPORT[..], &LOOP[..]].concat());
    }
}
