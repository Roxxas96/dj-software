//! The small dialogs of the browser tree's playlist menus: "New playlist", "Rename
//! playlist" (a name field, OK and Cancel) and "Delete playlist" (which asks first, with
//! Cancel focused). What they decide goes out as [`LibraryStep`]s; the session checks the
//! name, where the playlists are known. Enter in the name field is OK and Escape is
//! Cancel; the dialogs are modal, so a click outside cancels and does nothing else.
//!
//! The open dialog is kept in egui's memory: the tree's menus open it ([`open`]) and the
//! browser draws it once a frame on top of everything ([`show`]).

use egui::text::{CCursor, CCursorRange};
use egui::{Context, Id, Key, Modal, TextEdit, Ui};

use crate::command::LibraryStep;

/// What a dialog is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    /// A name for a new playlist.
    New,
    /// A new name for playlist `playlist`, which is called `name` now.
    Rename { playlist: i64, name: String },
    /// The question whether to delete playlist `playlist`, called `name`.
    Delete { playlist: i64, name: String },
}

/// An open dialog.
#[derive(Debug, Clone)]
struct Open {
    dialog: Dialog,
    /// The name field's text.
    text: String,
    /// The dialog has not been drawn yet: its field (or Cancel) takes the keyboard.
    fresh: bool,
}

fn dialog_id() -> Id {
    Id::NULL.with("open-jockey-playlist-dialog")
}

/// Opens `dialog`, replacing any open one.
pub fn open(ctx: &Context, dialog: Dialog) {
    let text = match &dialog {
        Dialog::Rename { name, .. } => name.clone(),
        Dialog::New | Dialog::Delete { .. } => String::new(),
    };
    let open = Open {
        dialog,
        text,
        fresh: true,
    };
    ctx.data_mut(|d| d.insert_temp(dialog_id(), open));
}

/// Draws the open dialog, if there is one, pushing what it decides onto `steps`.
pub(crate) fn show(ctx: &Context, steps: &mut Vec<LibraryStep>) {
    let id = dialog_id();
    let Some(mut open) = ctx.data(|d| d.get_temp::<Open>(id)) else {
        return;
    };
    let escape = ctx.input(|i| i.key_pressed(Key::Escape));
    let shown = Modal::new(id.with("modal")).show(ctx, |ui| contents(ui, &mut open, steps));
    open.fresh = false;
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

/// The dialog's contents; `true` once it is done with.
fn contents(ui: &mut Ui, open: &mut Open, steps: &mut Vec<LibraryStep>) -> bool {
    ui.set_width(260.0);
    let (title, verb) = match &open.dialog {
        Dialog::New => ("New playlist", "OK"),
        Dialog::Rename { .. } => ("Rename playlist", "OK"),
        Dialog::Delete { .. } => ("Delete playlist", "Delete"),
    };
    ui.heading(title);
    if let Dialog::Delete { playlist, name } = &open.dialog {
        ui.label(format!("Delete {name}? Its tracks stay in the library."));
        let playlist = *playlist;
        let mut done = false;
        ui.horizontal(|ui| {
            if ui.button(verb).clicked() {
                steps.push(LibraryStep::DeletePlaylist { playlist });
                done = true;
            }
            let cancel = ui.button("Cancel");
            // Enter then cancels; Delete takes a Tab to, or a click.
            if open.fresh {
                cancel.request_focus();
            }
            done |= cancel.clicked();
        });
        return done;
    }
    let mut edit = TextEdit::singleline(&mut open.text)
        .hint_text("Name")
        .desired_width(f32::INFINITY)
        .show(ui);
    let field = edit.response;
    if open.fresh {
        field.request_focus();
        // The name being renamed starts selected, so typing replaces it.
        let end = open.text.chars().count();
        let all = CCursorRange::two(CCursor::new(0), CCursor::new(end));
        edit.state.cursor.set_char_range(Some(all));
        edit.state.store(ui.ctx(), field.id);
    }
    let name = open.text.trim().to_owned();
    let entered = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    let mut done = false;
    ui.horizontal(|ui| {
        let ok = ui.add_enabled(!name.is_empty(), egui::Button::new(verb));
        if (ok.clicked() || entered) && !name.is_empty() {
            steps.push(match &open.dialog {
                Dialog::Rename { playlist, .. } => LibraryStep::RenamePlaylist {
                    playlist: *playlist,
                    name: name.clone(),
                },
                _ => LibraryStep::CreatePlaylist { name: name.clone() },
            });
            done = true;
        }
        done |= ui.button("Cancel").clicked();
    });
    // Enter on an empty name keeps the dialog, with the field still taking the keyboard.
    if entered && !done {
        field.request_focus();
    }
    done
}
