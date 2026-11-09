//! The window's root drawing runs headless, without a GPU.

use std::sync::Arc;
use std::thread;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ui::{ControlFrame, Published, UiCommand};

#[test]
fn window_draws_a_default_frame() {
    let frame = ControlFrame::default();
    let audio = frame.audio.clone().unwrap_err();
    let mut screen = ui::Screen::default();
    let mut harness = Harness::builder()
        .with_size([1440.0, 900.0])
        .build_ui(|ui| ui::root(ui, &frame, None, &mut screen));
    harness.run_steps(3);
    harness.get_by_label("no controller");
    harness.get_by_label(&audio);
}

#[test]
fn published_returns_the_latest() {
    let published = Published::new(0_u32);
    published.publish(1);
    published.publish(2);
    let reader = published.clone();
    let seen = thread::spawn(move || reader.latest()).join().unwrap();
    assert_eq!(*seen, 2);
    assert!(Arc::ptr_eq(&seen, &published.latest()));
}

#[test]
fn the_zoom_buttons_ask_for_steps() {
    let frame = ControlFrame::default();
    let (commands, asked) = std::sync::mpsc::channel();
    let mut screen = ui::Screen {
        commands,
        ..ui::Screen::default()
    };
    let mut harness = Harness::builder()
        .with_size([1440.0, 900.0])
        .build_ui(|ui| ui::root(ui, &frame, None, &mut screen));
    harness.run_steps(2);
    harness.get_by_label("+").click();
    harness.run_steps(2);
    assert_eq!(asked.try_iter().collect::<Vec<_>>(), [UiCommand::Zoom(1)]);
    harness.get_by_label("-").click();
    harness.get_by_label("-").click();
    harness.run_steps(2);
    let steps: i32 = asked
        .try_iter()
        .map(|command| match command {
            UiCommand::Zoom(steps) => steps,
            other => panic!("not a zoom: {other:?}"),
        })
        .sum();
    assert_eq!(steps, -2);
}

#[test]
fn a_long_status_is_cut_short_at_the_smallest_window() {
    let frame = ui::ControlFrame {
        error: Some("a very long reason the session stopped, ".repeat(8)),
        ..ui::ControlFrame::default()
    };
    let mut screen = ui::Screen::default();
    let mut harness = egui_kittest::Harness::builder()
        .with_size([1024.0, 700.0])
        .build_ui(|ui| ui::root(ui, &frame, None, &mut screen));
    harness.run_steps(3);
    let status = harness
        .get_all_by_label_contains("a very long reason")
        .map(|n| n.rect())
        .min_by(|a, b| a.top().total_cmp(&b.top()))
        .unwrap();
    let right = harness.get_by_label("no controller").rect();
    let cpu = harness.get_by_label("CPU \u{2013}").rect();
    assert!(
        status.right() < cpu.left().min(right.left()),
        "{status:?} {cpu:?}"
    );
    assert!(status.right() <= 1024.0);
}
