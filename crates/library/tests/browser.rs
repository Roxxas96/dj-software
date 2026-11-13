//! The browser model behind the BROWSE knob: a tree of the drives' folders and the
//! playlists, and a paged list of the highlighted folder's files or playlist's tracks.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use library::{
    BROWSER_PAGE_ROWS, Browser, BrowserEdit, BrowserRequest, BrowserView, Drive, Focus, Library,
    RawStatement, TrackSortKey, TreeKind, WriteOp, WriteOutcome,
};
use rusqlite::types::Value;

fn open() -> (tempfile::TempDir, Library) {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(&dir.path().join("library.sqlite3")).unwrap();
    (dir, lib)
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

/// A library track for the file at `path`, titled `title`.
fn add_file_track(lib: &Library, path: &Path, title: &str) -> i64 {
    let path = path.to_str().unwrap().to_owned();
    add_tracks(lib, &[(path, title.to_owned())])[0]
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

fn drive(name: &str, path: &Path) -> Drive {
    Drive {
        name: name.to_owned(),
        path: path.to_owned(),
    }
}

/// A browser on `roots` (and the library's playlists).
fn browser(lib: &Library, roots: Vec<Drive>) -> Browser {
    Browser::with_roots(lib.reader().unwrap(), move || roots.clone()).unwrap()
}

fn scroll(b: &mut Browser, steps: i32) -> BrowserView {
    b.apply(BrowserRequest::Scroll(steps)).unwrap()
}

fn toggle(b: &mut Browser) -> BrowserView {
    b.apply(BrowserRequest::ToggleFocus).unwrap()
}

fn toggle_folder(b: &mut Browser) -> BrowserView {
    b.apply(BrowserRequest::ToggleFolder).unwrap()
}

fn labels(view: &BrowserView) -> Vec<&str> {
    view.tree.iter().map(|r| r.label.as_str()).collect()
}

fn file_names(view: &BrowserView) -> Vec<&str> {
    view.list.iter().map(|r| r.file_name.as_str()).collect()
}

/// `root/{A/{a1.mp3,a2.wav,sub/s1.mp3},B/,.hidden/,notes.txt}`. The root is canonical, as
/// the library stores paths.
fn disk() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    for folder in ["A/sub", "B", ".hidden"] {
        fs::create_dir_all(root.join(folder)).unwrap();
    }
    for file in [
        "A/a1.mp3",
        "A/a2.wav",
        "A/sub/s1.mp3",
        "notes.txt",
        ".hidden/h.mp3",
    ] {
        fs::write(root.join(file), b"").unwrap();
    }
    (dir, root)
}

#[test]
fn tree_starts_with_roots_and_playlists() {
    let (_lib_dir, lib) = open();
    let (_d1, root1) = disk();
    let (_d2, root2) = disk();
    create_playlist(&lib, "Peak", &[]);
    let b = browser(&lib, vec![drive("root1", &root1), drive("root2", &root2)]);
    let view = b.view();
    assert_eq!(labels(&view), ["root1", "root2", "Playlists", "Peak"]);
    let kinds: Vec<_> = view.tree.iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        [
            TreeKind::Drive,
            TreeKind::Drive,
            TreeKind::Playlists,
            TreeKind::Playlist
        ]
    );
    let depths: Vec<_> = view.tree.iter().map(|r| r.depth).collect();
    assert_eq!(depths, [0, 0, 0, 1]);
    let expanded: Vec<_> = view.tree.iter().map(|r| r.expanded).collect();
    assert_eq!(expanded, [false, false, true, false]);
    assert!(view.tree.iter().all(|r| r.note.is_none()));
    assert_eq!((view.tree_cursor, view.focus), (0, Focus::Tree));
    assert_eq!(
        (view.path.as_str(), view.index, view.count),
        ("root1", 0, 4)
    );

    // No drives at all still gives the playlists.
    let b = browser(&lib, Vec::new());
    assert_eq!(labels(&b.view()), ["Playlists", "Peak"]);
}

#[test]
fn toggle_folder_opens_and_closes() {
    let (_lib_dir, lib) = open();
    let (_d, root) = disk();
    create_playlist(&lib, "Peak", &[]);
    let mut b = browser(&lib, vec![drive("root", &root)]);
    let view = toggle_folder(&mut b);
    assert_eq!(labels(&view), ["root", "A", "B", "Playlists", "Peak"]);
    let depths: Vec<_> = view.tree.iter().map(|r| r.depth).collect();
    assert_eq!(depths, [0, 1, 1, 0, 1]);
    assert!(view.tree[0].expanded);
    assert_eq!(view.tree[1].kind, TreeKind::Folder);

    // Into A, open it: its subfolder at depth 2. The path names the branch.
    scroll(&mut b, 1);
    let view = toggle_folder(&mut b);
    assert_eq!(
        labels(&view),
        ["root", "A", "sub", "B", "Playlists", "Peak"]
    );
    assert_eq!(view.tree[2].depth, 2);
    assert_eq!(view.path, "root ▸ A");
    assert_eq!(scroll(&mut b, 1).path, "root ▸ A ▸ sub");

    // Closing the root hides everything under it; the cursor goes back up with it.
    scroll(&mut b, -2);
    let view = toggle_folder(&mut b);
    assert_eq!(labels(&view), ["root", "Playlists", "Peak"]);
    assert!(!view.tree[0].expanded);
    assert_eq!(view.tree_cursor, 0);

    // On a playlist (or the Playlists node) it does nothing.
    let at_playlist = scroll(&mut b, 2);
    assert_eq!(at_playlist.path, "Playlists ▸ Peak");
    assert_eq!(toggle_folder(&mut b), at_playlist);
    scroll(&mut b, -1);
    assert_eq!(
        labels(&toggle_folder(&mut b)),
        ["root", "Playlists", "Peak"]
    );
}

#[test]
fn folder_list_is_its_own_audio_files() {
    let (_lib_dir, lib) = open();
    let (_d, root) = disk();
    let one = add_file_track(&lib, &root.join("A").join("a1.mp3"), "One");
    add_file_track(&lib, &root.join("A").join("sub").join("s1.mp3"), "Sub");
    let mut b = browser(&lib, vec![drive("root", &root)]);
    // The root holds no audio file of its own.
    let view = b.view();
    assert_eq!((view.list_count, view.list.len()), (0, 0));
    assert_eq!(view.track, None);

    toggle_folder(&mut b);
    let view = scroll(&mut b, 1);
    assert_eq!(view.path, "root ▸ A");
    assert_eq!(file_names(&view), ["a1.mp3", "a2.wav"]);
    assert_eq!(
        (view.list_count, view.list_offset, view.list_cursor),
        (2, 0, 0)
    );
    assert_eq!(view.list[0].path, root.join("A").join("a1.mp3"));
    let track = view.list[0].track.as_ref().unwrap();
    assert_eq!((track.id, track.title.as_deref()), (one, Some("One")));
    assert_eq!(view.list[1].track, None);
    // The list cursor's library track is the selected one.
    assert_eq!(view.track.as_ref().map(|t| t.id), Some(one));
    assert_eq!(b.selected_track().map(|t| t.id), Some(one));

    // A file the library doesn't have selects no track.
    toggle(&mut b);
    let view = scroll(&mut b, 1);
    assert_eq!((view.index, view.count, view.list_cursor), (1, 2, 1));
    assert_eq!(view.track, None);
    assert_eq!(b.selected_track(), None);
}

#[cfg(unix)]
#[test]
fn unreadable_folder_says_so() {
    use std::os::unix::fs::PermissionsExt;

    let (_lib_dir, lib) = open();
    let (_d, root) = disk();
    let locked = root.join("Locked");
    fs::create_dir(&locked).unwrap();
    fs::write(locked.join("x.mp3"), b"").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    let mut b = browser(&lib, vec![drive("root", &root)]);
    toggle_folder(&mut b);
    let view = scroll(&mut b, 3);
    assert_eq!(view.path, "root ▸ Locked");
    // Its list cannot be read either: empty.
    assert_eq!(view.list_count, 0);
    let view = toggle_folder(&mut b);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(labels(&view), ["root", "A", "B", "Locked", "Playlists"]);
    assert!(view.tree[3].expanded);
    assert_eq!(view.tree[3].note, Some("cannot read"));
    let view = scroll(&mut b, 1);
    assert_eq!(view.path, "Playlists");
    assert_eq!(view.tree_cursor, 4);
}

#[test]
fn big_folder_pages() {
    let (_lib_dir, lib) = open();
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    for i in 0..5_000 {
        fs::write(root.join(format!("{i:04}.mp3")), b"").unwrap();
    }
    let mut b = browser(&lib, vec![drive("big", &root)]);
    let view = b.view();
    assert_eq!(view.list_count, 5_000);
    assert!(view.list.len() <= BROWSER_PAGE_ROWS, "{}", view.list.len());

    toggle(&mut b);
    let start = Instant::now();
    let view = scroll(&mut b, 4_999);
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
    assert_eq!((view.index, view.list_cursor), (4_999, 4_999));
    assert!(view.list.len() <= BROWSER_PAGE_ROWS, "{}", view.list.len());
    assert_eq!(
        view.list[view.list_cursor - view.list_offset].file_name,
        "4999.mp3"
    );

    // Stepping back through a page boundary one row at a time.
    for i in (4_700..4_999).rev() {
        let view = scroll(&mut b, -1);
        assert_eq!(view.list_cursor, i);
        let name = &view.list[i - view.list_offset].file_name;
        assert_eq!(name, &format!("{i:04}.mp3"));
    }
}

#[test]
fn select_tree_reads_that_entry() {
    let (_lib_dir, lib) = open();
    let (_d, root) = disk();
    let one = add_file_track(&lib, &root.join("A").join("a1.mp3"), "One");
    create_playlist(&lib, "Peak", &[one]);
    let mut b = browser(&lib, vec![drive("root", &root)]);
    toggle_folder(&mut b);
    // From the list pane: a click on a tree row focuses the tree.
    toggle(&mut b);
    let view = b.edit(BrowserEdit::SelectTree(1)).unwrap();
    assert_eq!(view.path, "root ▸ A");
    assert_eq!((view.tree_cursor, view.focus), (1, Focus::Tree));
    assert_eq!(file_names(&view), ["a1.mp3", "a2.wav"]);
    assert_eq!(view.track.map(|t| t.id), Some(one));

    let view = b.edit(BrowserEdit::SelectTree(4)).unwrap();
    assert_eq!(view.path, "Playlists ▸ Peak");
    assert_eq!(file_names(&view), ["a1.mp3"]);
    // Past the last row: the last row.
    assert_eq!(b.edit(BrowserEdit::SelectTree(99)).unwrap().tree_cursor, 4);
}

#[test]
fn select_row_moves_the_list_cursor() {
    let (_lib_dir, lib) = open();
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    for i in 0..1_000 {
        fs::write(root.join(format!("{i:04}.mp3")), b"").unwrap();
    }
    let mut b = browser(&lib, vec![drive("big", &root)]);
    let view = b.edit(BrowserEdit::SelectRow(3)).unwrap();
    assert_eq!(
        (view.focus, view.list_cursor, view.index),
        (Focus::List, 3, 3)
    );
    assert_eq!(view.list_offset, 0);

    // Beyond the loaded page: its page loads around it.
    let view = b.edit(BrowserEdit::SelectRow(700)).unwrap();
    assert_eq!(view.list_cursor, 700);
    assert!(view.list_offset <= 700 && 700 < view.list_offset + view.list.len());
    assert_eq!(view.list[700 - view.list_offset].file_name, "0700.mp3");
    // Past the end: the last row.
    assert_eq!(
        b.edit(BrowserEdit::SelectRow(5_000)).unwrap().list_cursor,
        999
    );
}

#[test]
fn toggle_tree_at_opens_a_folder() {
    let (_lib_dir, lib) = open();
    let (_d, root) = disk();
    create_playlist(&lib, "Peak", &[]);
    let mut b = browser(&lib, vec![drive("root", &root)]);
    // The cursor stays on its entry while rows open above it.
    b.edit(BrowserEdit::SelectTree(2)).unwrap();
    let view = b.edit(BrowserEdit::ToggleTreeAt(0)).unwrap();
    assert_eq!(labels(&view), ["root", "A", "B", "Playlists", "Peak"]);
    assert_eq!(
        (view.tree_cursor, view.path.as_str()),
        (4, "Playlists ▸ Peak")
    );

    // A row below the cursor opens without moving it.
    b.edit(BrowserEdit::SelectTree(0)).unwrap();
    let view = b.edit(BrowserEdit::ToggleTreeAt(1)).unwrap();
    assert_eq!(
        labels(&view),
        ["root", "A", "sub", "B", "Playlists", "Peak"]
    );
    assert_eq!(view.tree_cursor, 0);

    // Closing a folder the cursor is inside moves the cursor to it.
    b.edit(BrowserEdit::SelectTree(2)).unwrap();
    let view = b.edit(BrowserEdit::ToggleTreeAt(1)).unwrap();
    assert_eq!(labels(&view), ["root", "A", "B", "Playlists", "Peak"]);
    assert_eq!((view.tree_cursor, view.path.as_str()), (1, "root ▸ A"));
    assert_eq!(file_names(&view), ["a1.mp3", "a2.wav"]);

    // A playlist, or a row past the end, is left alone.
    let before = b.view();
    assert_eq!(b.edit(BrowserEdit::ToggleTreeAt(4)).unwrap(), before);
    assert_eq!(b.edit(BrowserEdit::ToggleTreeAt(99)).unwrap(), before);
}

#[test]
fn search_and_sort_leave_the_view_for_now() {
    let (_lib_dir, lib) = open();
    let (_d, root) = disk();
    let mut b = browser(&lib, vec![drive("root", &root)]);
    let before = b.view();
    let search = BrowserEdit::Search {
        text: Some("a1".into()),
        seq: 1,
    };
    assert_eq!(b.edit(search).unwrap(), before);
    let sort = BrowserEdit::Sort {
        key: TrackSortKey::Bpm,
        descending: true,
        seq: 2,
    };
    assert_eq!(b.edit(sort).unwrap(), before);
}

#[test]
fn remix_of() {
    assert_eq!(
        library::remix_of("Just Us (Extended Mix)"),
        Some("Extended Mix")
    );
    assert_eq!(library::remix_of("Mui [ezmp3.cc]"), Some("ezmp3.cc"));
    assert_eq!(library::remix_of("Plain"), None);
    // The last group wins; an empty one is no remix.
    assert_eq!(
        library::remix_of("Song (feat. X) [ Club Mix ]"),
        Some("Club Mix")
    );
    assert_eq!(library::remix_of("Song ()"), None);
    assert_eq!(library::remix_of("Song (unclosed"), None);
}

/// Two playlists over five tracks, and no drives.
struct Fixture {
    _dir: tempfile::TempDir,
    lib: Library,
    ids: Vec<i64>,
}

fn fixture() -> Fixture {
    let (dir, lib) = open();
    let tracks: Vec<(String, String)> = ["h1", "h2", "h3", "t1", "t2"]
        .into_iter()
        .map(|t| (format!("/m/{t}.flac"), t.to_owned()))
        .collect();
    let ids = add_tracks(&lib, &tracks);
    create_playlist(&lib, "Peak time", &[ids[4], ids[0], ids[3]]);
    create_playlist(&lib, "Warm up", &[ids[1]]);
    Fixture {
        _dir: dir,
        lib,
        ids,
    }
}

#[test]
fn playlists_list_their_tracks() {
    let f = fixture();
    let mut b = browser(&f.lib, Vec::new());
    // The Playlists node itself lists nothing.
    let view = b.view();
    assert_eq!(view.path, "Playlists");
    assert_eq!((view.index, view.count, view.list_count), (0, 3, 0));

    let view = scroll(&mut b, 1);
    assert_eq!(view.path, "Playlists ▸ Peak time");
    assert_eq!(view.list_count, 3);
    assert_eq!(file_names(&view), ["t2.flac", "h1.flac", "t1.flac"]);
    assert_eq!(view.list[0].path, PathBuf::from("/m/t2.flac"));
    let ids: Vec<_> = view
        .list
        .iter()
        .map(|r| r.track.as_ref().unwrap().id)
        .collect();
    assert_eq!(ids, [f.ids[4], f.ids[0], f.ids[3]]);
}

#[test]
fn scroll_clamps() {
    let f = fixture();
    let mut b = browser(&f.lib, Vec::new());
    // The tree does not wrap at either end.
    assert_eq!(scroll(&mut b, -1).index, 0);
    assert_eq!(scroll(&mut b, 100).index, 2);
    assert_eq!(scroll(&mut b, 1).index, 2);
    assert_eq!(scroll(&mut b, i32::MAX).path, "Playlists ▸ Warm up");
    assert_eq!(scroll(&mut b, i32::MIN).index, 0);

    // Nor does the list.
    scroll(&mut b, 1);
    toggle(&mut b);
    assert_eq!(scroll(&mut b, -3).index, 0);
    assert_eq!(scroll(&mut b, 2).index, 2);
    assert_eq!(scroll(&mut b, 7).index, 2);
    assert_eq!(scroll(&mut b, i32::MAX).index, 2);
    assert_eq!(scroll(&mut b, i32::MIN).index, 0);
}

#[test]
fn toggle_focus_and_tree_moves() {
    let f = fixture();
    let mut b = browser(&f.lib, Vec::new());
    scroll(&mut b, 1);
    let list = toggle(&mut b);
    assert_eq!(list.focus, Focus::List);
    // Scrolling now moves the list, not the tree.
    let view = scroll(&mut b, 1);
    assert_eq!((view.index, view.count), (1, 3));
    assert_eq!((view.tree_cursor, view.list_cursor), (1, 1));
    assert_eq!(view.path, "Playlists ▸ Peak time");

    let tree = toggle(&mut b);
    assert_eq!(tree.focus, Focus::Tree);
    assert_eq!((tree.index, tree.count), (1, 3));
    // A tree scroll that does not move keeps the list cursor.
    assert_eq!(scroll(&mut b, 0).list_cursor, 1);
    // A tree move starts the new list at its top.
    scroll(&mut b, 1);
    let view = scroll(&mut b, -1);
    assert_eq!((view.list_cursor, view.list_count), (0, 3));
}

#[test]
fn selected_track_follows_cursor() {
    let f = fixture();
    let mut b = browser(&f.lib, Vec::new());
    assert_eq!(b.selected_track(), None);
    scroll(&mut b, 1);
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
fn pages_large_playlists() {
    let (_dir, lib) = open();
    let tracks: Vec<(String, String)> = (0..5_000)
        .map(|i| (format!("/m/{i:04}.flac"), format!("t{i:04}")))
        .collect();
    let ids = add_tracks(&lib, &tracks);
    let playlist = create_playlist(&lib, "Big", &[]);
    let stmts = ids
        .iter()
        .enumerate()
        .map(|(position, &id)| RawStatement {
            sql: "INSERT INTO playlist_tracks(playlist_id, track_id, position) \
                  VALUES (?1, ?2, ?3)"
                .into(),
            params: vec![
                Value::Integer(playlist),
                Value::Integer(id),
                Value::Integer(i64::try_from(position).unwrap()),
            ],
        })
        .collect();
    lib.writer().submit_wait(WriteOp::Raw(stmts)).unwrap();
    let mut b = browser(&lib, Vec::new());
    scroll(&mut b, 1);
    let list = toggle(&mut b);
    assert_eq!((list.index, list.count), (0, 5_000));
    assert!(list.list.len() <= BROWSER_PAGE_ROWS);

    let view = scroll(&mut b, 4_999);
    assert_eq!(view.index, 4_999);
    assert_eq!(view.track.map(|t| t.id), Some(ids[4_999]));
    assert!(view.list.len() <= BROWSER_PAGE_ROWS, "{}", view.list.len());

    // Stepping through a page boundary one row at a time.
    for i in (4_700..4_999).rev() {
        let view = scroll(&mut b, -1);
        assert_eq!(view.index, i);
        assert_eq!(view.track.map(|t| t.id), Some(ids[i]), "row {i}");
    }
    let view = scroll(&mut b, -10_000);
    assert_eq!(view.track.map(|t| t.id), Some(ids[0]));
    assert!(view.list.len() <= BROWSER_PAGE_ROWS);
}

#[test]
fn refresh_sees_new_playlists() {
    let f = fixture();
    let mut b = browser(&f.lib, Vec::new());
    scroll(&mut b, 2);
    create_playlist(&f.lib, "Closing", &[f.ids[2]]);
    let view = b.refresh().unwrap();
    // The selected entry stays selected; the new playlist is listed after it.
    assert_eq!(view.path, "Playlists ▸ Warm up");
    assert_eq!((view.index, view.count), (2, 4));
    let view = scroll(&mut b, 1);
    assert_eq!(view.path, "Playlists ▸ Closing");
    assert_eq!(view.track.map(|t| t.id), Some(f.ids[2]));
}

#[test]
fn refresh_keeps_the_selected_playlist_entry_or_falls_back_to_the_row() {
    let f = fixture();
    let mut b = browser(&f.lib, Vec::new());
    scroll(&mut b, 1);
    toggle(&mut b); // Peak time: ids 4, 0, 3
    let view = scroll(&mut b, 1);
    assert_eq!(view.track.as_ref().map(|t| t.id), Some(f.ids[0]));
    let playlist = f.lib.reader().unwrap().playlists().unwrap()[0].id;
    let writer = f.lib.writer();
    writer
        .submit_wait(WriteOp::AddToPlaylist {
            playlist,
            track: f.ids[2],
            at: Some(0),
        })
        .unwrap();
    let view = b.refresh().unwrap();
    assert_eq!(view.track.map(|t| t.id), Some(f.ids[0]));
    assert_eq!((view.index, view.count), (2, 4));
    // The selected entry is removed: the cursor stays on the same row.
    writer
        .submit_wait(WriteOp::RemoveFromPlaylist {
            playlist,
            position: 2,
        })
        .unwrap();
    let view = b.refresh().unwrap();
    assert_eq!(view.track.map(|t| t.id), Some(f.ids[3]));
    assert_eq!(view.index, 2);

    // The playlist is deleted: the cursor stays on the same tree row.
    writer
        .submit_wait(WriteOp::DeletePlaylist(playlist))
        .unwrap();
    let view = b.refresh().unwrap();
    assert_eq!(view.path, "Playlists ▸ Warm up");
    assert_eq!((view.tree_cursor, view.list_cursor), (1, 0));
}

#[test]
fn refresh_rereads_the_folder() {
    let (_lib_dir, lib) = open();
    let (_d, root) = disk();
    let a = root.join("A");
    let mut b = browser(&lib, vec![drive("root", &root)]);
    toggle_folder(&mut b);
    scroll(&mut b, 1);
    toggle_folder(&mut b);
    toggle(&mut b);
    let view = scroll(&mut b, 1);
    assert_eq!(view.list[view.list_cursor].file_name, "a2.wav");
    assert_eq!(view.track, None);

    // A file sorting first appears, and the selected one joins the library.
    fs::write(a.join("a0.flac"), b"").unwrap();
    let id = add_file_track(&lib, &a.join("a2.wav"), "Two");
    let view = b.refresh().unwrap();
    assert_eq!(file_names(&view), ["a0.flac", "a1.mp3", "a2.wav"]);
    assert_eq!((view.list_cursor, view.list_count), (2, 3));
    assert_eq!(view.track.as_ref().map(|t| t.id), Some(id));
    // The open folders stay open.
    assert_eq!(labels(&view), ["root", "A", "sub", "B", "Playlists"]);

    // The selected file is gone: the cursor stays on the same row where it fits.
    fs::remove_file(a.join("a2.wav")).unwrap();
    let view = b.refresh().unwrap();
    assert_eq!((view.list_cursor, view.list_count), (1, 2));
}

#[test]
fn refresh_rereads_the_drives() {
    let (_lib_dir, lib) = open();
    let (_d1, root1) = disk();
    let (_d2, root2) = disk();
    let plugged = Arc::new(Mutex::new(vec![drive("root1", &root1)]));
    let roots = {
        let plugged = Arc::clone(&plugged);
        move || plugged.lock().unwrap().clone()
    };
    let mut b = Browser::with_roots(lib.reader().unwrap(), roots).unwrap();
    toggle_folder(&mut b);
    let view = scroll(&mut b, 1);
    assert_eq!(view.path, "root1 ▸ A");

    // A drive is plugged in above the open one: the open one keeps its folders and the
    // highlight.
    plugged.lock().unwrap().insert(0, drive("root2", &root2));
    let view = b.refresh().unwrap();
    assert_eq!(labels(&view), ["root2", "root1", "A", "B", "Playlists"]);
    assert_eq!((view.path.as_str(), view.tree_cursor), ("root1 ▸ A", 2));

    // The open drive is unplugged: its rows go, the cursor stays on the same row.
    plugged.lock().unwrap().remove(1);
    let view = b.refresh().unwrap();
    assert_eq!(labels(&view), ["root2", "Playlists"]);
    assert_eq!(view.tree_cursor, 1);
}

#[test]
fn waveform_reads_the_stored_overview() {
    let (_dir, lib) = open();
    let id = add_tracks(&lib, &[("/m/1.flac".into(), "one".into())])[0];
    let reader = lib.reader().unwrap();
    assert_eq!(reader.waveform(id).unwrap(), None);
    let waveform = analysis::Waveform {
        frames_per_bucket: 1024,
        buckets: vec![[1, 2, 3, 4], [5, 6, 7, 8]],
    };
    lib.writer()
        .submit_wait(WriteOp::Raw(vec![RawStatement {
            sql: "INSERT INTO analysis(track_id, analyzer_version, waveform) VALUES (?1, 1, ?2)"
                .into(),
            params: vec![Value::Integer(id), Value::Blob(waveform.to_bytes())],
        }]))
        .unwrap();
    assert_eq!(reader.waveform(id).unwrap(), Some(waveform));
    // Analyzed without a waveform (or cleared): none.
    lib.writer()
        .submit_wait(WriteOp::Raw(vec![RawStatement {
            sql: "UPDATE analysis SET waveform = NULL".into(),
            params: vec![],
        }]))
        .unwrap();
    assert_eq!(reader.waveform(id).unwrap(), None);
}
