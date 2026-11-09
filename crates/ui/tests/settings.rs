//! The settings window, headless: it opens from the gear, lists the settings, and every
//! edit is a command for the session.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use ui::{
    ControlFrame, ImportStep, ImportSummary, ImportView, Layout, Resolution, SettingRow,
    TrackLabel, UiCommand, ViewChange,
};

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

#[test]
fn four_decks_disables_the_interface_tiles() {
    let mut frame = frame(rows());
    frame.view.four_decks = true;
    let (mut harness, asked) = open(&frame);
    harness.get_by_label("Interface").click();
    harness.run_steps(3);
    harness.get_by_label_contains("4-Decks is on");
    let tile = harness.get_by_label("Performance");
    assert!(tile.accesskit_node().is_disabled());
    tile.click();
    harness.run_steps(3);
    assert_eq!(asked.try_iter().collect::<Vec<_>>(), []);
}

#[test]
fn options_without_settings_say_so() {
    let frame = frame(Vec::new());
    let (mut harness, _) = open(&frame);
    harness.get_by_label("Options").click();
    harness.run_steps(3);
    harness.get_by_label("settings are not available yet");
    drop(harness);
    let stopped = ControlFrame {
        error: Some("the session stopped: no audio".to_owned()),
        ..ControlFrame::default()
    };
    let (mut harness, _) = open(&stopped);
    harness.get_by_label("Options").click();
    harness.run_steps(3);
    // The top bar and the browser's place show it too.
    assert_eq!(
        harness
            .get_all_by_label("the session stopped: no audio")
            .count(),
        3
    );
}

#[test]
fn closing_the_window_drops_what_was_typed() {
    let frame = frame(vec![row("stems.kicks_hz", "Stems", "417", "300")]);
    let mut harness = Harness::builder()
        .with_size([1440.0, 900.0])
        .build_ui_state(
            |ui, screen: &mut ui::Screen| ui::root(ui, &frame, None, screen),
            ui::Screen::default(),
        );
    harness.state_mut().settings.open = true;
    harness.run_steps(3);
    harness.get_by_label("Options").click();
    harness.run_steps(3);
    let field = |value: &'static str| {
        move |node: &egui_kittest::kittest::AccessKitNode<'_>| {
            node.role() == egui::accesskit::Role::TextInput
                && node.value().as_deref() == Some(value)
        }
    };
    harness.get_by(field("417")).focus();
    harness.run_steps(2);
    harness.get_by(field("417")).type_text("9");
    harness.run_steps(2);
    harness.get_by(field("4179"));
    // Closed as its close button does, while the field still has the focus.
    harness.state_mut().settings.open = false;
    harness.run_steps(3);
    harness.state_mut().settings.open = true;
    harness.run_steps(3);
    harness.get_by_label("Options").click();
    harness.run_steps(3);
    harness.get_by(field("417"));
}

/// The Library page of a window whose frame shows `import`.
fn library_page(frame: &ControlFrame) -> (Harness<'_>, Receiver<UiCommand>) {
    let (mut harness, asked) = open(frame);
    harness.get_by_label("Library").click();
    harness.run_steps(3);
    (harness, asked)
}

fn summary() -> ImportSummary {
    ImportSummary {
        databases: 2,
        tracks: 40,
        with_marks: 31,
        conflicting: 3,
        hotcues: 95,
        loops: 7,
        dropped: 1,
        other_markers: 12,
        playlists_new: 4,
        playlists_here: 1,
        playlists_skipped: 2,
        unmatched: 6,
    }
}

#[test]
fn the_library_page_starts_the_import() {
    let frame = frame(rows());
    let (mut harness, asked) = library_page(&frame);
    harness.get_by_label("Import from VirtualDJ…").click();
    harness.run_steps(3);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [UiCommand::Import(ImportStep::Scan)]
    );
}

#[test]
fn the_summary_shows_the_counts() {
    let frame = ControlFrame {
        import: ImportView::Summary(summary()),
        ..frame(rows())
    };
    let (mut harness, asked) = library_page(&frame);
    harness.get_by_label("VirtualDJ databases: 2");
    harness.get_by_label("Tracks found in the library: 40, with hotcues or loops: 31");
    harness.get_by_label("Tracks with hotcues here too, you choose for each: 3");
    harness.get_by_label("Hotcues: 95, saved loops: 7");
    harness.get_by_label("Dropped, over 8 slots: 1");
    harness.get_by_label("Other markers, not imported: 12");
    harness.get_by_label("Playlists to create: 4, already here: 1, skipped: 2");
    harness.get_by_label("Not in the library, not added: 6");
    harness.get_by_label("Import").click();
    harness.run_steps(3);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [UiCommand::Import(ImportStep::Apply)]
    );
}

#[test]
fn cancel_sends_cancel() {
    let frame = ControlFrame {
        import: ImportView::Summary(summary()),
        ..frame(rows())
    };
    let (mut harness, asked) = library_page(&frame);
    harness.get_by_label("Cancel").click();
    harness.run_steps(3);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [UiCommand::Import(ImportStep::Cancel)]
    );
}

fn ask() -> ImportView {
    ImportView::Ask {
        track: TrackLabel {
            id: 4,
            name: "Artist – Title".to_owned(),
        },
        existing: 3,
        incoming: 5,
        left: 2,
    }
}

#[test]
fn a_conflict_asks_keep_replace_or_merge() {
    let frame = ControlFrame {
        import: ask(),
        ..frame(rows())
    };
    let (mut harness, asked) = library_page(&frame);
    harness.get_by_label("Artist – Title has 3 hotcues here and 5 in VirtualDJ.");
    harness.get_by_label("Tracks left to choose for: 2");
    harness.get_by_label("Use VirtualDJ's").click();
    harness.run_steps(3);
    harness.get_by_label("Keep mine").click();
    harness.run_steps(3);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [
            UiCommand::Import(ImportStep::Resolve {
                track: 4,
                resolution: Resolution::Replace,
                rest_too: false,
            }),
            UiCommand::Import(ImportStep::Resolve {
                track: 4,
                resolution: Resolution::Keep,
                rest_too: false,
            }),
        ]
    );
}

#[test]
fn choosing_for_all_remaining_sends_it() {
    let frame = ControlFrame {
        import: ask(),
        ..frame(rows())
    };
    let (mut harness, asked) = library_page(&frame);
    harness.get_by_label("for all remaining").click();
    harness.run_steps(3);
    harness.get_by_label("Merge").click();
    harness.run_steps(3);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [UiCommand::Import(ImportStep::Resolve {
            track: 4,
            resolution: Resolution::Merge,
            rest_too: true,
        })]
    );
}

#[test]
fn busy_decks_and_the_result_show() {
    let busy = ControlFrame {
        import: ImportView::DecksBusy,
        ..frame(rows())
    };
    let (harness, _) = library_page(&busy);
    harness.get_by_label("Unload the decks to import: a loaded track would keep its old hotcues.");
    harness.get_by_label("Import from VirtualDJ…");
    drop(harness);
    let done = "Imported 2 tracks and 1 playlists. Backup: /tmp/backup.sqlite3";
    let finished = ControlFrame {
        import: ImportView::Done(done.to_owned()),
        ..frame(rows())
    };
    let (mut harness, asked) = library_page(&finished);
    harness.get_by_label(done);
    harness.get_by_label("OK").click();
    harness.run_steps(3);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [UiCommand::Import(ImportStep::Cancel)]
    );
}
