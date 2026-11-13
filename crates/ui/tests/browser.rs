//! The browser draws headless, without a GPU.

#[allow(dead_code, reason = "only the loaded decks are used here")]
mod support;

use std::path::PathBuf;
use std::sync::mpsc::channel;

use analysis::KeyNotation;
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use engine::DeckId;
use library::{
    BrowserEdit, BrowserView, Focus, ListRow, TrackRow, TrackSortKey, TreeKind, TreeRow,
};
use ui::browser::{Arrangement, COLUMNS, Decks};
use ui::cover::Covers;
use ui::theme;
use ui::{Layout, LibraryStep, UiCommand};

fn tree_row(label: &str, depth: u8, kind: TreeKind) -> TreeRow {
    TreeRow {
        label: label.into(),
        depth,
        kind,
        expanded: false,
        note: None,
        playlist: None,
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
                position: None,
            },
            ListRow {
                path: PathBuf::from("/music/lost.mp3"),
                track: Some(gone),
                file_name: "lost.mp3".into(),
                position: None,
            },
        ],
        list_offset: 0,
        list_count: 2,
        list_cursor: 0,
        search: None,
        sort: None,
        seq: 0,
        playlist: None,
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
                Decks {
                    loaded: [None; 4],
                    ..Decks::default()
                },
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
                Decks {
                    loaded: [None; 4],
                    ..Decks::default()
                },
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
                Decks {
                    loaded,
                    ..Decks::default()
                },
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
                    Decks {
                        loaded: [None; 4],
                        ..Decks::default()
                    },
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
                Decks {
                    loaded: [None; 4],
                    ..Decks::default()
                },
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
    // Row-wide fills only: the search box shares the shade.
    let mut rects: Vec<_> = painted(&view, [None; 4])
        .into_iter()
        .filter(|(rect, fill)| {
            *fill == alt && rect.height() > 10.0 && rect.height() < 30.0 && rect.width() > 250.0
        })
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
fn the_cue_dot_shows_right_of_the_loaded_marker() {
    let view = view();
    let loaded = [Some(1), None, None, None];
    let marker = painted(&view, loaded)
        .into_iter()
        .find(|(rect, _)| (rect.width() - 3.0).abs() < 0.01 && rect.height() > 10.0)
        .expect("the loaded track's marker")
        .0;
    let dot = painted_text_rects(&view, loaded)
        .into_iter()
        .find(|(text, rect)| text == "•" && rect.center().y >= marker.min.y)
        .expect("the loaded track's cue dot")
        .1;
    assert!(dot.min.x >= marker.max.x, "{dot:?} under {marker:?}");
}

/// The texts the browser paints for `view` with `loaded`, with where each is drawn.
fn painted_text_rects(view: &BrowserView, loaded: [Option<i64>; 4]) -> Vec<(String, egui::Rect)> {
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
                Decks {
                    loaded,
                    ..Decks::default()
                },
                KeyNotation::default(),
                &mut covers,
                &Arrangement::default(),
                |_| {},
            );
        });
        output.textures_delta.clear();
        for clipped in &output.shapes {
            if let egui::Shape::Text(text) = &clipped.shape {
                let rect = text.galley.rect.translate(text.pos.to_vec2());
                texts.push((text.galley.text().to_owned(), rect));
            }
        }
    }
    texts
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
                Decks {
                    loaded: [None; 4],
                    ..Decks::default()
                },
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
                Decks {
                    loaded: [None; 4],
                    ..Decks::default()
                },
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

/// What a browser driven by the mouse is shown, and what it asked of the library.
struct Clicking {
    view: BrowserView,
    decks: Decks,
    covers: Covers,
    steps: Vec<LibraryStep>,
}

/// `count` library tracks titled "Song N" by their row in the whole list, the page at
/// `offset` of a list `total` long, with the cursor on its first row.
fn numbered(offset: usize, count: usize, total: usize) -> BrowserView {
    let mut view = view();
    let template = view.list[0].clone();
    view.list = (offset..offset + count)
        .map(|i| {
            let mut row = template.clone();
            let track = row.track.as_mut().unwrap();
            track.id = 1000 + i as i64;
            track.title = Some(format!("Song {i}"));
            row.file_name = format!("song-{i}.mp3");
            row
        })
        .collect();
    view.list_offset = offset;
    view.list_count = total;
    view.list_cursor = offset;
    view
}

/// Draws `view` without its info panel (which repeats the cursor's title), collecting
/// the library steps. Short steps, so two clicks in a row make a double-click.
fn clicking(view: BrowserView, decks: Decks) -> Harness<'static, Clicking> {
    let state = Clicking {
        view,
        decks,
        covers: Covers::default(),
        steps: Vec::new(),
    };
    let arrangement = Arrangement {
        info: false,
        ..Arrangement::default()
    };
    let mut harness = Harness::builder()
        .with_size([1000.0, 300.0])
        .with_step_dt(0.01)
        .build_ui_state(
            move |ui, state: &mut Clicking| {
                let changes = ui::browser::show(
                    ui,
                    &state.view,
                    state.decks,
                    KeyNotation::default(),
                    &mut state.covers,
                    &arrangement,
                    |_| {},
                );
                state.steps.extend(changes.library);
            },
            state,
        );
    harness.run_steps(3);
    harness
}

fn button(button: PointerButton, pressed: bool, at: Pos2) -> Event {
    Event::PointerButton {
        pos: at,
        button,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

fn click_with<S>(harness: &mut Harness<'_, S>, which: PointerButton, at: Pos2) {
    harness.event(Event::PointerMoved(at));
    harness.run_steps(1);
    harness.event(button(which, true, at));
    harness.run_steps(1);
    harness.event(button(which, false, at));
    harness.run_steps(1);
}

fn click<S>(harness: &mut Harness<'_, S>, at: Pos2) {
    click_with(harness, PointerButton::Primary, at);
}

fn double_click<S>(harness: &mut Harness<'_, S>, at: Pos2) {
    click(harness, at);
    click(harness, at);
    harness.run_steps(2);
}

fn at_label(harness: &Harness<'_, Clicking>, label: &str) -> Pos2 {
    harness.get_by_label(label).rect().center()
}

fn browse(edit: BrowserEdit) -> LibraryStep {
    LibraryStep::Browse(edit)
}

fn load(number: usize, track: i64) -> LibraryStep {
    LibraryStep::Load {
        deck: DeckId::new(number - 1).unwrap(),
        track,
    }
}

fn loads(steps: &[LibraryStep]) -> Vec<&LibraryStep> {
    steps
        .iter()
        .filter(|s| matches!(s, LibraryStep::Load { .. }))
        .collect()
}

#[test]
fn clicking_a_row_selects_it() {
    // The row's index in the whole list, not in the page.
    let mut harness = clicking(numbered(100, 50, 1000), Decks::default());
    let at = at_label(&harness, "Song 103");
    click(&mut harness, at);
    assert_eq!(harness.state().steps, [browse(BrowserEdit::SelectRow(103))]);
}

#[test]
fn clicking_the_tree_selects_and_opens() {
    let mut harness = clicking(view(), Decks::default());
    let house = at_label(&harness, "House");
    click(&mut harness, house);
    assert_eq!(harness.state().steps, [browse(BrowserEdit::SelectTree(1))]);

    // The arrow opens the drive, without selecting it.
    harness.state_mut().steps.clear();
    let drive = harness.get_by_label("Macintosh HD").rect();
    // The label starts 28 points right of the arrow's row inset; the arrow sits 4 in.
    let arrow = pos2(drive.min.x - 24.0, drive.center().y);
    click(&mut harness, arrow);
    assert_eq!(
        harness.state().steps,
        [browse(BrowserEdit::ToggleTreeAt(0))]
    );

    // A double-click on a folder opens it too.
    harness.state_mut().steps.clear();
    harness.run_steps(60);
    double_click(&mut harness, house);
    assert!(
        harness
            .state()
            .steps
            .contains(&browse(BrowserEdit::ToggleTreeAt(1))),
        "{:?}",
        harness.state().steps
    );
}

#[test]
fn the_arrow_of_a_node_that_cannot_open_selects_it() {
    let mut view = view();
    let mut playlists = tree_row("Playlists", 0, TreeKind::Playlists);
    playlists.expanded = true;
    view.tree.push(playlists);
    let mut harness = clicking(view, Decks::default());
    let node = harness.get_by_label("Playlists").rect();
    let arrow = pos2(node.min.x - 24.0, node.center().y);
    click(&mut harness, arrow);
    assert_eq!(harness.state().steps, [browse(BrowserEdit::SelectTree(2))]);
}

#[test]
fn double_click_loads_on_the_first_free_deck() {
    let decks = Decks {
        playing: [true, false, false, false],
        ..Decks::default()
    };
    let mut harness = clicking(numbered(0, 20, 20), decks);
    let at = at_label(&harness, "Song 4");
    double_click(&mut harness, at);
    assert_eq!(loads(&harness.state().steps), [&load(2, 1004)]);
    // The first click selects the row; the second, the double-click's, does not again.
    let selects: Vec<_> = harness
        .state()
        .steps
        .iter()
        .filter(|s| matches!(s, LibraryStep::Browse(_)))
        .collect();
    assert_eq!(selects, [&browse(BrowserEdit::SelectRow(4))]);

    // In the four-deck view decks 3 and 4 come after 1 and 2.
    let decks = Decks {
        playing: [true, true, false, false],
        four: true,
        ..Decks::default()
    };
    let mut harness = clicking(numbered(0, 20, 20), decks);
    let at = at_label(&harness, "Song 4");
    double_click(&mut harness, at);
    assert_eq!(loads(&harness.state().steps), [&load(3, 1004)]);
}

#[test]
fn double_click_with_every_deck_playing_sends_nothing() {
    // Decks 3 and 4 are free but not shown.
    let decks = Decks {
        playing: [true, true, false, false],
        ..Decks::default()
    };
    let mut harness = clicking(numbered(0, 20, 20), decks);
    assert_eq!(ui::browser::notice(&harness.ctx), None);
    let at = at_label(&harness, "Song 4");
    double_click(&mut harness, at);
    assert!(loads(&harness.state().steps).is_empty());
    let notice = ui::browser::notice(&harness.ctx).map(|(_, text)| text);
    assert_eq!(notice, Some("Every deck is playing"));
}

#[test]
fn context_menu_loads_on_the_chosen_deck() {
    let mut harness = clicking(numbered(0, 20, 20), Decks::default());
    let at = at_label(&harness, "Song 3");
    click_with(&mut harness, PointerButton::Secondary, at);
    harness.run_steps(2);
    // The two-deck view offers decks 1 and 2.
    harness.get_by_label("Load on deck 1");
    assert!(harness.query_by_label("Load on deck 3").is_none());
    let item = at_label(&harness, "Load on deck 2");
    click(&mut harness, item);
    harness.run_steps(2);
    assert_eq!(loads(&harness.state().steps), [&load(2, 1003)]);
    assert!(harness.query_by_label("Load on deck 1").is_none());

    let decks = Decks {
        four: true,
        ..Decks::default()
    };
    let mut harness = clicking(numbered(0, 20, 20), decks);
    let at = at_label(&harness, "Song 3");
    click_with(&mut harness, PointerButton::Secondary, at);
    harness.run_steps(2);
    harness.get_by_label("Load on deck 4");
}

fn wheel<S>(harness: &mut Harness<'_, S>, at: Pos2, lines: f32) {
    harness.event(Event::PointerMoved(at));
    harness.run_steps(1);
    harness.event(Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        delta: vec2(0.0, lines),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
        modifiers: Modifiers::NONE,
    });
    harness.run_steps(30);
}

#[test]
fn wheel_scrolls_the_list() {
    // Inside the loaded page the window scrolls on its own and sends nothing.
    let mut harness = clicking(numbered(0, 200, 200), Decks::default());
    let at = at_label(&harness, "Song 2");
    assert!(harness.query_by_label("Song 30").is_none());
    wheel(&mut harness, at, -15.0);
    assert!(harness.query_by_label("Song 0").is_none());
    assert!(harness.query_by_label("Song 30").is_some());
    assert!(
        harness.state().steps.is_empty(),
        "{:?}",
        harness.state().steps
    );
    // And back up.
    wheel(&mut harness, at, 30.0);
    assert!(harness.query_by_label("Song 0").is_some());

    // Past the loaded page it asks the session for the next row, once.
    let mut harness = clicking(numbered(0, 200, 1000), Decks::default());
    let at = at_label(&harness, "Song 2");
    for _ in 0..10 {
        wheel(&mut harness, at, -30.0);
    }
    assert_eq!(harness.state().steps, [browse(BrowserEdit::SelectRow(200))]);

    // The tree scrolls too, and asks nothing.
    let mut view = numbered(0, 20, 20);
    view.tree_cursor = 0;
    view.tree = (0..60)
        .map(|i| tree_row(&format!("Folder {i}"), 0, TreeKind::Folder))
        .collect();
    let mut harness = clicking(view, Decks::default());
    let at = at_label(&harness, "Folder 1");
    wheel(&mut harness, at, -10.0);
    assert!(harness.query_by_label("Folder 0").is_none());
    assert!(harness.state().steps.is_empty());
}

fn search_step(text: Option<&str>, seq: u64) -> LibraryStep {
    browse(BrowserEdit::Search {
        text: text.map(str::to_owned),
        seq,
    })
}

fn type_in_search(harness: &mut Harness<'_, Clicking>, text: &str) {
    harness.get_by_role(Role::TextInput).focus();
    harness.run_steps(1);
    harness.get_by_role(Role::TextInput).type_text(text);
    harness.run_steps(2);
}

#[test]
fn typing_sends_search() {
    let mut harness = clicking(numbered(0, 20, 20), Decks::default());
    // The box above the list shows its hint while empty.
    assert!(
        painted_text(&harness.state().view)
            .iter()
            .any(|t| t == "Search")
    );
    type_in_search(&mut harness, "dee");
    type_in_search(&mut harness, "p");
    // One query per change, each newer than the last.
    assert_eq!(
        harness.state().steps,
        [search_step(Some("dee"), 1), search_step(Some("deep"), 2)]
    );
}

#[test]
fn escape_clears_search() {
    let mut harness = clicking(numbered(0, 20, 20), Decks::default());
    type_in_search(&mut harness, "deep");
    harness.key_press(Key::Escape);
    harness.run_steps(2);
    assert_eq!(
        harness.state().steps,
        [search_step(Some("deep"), 1), search_step(None, 2)]
    );
    // The box is empty again: typing starts afresh.
    type_in_search(&mut harness, "x");
    assert_eq!(harness.state().steps[2], search_step(Some("x"), 3));
}

#[test]
fn header_click_sends_sort_then_reverses() {
    let mut harness = clicking(numbered(0, 20, 20), Decks::default());
    let sort = |key, descending, seq| {
        browse(BrowserEdit::Sort {
            key,
            descending,
            seq,
        })
    };
    let bpm = at_label(&harness, "BPM");
    click(&mut harness, bpm);
    harness.run_steps(2);
    assert_eq!(
        harness.state().steps,
        [sort(Some(TrackSortKey::Bpm), false, 1)]
    );
    // The session answers; the next click reverses, and the third returns to "#".
    harness.state_mut().view.sort = Some((TrackSortKey::Bpm, false));
    harness.state_mut().view.seq = 1;
    harness.run_steps(2);
    click(&mut harness, bpm);
    harness.run_steps(2);
    harness.state_mut().view.sort = Some((TrackSortKey::Bpm, true));
    harness.state_mut().view.seq = 2;
    harness.run_steps(2);
    click(&mut harness, bpm);
    harness.run_steps(2);
    // Another column starts ascending.
    let title = at_label(&harness, "Title");
    click(&mut harness, title);
    harness.run_steps(2);
    assert_eq!(
        harness.state().steps,
        [
            sort(Some(TrackSortKey::Bpm), false, 1),
            sort(Some(TrackSortKey::Bpm), true, 2),
            sort(None, false, 3),
            sort(Some(TrackSortKey::Title), false, 4),
        ]
    );
    // Remix has no sort.
    let remix = at_label(&harness, "Remix");
    click(&mut harness, remix);
    harness.run_steps(2);
    assert_eq!(harness.state().steps.len(), 4);
}

#[test]
fn a_view_older_than_the_latest_query_is_dropped() {
    let ctx = egui::Context::default();
    let mut fresh = ui::browser::Fresh::default();
    let mut old = numbered(0, 20, 20);
    old.seq = 1;
    assert_eq!(fresh.pick(&ctx, &old), &old);
    // The window sent query 3: the view of query 2 is not shown, the last fresh one is.
    ui::browser::set_sent_seq(&ctx, 3);
    let mut stale = numbered(0, 5, 5);
    stale.seq = 2;
    assert_eq!(fresh.pick(&ctx, &stale), &old);
    let mut latest = numbered(0, 3, 3);
    latest.seq = 3;
    assert_eq!(fresh.pick(&ctx, &latest), &latest);
    let mut newer = numbered(0, 4, 4);
    newer.seq = 4;
    assert_eq!(fresh.pick(&ctx, &newer), &newer);
}

/// Presses at `from`, moves past the drag threshold and on to `to` in steps, and lets go.
fn drag_to<S>(harness: &mut Harness<'_, S>, from: Pos2, to: Pos2) {
    harness.event(Event::PointerMoved(from));
    harness.run_steps(1);
    harness.event(button(PointerButton::Primary, true, from));
    harness.run_steps(1);
    for t in [0.1_f32, 0.3, 0.6, 1.0] {
        harness.event(Event::PointerMoved(from + (to - from) * t));
        harness.run_steps(1);
    }
    harness.event(button(PointerButton::Primary, false, to));
    harness.run_steps(2);
}

/// The whole window with two decks, sending what it asks for to the returned receiver.
fn window() -> (Harness<'static>, std::sync::mpsc::Receiver<UiCommand>) {
    let (frame, reads) = support::loaded(Layout::Pro, false);
    let (commands, asked) = channel();
    let mut screen = ui::Screen {
        commands,
        ..ui::Screen::default()
    };
    let mut harness = Harness::builder()
        .with_size([1440.0, 900.0])
        .build_ui(move |ui| ui::root_with_reads(ui, &frame, &reads, &mut screen));
    harness.run_steps(3);
    (harness, asked)
}

/// The library steps among `commands`, the browser's own moves left out.
fn library_steps(commands: impl Iterator<Item = UiCommand>) -> Vec<LibraryStep> {
    commands
        .filter_map(|c| match c {
            UiCommand::Library(LibraryStep::Browse(_)) => None,
            UiCommand::Library(step) => Some(step),
            _ => None,
        })
        .collect()
}

#[test]
fn drag_track_onto_deck_two_loads_it() {
    let (mut harness, asked) = window();
    let row = harness.get_by_label("Track Number 5").rect().center();
    let deck = harness.get_by_label("deck panel 2").rect().center();
    drag_to(&mut harness, row, deck);
    assert_eq!(library_steps(asked.try_iter()), [load(2, 5)]);
    // The lanes take drops too: deck 1's lane is the top one.
    let lanes = harness.get_by_label("lanes").rect();
    let lane = pos2(lanes.center().x, lanes.min.y + 10.0);
    let row = harness.get_by_label("Track Number 7").rect().center();
    drag_to(&mut harness, row, lane);
    assert_eq!(library_steps(asked.try_iter()), [load(1, 7)]);
}

#[test]
fn a_drop_outside_any_target_does_nothing() {
    let (mut harness, asked) = window();
    let row = harness.get_by_label("Track Number 5").rect().center();
    // The mixer, between the decks.
    let row_of_decks = harness.get_by_label("deck row").rect();
    let mixer = pos2(row_of_decks.center().x, row_of_decks.max.y - 20.0);
    drag_to(&mut harness, row, mixer);
    // The list's header, which is no playlist position either.
    let header = harness.get_by_label("Title").rect().center();
    drag_to(&mut harness, row, header);
    harness.run_steps(2);
    assert_eq!(library_steps(asked.try_iter()), []);
    assert!(!egui::DragAndDrop::has_any_payload(&harness.ctx));
}

/// Presses and lets go at `at` with `modifiers` held.
fn click_holding<S>(harness: &mut Harness<'_, S>, at: Pos2, modifiers: Modifiers) {
    harness.event(Event::ModifiersChanged(modifiers));
    harness.event(Event::PointerMoved(at));
    harness.run_steps(1);
    harness.event(Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: true,
        modifiers,
    });
    harness.run_steps(1);
    harness.event(Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: false,
        modifiers,
    });
    harness.run_steps(1);
    harness.event(Event::ModifiersChanged(Modifiers::NONE));
    // Past the double-click time.
    harness.run_steps(60);
}

#[test]
fn shift_and_cmd_click_build_a_selection() {
    let mut harness = clicking(numbered(0, 20, 20), Decks::default());
    let song = |harness: &Harness<'_, Clicking>, n: usize| at_label(harness, &format!("Song {n}"));
    let picked = |harness: &Harness<'_, Clicking>| ui::browser::selected_rows(&harness.ctx);
    let at = song(&harness, 2);
    click_holding(&mut harness, at, Modifiers::NONE);
    assert_eq!(picked(&harness), [2]);
    for n in [5, 7, 5] {
        let at = song(&harness, n);
        click_holding(&mut harness, at, Modifiers::COMMAND);
    }
    // 5 went in and out again.
    assert_eq!(picked(&harness), [2, 7]);
    // Shift picks from the last row Cmd-clicked in.
    let at = song(&harness, 4);
    click_holding(&mut harness, at, Modifiers::SHIFT);
    assert_eq!(picked(&harness), [4, 5, 6, 7]);
    // A plain click picks that row only.
    let at = song(&harness, 3);
    click_holding(&mut harness, at, Modifiers::NONE);
    assert_eq!(picked(&harness), [3]);
    // The cursor follows every click but the one that took a row out.
    let selects: Vec<_> = harness
        .state()
        .steps
        .iter()
        .filter_map(|s| match s {
            LibraryStep::Browse(BrowserEdit::SelectRow(at)) => Some(*at),
            _ => None,
        })
        .collect();
    assert_eq!(selects, [2, 5, 7, 4, 3]);
    // Another list starts with nothing picked: a search answered, here.
    let at = song(&harness, 6);
    click_holding(&mut harness, at, Modifiers::COMMAND);
    assert_eq!(picked(&harness), [3, 6]);
    harness.state_mut().view.seq = 1;
    harness.run_steps(2);
    assert!(picked(&harness).is_empty());
}

/// `numbered` rows with "Warm up" (id 40) and "Peak" (id 41) under the Playlists node.
fn with_playlists(mut view: BrowserView) -> BrowserView {
    let mut node = tree_row("Playlists", 0, TreeKind::Playlists);
    node.expanded = true;
    view.tree.push(node);
    for (name, id) in [("Warm up", 40), ("Peak", 41)] {
        let mut row = tree_row(name, 1, TreeKind::Playlist);
        row.playlist = Some(id);
        view.tree.push(row);
    }
    view
}

#[test]
fn dragging_a_selection_onto_a_playlist_adds_it_in_list_order() {
    let mut view = with_playlists(numbered(0, 20, 20));
    // Row 4 is a file the library does not have.
    view.list[4].track = None;
    view.list[4].file_name = "unscanned.mp3".into();
    let mut harness = clicking(view, Decks::default());
    let song = |harness: &Harness<'_, Clicking>, n: usize| at_label(harness, &format!("Song {n}"));
    let at = song(&harness, 6);
    click_holding(&mut harness, at, Modifiers::NONE);
    let at = song(&harness, 2);
    click_holding(&mut harness, at, Modifiers::COMMAND);
    let at = at_label(&harness, "unscanned.mp3");
    click_holding(&mut harness, at, Modifiers::COMMAND);
    harness.state_mut().steps.clear();
    let from = song(&harness, 6);
    let to = at_label(&harness, "Peak");
    drag_to(&mut harness, from, to);
    // In list order, the unscanned file left out.
    assert_eq!(
        harness.state().steps,
        [LibraryStep::AddToPlaylist {
            playlist: 41,
            tracks: vec![1002, 1006],
        }]
    );
    // A row not picked goes alone.
    harness.state_mut().steps.clear();
    let from = song(&harness, 9);
    let to = at_label(&harness, "Warm up");
    drag_to(&mut harness, from, to);
    assert_eq!(
        harness.state().steps,
        [LibraryStep::AddToPlaylist {
            playlist: 40,
            tracks: vec![1009],
        }]
    );
    // An unscanned file alone carries nothing; a drop on a folder does nothing either.
    let from = at_label(&harness, "unscanned.mp3");
    click_holding(&mut harness, from, Modifiers::NONE);
    harness.state_mut().steps.clear();
    drag_to(&mut harness, from, to);
    let from = song(&harness, 9);
    let folder = at_label(&harness, "House");
    drag_to(&mut harness, from, folder);
    assert_eq!(harness.state().steps, []);
}

/// `numbered` rows as the playlist with id 41, each at its row's position.
fn in_playlist(offset: usize, count: usize) -> BrowserView {
    let mut view = with_playlists(numbered(offset, count, offset + count));
    view.playlist = Some(41);
    for (at, row) in view.list.iter_mut().enumerate() {
        row.position = Some(offset + at);
    }
    view
}

#[test]
fn dragging_within_the_playlist_moves_the_rows() {
    let mut harness = clicking(in_playlist(0, 20), Decks::default());
    let song = |harness: &Harness<'_, Clicking>, n: usize| {
        harness.get_by_label(&format!("Song {n}")).rect()
    };
    // Onto the upper half of row 1: before it.
    let from = song(&harness, 5).center();
    let to = song(&harness, 1);
    drag_to(&mut harness, from, pos2(to.center().x, to.min.y + 3.0));
    // The lower half of row 7: after it.
    let to = song(&harness, 7);
    drag_to(&mut harness, from, pos2(to.center().x, to.max.y - 3.0));
    // Each row's position goes with the track the window saw there.
    let moves = |from: Vec<usize>, to| LibraryStep::MoveInPlaylist {
        playlist: 41,
        from: from.into_iter().map(|at| (at, 1000 + at as i64)).collect(),
        to,
    };
    assert_eq!(
        harness.state().steps,
        [moves(vec![5], 1), moves(vec![5], 8)]
    );

    // A picked pair moves together, by their positions.
    harness.state_mut().steps.clear();
    for (n, modifiers) in [(3, Modifiers::NONE), (9, Modifiers::COMMAND)] {
        let at = song(&harness, n).center();
        click_holding(&mut harness, at, modifiers);
    }
    harness.state_mut().steps.clear();
    let from = song(&harness, 9).center();
    let to = song(&harness, 0);
    drag_to(&mut harness, from, pos2(to.center().x, to.min.y + 3.0));
    assert_eq!(harness.state().steps, [moves(vec![3, 9], 0)]);

    // Searched, the rows are not in playlist order: no move.
    harness.state_mut().steps.clear();
    harness.state_mut().view.search = Some("song".into());
    harness.run_steps(2);
    let from = song(&harness, 5).center();
    let to = song(&harness, 1);
    drag_to(&mut harness, from, pos2(to.center().x, to.min.y + 3.0));
    assert_eq!(harness.state().steps, []);
}

#[test]
fn the_track_menu_adds_to_and_removes_from_a_playlist() {
    // A search shows the playlist's rows 10 to 19 from list row 0: their positions are
    // not their rows.
    let mut view = in_playlist(0, 10);
    for row in &mut view.list {
        row.position = row.position.map(|p| p + 10);
    }
    view.search = Some("song".into());
    let mut harness = clicking(view, Decks::default());
    let song = |harness: &Harness<'_, Clicking>, n: usize| at_label(harness, &format!("Song {n}"));
    for (n, modifiers) in [(1, Modifiers::NONE), (4, Modifiers::COMMAND)] {
        let at = song(&harness, n);
        click_holding(&mut harness, at, modifiers);
    }
    harness.state_mut().steps.clear();
    // Right-click a picked row: the menu acts on the pick.
    let at = song(&harness, 4);
    click_with(&mut harness, PointerButton::Secondary, at);
    harness.run_steps(2);
    let item = at_label(&harness, "Add to playlist");
    harness.event(Event::PointerMoved(item));
    harness.run_steps(10);
    // The submenu's entry, right of the tree's.
    let warm = harness
        .get_all_by_label("Warm up")
        .map(|n| n.rect().center())
        .max_by(|a, b| a.x.total_cmp(&b.x))
        .unwrap();
    click(&mut harness, warm);
    harness.run_steps(2);
    assert_eq!(
        harness.state().steps,
        [LibraryStep::AddToPlaylist {
            playlist: 40,
            tracks: vec![1001, 1004],
        }]
    );
    // Remove from playlist takes the pick's positions in the playlist.
    harness.state_mut().steps.clear();
    let at = song(&harness, 1);
    click_with(&mut harness, PointerButton::Secondary, at);
    harness.run_steps(2);
    let item = at_label(&harness, "Remove from playlist");
    click(&mut harness, item);
    harness.run_steps(2);
    assert_eq!(
        harness.state().steps,
        [LibraryStep::RemoveFromPlaylist {
            playlist: 41,
            positions: vec![(11, 1001), (14, 1004)],
        }]
    );
    // A row not picked: that row alone. Outside a playlist there is no removal.
    harness.state_mut().steps.clear();
    let at = song(&harness, 7);
    click_with(&mut harness, PointerButton::Secondary, at);
    harness.run_steps(2);
    let item = at_label(&harness, "Remove from playlist");
    click(&mut harness, item);
    harness.run_steps(2);
    assert_eq!(
        harness.state().steps,
        [LibraryStep::RemoveFromPlaylist {
            playlist: 41,
            positions: vec![(17, 1007)],
        }]
    );
    let mut harness = clicking(with_playlists(numbered(0, 20, 20)), Decks::default());
    let at = song(&harness, 7);
    click_with(&mut harness, PointerButton::Secondary, at);
    harness.run_steps(2);
    harness.get_by_label("Add to playlist");
    assert!(harness.query_by_label("Remove from playlist").is_none());
}

#[test]
fn rows_dropped_back_on_their_own_playlist_do_nothing() {
    // The list is "Peak" (41): its rows dropped on "Peak" in the tree overshot a move.
    let mut harness = clicking(in_playlist(0, 20), Decks::default());
    let from = at_label(&harness, "Song 5");
    let peak = at_label(&harness, "Peak");
    drag_to(&mut harness, from, peak);
    assert_eq!(harness.state().steps, []);
    assert!(!egui::DragAndDrop::has_any_payload(&harness.ctx));
    // On another playlist they are added.
    let warm = at_label(&harness, "Warm up");
    drag_to(&mut harness, from, warm);
    assert_eq!(
        harness.state().steps,
        [LibraryStep::AddToPlaylist {
            playlist: 40,
            tracks: vec![1005],
        }]
    );
}

/// Right-clicks the tree row labelled `row` and clicks the menu item `item`.
fn tree_menu(harness: &mut Harness<'_, Clicking>, row: &str, item: &str) {
    let at = at_label(harness, row);
    click_with(harness, PointerButton::Secondary, at);
    harness.run_steps(2);
    let item = at_label(harness, item);
    click(harness, item);
    harness.run_steps(3);
}

fn playlist_harness() -> Harness<'static, Clicking> {
    clicking(with_playlists(numbered(0, 20, 20)), Decks::default())
}

#[test]
fn new_playlist_from_the_playlists_node() {
    let mut harness = playlist_harness();
    // Only the Playlists node offers it; a playlist offers rename and delete.
    tree_menu(&mut harness, "Playlists", "New playlist\u{2026}");
    harness.get_by_label("New playlist");
    // The name field has the keyboard: typing and OK send the step.
    harness.event(Event::Text("Fresh".into()));
    harness.run_steps(2);
    let ok = at_label(&harness, "OK");
    click(&mut harness, ok);
    harness.run_steps(2);
    assert_eq!(
        harness.state().steps,
        [LibraryStep::CreatePlaylist {
            name: "Fresh".into()
        }]
    );
    assert!(harness.query_by_label("OK").is_none());

    // Enter means OK.
    harness.state_mut().steps.clear();
    tree_menu(&mut harness, "Playlists", "New playlist\u{2026}");
    harness.event(Event::Text("Second".into()));
    harness.run_steps(2);
    harness.key_press(Key::Enter);
    harness.run_steps(3);
    assert_eq!(
        harness.state().steps,
        [LibraryStep::CreatePlaylist {
            name: "Second".into()
        }]
    );
    assert!(harness.query_by_label("OK").is_none());

    // Escape means Cancel, and so does Cancel.
    harness.state_mut().steps.clear();
    tree_menu(&mut harness, "Playlists", "New playlist\u{2026}");
    harness.event(Event::Text("Never".into()));
    harness.run_steps(2);
    harness.key_press(Key::Escape);
    harness.run_steps(3);
    assert!(harness.query_by_label("OK").is_none());
    tree_menu(&mut harness, "Playlists", "New playlist\u{2026}");
    let cancel = at_label(&harness, "Cancel");
    click(&mut harness, cancel);
    harness.run_steps(3);
    assert!(harness.query_by_label("OK").is_none());
    assert_eq!(harness.state().steps, []);

    // An empty name cannot be sent from the window either.
    tree_menu(&mut harness, "Playlists", "New playlist\u{2026}");
    let ok = at_label(&harness, "OK");
    click(&mut harness, ok);
    harness.key_press(Key::Enter);
    harness.run_steps(3);
    assert_eq!(harness.state().steps, []);
    harness.get_by_label("OK");
}

#[test]
fn a_playlist_menu_has_no_new_and_the_node_no_delete() {
    let mut harness = playlist_harness();
    let at = at_label(&harness, "Peak");
    click_with(&mut harness, PointerButton::Secondary, at);
    harness.run_steps(2);
    harness.get_by_label("Rename\u{2026}");
    harness.get_by_label("Delete\u{2026}");
    assert!(harness.query_by_label("New playlist\u{2026}").is_none());
}

#[test]
fn rename_playlist() {
    let mut harness = playlist_harness();
    tree_menu(&mut harness, "Peak", "Rename\u{2026}");
    harness.get_by_label("Rename playlist");
    // The field holds the current name, selected: typing replaces it.
    harness.event(Event::Text("Closing".into()));
    harness.run_steps(2);
    harness.key_press(Key::Enter);
    harness.run_steps(3);
    assert_eq!(
        harness.state().steps,
        [LibraryStep::RenamePlaylist {
            playlist: 41,
            name: "Closing".into()
        }]
    );
    assert!(harness.query_by_label("OK").is_none());
}

#[test]
fn delete_asks_first_and_cancel_keeps_it() {
    let mut harness = playlist_harness();
    tree_menu(&mut harness, "Peak", "Delete\u{2026}");
    harness.get_by_label("Delete playlist");
    harness.get_by_label("Delete Peak? Its tracks stay in the library.");
    assert_eq!(harness.state().steps, []);
    // Cancel has the focus: Enter cancels.
    harness.key_press(Key::Enter);
    harness.run_steps(3);
    assert!(harness.query_by_label("Delete playlist").is_none());
    assert_eq!(harness.state().steps, []);
    // Escape and the Cancel button too.
    tree_menu(&mut harness, "Peak", "Delete\u{2026}");
    harness.key_press(Key::Escape);
    harness.run_steps(3);
    assert!(harness.query_by_label("Delete playlist").is_none());
    tree_menu(&mut harness, "Peak", "Delete\u{2026}");
    let cancel = at_label(&harness, "Cancel");
    click(&mut harness, cancel);
    harness.run_steps(3);
    assert!(harness.query_by_label("Delete playlist").is_none());
    assert_eq!(harness.state().steps, []);
}

#[test]
fn delete_confirmed_deletes() {
    let mut harness = playlist_harness();
    tree_menu(&mut harness, "Peak", "Delete\u{2026}");
    let delete = at_label(&harness, "Delete");
    click(&mut harness, delete);
    harness.run_steps(3);
    assert_eq!(
        harness.state().steps,
        [LibraryStep::DeletePlaylist { playlist: 41 }]
    );
    assert!(harness.query_by_label("Delete playlist").is_none());
}

#[test]
fn the_press_that_closes_a_dialog_reaches_nothing_under_it() {
    let mut harness = playlist_harness();
    tree_menu(&mut harness, "Peak", "Delete\u{2026}");
    // A click on a track row outside the dialog closes it and selects nothing.
    let row = at_label(&harness, "Song 3");
    click(&mut harness, row);
    harness.run_steps(3);
    assert!(harness.query_by_label("Delete playlist").is_none());
    assert_eq!(harness.state().steps, []);
}
