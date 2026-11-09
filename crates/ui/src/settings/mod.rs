//! The settings window, in the style of VirtualDJ's: a sidebar (Audio, Interface,
//! Controllers, Options and what comes Later) and a page beside it. Every change is a
//! [`UiCommand`] for the session; the pages draw the rows the session published, so an
//! edit shows one frame later.

mod interface;
pub mod options;

use std::sync::mpsc::Sender;

use egui::{Context, RichText, Ui};

use crate::command::UiCommand;
use crate::model::{ControlFrame, SettingRow};
use crate::theme;

/// The window takes this share of the screen when it opens.
const SHARE: f32 = 0.7;
const SIDEBAR_WIDTH: f32 = 130.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Page {
    #[default]
    Audio,
    Interface,
    Controllers,
    Options,
    Later,
}

impl Page {
    const ALL: [Page; 5] = [
        Page::Audio,
        Page::Interface,
        Page::Controllers,
        Page::Options,
        Page::Later,
    ];

    fn name(self) -> &'static str {
        match self {
            Page::Audio => "Audio",
            Page::Interface => "Interface",
            Page::Controllers => "Controllers",
            Page::Options => "Options",
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
}

/// Draws the window while it is open.
pub fn show(
    ctx: &Context,
    frame: &ControlFrame,
    state: &mut SettingsWindow,
    commands: &Sender<UiCommand>,
) {
    if !state.open {
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
        Page::Controllers => controllers(ui, rows, state, commands),
        Page::Options => options::show(ui, rows, &mut state.options, &mut state.drafts, commands),
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

/// A dropdown of `names` for the setting `key`; sends the one chosen as a string.
fn choose(
    ui: &mut Ui,
    label: &str,
    name: &str,
    key: &str,
    current: &str,
    names: &[&str],
    commands: &Sender<UiCommand>,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        if let Some(name) = options::dropdown(ui, label, name, current, names) {
            let _ = commands.send(UiCommand::SetSetting {
                key: key.to_owned(),
                value: format!("\"{name}\""),
            });
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
        "default output"
    } else {
        current.as_str()
    };
    let names: Vec<&str> = frame.devices.iter().map(String::as_str).collect();
    choose(
        ui,
        "Device",
        "Output device",
        "audio.device",
        current,
        &names,
        commands,
    );
    for row in rows_of(rows, "audio.") {
        options::show_row(ui, row, rows, &mut state.drafts, commands);
    }
}

fn controllers(
    ui: &mut Ui,
    rows: &[SettingRow],
    state: &mut SettingsWindow,
    commands: &Sender<UiCommand>,
) {
    let names: Vec<&str> = controller::builtin::builtin_names().collect();
    choose(
        ui,
        "Mapping",
        "Controller mapping",
        "controller.mapping",
        &value_of(rows, "controller.mapping"),
        &names,
        commands,
    );
    for row in rows_of(rows, "controller.") {
        options::show_row(ui, row, rows, &mut state.drafts, commands);
    }
}

/// What will come, with nothing to set yet.
fn later(ui: &mut Ui) {
    let dim = theme::palette(ui.ctx()).text_dim;
    ui.label("Coming in later versions:");
    for item in ["Recording", "Broadcast", "Automix"] {
        ui.label(RichText::new(item).color(dim));
    }
}
