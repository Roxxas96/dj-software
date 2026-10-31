//! The browser model behind the BROWSE knob: a tree of playlists, folders and all tracks,
//! and a paged track list.

use std::path::MAIN_SEPARATOR_STR as SEP;

use library::{
    BROWSER_PAGE_ROWS, Browser, BrowserRequest, Focus, Library, RawStatement, WriteOp, WriteOutcome,
};
use rusqlite::types::Value;

fn open() -> (tempfile::TempDir, Library) {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(&dir.path().join("library.sqlite3")).unwrap();
    (dir, lib)
}

/// `parts` joined with the platform's separator, as an absolute path text.
fn path(parts: &[&str]) -> String {
    format!("{SEP}{}", parts.join(SEP))
}

/// Inserts one track per `(path, title)` (artist "a") and returns their ids in order.
fn add_tracks(lib: &Library, tracks: &[(String, String)]) -> Vec<i64> {
    let stmts = tracks
        .iter()
        .map(|(p, t)| RawStatement {
            sql: "INSERT INTO tracks(path, fingerprint, title, artist, added_at) \
                  VALUES (?1, x'00', ?2, 'a', 0)"
                .into(),
            params: vec![Value::Text(p.clone()), Value::Text(t.clone())],
        })
        .collect();
    lib.writer().submit_wait(WriteOp::Raw(stmts)).unwrap();
    let reader = lib.reader().unwrap();
    tracks
        .iter()
        .map(|(p, _)| {
            reader
                .connection()
                .query_row("SELECT id FROM tracks WHERE path = ?1", [p], |r| r.get(0))
                .unwrap()
        })
        .collect()
}

fn create_playlist(lib: &Library, name: &str, tracks: &[i64]) -> i64 {
    let id = match lib
        .writer()
        .submit_wait(WriteOp::CreatePlaylist { name: name.into() })
        .unwrap()
    {
        WriteOutcome::Created { id } => id,
        other => panic!("unexpected outcome {other:?}"),
    };
    for &track in tracks {
        lib.writer()
            .submit_wait(WriteOp::AddToPlaylist {
                playlist: id,
                track,
                at: None,
            })
            .unwrap();
    }
    id
}

fn add_folder(lib: &Library, folder: &str) {
    lib.writer()
        .submit_wait(WriteOp::AddFolder(folder.to_owned()))
        .unwrap();
}

/// Two playlists, two folders and six tracks: three under `/m/house`, two under
/// `/m/techno` and one elsewhere.
struct Fixture {
    _dir: tempfile::TempDir,
    lib: Library,
    ids: Vec<i64>,
}

fn fixture() -> Fixture {
    let (dir, lib) = open();
    let tracks: Vec<(String, String)> = [
        (path(&["m", "house", "1.flac"]), "h1"),
        (path(&["m", "house", "2.flac"]), "h2"),
        (path(&["m", "house", "3.flac"]), "h3"),
        (path(&["m", "techno", "1.flac"]), "t1"),
        (path(&["m", "techno", "2.flac"]), "t2"),
        (path(&["other", "x.flac"]), "x"),
    ]
    .into_iter()
    .map(|(p, t)| (p, t.to_owned()))
    .collect();
    let ids = add_tracks(&lib, &tracks);
    create_playlist(&lib, "Peak time", &[ids[4], ids[0], ids[3]]);
    create_playlist(&lib, "Warm up", &[ids[1]]);
    add_folder(&lib, &path(&["m", "techno"]));
    add_folder(&lib, &path(&["m", "house"]));
    Fixture {
        _dir: dir,
        lib,
        ids,
    }
}

fn browser(lib: &Library) -> Browser {
    Browser::new(lib.reader().unwrap()).unwrap()
}

fn scroll(b: &mut Browser, steps: i32) -> library::BrowserView {
    b.apply(BrowserRequest::Scroll(steps)).unwrap()
}

fn toggle(b: &mut Browser) -> library::BrowserView {
    b.apply(BrowserRequest::ToggleFocus).unwrap()
}

#[test]
fn tree_lists_playlists_folders_all() {
    let f = fixture();
    let mut b = browser(&f.lib);
    let start = b.view();
    assert_eq!(start.focus, Focus::Tree);
    assert_eq!((start.index, start.count), (0, 5));

    // Each tree entry with the length of its track list.
    let house = format!("Folders ▸ {}", path(&["m", "house"]));
    let techno = format!("Folders ▸ {}", path(&["m", "techno"]));
    let expected = [
        ("Playlists ▸ Peak time", 3),
        ("Playlists ▸ Warm up", 1),
        (house.as_str(), 3),
        (techno.as_str(), 2),
        ("All tracks", 6),
    ];
    for (i, (label, len)) in expected.into_iter().enumerate() {
        let view = if i == 0 { b.view() } else { scroll(&mut b, 1) };
        assert_eq!(view.path, label);
        assert_eq!((view.index, view.count), (i, 5), "{label}");
        let list = toggle(&mut b);
        assert_eq!(list.focus, Focus::List);
        assert_eq!(list.path, label);
        assert_eq!((list.index, list.count), (0, len), "{label}");
        toggle(&mut b);
    }

    // An empty library still has "All tracks".
    let (_dir, empty) = open();
    let mut b = browser(&empty);
    let view = b.view();
    assert_eq!(view.path, "All tracks");
    assert_eq!((view.index, view.count), (0, 1));
    let list = toggle(&mut b);
    assert_eq!((list.index, list.count, list.track), (0, 0, None));
    assert_eq!(b.selected_track(), None);
}

#[test]
fn scroll_clamps() {
    let f = fixture();
    let mut b = browser(&f.lib);
    // The tree does not wrap at either end.
    assert_eq!(scroll(&mut b, -1).index, 0);
    assert_eq!(scroll(&mut b, 100).index, 4);
    assert_eq!(scroll(&mut b, 1).index, 4);
    assert_eq!(scroll(&mut b, i32::MAX).path, "All tracks");
    assert_eq!(scroll(&mut b, i32::MIN).index, 0);

    // Nor does the list.
    toggle(&mut b);
    assert_eq!(scroll(&mut b, -3).index, 0);
    assert_eq!(scroll(&mut b, 2).index, 2);
    assert_eq!(scroll(&mut b, 7).index, 2);
    assert_eq!(scroll(&mut b, i32::MAX).index, 2);
    assert_eq!(scroll(&mut b, i32::MIN).index, 0);
}

#[test]
fn tree_move_resets_list() {
    let f = fixture();
    let mut b = browser(&f.lib);
    // Into the last entry, "All tracks", down its list.
    scroll(&mut b, 4);
    toggle(&mut b);
    assert_eq!(scroll(&mut b, 4).index, 4);

    // A tree scroll that does not move (the end of the tree) keeps the list cursor.
    toggle(&mut b);
    scroll(&mut b, 1);
    assert_eq!(toggle(&mut b).index, 4);

    // A tree move starts the new list at its top.
    toggle(&mut b);
    let view = scroll(&mut b, -1);
    assert_eq!(view.path, format!("Folders ▸ {}", path(&["m", "techno"])));
    let list = toggle(&mut b);
    assert_eq!((list.index, list.count), (0, 2));
}

#[test]
fn toggle_focus() {
    let f = fixture();
    let mut b = browser(&f.lib);
    assert_eq!(b.view().focus, Focus::Tree);
    let list = toggle(&mut b);
    assert_eq!(list.focus, Focus::List);
    // Scrolling now moves the list, not the tree.
    let view = scroll(&mut b, 1);
    assert_eq!((view.index, view.count), (1, 3));
    assert_eq!(view.path, "Playlists ▸ Peak time");

    let tree = toggle(&mut b);
    assert_eq!(tree.focus, Focus::Tree);
    assert_eq!((tree.index, tree.count), (0, 5));
    // Toggling back keeps the list where it was.
    assert_eq!(toggle(&mut b).index, 1);
}

#[test]
fn selected_track_follows_cursor() {
    let f = fixture();
    let mut b = browser(&f.lib);
    // "Peak time" holds tracks 4, 0 and 3, in that order. The list cursor marks a track
    // whichever pane has focus.
    assert_eq!(b.selected_track().map(|t| t.id), Some(f.ids[4]));
    assert_eq!(b.view().track.map(|t| t.id), Some(f.ids[4]));
    toggle(&mut b);
    for (step, expected) in [(1, 0), (1, 3), (-2, 4)] {
        let view = scroll(&mut b, step);
        assert_eq!(view.track.as_ref().map(|t| t.id), Some(f.ids[expected]));
        assert_eq!(b.selected_track(), view.track);
    }
    assert_eq!(b.selected_track().unwrap().title.as_deref(), Some("t2"));

    // A new tree entry selects its first track.
    toggle(&mut b);
    let view = scroll(&mut b, 1);
    assert_eq!(view.track.map(|t| t.id), Some(f.ids[1]));
}

#[test]
fn pages_large_lists() {
    let (_dir, lib) = open();
    let tracks: Vec<(String, String)> = (0..5_000)
        .map(|i| (path(&["m", &format!("{i:04}.flac")]), format!("t{i:04}")))
        .collect();
    let ids = add_tracks(&lib, &tracks);
    let mut b = browser(&lib);
    assert_eq!(b.view().path, "All tracks");
    let list = toggle(&mut b);
    assert_eq!((list.index, list.count), (0, 5_000));
    assert!(b.loaded_rows() <= BROWSER_PAGE_ROWS);

    // Every artist is "a", so the list is in id order.
    let view = scroll(&mut b, 4_999);
    assert_eq!(view.index, 4_999);
    assert_eq!(view.track.map(|t| t.id), Some(ids[4_999]));
    assert!(b.loaded_rows() <= BROWSER_PAGE_ROWS, "{}", b.loaded_rows());

    // Stepping through a page boundary one row at a time.
    for i in (4_700..4_999).rev() {
        let view = scroll(&mut b, -1);
        assert_eq!(view.index, i);
        assert_eq!(view.track.map(|t| t.id), Some(ids[i]), "row {i}");
    }
    let view = scroll(&mut b, -10_000);
    assert_eq!(view.track.map(|t| t.id), Some(ids[0]));
    assert!(b.loaded_rows() <= BROWSER_PAGE_ROWS);
}

#[test]
fn refresh_sees_library_changes() {
    let f = fixture();
    let mut b = browser(&f.lib);
    scroll(&mut b, 1);
    create_playlist(&f.lib, "Closing", &[f.ids[5], f.ids[2]]);
    let view = b.refresh().unwrap();
    // The selected entry stays selected; the new playlist is listed after it.
    assert_eq!(view.path, "Playlists ▸ Warm up");
    assert_eq!((view.index, view.count), (1, 6));
    let view = scroll(&mut b, 1);
    assert_eq!(view.path, "Playlists ▸ Closing");
    assert_eq!(view.track.map(|t| t.id), Some(f.ids[5]));
}
