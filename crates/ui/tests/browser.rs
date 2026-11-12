//! The browser draws headless, without a GPU.

use std::path::PathBuf;

use analysis::KeyNotation;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use library::{BrowserView, Focus, ListRow, TrackRow, TreeKind, TreeRow};
use ui::browser::{Arrangement, COLUMNS};
use ui::cover::Covers;
use ui::theme;

fn tree_row(label: &str, depth: u8, kind: TreeKind) -> TreeRow {
    TreeRow {
        label: label.into(),
        depth,
        kind,
        expanded: false,
        note: None,
    }
}

fn view() -> BrowserView {
    let track = TrackRow {
        id: 1,
        title: Some("Just Us (Extended Mix)".into()),
        artist: Some("Someone".into()),
        bpm: Some(126.0),
        key: Some(21),
        duration_frames: Some(14_532_480),
        sample_rate: Some(44_100),
        has_cues: true,
        missing: false,
    };
    let mut gone = track.clone();
    gone.id = 2;
    gone.title = Some("Lost".into());
    gone.missing = true;
    BrowserView {
        path: "Macintosh HD ▸ House".into(),
        index: 0,
        count: 2,
        track: Some(track.clone()),
        focus: Focus::List,
        tree: vec![
            tree_row("Macintosh HD", 0, TreeKind::Drive),
            tree_row("House", 1, TreeKind::Folder),
        ],
        tree_cursor: 1,
        list: vec![
            ListRow {
                path: PathBuf::from("/music/just-us.mp3"),
                track: Some(track),
                file_name: "just-us.mp3".into(),
            },
            ListRow {
                path: PathBuf::from("/music/lost.mp3"),
                track: Some(gone),
                file_name: "lost.mp3".into(),
            },
        ],
        list_offset: 0,
        list_count: 2,
        list_cursor: 0,
    }
}

fn draw(view: &BrowserView) -> Harness<'_> {
    let mut covers = Covers::default();
    let mut harness = Harness::builder()
        .with_size([1000.0, 300.0])
        .build_ui(move |ui| {
            ui::browser::show(
                ui,
                view,
                [None; 4],
                KeyNotation::default(),
                &mut covers,
                &Arrangement::default(),
                |_| {},
            );
        });
    harness.run_steps(3);
    harness
}

#[test]
fn browser_shows_tree_and_rows() {
    let view = view();
    let harness = draw(&view);
    harness.get_by_label("House");
    // The title, the remix and the info panel's copy of each.
    assert_eq!(harness.get_all_by_label("Just Us").count(), 2);
    assert_eq!(harness.get_all_by_label("Extended Mix").count(), 2);
    harness.get_by_label("2 files");
    assert_eq!(harness.get_all_by_label("5:29").count(), 2);
    harness.get_by_label("missing");
    assert_eq!(harness.get_all_by_label("126.0").count(), 3);
}

#[test]
fn a_cursor_outside_the_page_still_draws_the_page() {
    let mut view = view();
    view.list_cursor = 500;
    view.list_count = 1000;
    let harness = draw(&view);
    harness.get_by_label("Lost");
}

#[test]
fn non_latin_text_draws_without_panicking() {
    let mut view = view();
    let track = view.list[0].track.as_mut().unwrap();
    track.title = Some("COMسN GO".into());
    track.artist = Some("日本語".into());
    let mut covers = Covers::default();
    let view = &view;
    let mut harness = Harness::builder()
        .with_size([1000.0, 300.0])
        .allow_missing_glyphs()
        .build_ui(move |ui| {
            ui::browser::show(
                ui,
                view,
                [None; 4],
                KeyNotation::default(),
                &mut covers,
                &Arrangement::default(),
                |_| {},
            );
        });
    harness.run_steps(3);
    assert_eq!(harness.get_all_by_label("COMسN GO").count(), 2);
}

/// Draws `view` in a plain context and returns the filled rectangles it painted.
fn painted(view: &BrowserView, loaded: [Option<i64>; 4]) -> Vec<(egui::Rect, egui::Color32)> {
    fn collect(shape: &egui::Shape, out: &mut Vec<(egui::Rect, egui::Color32)>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
            egui::Shape::Rect(r) => out.push((r.rect, r.fill)),
            _ => {}
        }
    }
    let ctx = egui::Context::default();
    let mut covers = Covers::default();
    let mut out = Vec::new();
    for _ in 0..3 {
        out.clear();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 300.0),
            )),
            ..Default::default()
        };
        let output = ctx.run_ui(input, |ui| {
            ui::browser::show(
                ui,
                view,
                loaded,
                KeyNotation::default(),
                &mut covers,
                &Arrangement::default(),
                |_| {},
            );
        });
        let mut output = output;
        output.textures_delta.clear();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut out);
        }
    }
    out
}

fn many_rows(count: usize) -> BrowserView {
    let mut view = view();
    let template = view.list[0].clone();
    view.list = (0..count)
        .map(|i| {
            let mut row = template.clone();
            row.track.as_mut().unwrap().id = 100 + i as i64;
            row.file_name = format!("track-{i}.mp3");
            row
        })
        .collect();
    view.list_count = count;
    view.list_cursor = 0;
    view
}

#[test]
fn tree_labels_are_left_aligned() {
    let view = view();
    let mut margin = None;
    for width in [700.0_f32, 1000.0, 1400.0] {
        let mut covers = Covers::default();
        let view = &view;
        let mut harness = Harness::builder()
            .with_size([width, 300.0])
            .build_ui(move |ui| {
                ui::browser::show(
                    ui,
                    view,
                    [None; 4],
                    KeyNotation::default(),
                    &mut covers,
                    &Arrangement::default(),
                    |_| {},
                );
            });
        harness.run_steps(3);
        // Pane inset 4, row indent 4, per depth 14, glyph offset 28.
        // The harness's own margin shifts all of it by the same amount.
        let house = harness.get_by_label("House").rect().min.x;
        let drive = harness.get_by_label("Macintosh HD").rect().min.x;
        assert!((house - drive - 14.0).abs() <= 1.0, "{house} {drive}");
        let origin = *margin.get_or_insert(drive - (4.0 + 4.0 + 28.0));
        assert!(
            (drive - origin - (4.0 + 4.0 + 28.0)).abs() <= 1.0,
            "{drive}"
        );
    }
}

#[test]
fn list_has_column_headers() {
    let view = view();
    let mut covers = Covers::default();
    let view = &view;
    let arrangement = Arrangement {
        info: false,
        ..Arrangement::default()
    };
    let mut harness = Harness::builder()
        .with_size([1000.0, 300.0])
        .build_ui(move |ui| {
            ui::browser::show(
                ui,
                view,
                [None; 4],
                KeyNotation::default(),
                &mut covers,
                &arrangement,
                |_| {},
            );
        });
    harness.run_steps(3);
    // A cell of the first row under each header.
    let cells = [
        "Just Us",
        "Someone",
        "Extended Mix",
        "5:29",
        "126.0",
        "A\u{266d}m",
    ];
    for (name, cell) in COLUMNS.iter().zip(&cells[..5]) {
        let head = harness.get_by_label(name).rect();
        let body = harness
            .get_all_by_label(cell)
            .map(|n| n.rect())
            .min_by(|a, b| a.min.y.total_cmp(&b.min.y))
            .unwrap();
        assert!((head.min.x - body.min.x).abs() <= 1.0, "{name}");
        assert!(head.min.y < body.min.y, "{name} is above the rows");
    }
    // The key's cell text depends on the notation; its header still shares its column.
    let key = harness.get_by_label("Key").rect();
    let bpm = harness.get_by_label("BPM").rect();
    assert!(key.min.x > bpm.min.x);
}

#[test]
fn rows_alternate_shades() {
    let view = many_rows(6);
    let alt = theme::Palette::default().row_alt;
    let mut rects: Vec<_> = painted(&view, [None; 4])
        .into_iter()
        .filter(|(rect, fill)| *fill == alt && rect.height() > 10.0 && rect.height() < 30.0)
        .map(|(rect, _)| rect.min.y)
        .collect();
    rects.sort_by(f32::total_cmp);
    rects.dedup_by(|a, b| (*a - *b).abs() < 0.5);
    assert_eq!(rects.len(), 3, "{rects:?}");
    // Every other row: 2 rows apart.
    let pitch = rects[1] - rects[0];
    assert!(pitch >= 40.0, "{rects:?}");
    assert!((rects[2] - rects[1] - pitch).abs() < 1.0, "{rects:?}");
}

#[test]
fn loaded_tracks_are_marked() {
    let view = many_rows(4);
    let p = theme::Palette::default();
    let marks = |loaded| {
        painted(&view, loaded)
            .into_iter()
            .filter(|(rect, _)| (rect.width() - 3.0).abs() < 0.01 && rect.height() > 10.0)
            .collect::<Vec<_>>()
    };
    let one = marks([Some(101), None, None, None]);
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].1, p.deck_accent(engine::DeckId::ALL[0]));
    let two = marks([None, Some(102), None, None]);
    assert_eq!(two.len(), 1);
    assert_eq!(two[0].1, p.deck_accent(engine::DeckId::ALL[1]));
    // The same track on decks 1 and 2: the lower deck's colour.
    let both = marks([Some(101), Some(101), None, None]);
    assert_eq!(both.len(), 1);
    assert_eq!(both[0].1, p.deck_accent(engine::DeckId::ALL[0]));
    // The marker sits at the row's left edge, level with the list's first cell.
    let lead = painted(&view, [None; 4]);
    let min_x = lead.iter().map(|(r, _)| r.min.x).fold(f32::MAX, f32::min);
    assert!(one[0].0.min.x >= min_x);
    assert!(marks([None; 4]).is_empty());
}

#[test]
fn long_names_are_cut_and_left_aligned() {
    let mut view = view();
    view.list[0].track.as_mut().unwrap().title = Some("x".repeat(200));
    view.tree.push(tree_row(
        "Mix é 🎵 ".repeat(30).trim(),
        1,
        TreeKind::Playlist,
    ));
    let mut covers = Covers::default();
    let view = &view;
    let mut harness = Harness::builder()
        .with_size([1000.0, 300.0])
        .allow_missing_glyphs()
        .build_ui(move |ui| {
            ui::browser::show(
                ui,
                view,
                [None; 4],
                KeyNotation::default(),
                &mut covers,
                &Arrangement::default(),
                |_| {},
            );
        });
    harness.run_steps(3);
    // The title's cell and the playlist's label both end in an ellipsis.
    let texts = painted_text(view);
    let cut = texts.iter().filter(|t| t.ends_with('…')).count();
    assert!(cut >= 2, "{texts:?}");
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("xxxx") && t.ends_with('…'))
    );
    assert!(texts.iter().any(|t| t.contains('🎵') && t.ends_with('…')));
    let title = harness
        .get_all_by_label_contains("xxxx")
        .map(|n| n.rect())
        .min_by(|a, b| a.min.y.total_cmp(&b.min.y))
        .unwrap();
    let artist = harness
        .get_all_by_label("Artist")
        .map(|n| n.rect())
        .min_by(|a, b| a.min.y.total_cmp(&b.min.y))
        .unwrap();
    assert!(title.max.x <= artist.min.x + 1.0);
    let playlist = harness.get_by_label_contains("🎵").rect();
    let house = harness.get_by_label("House").rect();
    assert!((playlist.min.x - house.min.x).abs() <= 1.0);
}

/// The text the browser paints for `view`, row by row.
fn painted_text(view: &BrowserView) -> Vec<String> {
    let ctx = egui::Context::default();
    let mut covers = Covers::default();
    let mut texts = Vec::new();
    for _ in 0..3 {
        texts.clear();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 300.0),
            )),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            ui::browser::show(
                ui,
                view,
                [None; 4],
                KeyNotation::default(),
                &mut covers,
                &Arrangement::default(),
                |_| {},
            );
        });
        output.textures_delta.clear();
        for clipped in &output.shapes {
            if let egui::Shape::Text(text) = &clipped.shape {
                texts.push(
                    text.galley
                        .rows
                        .iter()
                        .map(|r| r.text())
                        .collect::<String>(),
                );
            }
        }
    }
    texts
}
