//! The stems cache: Ogg Vorbis entries on disk, their rows, and eviction within a budget.

use std::path::{Path, PathBuf};

use engine::{Stem, StemSet};
use library::{Library, RawStatement, StemCache, StemKey, WriteOp};
use rusqlite::types::Value;

const DAY: i64 = 24 * 60 * 60;

struct Fixture {
    _dir: tempfile::TempDir,
    db: PathBuf,
    stems: PathBuf,
    lib: Library,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("library.sqlite3");
    let stems = dir.path().join("stems");
    let lib = Library::open(&db).unwrap();
    Fixture {
        _dir: dir,
        db,
        stems,
        lib,
    }
}

fn raw(sql: &str, params: Vec<Value>) -> WriteOp {
    WriteOp::Raw(vec![RawStatement {
        sql: sql.into(),
        params,
    }])
}

/// Adds a track with fingerprint `[seed; 32]` and returns its id.
fn add_track(lib: &Library, seed: u8) -> i64 {
    let outcome = lib
        .writer()
        .submit_wait(raw(
            "INSERT INTO tracks(path, fingerprint, sample_rate, added_at) VALUES (?1, ?2, 44100, 0)",
            vec![
                Value::Text(format!("/music/{seed}.flac")),
                Value::Blob(vec![seed; 32]),
            ],
        ))
        .unwrap();
    match outcome {
        library::WriteOutcome::Raw {
            last_insert_rowid, ..
        } => last_insert_rowid,
        other => panic!("{other:?}"),
    }
}

fn set_loaded_at(lib: &Library, track: i64, at: Option<i64>) {
    lib.writer()
        .submit_wait(raw(
            "UPDATE tracks SET last_loaded_at = ?2 WHERE id = ?1",
            vec![
                Value::Integer(track),
                at.map_or(Value::Null, Value::Integer),
            ],
        ))
        .unwrap();
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// A cheap deterministic noise source in `[-1, 1]`.
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32 / (1u64 << 23) as f32) - 1.0
    }
}

fn to_i16(v: f32) -> i16 {
    (v * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16
}

/// Synthetic stems at `rate`: a vocal-like harmonic tone, a bass sine and noise-like hats
/// on every eighth note at 120 BPM.
fn stems_at(rate: u32, seconds: f32) -> StemSet {
    let frames = (rate as f32 * seconds) as usize;
    let mut noise = Noise(7);
    let (mut vocals, mut bass, mut rhythm) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..frames {
        let t = i as f32 / rate as f32;
        let tau = std::f32::consts::TAU;
        let vibrato = 440.0 * (1.0 + 0.01 * (tau * 5.0 * t).sin());
        let v = 0.15 * (tau * vibrato * t).sin() + 0.05 * (tau * 2.0 * vibrato * t).sin();
        let b = 0.3 * (tau * 55.0 * t).sin();
        let beat = (t * 4.0).fract();
        let envelope = (-beat * 40.0).exp();
        let left = 0.1 * envelope * noise.next();
        let right = 0.1 * envelope * noise.next();
        vocals.extend([to_i16(v), to_i16(v * 0.9)]);
        bass.extend([to_i16(b), to_i16(b)]);
        rhythm.extend([to_i16(left), to_i16(right)]);
    }
    StemSet::new(vocals.into(), bass.into(), rhythm.into(), frames).unwrap()
}

fn energy(samples: impl Iterator<Item = f64>) -> f64 {
    samples.map(|s| s * s).sum()
}

/// Per stored stem, `10 log10(energy(original − decoded) / energy(mix))`.
fn error_db(original: &StemSet, decoded: &StemSet) -> [f64; 3] {
    let stems = [Stem::Vocals, Stem::Bass, Stem::Rhythm];
    fn stored(set: &StemSet, s: Stem) -> &[i16] {
        set.stored(s).unwrap()
    }
    let originals = stems.map(|s| stored(original, s));
    let mix_energy = energy(
        (0..original.frames() * 2).map(|i| originals.iter().map(|o| f64::from(o[i])).sum::<f64>()),
    );
    stems.map(|s| {
        let (a, b) = (stored(original, s), stored(decoded, s));
        let err = energy(a.iter().zip(b).map(|(x, y)| f64::from(*x) - f64::from(*y)));
        10.0 * (err / mix_energy).log10()
    })
}

/// The lag in `-64..=64` frames at which `decoded` best matches `original` (left channel,
/// over the first half second).
fn best_lag(original: &[i16], decoded: &[i16]) -> i64 {
    let frames = (original.len() / 2).min(24_000);
    (-64i64..=64)
        .max_by(|&a, &b| {
            let corr = |lag: i64| -> f64 {
                (0..frames)
                    .filter_map(|i| {
                        let j = i as i64 + lag;
                        (0..frames as i64).contains(&j).then(|| {
                            f64::from(original[2 * i]) * f64::from(decoded[2 * j as usize])
                        })
                    })
                    .sum()
            };
            corr(a).total_cmp(&corr(b))
        })
        .unwrap()
}

fn key(track: i64, fingerprint: &[u8], rate: u32, frames: usize) -> StemKey<'_> {
    StemKey {
        track,
        fingerprint,
        sample_rate: rate,
        frames,
    }
}

fn entry_dir(stems: &Path, track: i64, fingerprint: &[u8]) -> PathBuf {
    let hex: String = fingerprint.iter().map(|b| format!("{b:02x}")).collect();
    stems.join(format!("{track}-{hex}"))
}

fn row_count(lib: &Library) -> i64 {
    lib.reader()
        .unwrap()
        .connection()
        .query_row("SELECT count(*) FROM stems", [], |r| r.get(0))
        .unwrap()
}

fn cached_tracks(lib: &Library) -> Vec<i64> {
    let reader = lib.reader().unwrap();
    let mut stmt = reader
        .connection()
        .prepare("SELECT track_id FROM stems ORDER BY track_id")
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn round_trip(rate: u32) {
    let f = fixture();
    let track = add_track(&f.lib, 1);
    let fingerprint = [1u8; 32];
    let original = stems_at(rate, 3.0);
    let k = key(track, &fingerprint, rate, original.frames());
    let mut cache = StemCache::new(&f.lib, &f.stems, u64::MAX).unwrap();
    cache.put(&k, &original, false, &[]).unwrap();

    let dir = entry_dir(&f.stems, track, &fingerprint);
    for file in ["vocals.ogg", "bass.ogg", "rhythm.ogg", "stems.toml"] {
        assert!(dir.join(file).is_file(), "{file} missing");
    }
    let toml = std::fs::read_to_string(dir.join("stems.toml")).unwrap();
    assert!(toml.contains("format = 1"), "{toml}");
    assert!(toml.contains(&format!("sample_rate = {rate}")), "{toml}");
    assert!(
        toml.contains(&format!("model = \"{}\"", stems::MODEL_ID)),
        "{toml}"
    );
    assert!(toml.contains(stems::WEIGHTS_SHA256) && toml.contains(stems::GRAPH_SHA256));

    let decoded = cache.get(&k).expect("a fresh entry is a hit");
    assert_eq!(decoded.frames(), original.frames());
    for stem in [Stem::Vocals, Stem::Bass, Stem::Rhythm] {
        assert_eq!(
            best_lag(
                original.stored(stem).unwrap(),
                decoded.stored(stem).unwrap()
            ),
            0,
            "{stem:?} at {rate} Hz"
        );
    }
    let [vocals, bass, rhythm] = error_db(&original, &decoded);
    eprintln!(
        "{rate} Hz error vs mix: vocals {vocals:.1} dB, bass {bass:.1} dB, rhythm {rhythm:.1} dB"
    );
    // docs/stems-backend.md measured −59.5 / −36.0 / −23.3 dB on a real track.
    // Here (pure tones and hats): about −38 / −39 / −47 dB.
    assert!(vocals < -30.0, "vocals {vocals:.1} dB");
    assert!(bass < -30.0, "bass {bass:.1} dB");
    assert!(rhythm < -20.0, "rhythm {rhythm:.1} dB");

    let bytes: i64 = f
        .lib
        .reader()
        .unwrap()
        .connection()
        .query_row(
            "SELECT bytes FROM stems WHERE track_id = ?1",
            [track],
            |r| r.get(0),
        )
        .unwrap();
    let on_disk: u64 = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    assert_eq!(bytes as u64, on_disk);
}

#[test]
fn cache_round_trip_within_tolerance() {
    round_trip(44_100);
}

#[test]
fn cache_round_trip_at_48_khz() {
    round_trip(48_000);
}

#[test]
fn corrupt_cache_entry_is_redone() {
    let f = fixture();
    let track = add_track(&f.lib, 2);
    let fingerprint = [2u8; 32];
    let set = stems_at(44_100, 1.0);
    let k = key(track, &fingerprint, 44_100, set.frames());
    let mut cache = StemCache::new(&f.lib, &f.stems, u64::MAX).unwrap();
    let dir = entry_dir(&f.stems, track, &fingerprint);

    type Damage = fn(&Path);
    let damages: [(&str, Damage); 4] = [
        ("truncated", |dir| {
            let file = dir.join("vocals.ogg");
            let len = std::fs::metadata(&file).unwrap().len();
            std::fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_len(len / 2)
                .unwrap();
        }),
        ("deleted", |dir| {
            std::fs::remove_file(dir.join("bass.ogg")).unwrap()
        }),
        ("garbage toml", |dir| {
            std::fs::write(dir.join("stems.toml"), b"\xff\x00 not = [toml").unwrap();
        }),
        ("garbage ogg", |dir| {
            std::fs::write(dir.join("rhythm.ogg"), b"OggS but not really").unwrap();
        }),
    ];
    for (what, damage) in damages {
        cache.put(&k, &set, false, &[]).unwrap();
        assert!(cache.get(&k).is_some(), "{what}: intact entry");
        damage(&dir);
        assert!(cache.get(&k).is_none(), "{what}: damaged entry is a miss");
        assert!(!dir.exists(), "{what}: damaged entry's files are removed");
        assert_eq!(
            row_count(&f.lib),
            0,
            "{what}: damaged entry's row is removed"
        );
    }
    // Redone: separated again and put, it is a hit once more.
    cache.put(&k, &set, false, &[]).unwrap();
    assert_eq!(cache.get(&k).unwrap().frames(), set.frames());
}

#[test]
fn fingerprint_change_misses() {
    let f = fixture();
    let track = add_track(&f.lib, 3);
    let set = stems_at(44_100, 0.5);
    let old = [3u8; 32];
    let new = [4u8; 32];
    let mut cache = StemCache::new(&f.lib, &f.stems, u64::MAX).unwrap();

    cache
        .put(&key(track, &old, 44_100, set.frames()), &set, false, &[])
        .unwrap();
    assert!(cache.get(&key(track, &new, 44_100, set.frames())).is_none());
    assert!(
        !entry_dir(&f.stems, track, &old).exists(),
        "stale audio's entry removed"
    );
    assert_eq!(row_count(&f.lib), 0);

    // Another rate or frame count than the loaded track's is a miss too.
    let k = key(track, &new, 44_100, set.frames());
    for (rate, frames) in [(48_000, set.frames()), (44_100, set.frames() + 1)] {
        cache.put(&k, &set, false, &[]).unwrap();
        assert!(
            cache.get(&key(track, &new, rate, frames)).is_none(),
            "{rate} {frames}"
        );
        assert_eq!(row_count(&f.lib), 0);
    }

    // So is an entry made by another model.
    cache.put(&k, &set, false, &[]).unwrap();
    let toml_path = entry_dir(&f.stems, track, &new).join("stems.toml");
    let toml = std::fs::read_to_string(&toml_path).unwrap();
    std::fs::write(
        &toml_path,
        toml.replace(stems::GRAPH_SHA256, &"0".repeat(64)),
    )
    .unwrap();
    assert!(cache.get(&k).is_none(), "another graph is stale");
    assert_eq!(row_count(&f.lib), 0);
}

/// Puts a short entry for each track with a budget that never evicts.
fn put_all(f: &Fixture, tracks: &[(i64, bool)]) -> u64 {
    let set = stems_at(44_100, 0.25);
    let mut cache = StemCache::new(&f.lib, &f.stems, u64::MAX).unwrap();
    for &(track, prepared) in tracks {
        let fingerprint = [track as u8; 32];
        cache
            .put(
                &key(track, &fingerprint, 44_100, set.frames()),
                &set,
                prepared,
                &[],
            )
            .unwrap();
    }
    cache.bytes().unwrap() / tracks.len() as u64
}

#[test]
fn eviction_order_follows_the_spec() {
    let f = fixture();
    let ids: Vec<i64> = (1..=8).map(|seed| add_track(&f.lib, seed)).collect();
    let [
        never,
        old,
        cue_at_zero,
        in_playlist,
        hotcue,
        prepared,
        recent,
        cued,
    ] = ids.clone().try_into().unwrap();
    let entry = put_all(
        &f,
        &ids.iter().map(|&t| (t, t == prepared)).collect::<Vec<_>>(),
    );

    let w = f.lib.writer();
    let t = now();
    set_loaded_at(&f.lib, never, None);
    set_loaded_at(&f.lib, old, Some(t - 40 * DAY));
    set_loaded_at(&f.lib, cue_at_zero, Some(t - 45 * DAY));
    set_loaded_at(&f.lib, in_playlist, Some(t - 60 * DAY));
    set_loaded_at(&f.lib, hotcue, Some(t - 50 * DAY));
    set_loaded_at(&f.lib, prepared, Some(t - 35 * DAY));
    set_loaded_at(&f.lib, recent, Some(t - 5 * DAY));
    set_loaded_at(&f.lib, cued, Some(t - 55 * DAY));
    w.submit_wait(WriteOp::SetCue {
        track: cue_at_zero,
        frames: 0.0,
    })
    .unwrap();
    w.submit_wait(WriteOp::SetCue {
        track: cued,
        frames: 500.0,
    })
    .unwrap();
    w.submit_wait(WriteOp::SetHotcue {
        track: hotcue,
        slot: 1,
        frames: 100.0,
    })
    .unwrap();
    let playlist = match w
        .submit_wait(WriteOp::CreatePlaylist { name: "set".into() })
        .unwrap()
    {
        library::WriteOutcome::Created { id } => id,
        other => panic!("{other:?}"),
    };
    w.submit_wait(WriteOp::AddToPlaylist {
        playlist,
        track: in_playlist,
        at: None,
    })
    .unwrap();

    // Group 1 (no playlist, not prepared, not loaded in 30 days) by oldest load, never
    // loaded first; then the rest by oldest load.
    let expected = [
        never,
        cue_at_zero,
        old,
        in_playlist,
        cued,
        hotcue,
        prepared,
        recent,
    ];
    for (evicted, &track) in expected.iter().enumerate() {
        let keep = ids.len() - evicted - 1;
        // A budget a little over `keep` entries: exactly one more must go.
        let mut cache = StemCache::new(&f.lib, &f.stems, entry * keep as u64 + entry / 2).unwrap();
        let report = cache.evict(&[]).unwrap();
        assert_eq!(report.removed, vec![track], "eviction {}", evicted + 1);
        assert!(!entry_dir(&f.stems, track, &[track as u8; 32]).exists());
        assert_eq!(cached_tracks(&f.lib).len(), keep);
    }
}

#[test]
fn loaded_stems_are_never_evicted() {
    let f = fixture();
    let a = add_track(&f.lib, 1);
    let b = add_track(&f.lib, 2);
    put_all(&f, &[(a, false), (b, false)]);

    let mut cache = StemCache::new(&f.lib, &f.stems, 1).unwrap();
    let first = cache.evict(&[a, b]).unwrap();
    assert!(first.removed.is_empty());
    assert!(first.over_budget);
    assert!(
        first.notice.is_some(),
        "the first time over budget is logged"
    );
    let again = cache.evict(&[a, b]).unwrap();
    assert!(again.over_budget && again.notice.is_none(), "logged once");
    assert_eq!(cached_tracks(&f.lib), vec![a, b]);

    // A put never evicts the entry it just stored, nor a loaded one.
    let c = add_track(&f.lib, 3);
    let set = stems_at(44_100, 0.25);
    let report = cache
        .put(&key(c, &[3; 32], 44_100, set.frames()), &set, false, &[a])
        .unwrap();
    assert_eq!(report.removed, vec![b]);
    assert_eq!(cached_tracks(&f.lib), vec![a, c]);

    // Once the decks unload, everything over budget goes.
    let report = cache.evict(&[]).unwrap();
    assert_eq!(report.removed.len(), 2);
    assert!(!report.over_budget);
    assert_eq!(row_count(&f.lib), 0);
}

#[test]
fn orphan_entries_are_swept_on_new() {
    let f = fixture();
    let a = add_track(&f.lib, 1);
    put_all(&f, &[(a, false)]);
    // A crash between the rename and the row, a crash mid-write, a row whose files are
    // gone, and the CoreML compile cache, which shares the folder and must survive.
    let orphan = entry_dir(&f.stems, 99, &[9; 32]);
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("vocals.ogg"), b"x").unwrap();
    // A temporary folder another process may still be writing (fresh) is kept.
    let writing = f.stems.join(".tmp-1-abc");
    std::fs::create_dir_all(&writing).unwrap();
    #[cfg(unix)]
    let partial = {
        let partial = f.stems.join(".tmp-2-def");
        std::fs::create_dir_all(&partial).unwrap();
        age(&partial, 2 * 3600);
        partial
    };
    let coreml = f.stems.join("coreml-cache");
    std::fs::create_dir_all(&coreml).unwrap();
    std::fs::write(coreml.join("model.mlmodelc"), b"keep").unwrap();
    let b = add_track(&f.lib, 2);
    f.lib
        .writer()
        .submit_wait(raw(
            "INSERT INTO stems(track_id, fingerprint, path, bytes, prepared, created_at) \
             VALUES (?1, ?2, ?3, 1000, 0, 0)",
            vec![
                Value::Integer(b),
                Value::Blob(vec![2; 32]),
                Value::Text(entry_dir(Path::new(""), b, &[2; 32]).display().to_string()),
            ],
        ))
        .unwrap();

    let cache = StemCache::new(&f.lib, &f.stems, u64::MAX).unwrap();
    assert!(!orphan.exists());
    assert!(writing.exists(), "a fresh temporary folder is not swept");
    #[cfg(unix)]
    assert!(!partial.exists(), "an hour-old temporary folder is swept");
    assert!(coreml.join("model.mlmodelc").exists());
    assert!(entry_dir(&f.stems, a, &[a as u8; 32]).exists());
    assert_eq!(cached_tracks(&f.lib), vec![a]);
    let on_disk: u64 = std::fs::read_dir(entry_dir(&f.stems, a, &[a as u8; 32]))
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    assert_eq!(cache.bytes().unwrap(), on_disk);
}

#[test]
fn migration_adds_columns_and_keeps_data() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("library.sqlite3");
    {
        let v1 = Library::open_with_migrations(&db, &library::schema::MIGRATIONS[..1]).unwrap();
        let track = add_track(&v1, 5);
        v1.writer()
            .submit_wait(WriteOp::SetCue {
                track,
                frames: 1234.0,
            })
            .unwrap();
    }
    let lib = Library::open(&db).unwrap();
    let reader = lib.reader().unwrap();
    let conn = reader.connection();
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 2);
    let (fingerprint, loaded): (Vec<u8>, Option<i64>) = conn
        .query_row("SELECT fingerprint, last_loaded_at FROM tracks", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(fingerprint, vec![5; 32]);
    assert_eq!(loaded, None);
    let prepared: i64 = conn
        .query_row("SELECT stems_prepared FROM tracks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(prepared, 0);
    assert_eq!(reader.cue(1).unwrap(), Some(1234.0));
    let columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('stems') ORDER BY cid")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        columns,
        [
            "track_id",
            "fingerprint",
            "path",
            "bytes",
            "prepared",
            "created_at"
        ]
    );
}

#[test]
fn track_loaded_updates_last_loaded_at() {
    let f = fixture();
    let track = add_track(&f.lib, 6);
    let before = now();
    f.lib
        .writer()
        .submit_wait(WriteOp::TrackLoaded(track))
        .unwrap();
    let at: Option<i64> = f
        .lib
        .reader()
        .unwrap()
        .connection()
        .query_row(
            "SELECT last_loaded_at FROM tracks WHERE id = ?1",
            [track],
            |r| r.get(0),
        )
        .unwrap();
    let at = at.expect("set at load");
    assert!(at >= before && at <= now(), "{at} vs {before}");
    assert!(matches!(
        f.lib.writer().submit_wait(WriteOp::TrackLoaded(9999)),
        Err(library::LibraryError::NoSuchTrack(9999))
    ));
    let _ = &f.db;
}

#[test]
fn rates_without_a_bitrate_table_still_cache() {
    // libvorbis has no 192 kbit/s table at these rates; the cache falls back to a quality.
    for rate in [8_000, 22_050, 88_200, 96_000, 192_000] {
        let f = fixture();
        let track = add_track(&f.lib, 1);
        let set = stems_at(rate, 0.5);
        let mut cache = StemCache::new(&f.lib, &f.stems, u64::MAX).unwrap();
        let k = key(track, &[1; 32], rate, set.frames());
        cache.put(&k, &set, false, &[]).unwrap();
        let decoded = cache.get(&k).unwrap_or_else(|| panic!("{rate} Hz"));
        assert_eq!(decoded.frames(), set.frames(), "{rate} Hz");
    }
}

/// Makes `path` (a folder) look `seconds` old. Opening a folder as a file works on Unix.
#[cfg(unix)]
fn age(path: &Path, seconds: u64) {
    std::fs::File::open(path)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(seconds))
        .unwrap();
}

/// Puts a read-only folder holding a file in track `track`'s entry folder, so the entry
/// cannot be removed; returns that folder, to unlock with [`unlock`].
#[cfg(unix)]
fn make_unremovable(stems: &Path, track: i64) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let locked = entry_dir(stems, track, &[track as u8; 32]).join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(locked.join("vocals.ogg"), b"x").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    locked
}

#[cfg(unix)]
fn unlock(locked: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(locked, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_stray_file_in_place_of_an_entry_is_removed() {
    let f = fixture();
    let track = add_track(&f.lib, 1);
    put_all(&f, &[(track, false)]);
    let dir = entry_dir(&f.stems, track, &[track as u8; 32]);
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::write(&dir, b"not a folder").unwrap();
    let mut cache = StemCache::new(&f.lib, &f.stems, 0).unwrap();
    let report = cache.evict(&[]).unwrap();
    assert_eq!((report.removed, report.failed), (vec![track], 0));
    assert!(!dir.exists());
    assert!(cached_tracks(&f.lib).is_empty(), "no stuck row");
}

#[cfg(unix)]
#[test]
fn a_failed_removal_is_skipped_and_retried() {
    let f = fixture();
    let ids: Vec<i64> = (1..=3).map(|seed| add_track(&f.lib, seed)).collect();
    put_all(&f, &ids.iter().map(|&t| (t, false)).collect::<Vec<_>>());
    let t = now();
    for (i, &track) in ids.iter().enumerate() {
        set_loaded_at(&f.lib, track, Some(t - (40 + 10 * (3 - i as i64)) * DAY));
    }
    // The oldest entry cannot be removed: eviction skips it and goes on.
    let locked = make_unremovable(&f.stems, ids[0]);
    let mut cache = StemCache::new(&f.lib, &f.stems, 0).unwrap();
    let report = cache.evict(&[]).unwrap();
    assert_eq!(report.failed, 1);
    assert_eq!(report.removed, vec![ids[1], ids[2]]);
    assert_eq!(
        cached_tracks(&f.lib),
        vec![ids[0]],
        "its row is kept, to retry"
    );
    assert!(report.over_budget && report.notice.is_some());

    // A put whose entry is stored succeeds even though eviction fails again.
    let d = add_track(&f.lib, 4);
    let set = stems_at(44_100, 0.25);
    let report = cache
        .put(&key(d, &[4; 32], 44_100, set.frames()), &set, false, &[])
        .unwrap();
    assert_eq!(report.failed, 1);
    assert!(cache.get(&key(d, &[4; 32], 44_100, set.frames())).is_some());

    // Once removable, a later pass removes it.
    unlock(&locked);
    let report = cache.evict(&[d]).unwrap();
    assert_eq!((report.removed, report.failed), (vec![ids[0]], 0));
}

#[cfg(unix)]
#[test]
fn sweep_skips_what_it_cannot_remove() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture();
    std::fs::create_dir_all(&f.stems).unwrap();
    // Two orphans; the first holds a read-only folder, so its files cannot be deleted.
    let stuck = entry_dir(&f.stems, 98, &[8; 32]);
    let locked = stuck.join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(locked.join("vocals.ogg"), b"x").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    let orphan = entry_dir(&f.stems, 99, &[9; 32]);
    std::fs::create_dir_all(&orphan).unwrap();

    let opened = StemCache::new(&f.lib, &f.stems, u64::MAX);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    opened.expect("a folder that cannot be removed does not fail opening the cache");
    assert!(locked.join("vocals.ogg").exists(), "skipped");
    assert!(!orphan.exists(), "the sweep went on");
}

#[test]
fn a_redone_entry_stays_prepared() {
    let f = fixture();
    let track = add_track(&f.lib, 1);
    let set = stems_at(44_100, 0.25);
    let k = key(track, &[1; 32], 44_100, set.frames());
    let mut cache = StemCache::new(&f.lib, &f.stems, u64::MAX).unwrap();
    cache.put(&k, &set, true, &[]).unwrap();
    std::fs::remove_file(entry_dir(&f.stems, track, &[1; 32]).join("bass.ogg")).unwrap();
    assert!(cache.get(&k).is_none());
    // Separated again on load, not with `library stems`: still prepared.
    cache.put(&k, &set, false, &[]).unwrap();
    let prepared: bool = f
        .lib
        .reader()
        .unwrap()
        .connection()
        .query_row(
            "SELECT prepared FROM stems WHERE track_id = ?1",
            [track],
            |r| r.get(0),
        )
        .unwrap();
    assert!(prepared);
}
