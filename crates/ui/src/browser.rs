//! The browser at the bottom of the window: the folder tree, the track list and the
//! highlighted track's info. The BROWSE knob drives the [`BrowserView`]; this only draws
//! it and scrolls to keep the highlights in sight.

use std::ops::Range;
use std::path::Path;

use analysis::KeyNotation;
use egui::{
    Color32, CornerRadius, Label, Rect, RichText, Sense, Stroke, Ui, UiBuilder, pos2, vec2,
};
use library::{BrowserView, Focus, ListRow, TreeKind, TreeRow};

use crate::cover::{self, Covers};
use crate::deck::{key_text, split_remix};
use crate::theme;

const ROW_HEIGHT: f32 = 20.0;
/// The share of the browser's width the folder tree takes.
const TREE_SHARE: f32 = 0.20;
/// The share of the browser's width the info panel takes.
const INFO_SHARE: f32 = 0.18;
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

/// Draws the browser into all of `ui`'s space.
pub fn show(ui: &mut Ui, view: &BrowserView, notation: KeyNotation, covers: &mut Covers) {
    let area = ui.available_rect_before_wrap();
    ui.painter().rect_filled(area, 0.0, theme::BACKGROUND);
    let tree_width = area.width() * TREE_SHARE;
    let info_width = area.width() * INFO_SHARE;
    let tree = Rect::from_min_max(area.min, pos2(area.min.x + tree_width, area.max.y));
    let info = Rect::from_min_max(pos2(area.max.x - info_width, area.min.y), area.max);
    let list = Rect::from_min_max(pos2(tree.max.x, area.min.y), pos2(info.min.x, area.max.y));
    pane(ui, tree, |ui| tree_pane(ui, view));
    pane(ui, list, |ui| list_pane(ui, view, notation));
    pane(ui, info, |ui| info_pane(ui, view, notation, covers));
}

fn pane(ui: &mut Ui, rect: Rect, add: impl FnOnce(&mut Ui)) {
    let rect = rect.shrink2(vec2(4.0, 2.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
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

fn highlight(selected: bool, focused: bool) -> Option<Color32> {
    selected.then_some(if focused {
        theme::HIGHLIGHT
    } else {
        theme::HIGHLIGHT_DIM
    })
}

/// Puts `text` in a cell starting at `x` and `width` wide, cut with "…" when too long.
fn cell(ui: &mut Ui, row: Rect, x: f32, width: f32, text: RichText) {
    let rect = Rect::from_min_size(pos2(x, row.min.y), vec2(width.max(0.0), row.height()));
    ui.put(rect, Label::new(text).truncate());
}

fn tree_pane(ui: &mut Ui, view: &BrowserView) {
    let rows = rows_fitting(ui.available_height());
    let window = visible_window(view.tree_cursor, view.tree.len(), rows);
    let focused = view.focus == Focus::Tree;
    for (at, entry) in view
        .tree
        .iter()
        .enumerate()
        .take(window.end)
        .skip(window.start)
    {
        let rect = row(ui, highlight(at == view.tree_cursor, focused));
        tree_row(ui, rect, entry);
    }
}

fn tree_row(ui: &mut Ui, rect: Rect, entry: &TreeRow) {
    let x = rect.min.x + 4.0 + f32::from(entry.depth) * INDENT;
    let mid = rect.center().y;
    // Only the nodes that can open get a triangle.
    if entry.kind != TreeKind::Playlist {
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
            theme::TEXT_DIM,
            Stroke::NONE,
        ));
    }
    glyph(ui, entry.kind, pos2(x + 16.0, mid));
    let label_x = x + 28.0;
    let width = (rect.max.x - label_x - 2.0).max(0.0);
    let Some(note) = entry.note else {
        cell(ui, rect, label_x, width, RichText::new(&entry.label));
        return;
    };
    // The note follows the label, dimmed.
    let font = egui::TextStyle::Body.resolve(ui.style());
    let label = ui.fonts_mut(|f| f.layout_no_wrap(entry.label.clone(), font, theme::TEXT));
    let label_width = label.size().x.min(width * 0.7);
    cell(ui, rect, label_x, label_width, RichText::new(&entry.label));
    cell(
        ui,
        rect,
        label_x + label_width + 6.0,
        width - label_width - 6.0,
        RichText::new(note).color(theme::TEXT_DIM),
    );
}

/// A small drive, folder or playlist icon centred on `at` (egui's fonts have no such glyphs).
fn glyph(ui: &Ui, kind: TreeKind, at: egui::Pos2) {
    let painter = ui.painter();
    let colour = theme::TEXT_DIM;
    match kind {
        TreeKind::Drive => {
            let body = Rect::from_center_size(at, vec2(11.0, 6.0));
            painter.rect_filled(body, CornerRadius::same(1), colour);
            painter.circle_filled(body.right_center() - vec2(2.0, 0.0), 0.9, theme::BACKGROUND);
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

/// The list rows to draw, with their absolute indices.
fn drawn_rows(view: &BrowserView, rows: usize) -> impl Iterator<Item = (usize, &ListRow)> {
    page_window(view, rows).map(move |at| (at, &view.list[at - view.list_offset]))
}

fn list_pane(ui: &mut Ui, view: &BrowserView, notation: KeyNotation) {
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
        RichText::new(path).color(theme::TEXT_DIM),
    );
    cell(
        ui,
        header,
        header.max.x - count_width,
        count_width,
        RichText::new(count).color(theme::TEXT_DIM),
    );
    let rows = rows_fitting(ui.available_height());
    let focused = view.focus == Focus::List;
    let total = ui.available_width();
    let fixed = CUE_WIDTH + LENGTH_WIDTH + BPM_WIDTH + KEY_WIDTH;
    let flexible = (total - fixed).max(0.0);
    let artist = flexible * ARTIST_SHARE;
    let remix = flexible * REMIX_SHARE;
    let title = flexible - artist - remix;
    for (at, entry) in drawn_rows(view, rows) {
        let rect = row(ui, highlight(at == view.list_cursor, focused));
        let fields = Fields::of(entry, notation);
        let colour = if fields.missing {
            theme::TEXT_DIM
        } else {
            theme::TEXT
        };
        let mut x = rect.min.x;
        let mut put = |ui: &mut Ui, width: f32, text: &str| {
            cell(ui, rect, x, width, RichText::new(text).color(colour));
            x += width;
        };
        put(ui, CUE_WIDTH, if fields.cues { "•" } else { "" });
        put(ui, title, &fields.title);
        put(ui, artist, &fields.artist);
        put(ui, remix, &fields.remix);
        put(
            ui,
            LENGTH_WIDTH,
            if fields.missing {
                "missing"
            } else {
                &fields.length
            },
        );
        put(ui, BPM_WIDTH, &fields.bpm);
        put(ui, KEY_WIDTH, &fields.key);
    }
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
                ui.label(RichText::new(name).color(theme::TEXT_DIM));
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
