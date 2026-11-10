//! The USB export page: the export of playlists to a stick, one step at a time as the
//! session publishes it ([`ExportView`]).

use std::sync::mpsc::Sender;

use egui::{Button, ComboBox, ProgressBar, RichText, Ui};

use crate::command::{ExportStep, UiCommand};
use crate::model::{ExportChoice, ExportView};
use crate::theme;

const NO_STICK: &str = "No USB stick found. Plug one in, then Refresh.";

/// What the page keeps between frames while the sticks and playlists are chosen.
#[derive(Debug, Default)]
pub(super) struct Draft {
    /// The chosen stick, an index in the choice's sticks.
    stick: usize,
    /// The ticked playlists, by library id.
    ticked: Vec<i64>,
    /// The name of the stick the ticks were taken from; when the chosen stick is another,
    /// the ticks become what that one already holds.
    ticks_from: Option<String>,
}

/// Draws the page; `draft` holds the choice so far.
pub(super) fn show(
    ui: &mut Ui,
    view: &ExportView,
    draft: &mut Draft,
    commands: &Sender<UiCommand>,
) {
    if matches!(view, ExportView::Idle) {
        // A new export starts from what the chosen stick holds.
        *draft = Draft::default();
    }
    let p = theme::palette(ui.ctx());
    let send = |step| {
        let _ = commands.send(UiCommand::Export(step));
    };
    ui.label(
        RichText::new(
            "Copies playlists, in order, with their hotcues and saved loops, to a USB stick \
             for Serato, Rekordbox and VirtualDJ. Your music files are never changed.",
        )
        .color(p.text_dim),
    );
    ui.add_space(6.0);
    match view {
        ExportView::Idle => {
            if ui.button("Export to USB stick…").clicked() {
                send(ExportStep::Open);
            }
        }
        ExportView::Listing => {
            ui.label("Looking for USB sticks…");
        }
        ExportView::Choose(choice) => {
            if let Some(step) = choose(ui, choice, draft) {
                send(step);
            }
        }
        ExportView::Checking => {
            ui.label("Checking the stick…");
        }
        ExportView::Summary(summary) => {
            for line in summary.lines() {
                ui.label(line);
            }
            if summary.too_small {
                ui.label(RichText::new(summary.too_small_line()).color(p.error));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!summary.too_small, Button::new("Export"))
                    .clicked()
                {
                    send(ExportStep::Write);
                }
                if ui.button("Back").clicked() {
                    send(ExportStep::Back);
                }
            });
        }
        ExportView::Writing { done, total } => {
            ui.label(format!("Exporting… {done} of {total}"));
            let share = *done as f32 / (*total).max(1) as f32;
            ui.add(ProgressBar::new(share));
        }
        ExportView::Done(text) => {
            ui.label(text.as_str());
            if ui.button("OK").clicked() {
                send(ExportStep::Close);
            }
        }
        ExportView::Failed(why) => {
            ui.label(RichText::new(why.as_str()).color(p.error));
            if ui.button("OK").clicked() {
                send(ExportStep::Close);
            }
        }
    }
}

/// The stick and playlists to export, and the buttons; the step clicked, if any.
fn choose(ui: &mut Ui, choice: &ExportChoice, draft: &mut Draft) -> Option<ExportStep> {
    let mut step = None;
    if choice.sticks.is_empty() {
        ui.label(NO_STICK);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Refresh").clicked() {
                step = Some(ExportStep::Open);
            }
            if ui.button("Cancel").clicked() {
                step = Some(ExportStep::Close);
            }
        });
        return step;
    }
    if draft.stick >= choice.sticks.len() {
        draft.stick = 0;
    }
    let label = |i: usize| {
        let stick = &choice.sticks[i];
        format!("{} ({} free)", stick.name, stick.free)
    };
    let combo = ComboBox::from_id_salt("export.stick")
        .selected_text(label(draft.stick))
        .show_ui(ui, |ui| {
            for i in 0..choice.sticks.len() {
                ui.selectable_value(&mut draft.stick, i, label(i));
            }
        });
    ui.ctx()
        .accesskit_node_builder(combo.response.id, |node| node.set_label("USB stick"));
    let stick = &choice.sticks[draft.stick];
    if draft.ticks_from.as_deref() != Some(stick.name.as_str()) {
        draft.ticked.clone_from(&stick.exported);
        draft.ticks_from = Some(stick.name.clone());
    }
    ui.add_space(6.0);
    for playlist in &choice.playlists {
        let mut ticked = draft.ticked.contains(&playlist.id);
        let text = format!("{} ({})", playlist.name, playlist.tracks);
        if ui.checkbox(&mut ticked, text).changed() {
            if ticked {
                draft.ticked.push(playlist.id);
            } else {
                draft.ticked.retain(|&id| id != playlist.id);
            }
        }
    }
    // In the library's order, and only the playlists still there.
    let ticked: Vec<i64> = choice
        .playlists
        .iter()
        .map(|p| p.id)
        .filter(|id| draft.ticked.contains(id))
        .collect();
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!ticked.is_empty(), Button::new("Check"))
            .clicked()
        {
            step = Some(ExportStep::Check {
                stick: draft.stick,
                playlists: ticked.clone(),
            });
        }
        if ui.button("Refresh").clicked() {
            step = Some(ExportStep::Open);
        }
        if ui.button("Cancel").clicked() {
            step = Some(ExportStep::Close);
        }
    });
    step
}
