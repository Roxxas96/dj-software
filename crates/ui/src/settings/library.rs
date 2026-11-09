//! The Library page: the import from VirtualDJ, one step at a time as the session
//! publishes it ([`ImportView`]).

use std::sync::mpsc::Sender;

use egui::{RichText, Ui};

use crate::command::{ImportStep, Resolution, UiCommand};
use crate::model::{ImportSummary, ImportView};
use crate::theme;

const IMPORT: &str = "Import from VirtualDJ…";
const DECKS_BUSY: &str = "Unload the decks to import: a loaded track would keep its old hotcues.";

/// Draws the page; `rest_too` is the "for all remaining" box, kept between frames.
pub(super) fn show(
    ui: &mut Ui,
    view: &ImportView,
    rest_too: &mut bool,
    commands: &Sender<UiCommand>,
) {
    if !matches!(view, ImportView::Ask { .. }) {
        // A new import starts with the box clear.
        *rest_too = false;
    }
    let p = theme::palette(ui.ctx());
    let send = |step| {
        let _ = commands.send(UiCommand::Import(step));
    };
    ui.label(
        RichText::new(
            "Brings VirtualDJ's hotcues, saved loops and playlists into the library. \
             VirtualDJ's files are only read.",
        )
        .color(p.text_dim),
    );
    ui.add_space(6.0);
    match view {
        ImportView::Idle => {
            if ui.button(IMPORT).clicked() {
                send(ImportStep::Scan);
            }
        }
        ImportView::Scanning => {
            ui.label("Reading VirtualDJ's data…");
        }
        ImportView::Summary(summary) => {
            for line in summary_lines(summary) {
                ui.label(line);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Import").clicked() {
                    send(ImportStep::Apply);
                }
                if ui.button("Cancel").clicked() {
                    send(ImportStep::Cancel);
                }
            });
        }
        ImportView::Ask {
            track,
            existing,
            incoming,
            left,
        } => {
            ui.label(format!(
                "{} has {existing} hotcues here and {incoming} in VirtualDJ.",
                track.name
            ));
            ui.label(RichText::new(format!("Tracks left to choose for: {left}")).color(p.text_dim));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                for (label, resolution) in [
                    ("Keep mine", Resolution::Keep),
                    ("Use VirtualDJ's", Resolution::Replace),
                    ("Merge", Resolution::Merge),
                ] {
                    if ui.button(label).clicked() {
                        send(ImportStep::Resolve {
                            track: track.id,
                            resolution,
                            rest_too: *rest_too,
                        });
                        *rest_too = false;
                    }
                }
            });
            ui.checkbox(rest_too, "for all remaining");
            if ui.button("Cancel").clicked() {
                send(ImportStep::Cancel);
            }
        }
        ImportView::Applying => {
            ui.label("Importing…");
        }
        ImportView::Done(text) => {
            ui.label(text.as_str());
            if ui.button("OK").clicked() {
                send(ImportStep::Cancel);
            }
        }
        ImportView::Failed(why) => {
            ui.label(RichText::new(why.as_str()).color(p.error));
            if ui.button("OK").clicked() {
                send(ImportStep::Cancel);
            }
        }
        ImportView::DecksBusy => {
            ui.label(RichText::new(DECKS_BUSY).color(p.error));
            if ui.button(IMPORT).clicked() {
                send(ImportStep::Scan);
            }
        }
    }
}

/// The summary, a count per line.
fn summary_lines(s: &ImportSummary) -> [String; 8] {
    [
        format!("VirtualDJ databases: {}", s.databases),
        format!(
            "Tracks found in the library: {}, with hotcues or loops: {}",
            s.tracks, s.with_marks
        ),
        format!(
            "Tracks with hotcues here too, you choose for each: {}",
            s.conflicting
        ),
        format!("Hotcues: {}, saved loops: {}", s.hotcues, s.loops),
        format!("Dropped, over 8 slots: {}", s.dropped),
        format!("Other markers, not imported: {}", s.other_markers),
        format!(
            "Playlists to create: {}, already here: {}, skipped: {}",
            s.playlists_new, s.playlists_here, s.playlists_skipped
        ),
        format!("Not in the library, not added: {}", s.unmatched),
    ]
}
