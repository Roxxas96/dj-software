//! Scanning folders: tags, audio fingerprints, relinking moved and retagged files.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use library::{Library, RawStatement, ScanProgress, ScanReport, WriteOp, fingerprint, scan_folder};
use rusqlite::types::Value;

/// A copy of a decode fixture; the originals are never modified.
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../decode/tests/fixtures")
        .join(name)
}

struct Setup {
    _dir: tempfile::TempDir,
    lib: Library,
    music: PathBuf,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(&dir.path().join("db").join("library.sqlite3")).unwrap();
    let music = dir.path().join("music");
    fs::create_dir_all(&music).unwrap();
    // Paths are stored canonical (macOS temp dirs live behind a /var symlink), the way
    // the library normalizes them.
    let music = dunce::canonicalize(&music).unwrap();
    Setup {
        _dir: dir,
        lib,
        music,
    }
}

fn copy_fixture(name: &str, to: &Path) {
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir).unwrap();
    }
    fs::copy(fixture(name), to).unwrap();
}

fn scan(s: &Setup) -> ScanReport {
    let mut progress = Vec::new();
    let report = scan_folder(&s.lib, &s.music, &mut |p: ScanProgress| progress.push(p)).unwrap();
    assert!(s.lib.take_failed_writes().is_empty());
    report
}

#[derive(Debug, Clone, PartialEq)]
struct Row {
    id: i64,
    path: String,
    missing: bool,
    title: Option<String>,
    sample_rate: Option<i64>,
    duration_frames: Option<i64>,
}

fn tracks(lib: &Library) -> Vec<Row> {
    let reader = lib.reader().unwrap();
    let mut stmt = reader
        .connection()
        .prepare(
            "SELECT id, path, missing, title, sample_rate, duration_frames \
             FROM tracks ORDER BY path",
        )
        .unwrap();
    stmt.query_map([], |r| {
        Ok(Row {
            id: r.get(0)?,
            path: r.get(1)?,
            missing: r.get(2)?,
            title: r.get(3)?,
            sample_rate: r.get(4)?,
            duration_frames: r.get(5)?,
        })
    })
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

fn track_at(lib: &Library, path: &Path) -> Row {
    tracks(lib)
        .into_iter()
        .find(|r| Path::new(&r.path) == path)
        .unwrap_or_else(|| panic!("no track at {}", path.display()))
}

fn assert_noop(report: &ScanReport) {
    assert_eq!(
        (
            report.added,
            report.updated,
            report.audio_changed,
            report.relinked,
            report.missing,
            report.skipped.len()
        ),
        (0, 0, 0, 0, 0, 0),
        "{report:?}"
    );
}

/// Rewrites a WAV's LIST/INFO chunk so its title is `title`, leaving the audio untouched.
/// The file grows, so its size (and mtime) change.
fn retag_wav(path: &Path, title: &str) {
    let bytes = fs::read(path).unwrap();
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let mut chunks = Vec::new();
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let end = (pos + 8 + len + (len & 1)).min(bytes.len());
        if id != b"LIST" {
            chunks.extend_from_slice(&bytes[pos..end]);
        }
        pos = end;
    }
    let mut value = title.as_bytes().to_vec();
    value.push(0);
    if value.len() % 2 == 1 {
        value.push(0);
    }
    let mut info = b"INFO".to_vec();
    info.extend_from_slice(b"INAM");
    info.extend_from_slice(&u32::try_from(value.len()).unwrap().to_le_bytes());
    info.extend_from_slice(&value);
    chunks.extend_from_slice(b"LIST");
    chunks.extend_from_slice(&u32::try_from(info.len()).unwrap().to_le_bytes());
    chunks.extend_from_slice(&info);

    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&u32::try_from(chunks.len() + 4).unwrap().to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(&chunks);
    fs::write(path, out).unwrap();
    touch_later(path);
}

/// Pushes the mtime forward so a change is visible even on coarse-mtime filesystems.
fn touch_later(path: &Path) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(10))
        .unwrap();
}

/// Flips a few sample bytes in the WAV's data chunk.
fn change_wav_audio(path: &Path) {
    let mut bytes = fs::read(path).unwrap();
    let data = bytes.windows(4).position(|w| w == b"data").unwrap() + 8;
    for b in &mut bytes[data + 1000..data + 1010] {
        *b ^= 0x55;
    }
    fs::write(path, bytes).unwrap();
}

#[test]
fn scan_adds_supported_files_with_tags() {
    let s = setup();
    for ext in ["wav", "aiff", "flac", "mp3", "m4a", "ogg"] {
        copy_fixture(
            &format!("sine440.{ext}"),
            &s.music.join(format!("sine440.{ext}")),
        );
    }
    copy_fixture("sine440.wav", &s.music.join("sub/Upper.WAV"));
    retag_wav(&s.music.join("sub/Upper.WAV"), "Tagged Title");

    let report = scan(&s);
    assert_eq!(report.added, 7, "{report:?}");
    assert_eq!((report.updated, report.relinked, report.missing), (0, 0, 0));
    assert!(report.skipped.is_empty(), "{report:?}");

    let rows = tracks(&s.lib);
    assert_eq!(rows.len(), 7);
    for row in &rows {
        assert!(!row.missing);
        assert_eq!(row.sample_rate, Some(44_100), "{row:?}");
        assert!(row.duration_frames.unwrap() > 0, "{row:?}");
    }
    let wav = track_at(&s.lib, &s.music.join("sine440.wav"));
    assert_eq!(
        wav.title.as_deref(),
        Some("sine440"),
        "falls back to the stem"
    );
    let tagged = track_at(&s.lib, &s.music.join("sub/Upper.WAV"));
    assert_eq!(tagged.title.as_deref(), Some("Tagged Title"));
}

#[test]
fn rescan_unchanged_is_a_noop() {
    let s = setup();
    copy_fixture("sine440.wav", &s.music.join("a.wav"));
    copy_fixture("sine440.flac", &s.music.join("b/b.flac"));
    let first = scan(&s);
    assert_eq!(first.added, 2);
    let before = tracks(&s.lib);

    assert_noop(&scan(&s));
    assert_eq!(tracks(&s.lib), before);
}

#[test]
fn moved_file_keeps_its_cues() {
    let s = setup();
    let old = s.music.join("track.wav");
    copy_fixture("sine440.wav", &old);
    scan(&s);
    let id = track_at(&s.lib, &old).id;
    s.lib
        .writer()
        .submit_wait(WriteOp::Raw(vec![RawStatement {
            sql: "INSERT INTO cues(track_id, cue_frames) VALUES (?1, 1234.0)".into(),
            params: vec![Value::Integer(id)],
        }]))
        .unwrap();

    let new = s.music.join("moved/renamed.wav");
    fs::create_dir_all(new.parent().unwrap()).unwrap();
    fs::rename(&old, &new).unwrap();
    let report = scan(&s);
    assert_eq!(
        (report.added, report.relinked, report.missing),
        (0, 1, 0),
        "{report:?}"
    );

    let rows = tracks(&s.lib);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);
    assert_eq!(Path::new(&rows[0].path), new);
    assert!(!rows[0].missing);
    let reader = s.lib.reader().unwrap();
    let cue: f64 = reader
        .connection()
        .query_row(
            "SELECT cue_frames FROM cues WHERE track_id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    assert!((cue - 1234.0).abs() < f64::EPSILON);
}

#[test]
fn file_flagged_missing_is_relinked_when_found_elsewhere() {
    let s = setup();
    let old = s.music.join("a/track.wav");
    copy_fixture("sine440.wav", &old);
    scan(&s);
    let id = track_at(&s.lib, &old).id;
    // Moved out of the library: flagged missing.
    let outside = s.music.parent().unwrap().join("outside.wav");
    fs::rename(&old, &outside).unwrap();
    assert_eq!(scan(&s).missing, 1);
    // Back under another name: relinked, no longer missing.
    let new = s.music.join("b/track again.wav");
    fs::create_dir_all(new.parent().unwrap()).unwrap();
    fs::rename(&outside, &new).unwrap();
    let report = scan(&s);
    assert_eq!((report.added, report.relinked), (0, 1), "{report:?}");
    let row = track_at(&s.lib, &new);
    assert_eq!(row.id, id);
    assert!(!row.missing);
}

#[test]
fn file_moved_between_library_folders_is_relinked() {
    let s = setup();
    let other = s.music.parent().unwrap().join("other");
    fs::create_dir_all(&other).unwrap();
    let other = dunce::canonicalize(other).unwrap();
    library::add_folder(&s.lib, &s.music).unwrap();
    library::add_folder(&s.lib, &other).unwrap();
    let old = s.music.join("track.wav");
    copy_fixture("sine440.wav", &old);
    scan(&s);
    let id = track_at(&s.lib, &old).id;

    let new = other.join("track.wav");
    fs::rename(&old, &new).unwrap();
    let report = scan_folder(&s.lib, &other, &mut |_| {}).unwrap();
    assert_eq!((report.added, report.relinked), (0, 1), "{report:?}");
    assert_eq!(track_at(&s.lib, &new).id, id);
    // The old folder's rescan finds nothing missing: the track lives elsewhere now.
    assert_noop(&scan(&s));
}

#[test]
fn track_on_an_absent_library_folder_is_not_relinked_to_a_local_copy() {
    let s = setup();
    let usb = s.music.parent().unwrap().join("usb");
    fs::create_dir_all(&usb).unwrap();
    let usb = dunce::canonicalize(usb).unwrap();
    library::add_folder(&s.lib, &s.music).unwrap();
    library::add_folder(&s.lib, &usb).unwrap();
    let on_usb = usb.join("track.wav");
    copy_fixture("sine440.wav", &on_usb);
    scan_folder(&s.lib, &usb, &mut |_| {}).unwrap();
    let usb_id = track_at(&s.lib, &on_usb).id;

    // The drive is unplugged (its whole folder is gone); a copy of the audio is local.
    fs::remove_dir_all(&usb).unwrap();
    let local = s.music.join("track.wav");
    copy_fixture("sine440.wav", &local);
    let report = scan(&s);
    assert_eq!((report.added, report.relinked), (1, 0), "{report:?}");
    assert_ne!(track_at(&s.lib, &local).id, usb_id);
    let usb_row = track_at(&s.lib, &on_usb);
    assert_eq!(
        usb_row.id, usb_id,
        "the drive's track keeps its path for when it returns"
    );
}

#[test]
fn track_outside_the_root_and_any_folder_is_not_relinked() {
    let s = setup();
    let elsewhere = s.music.parent().unwrap().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let elsewhere = dunce::canonicalize(elsewhere).unwrap();
    let old = elsewhere.join("track.wav");
    copy_fixture("sine440.wav", &old);
    scan_folder(&s.lib, &elsewhere, &mut |_| {}).unwrap();
    fs::remove_dir_all(&elsewhere).unwrap();
    copy_fixture("sine440.wav", &s.music.join("track.wav"));
    let report = scan(&s);
    assert_eq!((report.added, report.relinked), (1, 0), "{report:?}");
}

#[cfg(unix)]
#[test]
fn file_reached_twice_through_symlinks_is_one_track() {
    let s = setup();
    copy_fixture("sine440.wav", &s.music.join("a/x.wav"));
    std::os::unix::fs::symlink(s.music.join("a"), s.music.join("fav")).unwrap();
    std::os::unix::fs::symlink(s.music.join("a/x.wav"), s.music.join("link.wav")).unwrap();
    // Another library folder linking into this one.
    let other = s.music.parent().unwrap().join("other");
    fs::create_dir_all(&other).unwrap();
    std::os::unix::fs::symlink(s.music.join("a"), other.join("shared")).unwrap();

    let report = scan(&s);
    assert_eq!((report.added, report.skipped.len()), (1, 0), "{report:?}");
    let report = scan_folder(&s.lib, &other, &mut |_| {}).unwrap();
    assert_noop(&report);
    let rows = tracks(&s.lib);
    assert_eq!(rows.len(), 1);
    assert_eq!(Path::new(&rows[0].path), s.music.join("a/x.wav"));
    assert_noop(&scan(&s));
}

fn insert_analysis(lib: &Library, id: i64) {
    lib.writer()
        .submit_wait(WriteOp::Raw(vec![RawStatement {
            sql: "INSERT INTO analysis(track_id, analyzer_version, bpm) VALUES (?1, 1, 120.0)"
                .into(),
            params: vec![Value::Integer(id)],
        }]))
        .unwrap();
}

fn has_analysis(lib: &Library, id: i64) -> bool {
    lib.reader()
        .unwrap()
        .connection()
        .query_row(
            "SELECT count(*) FROM analysis WHERE track_id = ?1",
            [id],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        == 1
}

#[test]
fn changed_audio_keeps_the_id_but_drops_stale_analysis() {
    let s = setup();
    let path = s.music.join("track.wav");
    copy_fixture("sine440.wav", &path);
    scan(&s);
    let id = track_at(&s.lib, &path).id;
    insert_analysis(&s.lib, id);

    change_wav_audio(&path);
    touch_later(&path);
    let report = scan(&s);
    assert_eq!(
        (
            report.added,
            report.updated,
            report.audio_changed,
            report.relinked
        ),
        (0, 1, 1, 0),
        "{report:?}"
    );
    assert_eq!(track_at(&s.lib, &path).id, id);
    assert!(!has_analysis(&s.lib, id));
}

#[test]
fn duplicate_audio_in_two_files_is_never_merged() {
    let s = setup();
    copy_fixture("sine440.wav", &s.music.join("one.wav"));
    copy_fixture("sine440.wav", &s.music.join("two.wav"));
    let report = scan(&s);
    assert_eq!(report.added, 2, "{report:?}");

    // A third copy appears while both originals still exist: a new track, not a relink.
    copy_fixture("sine440.wav", &s.music.join("three.wav"));
    let report = scan(&s);
    assert_eq!((report.added, report.relinked), (1, 0), "{report:?}");

    // One original is moved: exactly that track is relinked, the others are untouched.
    let two = track_at(&s.lib, &s.music.join("two.wav")).id;
    fs::rename(s.music.join("two.wav"), s.music.join("two moved.wav")).unwrap();
    let report = scan(&s);
    assert_eq!(
        (report.added, report.relinked, report.missing),
        (0, 1, 0),
        "{report:?}"
    );
    assert_eq!(track_at(&s.lib, &s.music.join("two moved.wav")).id, two);
    assert_eq!(tracks(&s.lib).len(), 3);
}

#[test]
fn retagged_file_keeps_identity() {
    let s = setup();
    let path = s.music.join("track.wav");
    copy_fixture("sine440.wav", &path);
    scan(&s);
    let before = track_at(&s.lib, &path);
    insert_analysis(&s.lib, before.id);

    retag_wav(&path, "New Title");
    let report = scan(&s);
    assert_eq!(
        (
            report.added,
            report.updated,
            report.audio_changed,
            report.relinked
        ),
        (0, 1, 0, 0),
        "{report:?}"
    );
    assert!(
        has_analysis(&s.lib, before.id),
        "same audio keeps its analysis"
    );
    let after = track_at(&s.lib, &path);
    assert_eq!(after.id, before.id);
    assert_eq!(after.title.as_deref(), Some("New Title"));
    assert_eq!(tracks(&s.lib).len(), 1);
}

#[test]
fn deleted_file_is_flagged_missing_not_removed() {
    let s = setup();
    copy_fixture("sine440.wav", &s.music.join("keep.wav"));
    copy_fixture("sine440.flac", &s.music.join("gone.flac"));
    scan(&s);
    let gone = track_at(&s.lib, &s.music.join("gone.flac"));

    fs::remove_file(s.music.join("gone.flac")).unwrap();
    let report = scan(&s);
    assert_eq!(report.missing, 1, "{report:?}");
    let row = track_at(&s.lib, &s.music.join("gone.flac"));
    assert_eq!(row.id, gone.id);
    assert!(row.missing);
    assert!(!track_at(&s.lib, &s.music.join("keep.wav")).missing);
    // Still missing on the next scan, and not counted again.
    assert_noop(&scan(&s));
}

#[test]
fn tracks_outside_the_scanned_root_are_not_flagged() {
    let s = setup();
    copy_fixture("sine440.wav", &s.music.join("a/x.wav"));
    copy_fixture("sine440.flac", &s.music.join("ab/y.flac"));
    scan(&s);
    fs::remove_file(s.music.join("ab/y.flac")).unwrap();
    // Scanning `a` must not touch `ab`, although its path starts with the same text.
    let report = scan_folder(&s.lib, &s.music.join("a"), &mut |_| {}).unwrap();
    assert_noop(&report);
    assert!(!track_at(&s.lib, &s.music.join("ab/y.flac")).missing);
}

#[test]
fn scan_skips_unreadable_and_non_audio() {
    let s = setup();
    copy_fixture("sine440.wav", &s.music.join("good.wav"));
    copy_fixture("corrupt.mp3", &s.music.join("corrupt.mp3"));
    fs::write(s.music.join("empty.mp3"), b"").unwrap();
    fs::write(s.music.join("notes.txt"), b"not audio").unwrap();
    let mut expected = vec![s.music.join("corrupt.mp3"), s.music.join("empty.mp3")];

    #[cfg(unix)]
    let locked = {
        let looped = s.music.join("loop");
        fs::create_dir_all(&looped).unwrap();
        std::os::unix::fs::symlink(&s.music, looped.join("back")).unwrap();
        expected.push(looped.join("back"));

        use std::os::unix::fs::PermissionsExt;
        let locked = s.music.join("locked");
        copy_fixture("sine440.flac", &locked.join("hidden.flac"));
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        // Running as root ignores permissions; then the directory is simply readable.
        if fs::read_dir(&locked).is_err() {
            expected.push(locked.clone());
        }
        locked
    };

    let mut seen = 0;
    let report = scan_folder(&s.lib, &s.music, &mut |_| seen += 1);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let report = report.unwrap();

    let mut skipped: Vec<PathBuf> = report.skipped.iter().map(|(p, _)| p.clone()).collect();
    skipped.sort();
    expected.sort();
    assert_eq!(skipped, expected, "{report:?}");
    for (path, reason) in &report.skipped {
        assert!(!reason.is_empty(), "{}", path.display());
    }
    assert!(seen > 0);
    assert!(
        tracks(&s.lib)
            .iter()
            .any(|r| Path::new(&r.path) == s.music.join("good.wav"))
    );
    assert!(report.added >= 1);
    assert_eq!(report.ignored, 1, "notes.txt is counted, not listed");
}

#[test]
fn fingerprint_ignores_tags_but_not_audio() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original.wav");
    copy_fixture("sine440.wav", &original);
    let print = fingerprint::of_file(&original).unwrap();
    assert_eq!(print, fingerprint::of_file(&original).unwrap());

    let retagged = dir.path().join("retagged.wav");
    copy_fixture("sine440.wav", &retagged);
    retag_wav(&retagged, "Something Else");
    assert_ne!(fs::read(&original).unwrap(), fs::read(&retagged).unwrap());
    assert_eq!(fingerprint::of_file(&retagged).unwrap(), print);

    let altered = dir.path().join("altered.wav");
    copy_fixture("sine440.wav", &altered);
    change_wav_audio(&altered);
    assert_ne!(fingerprint::of_file(&altered).unwrap(), print);

    // Every format fingerprints, and different encodings of the same sine differ.
    let flac = dir.path().join("sine440.flac");
    copy_fixture("sine440.flac", &flac);
    let mp3 = dir.path().join("sine440.mp3");
    copy_fixture("sine440.mp3", &mp3);
    assert_ne!(
        fingerprint::of_file(&flac).unwrap(),
        fingerprint::of_file(&mp3).unwrap()
    );
    assert!(fingerprint::of_file(&dir.path().join("nope.wav")).is_err());
}

#[test]
fn folders_table_is_independent_of_tracks() {
    let s = setup();
    copy_fixture("sine440.wav", &s.music.join("a.wav"));
    library::add_folder(&s.lib, &s.music).unwrap();
    library::add_folder(&s.lib, &s.music).unwrap();
    scan(&s);
    let folders = || -> Vec<String> {
        let reader = s.lib.reader().unwrap();
        let mut stmt = reader
            .connection()
            .prepare("SELECT path FROM folders ORDER BY path")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert_eq!(folders(), vec![s.music.to_str().unwrap().to_owned()]);

    library::remove_folder(&s.lib, &s.music).unwrap();
    assert!(folders().is_empty());
    assert_eq!(
        tracks(&s.lib).len(),
        1,
        "removing a folder never deletes tracks"
    );
}

/// Writes a 16-bit stereo 44.1 kHz WAV: `silence` seconds of silence, then `tone`
/// seconds of a sine at `freq` Hz. Files with the same leading silence (3 s or more)
/// share a fingerprint, like a clean and a dirty edit of one song.
fn write_silent_intro_wav(path: &Path, silence: f64, tone: f64, freq: f64) {
    const SR: u32 = 44_100;
    let silent = (silence * f64::from(SR)) as usize;
    let total = silent + (tone * f64::from(SR)) as usize;
    let data_len = u32::try_from(total * 4).unwrap();
    let mut bytes = Vec::with_capacity(44 + total * 4);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&SR.to_le_bytes());
    bytes.extend_from_slice(&(SR * 4).to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..total {
        let v = if i < silent {
            0
        } else {
            let t = (i - silent) as f64 / f64::from(SR);
            ((std::f64::consts::TAU * freq * t).sin() * 8000.0) as i16
        };
        bytes.extend_from_slice(&v.to_le_bytes());
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).unwrap();
    }
    fs::write(path, bytes).unwrap();
}

fn set_cue(lib: &Library, id: i64, frames: f64) {
    lib.writer()
        .submit_wait(WriteOp::SetCue { track: id, frames })
        .unwrap();
}

fn cue(lib: &Library, id: i64) -> Option<f64> {
    lib.cue(id).unwrap()
}

#[test]
fn relink_is_refused_when_the_duration_differs() {
    let s = setup();
    let old = s.music.join("a/song.wav");
    write_silent_intro_wav(&old, 3.5, 5.0, 440.0);
    scan(&s);
    let id = track_at(&s.lib, &old).id;
    set_cue(&s.lib, id, 1000.0);
    // Gone; another file with the same intro but twice as long appears.
    fs::remove_file(&old).unwrap();
    let other = s.music.join("b/song.wav");
    write_silent_intro_wav(&other, 3.5, 10.0, 440.0);
    let report = scan(&s);
    assert_eq!(
        (report.added, report.relinked, report.missing),
        (1, 0, 1),
        "{report:?}"
    );
    assert_ne!(track_at(&s.lib, &other).id, id);
    let old_row = track_at(&s.lib, &old);
    assert!(old_row.missing);
    assert_eq!(cue(&s.lib, id), Some(1000.0), "the old track keeps its cue");
}

#[test]
fn clean_and_dirty_edits_moved_together_keep_their_own_cues() {
    let s = setup();
    let clean = s.music.join("old/song (clean).wav");
    let dirty = s.music.join("old/song (dirty).wav");
    // Same intro, same length, different audio after the fingerprinted seconds.
    write_silent_intro_wav(&clean, 3.5, 5.0, 440.0);
    write_silent_intro_wav(&dirty, 3.5, 5.0, 660.0);
    scan(&s);
    let clean_id = track_at(&s.lib, &clean).id;
    let dirty_id = track_at(&s.lib, &dirty).id;
    set_cue(&s.lib, clean_id, 111.0);
    set_cue(&s.lib, dirty_id, 222.0);

    // Moved together, keeping their names: each is relinked to its own track.
    let new_dir = s.music.join("new");
    fs::create_dir_all(&new_dir).unwrap();
    let (new_clean, new_dirty) = (
        new_dir.join("song (clean).wav"),
        new_dir.join("song (dirty).wav"),
    );
    fs::rename(&clean, &new_clean).unwrap();
    fs::rename(&dirty, &new_dirty).unwrap();
    let report = scan(&s);
    assert_eq!((report.added, report.relinked), (0, 2), "{report:?}");
    assert_eq!(track_at(&s.lib, &new_clean).id, clean_id);
    assert_eq!(track_at(&s.lib, &new_dirty).id, dirty_id);
    assert_eq!(cue(&s.lib, clean_id), Some(111.0));
    assert_eq!(cue(&s.lib, dirty_id), Some(222.0));

    // Renamed together to names that tell nothing: ambiguous, so neither is relinked.
    // The old tracks stay missing with their cues; the files come in as new tracks.
    let (a, b) = (new_dir.join("1.wav"), new_dir.join("2.wav"));
    fs::rename(&new_clean, &a).unwrap();
    fs::rename(&new_dirty, &b).unwrap();
    let report = scan(&s);
    assert_eq!(
        (report.added, report.relinked, report.missing),
        (2, 0, 2),
        "{report:?}"
    );
    for (path, id, frames) in [(&new_clean, clean_id, 111.0), (&new_dirty, dirty_id, 222.0)] {
        let row = track_at(&s.lib, path);
        assert_eq!(row.id, id);
        assert!(row.missing);
        assert_eq!(cue(&s.lib, id), Some(frames));
    }
    assert!(![clean_id, dirty_id].contains(&track_at(&s.lib, &a).id));
    assert!(![clean_id, dirty_id].contains(&track_at(&s.lib, &b).id));
}

#[test]
fn an_unambiguous_move_with_a_new_name_still_relinks() {
    let s = setup();
    let old = s.music.join("a/song.wav");
    write_silent_intro_wav(&old, 3.5, 5.0, 440.0);
    // A same-intro file that is still in place is no candidate.
    write_silent_intro_wav(&s.music.join("a/other.wav"), 3.5, 5.0, 660.0);
    scan(&s);
    let id = track_at(&s.lib, &old).id;
    set_cue(&s.lib, id, 333.0);
    let new = s.music.join("b/renamed.wav");
    fs::create_dir_all(new.parent().unwrap()).unwrap();
    fs::rename(&old, &new).unwrap();
    let report = scan(&s);
    assert_eq!((report.added, report.relinked), (0, 1), "{report:?}");
    assert_eq!(track_at(&s.lib, &new).id, id);
    assert_eq!(cue(&s.lib, id), Some(333.0));
}

fn analysis_row(lib: &Library, id: i64) -> (i64, Option<f64>, Option<f64>, bool, bool) {
    lib.reader()
        .unwrap()
        .connection()
        .query_row(
            "SELECT analyzer_version, bpm, first_beat_frames, grid_locked, waveform IS NOT NULL \
             FROM analysis WHERE track_id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap()
}

fn insert_locked_analysis(lib: &Library, id: i64) {
    lib.writer()
        .submit_wait(WriteOp::Raw(vec![RawStatement {
            sql: "INSERT INTO analysis(track_id, analyzer_version, bpm, first_beat_frames, \
                  grid_locked, key, waveform) VALUES (?1, 1, 126.0, 512.0, 1, 3, x'00')"
                .into(),
            params: vec![Value::Integer(id)],
        }]))
        .unwrap();
}

#[test]
fn same_fingerprint_change_marks_analysis_out_of_date_keeping_a_locked_grid() {
    let s = setup();
    let path = s.music.join("track.wav");
    copy_fixture("sine440.wav", &path);
    scan(&s);
    let id = track_at(&s.lib, &path).id;
    insert_locked_analysis(&s.lib, id);
    set_cue(&s.lib, id, 4321.0);

    // The file changes but its first seconds do not (a retag here; a copy that was
    // still in progress at the last scan looks the same).
    retag_wav(&path, "Done Copying");
    let report = scan(&s);
    assert_eq!((report.updated, report.audio_changed), (1, 0), "{report:?}");
    assert_eq!(
        analysis_row(&s.lib, id),
        (0, Some(126.0), Some(512.0), true, true),
        "version 0 so it is re-analyzed; the locked grid stays"
    );
    assert_eq!(cue(&s.lib, id), Some(4321.0));
    assert!(
        s.lib
            .reader()
            .unwrap()
            .track_ids_to_analyze(false)
            .unwrap()
            .contains(&id)
    );
}

#[test]
fn changed_audio_keeps_a_locked_grid() {
    let s = setup();
    let path = s.music.join("track.wav");
    copy_fixture("sine440.wav", &path);
    scan(&s);
    let id = track_at(&s.lib, &path).id;
    insert_locked_analysis(&s.lib, id);

    change_wav_audio(&path);
    touch_later(&path);
    let report = scan(&s);
    assert_eq!(report.audio_changed, 1, "{report:?}");
    assert_eq!(
        analysis_row(&s.lib, id),
        (0, Some(126.0), Some(512.0), true, false),
        "the locked grid stays; waveform and key are cleared"
    );
    let key: Option<i64> = s
        .lib
        .reader()
        .unwrap()
        .connection()
        .query_row("SELECT key FROM analysis WHERE track_id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(key, None);
}

#[test]
fn add_folder_files_is_shallow_and_unregistered() {
    let s = setup();
    copy_fixture("sine440.wav", &s.music.join("a1.wav"));
    copy_fixture("sine440_mono.wav", &s.music.join("sub/s1.wav"));
    let reader = s.lib.reader().unwrap();
    let report = library::add_folder_files(&s.lib.writer(), reader.connection(), &s.music).unwrap();
    assert!(s.lib.take_failed_writes().is_empty());
    assert_eq!((report.added, report.missing), (1, 0));
    let paths: Vec<String> = reader
        .tracks(&Default::default(), library::TrackSort::default(), 0, 100)
        .unwrap()
        .into_iter()
        .map(|t| {
            reader
                .track(t.id)
                .unwrap()
                .unwrap()
                .path
                .display()
                .to_string()
        })
        .collect();
    assert_eq!(paths.len(), 1);
    assert!(paths[0].ends_with("a1.wav"), "{paths:?}");
    assert!(reader.folders().unwrap().is_empty());
}
