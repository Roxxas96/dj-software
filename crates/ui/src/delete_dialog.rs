//! The question before anything is deleted from the browser: the Delete key, or a
//! "Delete…" menu item, opens it ([`open`]) and asks the session what deleting would do
//! ([`LibraryStep::AskDelete`]). Until the answer comes ([`answer`]) it says so, with
//! Cancel and the button that deletes held off; then the question with Cancel focused, or
//! why it cannot be deleted with OK in Cancel's place. Only the button right of Cancel
//! sends [`LibraryStep::Delete`].
//!
//! Nothing moves under the pointer when the answer comes: the text sits in a box of one
//! height and the buttons stay where they are, Cancel first. The button that deletes
//! takes clicks only [`ARMING`] after it is offered, so a click aimed at the dialog
//! before the answer cannot land on it.
//!
//! The dialog is modal: a click outside it cancels and does nothing else, and so does
//! Escape. The open dialog and the latest answer are kept in egui's memory.

use std::time::Duration;

use egui::{Button, Context, Id, Key, Modal, ScrollArea, Ui};

use crate::command::{DeleteTarget, LibraryStep};
use crate::model::DeleteCheck;

/// How long the button that deletes ignores clicks once the question shows.
pub const ARMING: Duration = Duration::from_millis(300);

/// The dialog's width, and the height of its text box, in points.
const WIDTH: f32 = 320.0;
const TEXT_HEIGHT: f32 = 72.0;

/// What is shown, to give its first button the keyboard once each time it changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// No answer yet.
    Waiting,
    /// The question, with the button that deletes.
    Asking,
    /// Why it cannot be deleted.
    Refused,
}

/// An open dialog.
#[derive(Debug, Clone)]
struct Open {
    target: DeleteTarget,
    /// The `seq` of the latest answer when the dialog opened: only a newer one is its own.
    after: u64,
    /// The stage whose button last took the keyboard.
    focused: Option<Stage>,
    /// When the question first showed (egui's time, in seconds).
    asked_at: Option<f64>,
}

fn dialog_id() -> Id {
    Id::NULL.with("open-jockey-delete-dialog")
}

fn answer_id() -> Id {
    Id::NULL.with("open-jockey-delete-answer")
}

/// Takes the session's latest answer (the frame's), for the open dialog to show. Called
/// once a frame, before the browser draws; an answer seen already is not copied again.
pub fn answer(ctx: &Context, check: Option<&DeleteCheck>) {
    let Some(check) = check else { return };
    let seen = ctx.data(|d| d.get_temp::<DeleteCheck>(answer_id()).map(|c| c.seq));
    if seen != Some(check.seq) {
        ctx.data_mut(|d| d.insert_temp(answer_id(), check.clone()));
    }
}

/// Whether the dialog is open: the browser's keys leave it the keyboard.
pub(crate) fn is_open(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<Open>(dialog_id()).is_some())
}

/// Opens the dialog for `target`, replacing any open one, and pushes the question to the
/// session onto `steps`.
pub(crate) fn open(ctx: &Context, target: DeleteTarget, steps: &mut Vec<LibraryStep>) {
    let after = ctx
        .data(|d| d.get_temp::<DeleteCheck>(answer_id()).map(|c| c.seq))
        .unwrap_or(0);
    let open = Open {
        target: target.clone(),
        after,
        focused: None,
        asked_at: None,
    };
    ctx.data_mut(|d| d.insert_temp(dialog_id(), open));
    steps.push(LibraryStep::AskDelete(target));
}

/// Draws the open dialog, if there is one, pushing what it decides onto `steps`.
pub(crate) fn show(ctx: &Context, steps: &mut Vec<LibraryStep>) {
    let id = dialog_id();
    let Some(mut open) = ctx.data(|d| d.get_temp::<Open>(id)) else {
        return;
    };
    let check = ctx
        .data(|d| d.get_temp::<DeleteCheck>(answer_id()))
        .filter(|c| c.seq > open.after && c.target == open.target);
    let escape = ctx.input(|i| i.key_pressed(Key::Escape));
    let shown = Modal::new(id.with("modal")).show(ctx, |ui| {
        contents(ui, &mut open, check.as_ref().map(|c| &c.answer), steps)
    });
    let backdrop = &shown.backdrop_response;
    let outside = backdrop.is_pointer_button_down_on() || backdrop.clicked();
    let close = escape || outside || shown.inner;
    ctx.data_mut(|d| {
        if close {
            d.remove::<Open>(id);
        } else {
            d.insert_temp(id, open);
        }
    });
}

/// The heading and the button that deletes, for `target`.
fn words(target: &DeleteTarget) -> (&'static str, &'static str) {
    match target {
        DeleteTarget::Entries { .. } | DeleteTarget::AllEntries { .. } => {
            ("Remove from playlist", "Remove")
        }
        DeleteTarget::Playlist { .. } => ("Delete playlist", "Delete"),
        DeleteTarget::Files { .. }
        | DeleteTarget::AllFiles { .. }
        | DeleteTarget::Folder { .. } => ("Move to the Trash", "Move to Trash"),
    }
}

/// The dialog's contents; `true` once it is done with.
fn contents(
    ui: &mut Ui,
    open: &mut Open,
    answer: Option<&Result<String, String>>,
    steps: &mut Vec<LibraryStep>,
) -> bool {
    ui.set_width(WIDTH);
    let (title, verb) = words(&open.target);
    ui.heading(title);
    let (stage, text) = match answer {
        None => (Stage::Waiting, "Checking\u{2026}"),
        Some(Ok(question)) => (Stage::Asking, question.as_str()),
        Some(Err(reason)) => (Stage::Refused, reason.as_str()),
    };
    // One height whatever the text: the buttons below never move.
    ScrollArea::vertical()
        .max_height(TEXT_HEIGHT)
        .min_scrolled_height(TEXT_HEIGHT)
        .auto_shrink([false, false])
        .show(ui, |ui| ui.label(text));
    // The button that does nothing takes the keyboard: Enter then cancels, and deleting
    // takes a Tab to the other button, or a click.
    let focus = open.focused != Some(stage);
    open.focused = Some(stage);
    let now = ui.input(|i| i.time);
    let armed = match (stage, open.asked_at) {
        (Stage::Asking, None) => {
            open.asked_at = Some(now);
            false
        }
        (Stage::Asking, Some(at)) => now - at >= ARMING.as_secs_f64(),
        _ => false,
    };
    if stage == Stage::Asking && !armed {
        ui.ctx().request_repaint_after(ARMING);
    }
    let mut done = false;
    ui.horizontal(|ui| {
        let first = ui.button(if stage == Stage::Refused {
            "OK"
        } else {
            "Cancel"
        });
        if focus {
            first.request_focus();
        }
        done = first.clicked();
        if stage != Stage::Refused && ui.add_enabled(armed, Button::new(verb)).clicked() {
            steps.push(LibraryStep::Delete(open.target.clone()));
            done = true;
        }
    });
    done
}
