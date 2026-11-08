//! The window's root drawing runs headless, without a GPU.

use std::sync::Arc;
use std::thread;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ui::{ControlFrame, Published};

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
    use std::sync::atomic::Ordering;
    let frame = ControlFrame::default();
    let mut screen = ui::Screen::default();
    let zoom = Arc::clone(&screen.zoom);
    let mut harness = Harness::builder()
        .with_size([1440.0, 900.0])
        .build_ui(|ui| ui::root(ui, &frame, None, &mut screen));
    harness.run_steps(2);
    harness.get_by_label("+").click();
    harness.run_steps(2);
    assert_eq!(zoom.load(Ordering::Relaxed), 1);
    harness.get_by_label("-").click();
    harness.get_by_label("-").click();
    harness.run_steps(2);
    assert_eq!(zoom.load(Ordering::Relaxed), -1);
}
