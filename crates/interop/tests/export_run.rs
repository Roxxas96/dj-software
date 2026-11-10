//! A whole export, from a temp library onto a temp dir standing in for the stick.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use interop::export::serato::markers::markers2_geob;
use interop::export::serato::tlv::{Body, Record, decode};
use interop::export::{
    ExportError, ExportPlan, ExportProgress, ExportReport, Manifest, README_TEXT, TrackAction,
    gather, plan_export, run,
};
use interop::{VdjPoi, read_database, read_playlists};
use library::{HotcuePoint, Library, RawStatement, WriteOp, add_folder, scan_folder};
use lofty::config::ParseOptions;
use lofty::file::AudioFile;
use lofty::id3::v2::{Frame, GeneralEncapsulatedObject};
use lofty::iff::wav::WavFile;
use quick_xml::Reader;
use quick_xml::events::Event;
use rusqlite::types::Value;

struct Setup {
    _dir: tempfile::TempDir,
    lib: Library,
    music: PathBuf,
    stick: PathBuf,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(&dir.path().join("db").join("library.sqlite3")).unwrap();
    let music = dir.path().join("music");
    let stick = dir.path().join("stick");
    fs::create_dir_all(&music).unwrap();
    fs::create_dir_all(&stick).unwrap();
    Setup {
        _dir: dir,
        lib,
        music,
        stick,
    }
}

/// Writes a 16-bit mono 44.1 kHz WAV; `seed` makes the fingerprint differ.
fn write_wav(path: &Path, seed: i16) {
    let frames = 44_100u32;
    let data_len = frames * 2;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&44_100u32.to_le_bytes());
    bytes.extend_from_slice(&88_200u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames {
        let v = if i % 100 < 50 { seed } else { -seed };
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    fs::write(path, bytes).unwrap();
}

fn raw(lib: &Library, sql: &str, params: Vec<Value>) {
    lib.writer()
        .submit_wait(WriteOp::Raw(vec![RawStatement {
            sql: sql.into(),
            params,
        }]))
        .unwrap();
}

/// Adds `names` as WAV files and returns their track ids in the same order.
fn add_tracks(s: &Setup, names: &[&str]) -> Vec<i64> {
    for (i, name) in names.iter().enumerate() {
        write_wav(&s.music.join(name), 1000 + i as i16 * 500);
    }
    add_folder(&s.lib, &s.music).unwrap();
    scan_folder(&s.lib, &s.music, &mut |_| {}).unwrap();
    let reader = s.lib.reader().unwrap();
    names
        .iter()
        .map(|n| {
            reader
                .connection()
                .query_row(
                    "SELECT id FROM tracks WHERE path LIKE ?1",
                    [format!("%{n}")],
                    |r| r.get(0),
                )
                .unwrap()
        })
        .collect()
}

fn playlist(s: &Setup, name: &str, tracks: &[i64]) -> i64 {
    raw(
        &s.lib,
        "INSERT INTO playlists(name, position) VALUES (?1, 0)",
        vec![Value::Text(name.into())],
    );
    let id: i64 = s
        .lib
        .reader()
        .unwrap()
        .connection()
        .query_row("SELECT id FROM playlists WHERE name = ?1", [name], |r| {
            r.get(0)
        })
        .unwrap();
    for (pos, t) in tracks.iter().enumerate() {
        raw(
            &s.lib,
            "INSERT INTO playlist_tracks(playlist_id, position, track_id) VALUES (?1, ?2, ?3)",
            vec![
                Value::Integer(id),
                Value::Integer(pos as i64),
                Value::Integer(*t),
            ],
        );
    }
    id
}

fn put_hotcue(s: &Setup, track: i64, frames: f64) {
    s.lib
        .writer()
        .submit_wait(WriteOp::PutHotcue {
            track,
            slot: 1,
            point: HotcuePoint {
                frames,
                loop_frames: None,
                color: None,
                name: None,
            },
        })
        .unwrap();
}

/// `relative` (with `/` separators) under `root`.
fn at(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |p, part| p.join(part))
}

fn plan(s: &Setup, playlists: &[i64]) -> ExportPlan {
    let set = gather(&s.lib.reader().unwrap(), playlists).unwrap();
    plan_export(set, &s.stick, std::slice::from_ref(&s.music)).unwrap()
}

/// Runs the plan and returns the report and every progress call.
fn run_plan(plan: &ExportPlan) -> (Result<ExportReport, ExportError>, Vec<ExportProgress>) {
    let calls = RefCell::new(Vec::new());
    let result = run(plan, &|p| calls.borrow_mut().push(p));
    (result, calls.into_inner())
}

fn export(s: &Setup, playlists: &[i64]) -> ExportReport {
    run_plan(&plan(s, playlists)).0.unwrap()
}

fn crate_path(s: &Setup, name: &str) -> PathBuf {
    s.stick
        .join("_Serato_")
        .join("Subcrates")
        .join(format!("{name}.crate"))
}

fn list_path(s: &Setup, name: &str) -> PathBuf {
    s.stick
        .join("VirtualDJ")
        .join("MyLists")
        .join(format!("{name}.vdjfolder"))
}

fn xml_path(s: &Setup) -> PathBuf {
    s.stick.join("Open Jockey").join("rekordbox.xml")
}

fn utf16be(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_be_bytes(*c))
        .collect();
    String::from_utf16_lossy(&units)
}

/// The file names of the tracks a crate lists, in order.
fn crate_tracks(path: &Path) -> Vec<String> {
    crate_paths(path).iter().map(|p| file_name(p)).collect()
}

/// The paths a crate lists, relative to the stick, in order.
fn crate_paths(path: &Path) -> Vec<String> {
    decode(&fs::read(path).unwrap())
        .unwrap()
        .into_iter()
        .filter(|r| &r.tag == b"otrk")
        .map(|r| {
            let Body::Records(fields) = r.body else {
                panic!("otrk holds records");
            };
            fields
                .iter()
                .find_map(|f: &Record| match &f.body {
                    Body::Bytes(b) if &f.tag == b"ptrk" => Some(utf16be(b)),
                    _ => None,
                })
                .unwrap()
        })
        .collect()
}

fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap().to_string()
}

type Attrs = HashMap<String, String>;

/// Every start or empty element in document order: name and unescaped attributes.
fn elements(text: &str) -> Vec<(String, Attrs)> {
    let mut reader = Reader::from_str(text);
    let mut out = Vec::new();
    loop {
        match reader.read_event().unwrap() {
            Event::Start(e) | Event::Empty(e) => {
                let attrs = e
                    .attributes()
                    .map(|a| {
                        let a = a.unwrap();
                        (
                            a.key.as_ref().to_string(),
                            a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                .unwrap()
                                .into_owned(),
                        )
                    })
                    .collect();
                out.push((e.name().as_ref().to_string(), attrs));
            }
            Event::Eof => break,
            _ => {}
        }
    }
    out
}

/// The Rekordbox XML's playlists: name to the file names of its tracks, in order.
fn xml_playlists(s: &Setup) -> BTreeMap<String, Vec<String>> {
    let text = fs::read_to_string(xml_path(s)).unwrap();
    let mut locations = HashMap::new();
    let mut playlists = BTreeMap::new();
    let mut current: Option<String> = None;
    let mut in_collection = true;
    for (name, attrs) in elements(&text) {
        match name.as_str() {
            "PLAYLISTS" => in_collection = false,
            "TRACK" if in_collection => {
                locations.insert(attrs["TrackID"].clone(), file_name(&attrs["Location"]));
            }
            "NODE" if attrs["Type"] == "1" => {
                current = Some(attrs["Name"].clone());
                playlists.insert(attrs["Name"].clone(), Vec::new());
            }
            "TRACK" => {
                let list = playlists.get_mut(current.as_ref().unwrap()).unwrap();
                list.push(locations[&attrs["Key"]].clone());
            }
            _ => {}
        }
    }
    playlists
}

fn vdj_list(s: &Setup, name: &str) -> Vec<String> {
    let (lists, errors) =
        read_playlists(&[s.stick.join("VirtualDJ").join("MyLists")], None).unwrap();
    assert!(errors.is_empty());
    let list = lists.into_iter().find(|l| l.name == name).unwrap();
    list.paths
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

/// Every file under `dir`, relative, sorted.
fn all_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for entry in fs::read_dir(&d).unwrap() {
            let path = entry.unwrap().path();
            if fs::symlink_metadata(&path).unwrap().is_dir() {
                todo.push(path);
            } else {
                out.push(path.strip_prefix(dir).unwrap().to_path_buf());
            }
        }
    }
    out.sort();
    out
}

#[test]
fn exports_all_three_formats() {
    let s = setup();
    let ids = add_tracks(&s, &["a.wav", "b.wav"]);
    let x = playlist(&s, "X", &[ids[0]]);
    let y = playlist(&s, "Y", &[ids[1]]);
    let p = plan(&s, &[x, y]);
    let (report, calls) = run_plan(&p);
    let report = report.unwrap();
    assert_eq!(report.copied, 2);
    assert_eq!(report.kept, 0);
    assert_eq!(report.retagged, 0);
    assert_eq!(report.removed, 0);
    assert_eq!(report.playlists, 2);
    for t in &p.tracks {
        assert_eq!(
            fs::read(at(&s.stick, &t.stick_path)).unwrap()[..4],
            *b"RIFF"
        );
    }
    assert!(s.stick.join("_Serato_").join("database V2").is_file());
    assert!(crate_path(&s, "X").is_file());
    assert!(crate_path(&s, "Y").is_file());
    assert!(xml_path(&s).is_file());
    assert!(s.stick.join("VirtualDJ").join("database.xml").is_file());
    assert!(list_path(&s, "X").is_file());
    assert!(list_path(&s, "Y").is_file());
    let oj = s.stick.join("Open Jockey");
    assert_eq!(
        fs::read_to_string(oj.join("README.txt")).unwrap(),
        README_TEXT
    );
    let manifest = Manifest::read(&s.stick).unwrap().unwrap();
    assert_eq!(manifest.playlists, ["X", "Y"]);
    assert_eq!(manifest.crates, ["X.crate", "Y.crate"]);
    assert_eq!(manifest.vdj_lists, ["X.vdjfolder", "Y.vdjfolder"]);
    assert_eq!(
        manifest.tracks,
        [
            "Music/Unknown Artist/a.wav".to_string(),
            "Music/Unknown Artist/b.wav".to_string()
        ]
    );
    let total = p.tracks.len() + 1;
    let done: Vec<usize> = calls.iter().map(|c| c.done).collect();
    assert_eq!(done, (1..=total).collect::<Vec<_>>());
    assert!(calls.iter().all(|c| c.total == total));
    assert!(README_TEXT.starts_with("This stick was made by Open Jockey."));
}

#[test]
fn shared_tracks_are_copied_once_and_listed_everywhere() {
    let s = setup();
    let ids = add_tracks(&s, &["a.wav", "b.wav"]);
    let (a, b) = (ids[0], ids[1]);
    let x = playlist(&s, "X", &[a, b, a]);
    let y = playlist(&s, "Y", &[b]);
    let report = export(&s, &[x, y]);
    assert_eq!(report.copied, 2);
    let copies = all_files(&s.stick.join("Open Jockey").join("Music"));
    assert_eq!(copies.len(), 2);
    let aba = ["a.wav", "b.wav", "a.wav"].map(String::from).to_vec();
    let just_b = vec!["b.wav".to_string()];
    assert_eq!(crate_tracks(&crate_path(&s, "X")), aba);
    assert_eq!(crate_tracks(&crate_path(&s, "Y")), just_b);
    let xml = xml_playlists(&s);
    assert_eq!(xml["X"], aba);
    assert_eq!(xml["Y"], just_b);
    assert_eq!(vdj_list(&s, "X"), aba);
    assert_eq!(vdj_list(&s, "Y"), just_b);
}

#[test]
fn reexport_after_hotcue_edit_retags_without_copying() {
    let s = setup();
    let id = add_tracks(&s, &["a.wav"])[0];
    put_hotcue(&s, id, 22_050.0);
    let x = playlist(&s, "X", &[id]);
    export(&s, &[x]);
    put_hotcue(&s, id, 11_025.0);
    let p = plan(&s, &[x]);
    assert_eq!(p.tracks[0].action, TrackAction::Keep);
    let report = run_plan(&p).0.unwrap();
    assert_eq!(report.copied, 0);
    assert_eq!(report.retagged, 1);
    assert_eq!(report.kept, 0);

    let copy = at(&s.stick, &p.tracks[0].stick_path);
    let mut file = fs::File::open(&copy).unwrap();
    let tag = WavFile::read_from(&mut file, ParseOptions::new())
        .unwrap()
        .id3v2()
        .cloned()
        .unwrap();
    let markers2 = tag
        .iter()
        .find_map(|f| match f {
            Frame::Binary(b) if b.id().as_str() == "GEOB" => {
                let g = GeneralEncapsulatedObject::parse(&b.data, b.flags()).unwrap();
                (g.descriptor.as_deref() == Some("Serato Markers2")).then_some(g.data)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(markers2, markers2_geob(&p.tracks[0].track));

    let xml = fs::read_to_string(xml_path(&s)).unwrap();
    let marks: Vec<String> = elements(&xml)
        .into_iter()
        .filter(|(n, a)| n == "POSITION_MARK" && a.get("Num").is_some_and(|n| n == "0"))
        .map(|(_, a)| a["Start"].clone())
        .collect();
    assert_eq!(marks, ["0.250"]);

    let songs = read_database(
        &s.stick.join("VirtualDJ").join("database.xml"),
        Some(&s.stick),
    )
    .unwrap();
    match &songs[0].pois[..] {
        [VdjPoi::Hotcue { seconds, .. }] => assert!((seconds - 0.25).abs() < 1e-6),
        other => panic!("{other:?}"),
    }
}

#[test]
fn unchanged_reexport_keeps_everything() {
    let s = setup();
    let id = add_tracks(&s, &["a.wav"])[0];
    put_hotcue(&s, id, 22_050.0);
    let x = playlist(&s, "X", &[id]);
    export(&s, &[x]);
    let report = export(&s, &[x]);
    assert_eq!((report.copied, report.kept, report.retagged), (0, 1, 0));
}

#[test]
fn unticked_playlist_is_removed() {
    let s = setup();
    let ids = add_tracks(&s, &["a.wav", "b.wav", "c.wav"]);
    let (a, b, c) = (ids[0], ids[1], ids[2]);
    raw(
        &s.lib,
        "UPDATE tracks SET artist = 'Bee' WHERE id = ?1",
        vec![Value::Integer(b)],
    );
    let x = playlist(&s, "X", &[a]);
    let y = playlist(&s, "Y", &[b, c]);
    export(&s, &[x, y]);
    let bee = s.stick.join("Open Jockey").join("Music").join("Bee");
    assert!(bee.join("b.wav").is_file());
    let report = export(&s, &[x]);
    assert_eq!(report.removed, 2);
    assert_eq!(report.playlists, 1);
    assert!(!crate_path(&s, "Y").exists());
    assert!(!list_path(&s, "Y").exists());
    assert!(crate_path(&s, "X").is_file());
    assert!(!bee.exists(), "the empty artist folder goes");
    let music = s.stick.join("Open Jockey").join("Music");
    assert_eq!(
        all_files(&music),
        [Path::new("Unknown Artist").join("a.wav")]
    );
    assert_eq!(xml_playlists(&s).into_keys().collect::<Vec<_>>(), ["X"]);
    let manifest = Manifest::read(&s.stick).unwrap().unwrap();
    assert_eq!(manifest.playlists, ["X"]);
    assert_eq!(manifest.crates, ["X.crate"]);
    assert_eq!(manifest.vdj_lists, ["X.vdjfolder"]);
}

#[test]
fn foreign_files_survive_export() {
    let s = setup();
    let foreign = [
        (s.stick.join("Music").join("friend.mp3"), b"mp3!".to_vec()),
        (
            s.stick
                .join("_Serato_")
                .join("Subcrates")
                .join("Friend.crate"),
            b"crate".to_vec(),
        ),
        (
            s.stick
                .join("VirtualDJ")
                .join("MyLists")
                .join("Friend.vdjfolder"),
            b"<VirtualFolder/>".to_vec(),
        ),
    ];
    for (path, bytes) in &foreign {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    let ids = add_tracks(&s, &["a.wav", "b.wav"]);
    let x = playlist(&s, "X", &[ids[0]]);
    let y = playlist(&s, "Y", &[ids[1]]);
    export(&s, &[x, y]);
    export(&s, &[x]);
    for (path, bytes) in &foreign {
        assert_eq!(&fs::read(path).unwrap(), bytes, "{}", path.display());
    }
}

#[test]
fn failed_write_leaves_previous_export_usable() {
    let s = setup();
    let ids = add_tracks(&s, &["a.wav", "b.wav"]);
    let x = playlist(&s, "X", &[ids[0]]);
    export(&s, &[x]);
    // The manifest changes: it claims what the export was about to write.
    let snapshot = |s: &Setup| [crate_path(s, "X"), xml_path(s)].map(|p| fs::read(p).unwrap());
    let before = snapshot(&s);

    raw(
        &s.lib,
        "INSERT INTO playlist_tracks(playlist_id, position, track_id) VALUES (?1, 1, ?2)",
        vec![Value::Integer(x), Value::Integer(ids[1])],
    );
    let p = plan(&s, &[x]);
    let new = p.tracks.iter().find(|t| t.track.id == ids[1]).unwrap();
    let mut blocker = at(&s.stick, &new.stick_path).into_os_string();
    blocker.push(".ojpart");
    let blocker = PathBuf::from(blocker);
    fs::create_dir_all(&blocker).unwrap();
    let (result, _) = run_plan(&p);
    assert!(result.is_err());
    assert_eq!(snapshot(&s), before);
    let manifest = Manifest::read(&s.stick).unwrap().unwrap();
    assert_eq!(
        manifest.tracks,
        ["Music/Unknown Artist/a.wav", "Music/Unknown Artist/b.wav"]
    );
    assert_eq!(manifest.crates, ["X.crate"]);

    fs::remove_dir(&blocker).unwrap();
    let report = export(&s, &[x]);
    assert_eq!(report.copied, 1);
    assert!(
        all_files(&s.stick)
            .iter()
            .all(|f| !f.to_string_lossy().ends_with(".ojpart"))
    );
    assert_eq!(crate_tracks(&crate_path(&s, "X")), ["a.wav", "b.wav"]);
}

#[test]
fn refuses_when_too_small() {
    let s = setup();
    let ids = add_tracks(&s, &["a.wav"]);
    let x = playlist(&s, "X", &ids);
    let mut p = plan(&s, &[x]);
    p.free_bytes = Some(0);
    let (result, calls) = run_plan(&p);
    match result {
        Err(ExportError::Refused(m)) => assert!(m.starts_with("the stick is too small: "), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(calls.is_empty());
    assert_eq!(fs::read_dir(&s.stick).unwrap().count(), 0);
}

/// Asserts that `result` is refused naming `file`, which exists; the plan names the stick by
/// its canonical path.
fn refused_naming(result: Result<ExportPlan, ExportError>, file: &Path) {
    let file = dunce::canonicalize(file).unwrap();
    match result {
        Err(ExportError::Refused(m)) => {
            assert!(m.contains(&file.display().to_string()), "{m}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn refuses_unreadable_serato_database() {
    let s = setup();
    let ids = add_tracks(&s, &["a.wav"]);
    let x = playlist(&s, "X", &ids);
    let db = s.stick.join("_Serato_").join("database V2");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    fs::write(&db, b"\x00\x01garbage").unwrap();
    let set = gather(&s.lib.reader().unwrap(), &[x]).unwrap();
    refused_naming(plan_export(set.clone(), &s.stick, &[]), &db);
    fs::remove_file(&db).unwrap();
    fs::create_dir(&db).unwrap();
    refused_naming(plan_export(set, &s.stick, &[]), &db);
    assert!(all_files(&s.stick).is_empty());
}

#[test]
fn refuses_unreadable_virtualdj_database() {
    let s = setup();
    let ids = add_tracks(&s, &["a.wav"]);
    let x = playlist(&s, "X", &ids);
    let db = s.stick.join("VirtualDJ").join("database.xml");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    fs::write(&db, b"<NotVirtualDJ/>").unwrap();
    let set = gather(&s.lib.reader().unwrap(), &[x]).unwrap();
    refused_naming(plan_export(set.clone(), &s.stick, &[]), &db);
    fs::remove_file(&db).unwrap();
    fs::create_dir(&db).unwrap();
    refused_naming(plan_export(set, &s.stick, &[]), &db);
}

#[cfg(unix)]
#[test]
fn a_linked_copy_is_replaced_not_followed() {
    let s = setup();
    let ids = add_tracks(&s, &["a.wav"]);
    let x = playlist(&s, "X", &ids);
    let p = plan(&s, &[x]);
    let copy = at(&s.stick, &p.tracks[0].stick_path);
    fs::create_dir_all(copy.parent().unwrap()).unwrap();
    // The same audio, outside the stick: a plan that followed the link would keep it.
    let outside = s.music.parent().unwrap().join("outside.wav");
    fs::copy(&p.tracks[0].track.path, &outside).unwrap();
    let snapshot = fs::read(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, &copy).unwrap();

    let p = plan(&s, &[x]);
    assert_eq!(p.tracks[0].action, TrackAction::Copy);
    let report = run_plan(&p).0.unwrap();
    assert_eq!(report.copied, 1);
    assert_eq!(fs::read(&outside).unwrap(), snapshot);
    assert!(fs::symlink_metadata(&copy).unwrap().file_type().is_file());
}

/// Every reference each format holds resolves to a regular file on the stick.
fn every_reference_resolves(s: &Setup, crate_name: &str, list_name: &str) {
    let crates = crate_paths(&crate_path(s, crate_name));
    assert!(!crates.is_empty());
    for p in crates {
        assert!(at(&s.stick, &p).is_file(), "crate: {p}");
    }
    let (lists, errors) =
        read_playlists(&[s.stick.join("VirtualDJ").join("MyLists")], None).unwrap();
    assert!(errors.is_empty());
    let list = lists.into_iter().find(|l| l.name == list_name).unwrap();
    assert!(!list.paths.is_empty());
    for p in &list.paths {
        assert!(p.is_file(), "list: {}", p.display());
    }
    let songs = read_database(
        &s.stick.join("VirtualDJ").join("database.xml"),
        Some(&s.stick),
    )
    .unwrap();
    for song in &songs {
        assert!(song.path.is_file(), "database: {}", song.path.display());
    }
    let manifest = Manifest::read(&s.stick).unwrap().unwrap();
    for t in &manifest.tracks {
        assert!(
            at(&s.stick.join("Open Jockey"), t).is_file(),
            "manifest: {t}"
        );
    }
    let xml = fs::read_to_string(xml_path(s)).unwrap();
    for t in &manifest.tracks {
        assert!(xml.contains(&file_name(t)), "xml: {t}");
    }
}

#[test]
fn artist_case_change_keeps_the_copy() {
    let s = setup();
    let id = add_tracks(&s, &["b.wav"])[0];
    let set_artist = |name: &str| {
        raw(
            &s.lib,
            "UPDATE tracks SET artist = ?1 WHERE id = ?2",
            vec![Value::Text(name.into()), Value::Integer(id)],
        );
    };
    set_artist("bee");
    let x = playlist(&s, "X", &[id]);
    export(&s, &[x]);
    set_artist("Bee");
    let report = export(&s, &[x]);
    assert_eq!(report.removed, 0);
    let manifest = Manifest::read(&s.stick).unwrap().unwrap();
    assert_eq!(manifest.tracks, ["Music/Bee/b.wav"]);
    every_reference_resolves(&s, "X", "X");
}

#[test]
fn playlist_case_change_keeps_the_crate_and_list() {
    let s = setup();
    let ids = add_tracks(&s, &["a.wav"]);
    let p = playlist(&s, "techno", &ids);
    export(&s, &[p]);
    raw(
        &s.lib,
        "UPDATE playlists SET name = 'Techno' WHERE id = ?1",
        vec![Value::Integer(p)],
    );
    export(&s, &[p]);
    let manifest = Manifest::read(&s.stick).unwrap().unwrap();
    assert_eq!(manifest.playlists, ["Techno"]);
    assert_eq!(manifest.crates, ["Techno.crate"]);
    assert_eq!(manifest.vdj_lists, ["Techno.vdjfolder"]);
    assert_eq!(crate_tracks(&crate_path(&s, "Techno")), ["a.wav"]);
    assert!(list_path(&s, "Techno").is_file());
    // The files carry the new case, and the old names are gone.
    let names = |dir: PathBuf| -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect()
    };
    assert_eq!(
        names(s.stick.join("_Serato_").join("Subcrates")),
        ["Techno.crate"]
    );
    assert_eq!(
        names(s.stick.join("VirtualDJ").join("MyLists")),
        ["Techno.vdjfolder"]
    );
    every_reference_resolves(&s, "Techno", "Techno");
}

/// A folder at `VirtualDJ/database.xml.ojpart`: VirtualDJ's database cannot be written, so
/// an export fails after Serato's crates and VirtualDJ's lists are written.
fn block_virtualdj(s: &Setup) -> PathBuf {
    let blocker = s.stick.join("VirtualDJ").join("database.xml.ojpart");
    fs::create_dir_all(&blocker).unwrap();
    blocker
}

/// The files in `dir` whose names hold "(Open Jockey)".
fn renamed(dir: &Path) -> Vec<PathBuf> {
    all_files(dir)
        .into_iter()
        .filter(|f| f.to_string_lossy().contains("(Open Jockey"))
        .collect()
}

#[test]
fn a_failed_reexport_leaves_its_crates_and_lists_ours() {
    let s = setup();
    let ids = add_tracks(&s, &["a.wav", "b.wav"]);
    let x = playlist(&s, "X", &[ids[0]]);
    let y = playlist(&s, "Set A", &[ids[1]]);
    export(&s, &[x]);
    let blocker = block_virtualdj(&s);
    assert!(run_plan(&plan(&s, &[y])).0.is_err());
    assert!(crate_path(&s, "Set A").is_file(), "Serato wrote first");

    fs::remove_dir(&blocker).unwrap();
    let report = export(&s, &[y]);
    assert_eq!(renamed(&s.stick), Vec::<PathBuf>::new());
    assert!(crate_path(&s, "Set A").is_file());
    assert!(list_path(&s, "Set A").is_file());
    assert!(!crate_path(&s, "X").exists());
    assert!(!list_path(&s, "X").exists());
    assert_eq!(report.removed, 1, "the first export's a.wav goes");
    let music = s.stick.join("Open Jockey").join("Music");
    assert_eq!(
        all_files(&music),
        [Path::new("Unknown Artist").join("b.wav")]
    );
    let manifest = Manifest::read(&s.stick).unwrap().unwrap();
    assert_eq!(manifest.crates, ["Set A.crate"]);
    assert_eq!(manifest.vdj_lists, ["Set A.vdjfolder"]);
    assert_eq!(manifest.tracks, ["Music/Unknown Artist/b.wav"]);
}

#[test]
fn a_failed_first_export_leaves_its_files_ours() {
    let s = setup();
    let ids = add_tracks(&s, &["a.wav", "b.wav"]);
    let y = playlist(&s, "Set A", &[ids[0]]);
    let blocker = block_virtualdj(&s);
    assert!(run_plan(&plan(&s, &[y])).0.is_err());
    let manifest = Manifest::read(&s.stick).unwrap().unwrap();
    assert_eq!(manifest.crates, ["Set A.crate"]);
    assert_eq!(manifest.tracks, ["Music/Unknown Artist/a.wav"]);

    fs::remove_dir(&blocker).unwrap();
    raw(
        &s.lib,
        "UPDATE playlist_tracks SET track_id = ?1 WHERE playlist_id = ?2",
        vec![Value::Integer(ids[1]), Value::Integer(y)],
    );
    let report = export(&s, &[y]);
    assert_eq!(renamed(&s.stick), Vec::<PathBuf>::new());
    assert_eq!(crate_tracks(&crate_path(&s, "Set A")), ["b.wav"]);
    assert_eq!(report.removed, 1, "the failed export's a.wav goes");
    let music = s.stick.join("Open Jockey").join("Music");
    assert_eq!(
        all_files(&music),
        [Path::new("Unknown Artist").join("b.wav")]
    );
}

#[test]
fn refuses_when_free_space_is_unknown() {
    let s = setup();
    let ids = add_tracks(&s, &["a.wav"]);
    let x = playlist(&s, "X", &ids);
    let mut p = plan(&s, &[x]);
    p.free_bytes = None;
    assert!(!p.too_small());
    let (result, calls) = run_plan(&p);
    match result {
        Err(ExportError::Refused(m)) => {
            assert_eq!(m, "Open Jockey cannot read the stick's free space.");
        }
        other => panic!("{other:?}"),
    }
    assert!(calls.is_empty());
    assert_eq!(fs::read_dir(&s.stick).unwrap().count(), 0);
}

#[test]
fn a_relative_stick_is_written_with_absolute_paths() {
    let s = setup();
    let ids = add_tracks(&s, &["a.wav"]);
    let x = playlist(&s, "X", &ids);
    // Integration tests run in the package's folder: a stick there has a relative path.
    let here = tempfile::tempdir_in(".").unwrap();
    let relative = Path::new(".").join(here.path().file_name().unwrap());
    let set = gather(&s.lib.reader().unwrap(), &[x]).unwrap();
    let p = plan_export(set, &relative, std::slice::from_ref(&s.music)).unwrap();
    assert_eq!(p.stick, dunce::canonicalize(here.path()).unwrap());
    run_plan(&p).0.unwrap();
    let (lists, errors) =
        read_playlists(&[here.path().join("VirtualDJ").join("MyLists")], None).unwrap();
    assert!(errors.is_empty());
    assert!(
        lists[0]
            .paths
            .iter()
            .all(|p| p.is_absolute() && p.is_file())
    );
    let xml = fs::read_to_string(here.path().join("Open Jockey").join("rekordbox.xml")).unwrap();
    let absolute = interop::export::rekordbox::location(&p.stick, &p.tracks[0].stick_path);
    assert!(
        xml.contains(&format!("Location=\"{absolute}\"")),
        "{absolute} not in {xml}"
    );
}

#[cfg(unix)]
#[test]
fn leftovers_behind_a_linked_open_jockey_folder_stay() {
    let s = setup();
    let ids = add_tracks(&s, &["a.wav"]);
    let x = playlist(&s, "X", &ids);
    let outside = s.music.parent().unwrap().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep.ojpart"), b"keep").unwrap();
    std::os::unix::fs::symlink(&outside, s.stick.join("Open Jockey")).unwrap();
    let p = plan(&s, &[x]);
    assert!(run_plan(&p).0.is_err());
    assert_eq!(fs::read(outside.join("keep.ojpart")).unwrap(), b"keep");
}

#[cfg(unix)]
#[test]
fn a_linked_manifest_is_refused() {
    let s = setup();
    let outside = s.music.parent().unwrap().join("export.toml");
    fs::write(
        &outside,
        "version = 1\nplaylists = []\ncrates = []\nvdj_lists = []\ntracks = []\n",
    )
    .unwrap();
    fs::create_dir(s.stick.join("Open Jockey")).unwrap();
    std::os::unix::fs::symlink(&outside, s.stick.join("Open Jockey").join("export.toml")).unwrap();
    assert!(matches!(
        Manifest::read(&s.stick),
        Err(ExportError::Refused(_))
    ));
}

#[test]
fn notes_count_one_in_the_singular() {
    let s = setup();
    let ids = add_tracks(&s, &["a.wav", "b.wav"]);
    let x = playlist(&s, "X", &ids);
    let mut p = plan(&s, &[x]);
    p.without_serato_cues = 1;
    let report = run_plan(&p).0.unwrap();
    assert_eq!(
        report.notes,
        ["1 track copied without Serato cues (not MP3 or WAV)"]
    );
    p.without_serato_cues = 2;
    let report = run_plan(&p).0.unwrap();
    assert_eq!(
        report.notes,
        ["2 tracks copied without Serato cues (not MP3 or WAV)"]
    );
}
