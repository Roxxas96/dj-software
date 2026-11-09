//! `Reader::track_source`: the file row an export copies from.

use std::fs;

use library::{Library, add_folder, scan_folder};

#[test]
fn track_source_returns_the_file_row() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(&dir.path().join("db").join("library.sqlite3")).unwrap();
    let music = dir.path().join("music");
    fs::create_dir_all(&music).unwrap();
    let wav = music.join("a.wav");
    fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../decode/tests/fixtures/sine440.wav"),
        &wav,
    )
    .unwrap();
    add_folder(&lib, &music).unwrap();
    scan_folder(&lib, &music, &mut |_| {}).unwrap();

    let reader = lib.reader().unwrap();
    let (id, stored): (i64, Vec<u8>) = reader
        .connection()
        .query_row("SELECT id, fingerprint FROM tracks", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    let source = reader.track_source(id).unwrap().unwrap();
    assert_eq!(
        dunce::canonicalize(&source.path).unwrap(),
        dunce::canonicalize(&wav).unwrap()
    );
    assert_eq!(source.size, fs::metadata(&wav).unwrap().len());
    assert_eq!(source.fingerprint.to_vec(), stored);
    assert_eq!(reader.track_source(id + 100).unwrap(), None);
}
