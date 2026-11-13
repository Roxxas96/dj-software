//! The browser at the bottom of the window: the folder tree, the track list, the
//! highlighted track's info and the sideview beside them. The BROWSE knob drives the
//! [`BrowserView`]; this only draws it and scrolls to keep the highlights in sight. The
//! dividers between the panes drag, and the info panel and the sideview fold away to a
//! small tab on the right edge.

use std::ops::Range;
use std::path::Path;
use std::time::Instant;

use analysis::KeyNotation;
use egui::{
    Align, Button, Color32, CornerRadius, CursorIcon, Id, Label, Layout, Pos2, Rect, RichText,
    Sense, Stroke, Ui, UiBuilder, Vec2, pos2, vec2,
};
use engine::DeckId;
use library::{BrowserEdit, BrowserView, Focus, ListRow, TreeKind, TreeRow};

use crate::cover::{self, Covers};
use crate::deck::{key_text, split_remix};
use crate::layout::clamp_sizes;
use crate::theme::{self, Palette};
use crate::view::PanelSizes;
use crate::{LibraryStep, ViewChange};

const ROW_HEIGHT: f32 = 20.0;
/// How wide the strip around a divider that grabs the pointer is, in points.
const GRAB: f32 = 8.0;
/// The size of a folded panel's tab, in points.
const TAB_HEIGHT: f32 = 20.0;
/// Rows kept in view past the cursor.
const MARGIN: usize = 2;
/// The tree's indent per depth level, in points.
const INDENT: f32 = 14.0;
/// The widths of the list's columns after the title, as shares of the list's width.
const ARTIST_SHARE: f32 = 0.22;
const REMIX_SHARE: f32 = 0.20;
const LENGTH_WIDTH: f32 = 52.0;
const BPM_WIDTH: f32 = 52.0;
const KEY_WIDTH: f32 = 44.0;
const CUE_WIDTH: f32 = 14.0;

/// The rows of `count` to draw in a pane `rows` tall so that `cursor` is in view with
/// [`MARGIN`] rows below it (less at the end of the list).
pub fn visible_window(cursor: usize, count: usize, rows: usize) -> Range<usize> {
    let end = (cursor + 1 + MARGIN).min(count).max(rows.min(count));
    end.saturating_sub(rows)..end
}

/// `frames` at `rate` as `M:SS`, cut to the second.
pub fn format_length(frames: u64, rate: u32) -> String {
    let seconds = frames / u64::from(rate.max(1));
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// How the browser's areas are arranged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arrangement {
    /// The tree's, info panel's and sideview's shares of the window's width.
    pub sizes: PanelSizes,
    /// Whether the info panel shows; folded, a tab brings it back.
    pub info: bool,
    /// Whether the sideview shows; folded, a tab brings it back.
    pub sideview: bool,
    /// The window's size, for keeping the shares within what it can show.
    pub window: Vec2,
}

impl Default for Arrangement {
    fn default() -> Self {
        Self {
            sizes: PanelSizes::default(),
            info: true,
            sideview: false,
            window: vec2(1440.0, 900.0),
        }
    }
}

/// What the viewer did to the browser's arrangement this frame.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Changes {
    /// The shares after a divider moved.
    pub sizes: Option<PanelSizes>,
    /// A divider was let go.
    pub stopped: bool,
    /// A folded panel's tab was clicked.
    pub unfold: Option<ViewChange>,
    /// What the clicks, double-clicks, menus and wheel asked of the library, in order.
    pub library: Vec<LibraryStep>,
}

/// What the browser knows of the decks.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Decks {
    /// Each deck's loaded track id, to mark those tracks in the list.
    pub loaded: [Option<i64>; 4],
    /// Each deck plays: a double-click loads on the first deck that does not.
    pub playing: [bool; 4],
    /// The window shows four decks, not two.
    pub four: bool,
}

/// The window's own event when a double-click finds every deck shown playing.
pub const EVERY_DECK_PLAYING: &str = "Every deck is playing";

/// The browser's latest event of its own (such as [`EVERY_DECK_PLAYING`]) and when it
/// happened, for the status line.
pub fn notice(ctx: &egui::Context) -> Option<(Instant, &'static str)> {
    ctx.data(|d| d.get_temp(notice_id()))
}

fn notice_id() -> Id {
    Id::NULL.with("open-jockey-browser-notice")
}

/// A drag on a divider.
pub(crate) struct Drag {
    /// Where the pointer is while the divider is held.
    pub pointer: Option<Pos2>,
    pub stopped: bool,
}

/// A divider on `rect` that can be dragged; `name` tells it from the others in `ui`.
pub(crate) fn divider(ui: &mut Ui, name: &str, rect: Rect, cursor: CursorIcon) -> Drag {
    let response = ui.interact(rect, ui.scope_id().with(("divider", name)), Sense::drag());
    if response.hovered() || response.dragged() {
        ui.output_mut(|o| o.cursor_icon = cursor);
        let p = theme::palette(ui.ctx());
        let line = if cursor == CursorIcon::ResizeHorizontal {
            [rect.center_top(), rect.center_bottom()]
        } else {
            [rect.left_center(), rect.right_center()]
        };
        ui.painter()
            .line_segment(line, Stroke::new(2.0, p.text_dim));
    }
    // A press that has not moved yet is no drag: a click leaves the shares alone.
    let moved = response
        .total_drag_delta()
        .is_some_and(|delta| delta != egui::Vec2::ZERO);
    Drag {
        pointer: moved.then(|| response.interact_pointer_pos()).flatten(),
        stopped: response.drag_stopped(),
    }
}

/// The list's column headers, after the cue dot's empty one.
pub const COLUMNS: [&str; 6] = ["Title", "Artist", "Remix", "Length", "BPM", "Key"];

/// The width of a deck marker at a loaded track's row, in points.
const MARKER_WIDTH: f32 = 3.0;

/// Draws the browser into all of `ui`'s space, with `sideview` drawn in the sideview's
/// column when `arrangement` shows it, and `decks` as they are.
pub fn show(
    ui: &mut Ui,
    view: &BrowserView,
    decks: Decks,
    notation: KeyNotation,
    covers: &mut Covers,
    arrangement: &Arrangement,
    sideview: impl FnOnce(&mut Ui),
) -> Changes {
    let p = theme::palette(ui.ctx());
    let area = ui.available_rect_before_wrap();
    ui.painter().rect_filled(area, 0.0, p.background);
    let mut sizes = arrangement.sizes;
    let width = area.width().max(1.0);
    let columns = |sizes: &PanelSizes| {
        let side = if arrangement.sideview {
            sizes.sideview * width
        } else {
            0.0
        };
        let info = if arrangement.info {
            sizes.info * width
        } else {
            0.0
        };
        let tree_edge = area.min.x + sizes.tree * width;
        let side_edge = area.max.x - side;
        (tree_edge, side_edge - info, side_edge)
    };
    let (tree_edge, info_edge, side_edge) = columns(&sizes);
    let strip = |x: f32| {
        Rect::from_min_max(
            pos2(x - GRAB / 2.0, area.min.y),
            pos2(x + GRAB / 2.0, area.max.y),
        )
    };
    let mut changes = Changes::default();
    // Moves the share `set` stores to where the pointer is.
    let mut drag = |ui: &mut Ui, name: &str, edge: f32, set: &dyn Fn(&mut PanelSizes, f32)| {
        let drag = divider(ui, name, strip(edge), CursorIcon::ResizeHorizontal);
        changes.stopped |= drag.stopped;
        if let Some(pointer) = drag.pointer {
            set(&mut sizes, pointer.x);
            // The browser moves only the columns; the rows stay as the window drew them.
            sizes = PanelSizes {
                lanes: arrangement.sizes.lanes,
                decks: arrangement.sizes.decks,
                ..clamp_sizes(sizes, arrangement.window, 0.0)
            };
        }
    };
    drag(ui, "tree_divider", tree_edge, &|sizes, x| {
        sizes.tree = (x - area.min.x) / width;
    });
    if arrangement.info {
        drag(ui, "info_divider", info_edge, &|sizes, x| {
            sizes.info = (side_edge - x) / width;
        });
    }
    if arrangement.sideview {
        drag(ui, "side_divider", side_edge, &|sizes, x| {
            sizes.sideview = (area.max.x - x) / width;
        });
    }
    if sizes != arrangement.sizes {
        changes.sizes = Some(sizes);
    }
    // The areas follow the shares as moved this frame.
    let (tree_edge, info_edge, side_edge) = columns(&sizes);
    let tree = Rect::from_min_max(area.min, pos2(tree_edge, area.max.y));
    let list = Rect::from_min_max(pos2(tree_edge, area.min.y), pos2(info_edge, area.max.y));
    let info = Rect::from_min_max(pos2(info_edge, area.min.y), pos2(side_edge, area.max.y));
    let side = Rect::from_min_max(pos2(side_edge, area.min.y), area.max);
    crate::landmark_rect(ui, tree, "tree");
    crate::landmark_rect(ui, list, "list");
    let steps = &mut changes.library;
    pane(ui, tree, |ui| tree_pane(ui, view, steps));
    pane(ui, list, |ui| list_pane(ui, view, decks, notation, steps));
    if arrangement.info {
        crate::landmark_rect(ui, info, "info panel");
        pane(ui, info, |ui| info_pane(ui, view, notation, covers));
    }
    if arrangement.sideview {
        crate::landmark_rect(ui, side, "sideview panel");
        ui.painter().rect_filled(side, 0.0, p.panel);
        pane(ui, side, sideview);
    }
    // The tabs of folded panels sit at the bottom of the right edge, the sideview's
    // outermost.
    let mut bottom = area.max.y - 2.0;
    let mut tab = |ui: &mut Ui, name: &str, right: f32, width: f32, change: ViewChange| {
        let rect = Rect::from_min_size(
            pos2(right - width, bottom - TAB_HEIGHT),
            vec2(width, TAB_HEIGHT),
        );
        bottom -= TAB_HEIGHT + 2.0;
        ui.put(rect, Button::new(RichText::new(name).small()))
            .clicked()
            .then_some(change)
    };
    if !arrangement.sideview {
        changes.unfold = tab(
            ui,
            "sideview",
            area.max.x,
            62.0,
            ViewChange::SetSideview(true),
        );
    }
    if !arrangement.info {
        let at = tab(ui, "info", side_edge, 40.0, ViewChange::SetInfoPanel(true));
        changes.unfold = changes.unfold.or(at);
    }
    changes
}

fn pane(ui: &mut Ui, rect: Rect, add: impl FnOnce(&mut Ui)) {
    let rect = rect.shrink2(vec2(4.0, 2.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    // A click on a row's text is a click on the row.
    child.style_mut().interaction.selectable_labels = false;
    add(&mut child);
}

fn rows_fitting(height: f32) -> usize {
    (height / ROW_HEIGHT).floor().max(0.0) as usize
}

/// The next row of `ui`, `ROW_HEIGHT` tall, with `fill` behind it when it is highlighted.
fn row(ui: &mut Ui, fill: Option<Color32>) -> Rect {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, CornerRadius::same(2), fill);
    }
    rect
}

fn highlight(p: &Palette, selected: bool, focused: bool) -> Option<Color32> {
    selected.then_some(if focused {
        p.highlight
    } else {
        p.highlight_dim
    })
}

/// Puts `text` in a cell starting at `x` and `width` wide, cut with "…" when too long.
fn cell(ui: &mut Ui, row: Rect, x: f32, width: f32, text: RichText) {
    let rect = Rect::from_min_size(pos2(x, row.min.y), vec2(width.max(0.0), row.height()));
    let layout = Layout::left_to_right(Align::Center);
    ui.scope_builder(UiBuilder::new().max_rect(rect).layout(layout), |ui| {
        ui.add(Label::new(text).truncate());
    });
}

fn tree_pane(ui: &mut Ui, view: &BrowserView, steps: &mut Vec<LibraryStep>) {
    let rows = rows_fitting(ui.available_height());
    let count = view.tree.len();
    let follow = visible_window(view.tree_cursor, count, rows);
    let seen = (view.tree_cursor, 0, count);
    let (window, _) = scroll(ui, "tree_scroll", seen, 0..count, count, follow);
    let focused = view.focus == Focus::Tree;
    let p = theme::palette(ui.ctx());
    for (at, entry) in view
        .tree
        .iter()
        .enumerate()
        .take(window.end)
        .skip(window.start)
    {
        let rect = row(ui, highlight(&p, at == view.tree_cursor, focused));
        let response = ui.interact(rect, Id::NULL.with(("browser tree", at)), Sense::click());
        let arrow = tree_row(ui, rect, entry);
        let opens = matches!(entry.kind, TreeKind::Drive | TreeKind::Folder);
        // Drawn over the row, the arrow takes its own clicks.
        let arrow_clicked = arrow.is_some_and(|arrow| {
            let id = Id::NULL.with(("browser tree arrow", at));
            ui.interact(arrow, id, Sense::click()).clicked()
        });
        if (arrow_clicked || response.double_clicked()) && opens {
            steps.push(LibraryStep::Browse(BrowserEdit::ToggleTreeAt(at)));
        } else if response.clicked() {
            steps.push(LibraryStep::Browse(BrowserEdit::SelectTree(at)));
        }
    }
}

/// Draws a tree row on `rect`; returns where its arrow is, for the rows that have one.
fn tree_row(ui: &mut Ui, rect: Rect, entry: &TreeRow) -> Option<Rect> {
    let p = theme::palette(ui.ctx());
    let x = rect.min.x + 4.0 + f32::from(entry.depth) * INDENT;
    let mid = rect.center().y;
    let mut arrow = None;
    // Only the nodes that can open get a triangle.
    if entry.kind != TreeKind::Playlist {
        arrow = Some(Rect::from_center_size(
            pos2(x + 4.0, mid),
            vec2(12.0, rect.height()),
        ));
        let c = pos2(x + 4.0, mid);
        let points = if entry.expanded {
            vec![
                c + vec2(-3.5, -2.0),
                c + vec2(3.5, -2.0),
                c + vec2(0.0, 3.0),
            ]
        } else {
            vec![
                c + vec2(-2.0, -3.5),
                c + vec2(3.0, 0.0),
                c + vec2(-2.0, 3.5),
            ]
        };
        ui.painter().add(egui::Shape::convex_polygon(
            points,
            p.text_dim,
            Stroke::NONE,
        ));
    }
    glyph(ui, entry.kind, pos2(x + 16.0, mid));
    let label_x = x + 28.0;
    let width = (rect.max.x - label_x - 2.0).max(0.0);
    let Some(note) = entry.note else {
        cell(ui, rect, label_x, width, RichText::new(&entry.label));
        return arrow;
    };
    // The note follows the label, dimmed.
    let font = egui::TextStyle::Body.resolve(ui.style());
    let label = ui.fonts_mut(|f| f.layout_no_wrap(entry.label.clone(), font, p.text));
    let label_width = label.size().x.min(width * 0.7);
    cell(ui, rect, label_x, label_width, RichText::new(&entry.label));
    cell(
        ui,
        rect,
        label_x + label_width + 6.0,
        width - label_width - 6.0,
        RichText::new(note).color(p.text_dim),
    );
    arrow
}

/// A small drive, folder or playlist icon centred on `at` (egui's fonts have no such glyphs).
fn glyph(ui: &Ui, kind: TreeKind, at: egui::Pos2) {
    let p = theme::palette(ui.ctx());
    let painter = ui.painter();
    let colour = p.text_dim;
    match kind {
        TreeKind::Drive => {
            let body = Rect::from_center_size(at, vec2(11.0, 6.0));
            painter.rect_filled(body, CornerRadius::same(1), colour);
            painter.circle_filled(body.right_center() - vec2(2.0, 0.0), 0.9, p.background);
        }
        TreeKind::Folder => {
            let body = Rect::from_center_size(at + vec2(0.0, 1.0), vec2(11.0, 7.0));
            painter.rect_filled(body, CornerRadius::same(1), colour);
            let tab = Rect::from_min_size(body.min - vec2(0.0, 2.0), vec2(5.0, 2.5));
            painter.rect_filled(tab, CornerRadius::same(1), colour);
        }
        TreeKind::Playlists | TreeKind::Playlist => {
            for line in [-3.0, 0.0, 3.0] {
                let from = at + vec2(-5.0, line);
                painter.line_segment([from, from + vec2(10.0, 0.0)], Stroke::new(1.5, colour));
            }
        }
    }
}

/// The absolute rows to draw: `rows` of the loaded page, slid to hold the cursor's window
/// where the page allows, and the page's nearest rows when the cursor is outside it.
fn page_window(view: &BrowserView, rows: usize) -> Range<usize> {
    let page = view.list_offset..view.list_offset + view.list.len();
    let wanted = visible_window(view.list_cursor, view.list_count, rows);
    let start = wanted
        .start
        .clamp(page.start, page.end.saturating_sub(rows).max(page.start));
    start..(start + rows).min(page.end)
}

/// What the wheel did to a pane, kept from frame to frame.
#[derive(Debug, Clone, Copy, Default)]
struct Scroll {
    /// The first row drawn, once the wheel moved it; `None` follows the cursor.
    start: Option<usize>,
    /// The part of a row the wheel moved past the last whole row.
    carry: f32,
    /// The cursor, page offset and row count this was for: the session moving any of them
    /// is a new view.
    seen: (usize, usize, usize),
    /// A row past the page was asked for since the view last changed.
    asked: bool,
}

/// The rows a pane shows, `follow` long, when the wheel over it scrolls within `page`
/// (the rows the view holds, of `count`). Where it follows the cursor, it draws `follow`;
/// once the wheel has turned, the rows it scrolled to, until the view moves its cursor out
/// of them. A scroll past the page returns the row beyond it for the session to load, once
/// per view.
fn scroll(
    ui: &Ui,
    name: &str,
    seen: (usize, usize, usize),
    page: Range<usize>,
    count: usize,
    follow: Range<usize>,
) -> (Range<usize>, Option<usize>) {
    let rows = follow.len();
    let id = Id::NULL.with(("browser scroll", name));
    let mut state: Scroll = ui.ctx().data(|d| d.get_temp(id)).unwrap_or_default();
    let last_start = page.end.saturating_sub(rows).max(page.start);
    if state.seen != seen {
        // A move that keeps the cursor in sight leaves the rows where the wheel put them.
        let (cursor, ..) = seen;
        state.start = state
            .start
            .filter(|&start| (start..start + rows).contains(&cursor))
            .map(|start| start.clamp(page.start, last_start));
        state.seen = seen;
        state.asked = false;
    }
    let mut ask = None;
    let hovered = ui.rect_contains_pointer(ui.max_rect());
    let delta = if hovered {
        ui.input(|i| i.smooth_scroll_delta().y)
    } else {
        0.0
    };
    if delta != 0.0 {
        ui.ctx().input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
        let total = state.carry - delta / ROW_HEIGHT;
        let steps = total.trunc();
        state.carry = total - steps;
        if steps != 0.0 {
            let from = state.start.unwrap_or(follow.start) as f32;
            let to = from + steps;
            if to > last_start as f32 && page.end < count && !state.asked {
                ask = Some(page.end);
            } else if to < page.start as f32 && page.start > 0 && !state.asked {
                ask = Some(page.start - 1);
            }
            state.asked |= ask.is_some();
            state.start = Some((to.max(0.0) as usize).clamp(page.start, last_start));
        }
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, state));
    let start = state.start.unwrap_or(follow.start);
    (start..(start + rows).min(page.end), ask)
}

/// The label of each deck's item in a track's menu.
const LOAD_ON: [&str; 4] = [
    "Load on deck 1",
    "Load on deck 2",
    "Load on deck 3",
    "Load on deck 4",
];

/// The decks `decks` shows, left before right: 1 and 2, then 3 and 4 in the four-deck
/// view.
fn shown_decks(decks: Decks) -> usize {
    if decks.four { 4 } else { 2 }
}

/// Loads `track` on the first deck shown that is not playing; with none, the browser's
/// own event says so.
fn load_on_free_deck(ui: &Ui, decks: Decks, track: i64, steps: &mut Vec<LibraryStep>) {
    match (0..shown_decks(decks)).find(|&i| !decks.playing[i]) {
        Some(i) => steps.push(LibraryStep::Load {
            deck: DeckId::ALL[i],
            track,
        }),
        None => {
            let now = Instant::now();
            ui.ctx()
                .data_mut(|d| d.insert_temp(notice_id(), (now, EVERY_DECK_PLAYING)));
        }
    }
}

fn list_pane(
    ui: &mut Ui,
    view: &BrowserView,
    decks: Decks,
    notation: KeyNotation,
    steps: &mut Vec<LibraryStep>,
) {
    let loaded = decks.loaded;
    let p = theme::palette(ui.ctx());
    let header = row(ui, None);
    // The library separates the path with "▸" (U+25B8), which egui's bundled fonts lack and
    // the system fallbacks may too.
    let path = view.path.replace('▸', ">");
    let files = if view.list_count == 1 {
        "file"
    } else {
        "files"
    };
    let count = format!("{} {files}", view.list_count);
    let count_width = 70.0;
    cell(
        ui,
        header,
        header.min.x,
        header.width() - count_width - 6.0,
        RichText::new(path).color(p.text_dim),
    );
    cell(
        ui,
        header,
        header.max.x - count_width,
        count_width,
        RichText::new(count).color(p.text_dim),
    );
    // The column headers, over the same cells as the rows.
    let head = row(ui, None);
    let cells = columns(head.min.x, head.width());
    for (&(x, width), name) in cells[1..].iter().zip(COLUMNS) {
        cell(ui, head, x, width, RichText::new(name).color(p.text_dim));
    }
    let rows = rows_fitting(ui.available_height());
    let focused = view.focus == Focus::List;
    let page = view.list_offset..view.list_offset + view.list.len();
    let seen = (view.list_cursor, view.list_offset, view.list_count);
    let follow = page_window(view, rows);
    let (window, ask) = scroll(ui, "list_scroll", seen, page, view.list_count, follow);
    if let Some(at) = ask {
        steps.push(LibraryStep::Browse(BrowserEdit::SelectRow(at)));
    }
    for at in window {
        let entry = &view.list[at - view.list_offset];
        let alt = (at % 2 == 1).then_some(p.row_alt);
        let fill = highlight(&p, at == view.list_cursor, focused).or(alt);
        let rect = row(ui, fill);
        let response = ui.interact(rect, Id::NULL.with(("browser row", at)), Sense::click());
        let track = entry.track.as_ref().map(|t| t.id);
        if response.clicked() {
            steps.push(LibraryStep::Browse(BrowserEdit::SelectRow(at)));
        }
        if let (true, Some(track)) = (response.double_clicked(), track) {
            load_on_free_deck(ui, decks, track, steps);
        }
        if let Some(track) = track {
            response.context_menu(|ui| {
                for (deck, label) in DeckId::ALL.iter().zip(LOAD_ON).take(shown_decks(decks)) {
                    if ui.button(label).clicked() {
                        steps.push(LibraryStep::Load { deck: *deck, track });
                    }
                }
            });
        }
        let fields = Fields::of(entry, notation);
        if let Some(deck) = entry
            .track
            .as_ref()
            .and_then(|t| loaded.iter().position(|id| *id == Some(t.id)))
        {
            let marker = Rect::from_min_size(rect.min, vec2(MARKER_WIDTH, rect.height()));
            let colour = p.deck_accent(DeckId::ALL[deck]);
            ui.painter().rect_filled(marker, 0.0, colour);
        }
        let colour = if fields.missing { p.text_dim } else { p.text };
        let length = if fields.missing {
            "missing"
        } else {
            &fields.length
        };
        let texts = [
            if fields.cues { "•" } else { "" },
            &fields.title,
            &fields.artist,
            &fields.remix,
            length,
            &fields.bpm,
            &fields.key,
        ];
        for (&(x, width), text) in cells.iter().zip(texts) {
            cell(ui, rect, x, width, RichText::new(text).color(colour));
        }
    }
}

/// The list's cells as `(x, width)`: the cue dot (right of a loaded track's marker), then
/// one per [`COLUMNS`] entry, across `total` points from `left`. The headers and the rows
/// both place their cells with this.
fn columns(left: f32, total: f32) -> [(f32, f32); 7] {
    let fixed = CUE_WIDTH + LENGTH_WIDTH + BPM_WIDTH + KEY_WIDTH;
    let flexible = (total - fixed).max(0.0);
    let artist = flexible * ARTIST_SHARE;
    let remix = flexible * REMIX_SHARE;
    let title = flexible - artist - remix;
    let widths = [
        CUE_WIDTH,
        title,
        artist,
        remix,
        LENGTH_WIDTH,
        BPM_WIDTH,
        KEY_WIDTH,
    ];
    let mut x = left;
    let mut cells = widths.map(|width| {
        let at = (x, width);
        x += width;
        at
    });
    cells[0] = (left + MARKER_WIDTH, CUE_WIDTH - MARKER_WIDTH);
    cells
}

/// A list row's cells, as text.
struct Fields {
    cues: bool,
    missing: bool,
    title: String,
    artist: String,
    remix: String,
    length: String,
    bpm: String,
    key: String,
}

impl Fields {
    fn of(entry: &ListRow, notation: KeyNotation) -> Self {
        let track = entry.track.as_ref();
        let full = track
            .and_then(|t| t.title.as_deref())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or(&entry.file_name);
        let (title, remix) = split_remix(full);
        Self {
            cues: track.is_some_and(|t| t.has_cues),
            missing: track.is_some_and(|t| t.missing),
            title,
            artist: track.and_then(|t| t.artist.clone()).unwrap_or_default(),
            remix: remix.unwrap_or_default(),
            length: track
                .and_then(|t| Some((t.duration_frames?, t.sample_rate?)))
                .map(|(frames, rate)| format_length(frames, rate))
                .unwrap_or_default(),
            bpm: track
                .and_then(|t| t.bpm)
                .map(|bpm| format!("{bpm:.1}"))
                .unwrap_or_default(),
            key: track
                .and_then(|t| t.key)
                .and_then(|key| key_text(key, notation))
                .unwrap_or_default(),
        }
    }
}

fn info_pane(ui: &mut Ui, view: &BrowserView, notation: KeyNotation, covers: &mut Covers) {
    let p = theme::palette(ui.ctx());
    // The highlighted row, when its page is loaded.
    let entry = view
        .list_cursor
        .checked_sub(view.list_offset)
        .and_then(|at| view.list.get(at));
    let Some(entry) = entry else { return };
    let fields = Fields::of(entry, notation);
    let side = ui.available_width().min(160.0);
    // Only the highlighted track's cover is asked for: one request per highlight change.
    let cover = if fields.missing {
        None
    } else {
        covers.get(ui.ctx(), Path::new(&entry.path))
    };
    cover::tile(ui, cover.as_ref(), side);
    ui.add_space(6.0);
    ui.add(Label::new(RichText::new(&fields.title).strong()).truncate());
    for (name, value) in [
        ("Artist", &fields.artist),
        ("Remix", &fields.remix),
        ("Length", &fields.length),
        ("BPM", &fields.bpm),
        ("Key", &fields.key),
    ] {
        if !value.is_empty() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(name).color(p.text_dim));
                ui.add(Label::new(value).truncate());
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_follows_the_cursor() {
        assert_eq!(visible_window(0, 100, 10), 0..10);
        let window = visible_window(50, 100, 10);
        assert!(window.contains(&50) && window.end >= 53 && window.len() == 10);
        assert_eq!(visible_window(99, 100, 10), 90..100);
        assert_eq!(visible_window(3, 5, 10), 0..5);
        assert_eq!(visible_window(0, 0, 10), 0..0);
    }

    fn paged(offset: usize, cursor: usize) -> BrowserView {
        let row = ListRow {
            path: std::path::PathBuf::new(),
            track: None,
            file_name: String::new(),
        };
        BrowserView {
            path: String::new(),
            index: 0,
            count: 0,
            track: None,
            focus: Focus::List,
            tree: Vec::new(),
            tree_cursor: 0,
            list: vec![row; 200],
            list_offset: offset,
            list_count: 5000,
            list_cursor: cursor,
        }
    }

    #[test]
    fn the_window_slides_inside_the_page() {
        // At the page's start the pane stays full and shows the cursor.
        let view = paged(100, 100);
        let window = page_window(&view, 12);
        assert_eq!(window.len(), 12);
        assert!(window.contains(&100) && window.start >= 100);
        // At the page's end too, with the cursor in view.
        let view = paged(100, 299);
        let window = page_window(&view, 12);
        assert_eq!(window, 288..300);
        assert!(window.contains(&299));
        // Outside the page: the nearest full pane.
        assert_eq!(page_window(&paged(100, 5), 12), 100..112);
    }

    #[test]
    fn format_length_cuts_to_the_second() {
        assert_eq!(format_length(14_532_480, 44_100), "5:29");
    }
}
