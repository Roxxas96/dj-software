//! The hotcue editor a right-click on a hotcue pad or on a waveform's hotcue flag opens.
//!
//! On a set hotcue it holds the hotcue's name (Enter saves it), the pad colours and "No
//! colour" (each saves at once and leaves the editor open) and "Delete". On an empty pad
//! it holds "Set hotcue here" alone, which sets the hotcue at the playhead. The name and
//! colour go to the session as a [`LibraryStep::HotcueLabel`]; "Delete" and "Set hotcue
//! here" press and release the hotcue actions a controller's pads send, through the
//! [`Sink`], so the release always follows the press. Escape, Enter or a click outside
//! closes the editor; it is modal, so that closing click does nothing else.
//!
//! The editor is kept in egui's memory: whatever draws a pad or a flag opens it
//! ([`open`]), and the window draws it once a frame on top of everything ([`show`]).

use actions::Action;
use egui::{
    Area, Color32, Context, CornerRadius, Id, Key, Modal, Order, Pos2, Response, Role, Sense,
    Stroke, StrokeKind, TextEdit, Ui, UiKind, Vec2, WidgetInfo, vec2,
};
use engine::{DeckId, HotcueSlot};

use crate::command::{LibraryStep, UiCommand};
use crate::controls::Sink;
use crate::model::HotcueMark;
use crate::theme;

/// The longest name kept, in characters (a flag shows 14 of them).
pub const NAME_CHARS: usize = 32;

/// The swatches, each with its name for screen readers and its colour as `0xRRGGBB`: the
/// pad colours, slot 1's to slot 8's (the colours a hotcue without its own takes in the
/// exports), then "No colour", the slot's default.
pub const SWATCHES: [(&str, Option<u32>); 9] = [
    ("red", Some(0xCC0000)),
    ("orange", Some(0xCC8800)),
    ("blue", Some(0x0000CC)),
    ("yellow", Some(0xCCCC00)),
    ("green", Some(0x00CC00)),
    ("magenta", Some(0xCC00CC)),
    ("cyan", Some(0x00CCCC)),
    ("purple", Some(0x8800CC)),
    ("No colour", None),
];

/// The editor's title, by slot index.
const TITLES: [&str; 8] = [
    "Hotcue 1", "Hotcue 2", "Hotcue 3", "Hotcue 4", "Hotcue 5", "Hotcue 6", "Hotcue 7", "Hotcue 8",
];

/// The popup frame's margin around the contents (`Frame::popup`'s, as far as keeping the
/// editor on the screen goes).
const POPUP_MARGIN: f32 = 6.0;
/// A swatch's side, and the name field's width.
const SWATCH: f32 = 18.0;
const FIELD_WIDTH: f32 = 9.0 * SWATCH + 8.0 * 4.0;

/// The hotcue an editor is opened on, as the window saw it then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotcueTarget {
    pub deck: DeckId,
    pub slot: HotcueSlot,
    /// The library track on the deck: the name and colour are saved for it. Without one
    /// only "Delete" shows.
    pub track: Option<i64>,
    /// The slot holds a hotcue.
    pub set: bool,
    pub name: Option<String>,
    /// As `0xRRGGBB`.
    pub color: Option<u32>,
}

impl HotcueTarget {
    /// Hotcue `slot` of `deck`, which holds `track`, with the colour and name of `mark`.
    pub fn new(
        deck: DeckId,
        slot: HotcueSlot,
        track: Option<i64>,
        set: bool,
        mark: Option<&HotcueMark>,
    ) -> Self {
        Self {
            deck,
            slot,
            track,
            set,
            name: mark.and_then(|m| m.name.clone()),
            color: mark
                .and_then(|m| m.color)
                .map(|c| u32::from_be_bytes([0, c.r(), c.g(), c.b()])),
        }
    }
}

/// An open editor.
#[derive(Debug, Clone)]
struct Editor {
    target: HotcueTarget,
    /// Its top left corner: where the right-click was.
    at: Pos2,
    /// The name field's text.
    text: String,
    /// The name and colour last saved (or as opened).
    name: Option<String>,
    color: Option<u32>,
    /// The name field takes the keyboard in the next frame drawn: on opening, and after a
    /// swatch took it.
    focus: bool,
    /// The editor's size when last drawn, to keep it on the screen.
    size: Vec2,
}

fn editor_id() -> Id {
    Id::NULL.with("open-jockey-hotcue-editor")
}

/// Opens the editor on `target` at `at`, replacing any open one.
pub fn open(ctx: &Context, target: HotcueTarget, at: Pos2) {
    let editor = Editor {
        text: target.name.clone().unwrap_or_default(),
        name: target.name.clone(),
        color: target.color,
        target,
        at,
        focus: true,
        size: Vec2::ZERO,
    };
    ctx.data_mut(|d| d.insert_temp(editor_id(), editor));
}

/// Draws the open editor, if there is one, sending what it asks for through `sink`.
///
/// It is modal: nothing under it takes the pointer while it is open, so the press outside
/// that closes it does nothing else (a misclick never plays a pad).
pub fn show(ctx: &Context, sink: &mut Sink<'_>) {
    let id = editor_id();
    let Some(mut editor) = ctx.data(|d| d.get_temp::<Editor>(id)) else {
        return;
    };
    let escape = ctx.input(|i| i.key_pressed(Key::Escape));
    // Kept on the screen, from the size it had last frame.
    let screen = ctx.content_rect();
    let at = editor.at.min(screen.max - editor.size).max(screen.min);
    let area = Area::new(id.with("area"))
        .kind(UiKind::Modal)
        .sense(Sense::hover())
        .order(Order::Foreground)
        .interactable(true)
        .fixed_pos(at)
        .constrain(false);
    let shown = Modal::new(id.with("modal"))
        .area(area)
        .backdrop_color(Color32::TRANSPARENT)
        .show(ctx, |ui| {
            let done = contents(ui, &mut editor, sink);
            (done, ui.min_rect().size() + Vec2::splat(2.0 * POPUP_MARGIN))
        });
    let (done, size) = shown.inner;
    editor.size = size;
    let backdrop = &shown.backdrop_response;
    let outside = backdrop.is_pointer_button_down_on() || backdrop.clicked();
    let close = escape || done || outside;
    ctx.data_mut(|d| {
        if close {
            d.remove::<Editor>(id);
        } else {
            d.insert_temp(id, editor);
        }
    });
}

/// The editor's contents; `true` once it is done with.
fn contents(ui: &mut Ui, editor: &mut Editor, sink: &mut Sink<'_>) -> bool {
    let HotcueTarget {
        deck, slot, track, ..
    } = editor.target;
    ui.spacing_mut().item_spacing = vec2(4.0, 6.0);
    ui.label(TITLES[slot.index()]);
    if !editor.target.set {
        let set = ui.button("Set hotcue here");
        sink.button(&set, Action::Hotcue(deck, slot));
        return set.clicked();
    }
    let mut done = false;
    if let Some(track) = track {
        let field = ui.add(
            TextEdit::singleline(&mut editor.text)
                .hint_text("Name")
                .char_limit(NAME_CHARS)
                .desired_width(FIELD_WIDTH),
        );
        if std::mem::take(&mut editor.focus) {
            field.request_focus();
        }
        if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            editor.name = clean_name(&editor.text);
            sink.send(label(deck, track, slot, editor.name.clone(), editor.color));
            done = true;
        }
        ui.horizontal(|ui| {
            for (name, colour) in SWATCHES {
                if swatch(ui, name, colour, editor.color == colour).clicked() {
                    // The name as typed goes with it, and Enter still saves.
                    editor.name = clean_name(&editor.text);
                    editor.color = colour;
                    editor.focus = true;
                    sink.send(label(deck, track, slot, editor.name.clone(), colour));
                }
            }
        });
    }
    let delete = ui.button("Delete");
    sink.button(&delete, Action::HotcueDelete(deck, slot));
    done || delete.clicked()
}

fn label(
    deck: DeckId,
    track: i64,
    slot: HotcueSlot,
    name: Option<String>,
    color: Option<u32>,
) -> UiCommand {
    UiCommand::Library(LibraryStep::HotcueLabel {
        deck,
        track,
        slot,
        name,
        color,
    })
}

/// A colour swatch named `name`, `colour` as a pad shows it, outlined when `chosen`.
/// "No colour" is the empty lane with a slash across it.
fn swatch(ui: &mut Ui, name: &str, colour: Option<u32>, chosen: bool) -> Response {
    let p = theme::palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(SWATCH, SWATCH), Sense::click());
    response.widget_info(|| WidgetInfo::selected(Role::Button, true, chosen, name));
    let painter = ui.painter();
    match colour {
        Some(rgb) => {
            let [_, r, g, b] = rgb.to_be_bytes();
            let fill = p.mark(Some(Color32::from_rgb(r, g, b)));
            painter.rect_filled(rect, CornerRadius::same(2), fill);
        }
        None => {
            painter.rect_filled(rect, CornerRadius::same(2), p.lane);
            let slash = Stroke::new(1.5, p.text_dim);
            painter.line_segment([rect.left_bottom(), rect.right_top()], slash);
        }
    }
    if chosen || response.hovered() {
        let ring = Stroke::new(2.0, p.text);
        painter.rect_stroke(rect, CornerRadius::same(2), ring, StrokeKind::Outside);
    }
    response
}

/// The name to save for the field's `text`: trimmed and cut to [`NAME_CHARS`]; an empty
/// one is no name.
pub fn clean_name(text: &str) -> Option<String> {
    let text = text.trim();
    let cut = match text.char_indices().nth(NAME_CHARS) {
        Some((end, _)) => text[..end].trim_end(),
        None => text,
    };
    (!cut.is_empty()).then(|| cut.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_and_cut() {
        assert_eq!(clean_name("  Drop  ").as_deref(), Some("Drop"));
        assert_eq!(clean_name("   "), None);
        assert_eq!(clean_name(""), None);
        let long = "é".repeat(40);
        assert_eq!(clean_name(&long).unwrap().chars().count(), NAME_CHARS);
    }

    #[test]
    fn a_target_reads_its_mark() {
        let deck = DeckId::new(0).unwrap();
        let slot = HotcueSlot::new(2).unwrap();
        let mark = HotcueMark::new(Some(0xCC8800), Some("Verse".into()));
        let target = HotcueTarget::new(deck, slot, Some(7), true, Some(&mark));
        assert_eq!(target.color, Some(0xCC8800));
        assert_eq!(target.name.as_deref(), Some("Verse"));
    }
}
