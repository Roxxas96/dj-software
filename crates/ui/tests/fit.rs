//! Every layout's deck panels and mixer fit the row the window gives them, headless: at
//! the smallest window and the default one, with loaded decks whose headers show a key,
//! the beat counter, a remix and the stems.

#[allow(
    dead_code,
    reason = "the rows are drawn here, without the window's render"
)]
mod support;

use std::sync::mpsc::channel;

use egui::{Rect, Vec2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT, Queryable};
use support::loaded;
use ui::cover::Covers;
use ui::deck::{DeckPanels, DeckReads};
use ui::layout::{drawn_sizes, shown_sections};
use ui::mixer::Mixer;
use ui::{ControlFrame, Layout};

const WINDOWS: [Vec2; 2] = [vec2(1024.0, 700.0), vec2(1440.0, 900.0)];
/// The deck panels' share of the row's width (two decks, then four).
const PANEL_SHARE: f32 = 0.38;
const FOUR_PANEL_SHARE: f32 = 0.34;
/// The harness's margin around what it draws.
const MARGIN: f32 = 8.0;

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
            mixer.show(ui, space, &frame, &reads, &sections);
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
const HEADER_BOTTOM: f32 = 4.0 + 64.0;
const TRANSPORT: [&str; 3] = ["CUE", "PLAY", "SYNC"];
const LOOP: [&str; 3] = ["4", "IN", "OUT"];
/// The hotcue pads: the fixture's second hotcue is a saved loop.
const PADS: [&str; 9] = ["pad mode", "1", "2 loop", "3", "4", "5", "6", "7", "8"];

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

/// The whole window at the smallest size, every layout with two and with four decks: the
/// deck row is as tall as its contents, the browser lists at least three rows, and the
/// sections only touch (they overlap by no more than the gap between them).
#[test]
fn every_layout_fits_the_smallest_window() {
    let window = WINDOWS[0];
    for layout in Layout::ALL {
        for four_decks in [false, true] {
            let (mut frame, reads) = loaded(*layout, four_decks);
            // Every pane of the browser is shown.
            frame.view.info_panel = true;
            frame.view.sideview = true;
            let sections = shown_sections(&frame.view);
            let drawn = drawn_sizes(frame.view.sizes, &sections, window);
            let at = format!("{layout:?} four_decks={four_decks}");
            let height = ui::deck::row_height(&sections);
            assert!(drawn.decks * window.y >= height - 1.0, "{at}: deck row");
            let mut screen = ui::Screen::default();
            let mut harness = Harness::builder().with_size(window).build_ui(move |ui| {
                let area = ui.ctx().content_rect();
                ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                    ui.set_clip_rect(area);
                    ui::root_with_reads(ui, &frame, &reads, &mut screen);
                });
            });
            harness.run_steps(3);
            let browser = harness.get_by_label("browser").rect();
            // The deck row's controls end above the browser.
            let play_bottom = harness
                .get_all_by_label("PLAY")
                .map(|n| n.rect().max.y)
                .fold(0.0, f32::max);
            assert!(
                play_bottom <= browser.min.y,
                "{at}: PLAY {play_bottom} under the browser {browser:?}"
            );
            let rows = harness
                .query_all_by_label_contains("Track Number")
                .filter(|n| {
                    let r = n.rect();
                    r.min.y >= browser.min.y && r.max.y <= window.y
                })
                .count();
            assert!(rows >= 3, "{at}: the browser shows {rows} rows");
            // Every section the window draws, by the name its area carries.
            let mut sections_rects: Vec<(String, Rect)> = Vec::new();
            for name in ["top bar", "lanes", "mixer", "tree", "list"] {
                sections_rects.push((name.into(), harness.get_by_label(name).rect()));
            }
            for name in ["info panel", "sideview panel"] {
                let rect = harness.get_by_label(name).rect();
                sections_rects.push((name.into(), rect));
            }
            let panels: Vec<_> = harness
                .query_all_by_label_contains("deck panel")
                .map(|n| (n.accesskit_node().label().unwrap_or_default(), n.rect()))
                .collect();
            assert_eq!(panels.len(), if four_decks { 4 } else { 2 }, "{at}");
            sections_rects.extend(panels);
            let gap = ui::theme::SECTION_GAP;
            for (i, (a_name, a)) in sections_rects.iter().enumerate() {
                for (b_name, b) in &sections_rects[i + 1..] {
                    let overlap = a.intersect(*b);
                    let depth = overlap.height().min(overlap.width());
                    assert!(
                        !overlap.is_positive() || depth <= gap,
                        "{at}: {a_name} {a:?} and {b_name} {b:?} overlap"
                    );
                }
            }
        }
    }
}
