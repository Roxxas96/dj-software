//! The settings window, headless: it opens from the gear, lists the settings, and every
//! edit is a command for the session.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ui::{ControlFrame, Layout, SettingRow, UiCommand, ViewChange};

fn row(key: &str, category: &'static str, value: &str, default: &str) -> SettingRow {
    SettingRow {
        key: key.to_owned(),
        category,
        value: value.to_owned(),
        default: default.to_owned(),
        help: "Some help",
        live: true,
        pending: false,
        error: None,
    }
}

fn frame(rows: Vec<SettingRow>) -> ControlFrame {
    ControlFrame {
        settings: Arc::new(rows),
        mappings: vec!["ddj-sb3".to_owned(), "other".to_owned()],
        devices: vec!["Built-in Output".to_owned(), "DDJ-SB3".to_owned()],
        ..ControlFrame::default()
    }
}

fn rows() -> Vec<SettingRow> {
    vec![
        row("audio.device", "Audio", "", ""),
        row("stems.knobs", "Stems", "\"vdj\"", "\"vdj\""),
        row("stems.kicks_hz", "Stems", "300", "300"),
    ]
}

fn open(frame: &ControlFrame) -> (Harness<'_>, Receiver<UiCommand>) {
    let (commands, asked) = channel();
    let mut screen = ui::Screen {
        commands,
        ..ui::Screen::default()
    };
    let mut harness = Harness::builder()
        .with_size([1440.0, 900.0])
        .build_ui(move |ui| ui::root(ui, frame, None, &mut screen));
    harness.run_steps(2);
    harness.get_by_label("Open settings").click();
    harness.run_steps(3);
    (harness, asked)
}

#[test]
fn settings_window_opens_on_options() {
    let frame = frame(rows());
    let (mut harness, _) = open(&frame);
    harness.get_by_label("Options").click();
    harness.run_steps(3);
    harness.get_by_label("stems.knobs");
    harness.get_by_label("audio.device");
    harness.get_by_label("stems.kicks_hz");
    harness.get_by_label("Search").focus();
    harness.run_steps(2);
    harness.get_by_label("Search").type_text("kicks");
    harness.run_steps(3);
    harness.get_by_label("stems.kicks_hz");
    assert!(harness.query_by_label("stems.knobs").is_none());
    assert!(harness.query_by_label("audio.device").is_none());
}

#[test]
fn editing_a_setting_sends_it() {
    let frame = frame(rows());
    let (mut harness, asked) = open(&frame);
    harness.get_by_label("Options").click();
    harness.run_steps(3);
    harness.get_by_label("stems.knobs choice").click();
    harness.run_steps(3);
    harness.get_by_label("raw").click();
    harness.run_steps(3);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [UiCommand::SetSetting {
            key: "stems.knobs".to_owned(),
            value: "\"raw\"".to_owned()
        }]
    );
}

#[test]
fn an_error_shows_on_its_row() {
    let why = "config.toml: stems.kicks_hz: expected a number from 50 to 1000";
    let mut rows = rows();
    rows[2].error = Some(why.to_owned());
    let frame = frame(rows);
    let (mut harness, _) = open(&frame);
    harness.get_by_label("Options").click();
    harness.run_steps(3);
    harness.get_by_label(why);
}

#[test]
fn interface_tiles_switch_layouts() {
    let frame = frame(rows());
    let (mut harness, asked) = open(&frame);
    harness.get_by_label("Interface").click();
    harness.run_steps(3);
    harness.get_by_label("Performance").click();
    harness.run_steps(3);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [UiCommand::View(ViewChange::SetLayout(Layout::Performance))]
    );
}

#[test]
fn reset_all_asks_first() {
    let frame = frame(rows());
    let (mut harness, asked) = open(&frame);
    harness.get_by_label("Options").click();
    harness.run_steps(3);
    harness.get_by_label("Reset all").click();
    harness.run_steps(3);
    assert!(asked.try_iter().next().is_none());
    harness.get_by_label("Reset").click();
    harness.run_steps(3);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [UiCommand::ResetSettings]
    );
}

#[test]
fn the_audio_page_lists_the_devices() {
    let frame = frame(rows());
    let (mut harness, asked) = open(&frame);
    harness.get_by_label("Audio").click();
    harness.run_steps(3);
    harness.get_by_label("audio.device");
    assert!(harness.query_by_label("stems.knobs").is_none());
    harness.get_by_label("Output device").click();
    harness.run_steps(3);
    harness.get_by_label("DDJ-SB3").click();
    harness.run_steps(3);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [UiCommand::SetSetting {
            key: "audio.device".to_owned(),
            value: "\"DDJ-SB3\"".to_owned()
        }]
    );
}

#[test]
fn the_controllers_page_lists_the_mappings() {
    let mut all = rows();
    all.push(row(
        "controller.mapping",
        "Controller",
        "\"ddj-sb3\"",
        "\"ddj-sb3\"",
    ));
    let frame = frame(all);
    let (mut harness, asked) = open(&frame);
    harness.get_by_label("Controllers").click();
    harness.run_steps(3);
    harness.get_by_label("controller.mapping");
    harness.get_by_label("Controller mapping").click();
    harness.run_steps(3);
    harness.get_by_label("other").click();
    harness.run_steps(3);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [UiCommand::SetSetting {
            key: "controller.mapping".to_owned(),
            value: "\"other\"".to_owned()
        }]
    );
}

#[test]
fn default_output_clears_the_device() {
    let frame = frame(rows());
    let (mut harness, asked) = open(&frame);
    harness.get_by_label("Audio").click();
    harness.run_steps(3);
    harness.get_by_label("Output device").click();
    harness.run_steps(3);
    harness.get_by_label("Default output").click();
    harness.run_steps(3);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [UiCommand::SetSetting {
            key: "audio.device".to_owned(),
            value: String::new()
        }]
    );
}
