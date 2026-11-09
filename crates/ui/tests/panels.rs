//! The panels fold away to a tab and the sideview shows the sampler, headless.

use std::sync::mpsc::channel;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use library::{BrowserView, Focus, ListRow, TrackRow};
use ui::{ControlFrame, UiCommand, ViewChange, ViewState};

fn browser() -> BrowserView {
    let track = TrackRow {
        id: 1,
        title: Some("Just Us".into()),
        artist: None,
        bpm: None,
        key: None,
        duration_frames: None,
        sample_rate: None,
        has_cues: false,
        missing: false,
    };
    BrowserView {
        path: "House".into(),
        index: 0,
        count: 1,
        track: Some(track.clone()),
        focus: Focus::List,
        tree: Vec::new(),
        tree_cursor: 0,
        list: vec![ListRow {
            path: "/music/just-us.mp3".into(),
            track: Some(track),
            file_name: "just-us.mp3".into(),
        }],
        list_offset: 0,
        list_count: 1,
        list_cursor: 0,
    }
}

fn frame(info_panel: bool, sideview: bool) -> ControlFrame {
    ControlFrame {
        browser: Some(browser()),
        view: ViewState {
            info_panel,
            sideview,
            ..ViewState::default()
        },
        ..ControlFrame::default()
    }
}

fn draw<'a>(frame: &'a ControlFrame, screen: &'a mut ui::Screen) -> Harness<'a> {
    let mut harness = Harness::builder()
        .with_size([1440.0, 900.0])
        .build_ui(|ui| ui::root(ui, frame, None, screen));
    harness.run_steps(3);
    harness
}

#[test]
fn folded_info_shows_its_tab() {
    let open = frame(true, false);
    let mut screen = ui::Screen::default();
    let harness = draw(&open, &mut screen);
    // The list's title and the info panel's copy of it.
    assert_eq!(harness.get_all_by_label("Just Us").count(), 2);
    assert!(harness.query_by_label("info").is_none());
    drop(harness);

    let folded = frame(false, false);
    let (commands, asked) = channel();
    let mut screen = ui::Screen {
        commands,
        ..ui::Screen::default()
    };
    let mut harness = draw(&folded, &mut screen);
    assert_eq!(harness.get_all_by_label("Just Us").count(), 1);
    harness.get_by_label("info").click();
    harness.run_steps(2);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [UiCommand::View(ViewChange::SetInfoPanel(true))]
    );
}

#[test]
fn a_folded_sideview_has_a_tab() {
    let folded = frame(true, false);
    let (commands, asked) = channel();
    let mut screen = ui::Screen {
        commands,
        ..ui::Screen::default()
    };
    let mut harness = draw(&folded, &mut screen);
    harness.get_by_label("sideview").click();
    harness.run_steps(2);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [UiCommand::View(ViewChange::SetSideview(true))]
    );
}

#[test]
fn sideview_shows_the_sampler() {
    let mut frame = frame(true, true);
    frame.samples_loaded[2] = true;
    let mut harness = Harness::builder()
        .with_size([300.0, 600.0])
        .build_ui(|ui| ui::sideview::show(ui, &frame, None));
    harness.run_steps(3);
    for slot in 1..=8 {
        harness.get_by_label(&slot.to_string());
    }
    // In the window it replaces the tab.
    let mut screen = ui::Screen::default();
    let harness = draw(&frame, &mut screen);
    harness.get_by_label("SAMPLER");
    assert!(harness.query_by_label("sideview").is_none());
}

#[test]
fn dragging_a_divider_sends_the_new_sizes() {
    let frame = frame(true, false);
    let (commands, asked) = channel();
    let mut screen = ui::Screen {
        commands,
        ..ui::Screen::default()
    };
    let mut harness = draw(&frame, &mut screen);
    // The divider between the tree and the list, near the bottom of the window.
    let from = egui::pos2(0.20 * 1440.0, 880.0);
    let to = egui::pos2(0.30 * 1440.0, 880.0);
    let button = |pressed| egui::Event::PointerButton {
        pos: if pressed { from } else { to },
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    harness.event(egui::Event::PointerMoved(from));
    harness.run_steps(2);
    harness.event(button(true));
    harness.run_steps(2);
    harness.event(egui::Event::PointerMoved(to));
    harness.run_steps(2);
    harness.event(button(false));
    harness.run_steps(2);
    let sent: Vec<_> = asked.try_iter().collect();
    let Some(UiCommand::View(ViewChange::SetSizes(sizes))) = sent.last() else {
        panic!("no sizes sent: {sent:?}");
    };
    assert!((sizes.tree - 0.30).abs() < 0.02, "{sizes:?}");
}
