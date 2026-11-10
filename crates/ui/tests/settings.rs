//! The settings window, headless: it opens from the gear, lists the settings, and every
//! edit is a command for the session.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use ui::{
    ControlFrame, ExportChoice, ExportStep, ExportSummary, ExportView, ImportStep, ImportSummary,
    ImportView, Layout, PlaylistLabel, Resolution, SettingRow, StickLabel, TrackLabel, UiCommand,
    ViewChange,
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

/// The USB export page of a window whose frame shows `export`.
fn export_page(export: ExportView) -> ControlFrame {
    ControlFrame {
        export,
        ..frame(rows())
    }
}

fn open_export(frame: &ControlFrame) -> (Harness<'_>, Receiver<UiCommand>) {
    let (mut harness, asked) = open(frame);
    harness.get_by_label("USB export").click();
    harness.run_steps(3);
    (harness, asked)
}

fn stick(name: &str, exported: Vec<i64>) -> StickLabel {
    StickLabel {
        name: name.to_owned(),
        free: "512.0 MB".to_owned(),
        exported,
    }
}

fn choice() -> ExportChoice {
    ExportChoice {
        sticks: vec![stick("ALPHA", Vec::new()), stick("BRAVO", vec![2])],
        playlists: vec![
            PlaylistLabel {
                id: 1,
                name: "Warmup".to_owned(),
                tracks: 12,
            },
            PlaylistLabel {
                id: 2,
                name: "Peak".to_owned(),
                tracks: 30,
            },
        ],
    }
}

fn export_summary(too_small: bool) -> ExportSummary {
    ExportSummary {
        stick: "BRAVO".to_owned(),
        playlists: 2,
        tracks: 40,
        to_copy: 5,
        copy_size: "52.4 MB".to_owned(),
        kept: 35,
        free: "12.0 MB".to_owned(),
        without_serato_cues: 3,
        skipped: 1,
        to_remove: 4,
        too_small,
        free_unknown: false,
    }
}

#[test]
fn the_export_page_starts_with_a_button() {
    let frame = export_page(ExportView::Idle);
    let (mut harness, asked) = open_export(&frame);
    harness.get_by_label_contains("Your music files are never changed.");
    harness.get_by_label("Export to USB stick…").click();
    harness.run_steps(3);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [UiCommand::Export(ExportStep::Open)]
    );
}

#[test]
fn export_page_lists_sticks_and_ticks_exported() {
    let frame = export_page(ExportView::Choose(choice()));
    let (mut harness, asked) = open_export(&frame);
    // The first stick has nothing on it: nothing ticked, so no check.
    assert!(harness.get_by_label("Check").accesskit_node().is_disabled());
    harness.get_by_label("Warmup (12)");
    harness.get_by_label("USB stick").click();
    harness.run_steps(3);
    harness.get_by_label("BRAVO (512.0 MB free)").click();
    harness.run_steps(3);
    harness.get_by_label("Check").click();
    harness.run_steps(3);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [UiCommand::Export(ExportStep::Check {
            stick: 1,
            playlists: vec![2],
        })]
    );
}

#[test]
fn refresh_keeps_the_chosen_stick_and_its_ticks() {
    let frame = std::cell::RefCell::new(export_page(ExportView::Choose(choice())));
    let (commands, asked) = channel();
    let mut screen = ui::Screen {
        commands,
        ..ui::Screen::default()
    };
    let mut harness = Harness::builder()
        .with_size([1440.0, 900.0])
        .build_ui(|ui| ui::root(ui, &frame.borrow(), None, &mut screen));
    harness.run_steps(2);
    harness.get_by_label("Open settings").click();
    harness.run_steps(3);
    harness.get_by_label("USB export").click();
    harness.run_steps(3);
    harness.get_by_label("USB stick").click();
    harness.run_steps(3);
    harness.get_by_label("BRAVO (512.0 MB free)").click();
    harness.run_steps(3);
    // BRAVO holds Peak; Warmup is ticked by hand.
    harness.get_by_label("Warmup (12)").click();
    harness.run_steps(3);
    harness.get_by_label("Refresh").click();
    harness.run_steps(3);
    frame.borrow_mut().export = ExportView::Listing;
    harness.run_steps(3);
    // A new stick comes first: BRAVO's index changes.
    let mut refreshed = choice();
    refreshed.sticks.insert(0, stick("AARDVARK", Vec::new()));
    frame.borrow_mut().export = ExportView::Choose(refreshed);
    harness.run_steps(3);
    harness.get_by_label("Check").click();
    harness.run_steps(3);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [
            UiCommand::Export(ExportStep::Open),
            UiCommand::Export(ExportStep::Check {
                stick: 2,
                playlists: vec![1, 2],
            }),
        ]
    );
}

#[test]
fn ticking_a_playlist_adds_it_to_the_check() {
    let frame = export_page(ExportView::Choose(choice()));
    let (mut harness, asked) = open_export(&frame);
    harness.get_by_label("Peak (30)").click();
    harness.run_steps(3);
    harness.get_by_label("Warmup (12)").click();
    harness.run_steps(3);
    harness.get_by_label("Check").click();
    harness.run_steps(3);
    harness.get_by_label("Refresh").click();
    harness.run_steps(3);
    harness.get_by_label("Cancel").click();
    harness.run_steps(3);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [
            UiCommand::Export(ExportStep::Check {
                stick: 0,
                playlists: vec![1, 2],
            }),
            UiCommand::Export(ExportStep::Open),
            UiCommand::Export(ExportStep::Close),
        ]
    );
}

#[test]
fn no_stick_message() {
    let frame = export_page(ExportView::Choose(ExportChoice {
        sticks: Vec::new(),
        ..choice()
    }));
    let (mut harness, asked) = open_export(&frame);
    harness.get_by_label("No USB stick found. Plug one in, then Refresh.");
    assert!(harness.query_by_label("Check").is_none());
    harness.get_by_label("Refresh").click();
    harness.run_steps(3);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [UiCommand::Export(ExportStep::Open)]
    );
}

#[test]
fn the_export_summary_shows_the_cli_lines() {
    let summary = export_summary(false);
    let frame = export_page(ExportView::Summary(summary.clone()));
    let (mut harness, asked) = open_export(&frame);
    harness.get_by_label("Stick: BRAVO");
    harness.get_by_label("Playlists: 2, tracks: 40");
    harness.get_by_label("To copy: 5 tracks (52.4 MB), already on the stick: 35");
    harness.get_by_label("Free on the stick: 12.0 MB");
    harness.get_by_label("Copied without Serato cues (not MP3 or WAV): 3");
    harness.get_by_label("Skipped (missing or unreadable): 1");
    harness.get_by_label("Removed from the stick (no longer exported): 4");
    assert!(harness.query_by_label(&summary.too_small_line()).is_none());
    harness.get_by_label("Export").click();
    harness.run_steps(3);
    harness.get_by_label("Back").click();
    harness.run_steps(3);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [
            UiCommand::Export(ExportStep::Write),
            UiCommand::Export(ExportStep::Back),
        ]
    );
}

#[test]
fn export_disabled_when_too_small() {
    let frame = export_page(ExportView::Summary(export_summary(true)));
    let (mut harness, asked) = open_export(&frame);
    harness.get_by_label("The stick is too small: 52.4 MB to copy, 12.0 MB free.");
    let export = harness.get_by_label("Export");
    assert!(export.accesskit_node().is_disabled());
    export.click();
    harness.run_steps(3);
    assert_eq!(asked.try_iter().collect::<Vec<_>>(), []);
}

#[test]
fn export_disabled_when_free_space_is_unknown() {
    let summary = ExportSummary {
        free: "?".to_owned(),
        free_unknown: true,
        ..export_summary(false)
    };
    let frame = export_page(ExportView::Summary(summary.clone()));
    let (mut harness, asked) = open_export(&frame);
    harness.get_by_label("Open Jockey cannot read the stick's free space.");
    assert!(harness.query_by_label(&summary.too_small_line()).is_none());
    let export = harness.get_by_label("Export");
    assert!(export.accesskit_node().is_disabled());
    export.click();
    harness.run_steps(3);
    assert_eq!(asked.try_iter().collect::<Vec<_>>(), []);
}

#[test]
fn the_export_shows_progress_then_the_result() {
    let frame = export_page(ExportView::Writing { done: 3, total: 9 });
    let (harness, _) = open_export(&frame);
    harness.get_by_label("Exporting… 3 of 9");
    drop(harness);
    let done = "Exported 2 playlists to BRAVO: 5 copied, 35 kept.\nTo open them in Rekordbox (also in IMPORT TO REKORDBOX.txt on the stick):\n\
                1. In Rekordbox, open Preferences > Advanced > Database.";
    let frame = export_page(ExportView::Done(format!("{done}\nA note")));
    let (mut harness, asked) = open_export(&frame);
    harness.get_by_label_contains("Exported 2 playlists to BRAVO");
    harness.get_by_label_contains("IMPORT TO REKORDBOX.txt");
    harness.get_by_label_contains("A note");
    harness.get_by_label("OK").click();
    harness.run_steps(3);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [UiCommand::Export(ExportStep::Close)]
    );
    drop(harness);
    let frame = export_page(ExportView::Failed("the stick is gone".to_owned()));
    let (mut harness, asked) = open_export(&frame);
    harness.get_by_label("the stick is gone");
    harness.get_by_label("OK").click();
    harness.run_steps(3);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [UiCommand::Export(ExportStep::Close)]
    );
}
