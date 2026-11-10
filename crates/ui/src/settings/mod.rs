//! The settings window, in the style of VirtualDJ's: a sidebar (Audio, Interface,
//! Controllers, Options, Library, USB export and what comes Later) and a page beside it. Every change is a
//! [`UiCommand`] for the session; the pages draw the rows the session published, so an
//! edit shows one frame later.

mod export;
mod interface;
mod library;
pub mod options;

use std::sync::mpsc::Sender;

use egui::{Context, RichText, Ui};

use crate::command::UiCommand;
use crate::model::{ControlFrame, SettingRow};
use crate::theme;

/// The window takes this share of the screen when it opens.
const SHARE: f32 = 0.7;
/// The audio device list's entry for the system's default output.
const DEFAULT_OUTPUT: &str = "Default output";
const SIDEBAR_WIDTH: f32 = 130.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Page {
    #[default]
    Audio,
    Interface,
    Controllers,
    Options,
    Library,
    Export,
    Later,
}

impl Page {
    const ALL: [Page; 7] = [
        Page::Audio,
        Page::Interface,
        Page::Controllers,
        Page::Options,
        Page::Library,
        Page::Export,
        Page::Later,
    ];

    fn name(self) -> &'static str {
        match self {
            Page::Audio => "Audio",
            Page::Interface => "Interface",
            Page::Controllers => "Controllers",
            Page::Options => "Options",
            Page::Library => "Library",
            Page::Export => "USB export",
            Page::Later => "Later",
        }
    }
}

/// What the settings window keeps from one frame to the next.
#[derive(Debug, Default)]
pub struct SettingsWindow {
    pub open: bool,
    page: Page,
    options: options::Options,
    drafts: options::Drafts,
    /// The import's "for all remaining" box.
    import_rest: bool,
    /// The export's chosen stick and ticked playlists.
    export: export::Draft,
}

/// Draws the window while it is open.
pub fn show(
    ctx: &Context,
    frame: &ControlFrame,
    state: &mut SettingsWindow,
    commands: &Sender<UiCommand>,
) {
    if !state.open {
        // What was typed and not sent is dropped with the window.
        state.drafts.clear();
        return;
    }
    let mut open = true;
    egui::Window::new("Settings")
        .open(&mut open)
        .collapsible(false)
        .default_size(ctx.content_rect().size() * SHARE)
        .show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(SIDEBAR_WIDTH);
                    for page in Page::ALL {
                        if ui
                            .selectable_label(state.page == page, page.name())
                            .clicked()
                        {
                            state.page = page;
                        }
                    }
                });
                ui.separator();
                ui.vertical(|ui| page(ui, frame, state, commands));
            });
        });
    options::confirm_reset(ctx, &mut state.options, commands);
    state.open = open;
}

fn page(
    ui: &mut Ui,
    frame: &ControlFrame,
    state: &mut SettingsWindow,
    commands: &Sender<UiCommand>,
) {
    let rows = frame.settings.as_slice();
    match state.page {
        Page::Audio => audio(ui, frame, state, commands),
        Page::Interface => interface::show(ui, &frame.view, commands),
        Page::Controllers => controllers(ui, frame, state, commands),
        Page::Options if rows.is_empty() => unavailable(ui, frame),
        Page::Options => options::show(ui, rows, &mut state.options, &mut state.drafts, commands),
        Page::Library => library::show(ui, &frame.import, &mut state.import_rest, commands),
        Page::Export => export::show(ui, &frame.export, &mut state.export, commands),
        Page::Later => later(ui),
    }
}

/// The rows whose key starts with `prefix`.
fn rows_of<'a>(rows: &'a [SettingRow], prefix: &str) -> impl Iterator<Item = &'a SettingRow> {
    let prefix = prefix.to_owned();
    rows.iter().filter(move |row| row.key.starts_with(&prefix))
}

fn value_of(rows: &[SettingRow], key: &str) -> String {
    rows.iter()
        .find(|row| row.key == key)
        .map(|row| options::unquote(&row.value))
        .unwrap_or_default()
}

/// Which setting a dropdown sets, and what it is called.
struct Choice<'a> {
    label: &'a str,
    name: &'a str,
    key: &'a str,
}

/// A dropdown of `names` for the setting `key`; sends the one chosen as a string.
fn choose(
    ui: &mut Ui,
    what: Choice<'_>,
    current: &str,
    names: &[&str],
    default: Option<&str>,
    commands: &Sender<UiCommand>,
) {
    let Choice { label, name, key } = what;
    ui.horizontal(|ui| {
        ui.label(label);
        let mut options: Vec<&str> = default.into_iter().collect();
        options.extend_from_slice(names);
        if let Some(chosen) = options::dropdown(ui, label, name, current, &options) {
            // An empty value removes the key: back to the default.
            let value = if default == Some(chosen.as_str()) {
                String::new()
            } else {
                format!("\"{chosen}\"")
            };
            let _ = commands.send(UiCommand::SetSetting {
                key: key.to_owned(),
                value,
            });
        }
        if names.is_empty() && default.is_some() {
            ui.label("no devices found");
        }
    });
    ui.add_space(6.0);
}

fn audio(
    ui: &mut Ui,
    frame: &ControlFrame,
    state: &mut SettingsWindow,
    commands: &Sender<UiCommand>,
) {
    let rows = frame.settings.as_slice();
    let current = value_of(rows, "audio.device");
    let current = if current.is_empty() {
        DEFAULT_OUTPUT
    } else {
        current.as_str()
    };
    let names: Vec<&str> = frame.devices.iter().map(String::as_str).collect();
    choose(
        ui,
        Choice {
            label: "Device",
            name: "Output device",
            key: "audio.device",
        },
        current,
        &names,
        Some(DEFAULT_OUTPUT),
        commands,
    );
    for row in rows_of(rows, "audio.") {
        options::show_row(ui, row, rows, &mut state.drafts, commands);
    }
}

fn controllers(
    ui: &mut Ui,
    frame: &ControlFrame,
    state: &mut SettingsWindow,
    commands: &Sender<UiCommand>,
) {
    let rows = frame.settings.as_slice();
    let names: Vec<&str> = frame.mappings.iter().map(String::as_str).collect();
    choose(
        ui,
        Choice {
            label: "Mapping",
            name: "Controller mapping",
            key: "controller.mapping",
        },
        &value_of(rows, "controller.mapping"),
        &names,
        None,
        commands,
    );
    for row in rows_of(rows, "controller.") {
        options::show_row(ui, row, rows, &mut state.drafts, commands);
    }
}

/// In place of the settings before the session has them: why, if it stopped.
fn unavailable(ui: &mut Ui, frame: &ControlFrame) {
    let p = theme::palette(ui.ctx());
    match &frame.error {
        Some(error) => ui.label(RichText::new(error.as_str()).color(p.error)),
        None => ui.label(RichText::new("settings are not available yet").color(p.text_dim)),
    };
}

/// What will come, with nothing to set yet.
fn later(ui: &mut Ui) {
    let dim = theme::palette(ui.ctx()).text_dim;
    ui.label("Coming in later versions:");
    for item in ["Recording", "Broadcast", "Automix"] {
        ui.label(RichText::new(item).color(dim));
    }
}
