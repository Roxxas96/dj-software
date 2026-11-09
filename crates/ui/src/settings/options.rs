//! The Options page: every setting in a searchable list, with an editor for its value,
//! "Show modified" and "Reset all". The Audio and Controllers pages reuse the row editor.

use std::collections::HashMap;
use std::sync::mpsc::Sender;

use egui::{ComboBox, RichText, Ui};
use engine::EffectKind;

use crate::command::UiCommand;
use crate::model::SettingRow;
use crate::theme;

/// Whether `row` belongs in the list for `query`: a case-insensitive substring of its
/// key, category or help; with `modified_only`, only if it is not at its default.
pub fn matches(row: &SettingRow, query: &str, modified_only: bool) -> bool {
    if modified_only && row.value == row.default {
        return false;
    }
    let query = query.to_lowercase();
    [row.key.as_str(), row.category, row.help]
        .iter()
        .any(|text| text.to_lowercase().contains(&query))
}

/// What the Options page keeps from one frame to the next.
#[derive(Debug, Default)]
pub struct Options {
    pub query: String,
    pub modified_only: bool,
    /// The confirm dialog of "Reset all" is up.
    pub confirm_reset: bool,
}

/// The text of the fields being edited, by key: a field shows the row's value until the
/// viewer starts typing, and sends what was typed when it loses focus.
#[derive(Debug, Default)]
pub struct Drafts(HashMap<String, String>);

/// Asks the session to set `key` to `value`, TOML value text.
fn set(commands: &Sender<UiCommand>, key: &str, value: String) {
    // The session going away is the window closing: nothing to tell.
    let _ = commands.send(UiCommand::SetSetting {
        key: key.to_owned(),
        value,
    });
}

/// The search box, the filters, and the matching rows.
pub fn show(
    ui: &mut Ui,
    rows: &[SettingRow],
    options: &mut Options,
    drafts: &mut Drafts,
    commands: &Sender<UiCommand>,
) {
    ui.horizontal(|ui| {
        let label = ui.label("Search");
        ui.add(
            egui::TextEdit::singleline(&mut options.query)
                .hint_text("key, group or help")
                .desired_width(220.0),
        )
        .labelled_by(label.id);
        ui.checkbox(&mut options.modified_only, "Show modified");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Reset all").clicked() {
                options.confirm_reset = true;
            }
        });
    });
    ui.separator();
    egui::ScrollArea::vertical().show(ui, |ui| {
        for row in rows
            .iter()
            .filter(|row| matches(row, &options.query, options.modified_only))
        {
            show_row(ui, row, rows, drafts, commands);
        }
    });
}

/// The confirm dialog of "Reset all", while it is up.
pub fn confirm_reset(ctx: &egui::Context, options: &mut Options, commands: &Sender<UiCommand>) {
    if !options.confirm_reset {
        return;
    }
    let modal = egui::Modal::new(egui::Id::unique("confirm_reset_settings")).show(ctx, |ui| {
        ui.label("Put every setting back to its default?");
        ui.horizontal(|ui| {
            if ui.button("Reset").clicked() {
                let _ = commands.send(UiCommand::ResetSettings);
                ui.close();
            }
            if ui.button("Cancel").clicked() {
                ui.close();
            }
        });
    });
    if modal.should_close() {
        options.confirm_reset = false;
    }
}

/// One setting: its key, its editor, what it does, and why it was refused.
pub fn show_row(
    ui: &mut Ui,
    row: &SettingRow,
    rows: &[SettingRow],
    drafts: &mut Drafts,
    commands: &Sender<UiCommand>,
) {
    let p = theme::palette(ui.ctx());
    ui.horizontal(|ui| {
        ui.label(RichText::new(row.key.as_str()).monospace().strong());
        editor(ui, row, rows, drafts, commands);
        if row.pending {
            ui.label(RichText::new("applies at next launch").color(p.text_dim));
        }
    });
    ui.label(RichText::new(row.help).small().color(p.text_dim));
    if let Some(error) = &row.error {
        ui.label(RichText::new(error.as_str()).color(p.error));
    }
    ui.add_space(6.0);
}

/// The enumerated keys' choices.
fn choices(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "stems.knobs" => Some(&["vdj", "raw"]),
        "stems.eq_mode" => Some(&["eq", "stems"]),
        _ => None,
    }
}

/// The editor for `row`'s value: a checkbox for a boolean, a dropdown for an enumerated
/// key, three dropdowns for a deck's effects, otherwise a text field.
fn editor(
    ui: &mut Ui,
    row: &SettingRow,
    rows: &[SettingRow],
    drafts: &mut Drafts,
    commands: &Sender<UiCommand>,
) {
    if matches!(row.value.as_str(), "true" | "false") {
        let mut on = row.value == "true";
        if ui.checkbox(&mut on, "").clicked() {
            set(commands, &row.key, on.to_string());
        }
    } else if let Some(options) = choices(&row.key) {
        if let Some(chosen) = dropdown(
            ui,
            &row.key,
            &format!("{} choice", row.key),
            &unquote(&row.value),
            options,
        ) {
            set(commands, &row.key, format!("\"{chosen}\""));
        }
    } else if row.key.starts_with("fx.") {
        effects_editor(ui, row, rows, commands);
    } else {
        text_editor(ui, row, drafts, commands);
    }
}

/// A dropdown of `options` showing `current`, named `name` for screen readers and tests;
/// the one chosen, if any.
pub fn dropdown(
    ui: &mut Ui,
    id: &str,
    name: &str,
    current: &str,
    options: &[&str],
) -> Option<String> {
    let mut chosen = None;
    let combo = ComboBox::from_id_salt(id)
        .selected_text(current)
        .show_ui(ui, |ui| {
            for option in options {
                if ui.selectable_label(current == *option, *option).clicked() {
                    chosen = Some((*option).to_owned());
                }
            }
        });
    ui.ctx()
        .accesskit_node_builder(combo.response.id, |node| node.set_label(name));
    chosen
}

/// Three effect dropdowns. A deck without effects of its own shows the shared ones.
fn effects_editor(
    ui: &mut Ui,
    row: &SettingRow,
    rows: &[SettingRow],
    commands: &Sender<UiCommand>,
) {
    let mut ids = quoted(&row.value);
    if ids.len() != 3 {
        ids = rows
            .iter()
            .find(|shared| shared.key == "fx.slots")
            .map(|shared| quoted(&shared.value))
            .filter(|shared| shared.len() == 3)
            .unwrap_or_else(|| ["echo", "reverb", "flanger"].map(str::to_owned).to_vec());
    }
    let all = EffectKind::ALL.map(EffectKind::id);
    for slot in 0..3 {
        if let Some(chosen) = dropdown(
            ui,
            &format!("{}-{slot}", row.key),
            &format!("{} effect {}", row.key, slot + 1),
            &ids[slot],
            &all,
        ) {
            let mut next = ids.clone();
            next[slot] = chosen;
            let list = next
                .iter()
                .map(|id| format!("\"{id}\""))
                .collect::<Vec<_>>()
                .join(", ");
            set(commands, &row.key, format!("[{list}]"));
        }
    }
}

fn text_editor(ui: &mut Ui, row: &SettingRow, drafts: &mut Drafts, commands: &Sender<UiCommand>) {
    let mut text = drafts
        .0
        .get(&row.key)
        .cloned()
        .unwrap_or_else(|| row.value.clone());
    let response = ui.add(egui::TextEdit::singleline(&mut text).desired_width(220.0));
    if response.lost_focus() {
        drafts.0.remove(&row.key);
        if text != row.value {
            set(commands, &row.key, text);
        }
    } else if response.has_focus() || response.changed() {
        drafts.0.insert(row.key.clone(), text);
    }
}

/// `text` without the quotes of a TOML string.
pub fn unquote(text: &str) -> String {
    text.trim_matches('"').to_owned()
}

/// The quoted strings in `text`, such as a TOML array of strings.
fn quoted(text: &str) -> Vec<String> {
    text.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SettingRow;

    fn row(key: &str, category: &'static str, help: &'static str, value: &str) -> SettingRow {
        SettingRow {
            key: key.to_owned(),
            category,
            value: value.to_owned(),
            default: "\"vdj\"".to_owned(),
            help,
            live: true,
            pending: false,
            error: None,
        }
    }

    #[test]
    fn search_and_modified_filter() {
        let knobs = row("stems.knobs", "Stems", "What the knobs do", "\"vdj\"");
        let kicks = row("stems.kicks_hz", "Stems", "Where kicks end", "\"vdj\"");
        let device = row("audio.device", "Audio", "The output", "\"vdj\"");
        assert!(matches(&knobs, "knob", false));
        assert!(!matches(&device, "knob", false));
        assert!(matches(&device, "", false));
        for r in [&knobs, &kicks] {
            assert!(matches(r, "STEMS", false));
        }
        assert!(!matches(&device, "STEMS", false));
        assert!(matches(&device, "OUTPUT", false));
        assert!(!matches(&knobs, "knob", true));
        let changed = row("stems.knobs", "Stems", "", "\"raw\"");
        assert!(matches(&changed, "knob", true));
    }
}
