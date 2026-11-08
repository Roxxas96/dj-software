//! The browser draws headless, without a GPU.

use std::path::PathBuf;

use analysis::KeyNotation;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use library::{BrowserView, Focus, ListRow, TrackRow, TreeKind, TreeRow};
use ui::cover::Covers;

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
        .build_ui(move |ui| ui::browser::show(ui, view, KeyNotation::default(), &mut covers));
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
