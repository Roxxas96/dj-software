//! Background analysis jobs: caching by analyzer version, forced re-analysis, locked
//! grids, missing files and the BPM range setting.

use std::f64::consts::TAU;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use library::{
    ANALYZER_VERSION, AnalysisProgress, AnalysisRow, AnalysisWarning, Analyzer, BpmRangeError,
    Library, LibraryError, PROGRESS_TAIL, REPORT_CAPACITY, RawStatement, ScanProgress,
    StoreAnalysis, WriteOp, parse_bpm_range, scan_folder, validate_bpm_range,
};
use rusqlite::types::Value;

const SR: u32 = 44_100;
const SECS: f64 = 30.0;

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
    let music = dunce::canonicalize(&music).unwrap();
    Setup {
        _dir: dir,
        lib,
        music,
    }
}

/// Kick-like clicks (a decaying 60 Hz burst plus a short transient) at `bpm`, the first at
/// `first_beat_secs`, as 16-bit mono samples.
fn clicks(bpm: f64, first_beat_secs: f64) -> Vec<i16> {
    let sr = f64::from(SR);
    let n = (SECS * sr) as usize;
    let mut out = vec![0.0f64; n];
    let period = 60.0 / bpm * sr;
    let burst = (0.25 * sr) as usize;
    let transient = (0.003 * sr) as usize;
    let mut beat = 0.0;
    loop {
        let start = (first_beat_secs * sr + beat * period).round() as usize;
        if start >= n {
            break;
        }
        for k in 0..burst.min(n - start) {
            let t = k as f64 / sr;
            let mut v = (TAU * 60.0 * t).sin() * (-t / 0.05).exp();
            if k < transient {
                // Deterministic broadband transient.
                v +=
                    0.5 * if k % 2 == 0 { 1.0 } else { -1.0 } * (1.0 - k as f64 / transient as f64);
            }
            out[start + k] += v;
        }
        beat += 1.0;
    }
    out.into_iter()
        .map(|v| (v.clamp(-1.0, 1.0) * 0.8 * f64::from(i16::MAX)) as i16)
        .collect()
}

/// Writes a minimal 16-bit stereo PCM WAV (the mono signal on both channels).
fn write_wav(path: &Path, mono: &[i16]) {
    let data_len = u32::try_from(mono.len() * 4).unwrap();
    let mut bytes = Vec::with_capacity(44 + mono.len() * 4);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&2u16.to_le_bytes()); // channels
    bytes.extend_from_slice(&SR.to_le_bytes());
    bytes.extend_from_slice(&(SR * 4).to_le_bytes()); // byte rate
    bytes.extend_from_slice(&4u16.to_le_bytes()); // block align
    bytes.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for s in mono {
        bytes.extend_from_slice(&s.to_le_bytes());
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    fs::write(path, bytes).unwrap();
}

/// Writes a 30 s click track at `bpm` named `name`, scans it in and returns its id.
fn add_track(s: &Setup, name: &str, bpm: f64, first_beat_secs: f64) -> i64 {
    let path = s.music.join(name);
    write_wav(&path, &clicks(bpm, first_beat_secs));
    scan_folder(&s.lib, &s.music, &mut |_: ScanProgress| {}).unwrap();
    assert!(s.lib.take_failed_writes().is_empty());
    track_id(&s.lib, &path)
}

fn track_id(lib: &Library, path: &Path) -> i64 {
    lib.reader()
        .unwrap()
        .connection()
        .query_row(
            "SELECT id FROM tracks WHERE path = ?1",
            [path.to_str().unwrap()],
            |r| r.get(0),
        )
        .unwrap()
}

fn raw(lib: &Library, sql: &str, params: Vec<Value>) {
    lib.writer()
        .submit_wait(WriteOp::Raw(vec![RawStatement {
            sql: sql.into(),
            params,
        }]))
        .unwrap();
}

/// Waits until nothing is queued or running (at most two minutes).
fn wait(analyzer: &Analyzer) -> AnalysisProgress {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let p = analyzer.progress();
        if p.queued == 0 {
            return p;
        }
        assert!(Instant::now() < deadline, "analysis did not finish: {p:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn analyze(s: &Setup, ids: &[i64], force: bool) -> AnalysisProgress {
    let analyzer = Analyzer::start(&s.lib, 2).unwrap();
    analyzer.enqueue(ids, force);
    wait(&analyzer)
}

fn row(s: &Setup, id: i64) -> AnalysisRow {
    s.lib.analysis(id).unwrap().expect("an analysis row")
}

fn assert_bpm(row: &AnalysisRow, expected: f64) {
    let bpm = row.bpm.expect("a bpm");
    assert!(
        (bpm - expected).abs() <= 0.05,
        "expected {expected} BPM, got {bpm}"
    );
}

/// Overwrites a stored analysis with a sentinel BPM, keeping its analyzer version.
fn set_sentinel_bpm(s: &Setup, id: i64, version: i64) {
    raw(
        &s.lib,
        "UPDATE analysis SET bpm = 99.0, analyzer_version = ?2 WHERE track_id = ?1",
        vec![Value::Integer(id), Value::Integer(version)],
    );
}

#[test]
fn enqueue_analyzes_and_stores() {
    let s = setup();
    let id = add_track(&s, "a.wav", 128.0, 0.25);
    assert_eq!(s.lib.analysis(id).unwrap(), None);
    let p = analyze(&s, &[id], false);
    assert_eq!((p.done, p.skipped), (1, 0), "{p:?}");
    assert!(p.failed.is_empty() && p.warnings.is_empty(), "{p:?}");
    let r = row(&s, id);
    assert_eq!(r.analyzer_version, i64::from(ANALYZER_VERSION));
    assert_bpm(&r, 128.0);
    let first = r.first_beat_frames.expect("a first beat") / f64::from(SR);
    assert!((first - 0.25).abs() <= 0.02, "first beat at {first} s");
    assert!(!r.grid_locked);
    assert!(r.key.is_some());
    assert!(r.analyzed_at.is_some());
    // Persisted at 1024 frames per bucket to bound the database size.
    let wf = r.waveform.expect("a waveform");
    assert_eq!(wf.frames_per_bucket, 1024);
    let frames = (SECS * f64::from(SR)) as usize;
    assert_eq!(wf.buckets.len(), frames.div_ceil(1024));
}

#[test]
fn up_to_date_tracks_are_skipped() {
    let s = setup();
    let id = add_track(&s, "a.wav", 128.0, 0.25);
    analyze(&s, &[id], false);
    set_sentinel_bpm(&s, id, i64::from(ANALYZER_VERSION));
    let p = analyze(&s, &[id], false);
    assert_eq!((p.done, p.skipped), (0, 1), "{p:?}");
    assert_eq!(row(&s, id).bpm, Some(99.0));
}

#[test]
fn stale_and_cleared_tracks_are_reanalyzed() {
    let s = setup();
    let id = add_track(&s, "a.wav", 128.0, 0.25);
    analyze(&s, &[id], false);
    // An older analyzer version is redone.
    set_sentinel_bpm(&s, id, i64::from(ANALYZER_VERSION) - 1);
    let p = analyze(&s, &[id], false);
    assert_eq!(p.done, 1, "{p:?}");
    assert_bpm(&row(&s, id), 128.0);
    // So is a track whose analysis a scan cleared.
    s.lib
        .writer()
        .submit_wait(WriteOp::ClearAnalysis(id))
        .unwrap();
    let p = analyze(&s, &[id], false);
    assert_eq!(p.done, 1, "{p:?}");
    assert_bpm(&row(&s, id), 128.0);
}

#[test]
fn force_reanalyzes() {
    let s = setup();
    let id = add_track(&s, "a.wav", 128.0, 0.25);
    analyze(&s, &[id], false);
    set_sentinel_bpm(&s, id, i64::from(ANALYZER_VERSION));
    let p = analyze(&s, &[id], true);
    assert_eq!((p.done, p.skipped), (1, 0), "{p:?}");
    assert_bpm(&row(&s, id), 128.0);
}

#[test]
fn locked_grid_survives_reanalysis() {
    let s = setup();
    let id = add_track(&s, "a.wav", 128.0, 0.25);
    analyze(&s, &[id], false);
    raw(
        &s.lib,
        "UPDATE analysis SET grid_locked = 1, bpm = 127.5, first_beat_frames = 1234.0, \
         key = NULL, waveform = NULL WHERE track_id = ?1",
        vec![Value::Integer(id)],
    );
    let p = analyze(&s, &[id], true);
    assert_eq!(p.done, 1, "{p:?}");
    let r = row(&s, id);
    assert!(r.grid_locked);
    assert_eq!(r.bpm, Some(127.5));
    assert_eq!(r.first_beat_frames, Some(1234.0));
    assert!(r.key.is_some(), "the key is still updated");
    assert!(r.waveform.is_some(), "the waveform is still updated");
}

#[test]
fn missing_file_fails_without_touching_analysis() {
    let s = setup();
    let id = add_track(&s, "a.wav", 128.0, 0.25);
    analyze(&s, &[id], false);
    let before = row(&s, id);

    // Deleted from disk but not yet flagged by a scan.
    fs::remove_file(s.music.join("a.wav")).unwrap();
    let p = analyze(&s, &[id], true);
    assert_eq!(p.done, 0, "{p:?}");
    assert_eq!(p.failed.len(), 1, "{p:?}");
    assert_eq!(p.failed[0].0, id);
    assert!(p.failed[0].1.contains("not found"), "{p:?}");
    assert_eq!(row(&s, id), before);

    // Flagged missing by a scan.
    s.lib
        .writer()
        .submit_wait(WriteOp::MarkMissing(vec![id]))
        .unwrap();
    let p = analyze(&s, &[id], true);
    assert_eq!(p.failed.len(), 1, "{p:?}");
    assert!(p.failed[0].1.contains("missing"), "{p:?}");
    assert_eq!(row(&s, id), before);
}

#[test]
fn unknown_track_fails() {
    let s = setup();
    let p = analyze(&s, &[4242], false);
    assert_eq!(p.failed.len(), 1, "{p:?}");
    assert_eq!(p.failed[0].0, 4242);
}

#[test]
fn bpm_range_setting_is_used() {
    let s = setup();
    let id = add_track(&s, "a.wav", 128.0, 0.25);
    s.lib
        .writer()
        .submit_wait(WriteOp::SetSetting {
            key: "bpm_range".into(),
            value: Some("60-120".into()),
        })
        .unwrap();
    assert_eq!(
        s.lib.setting("bpm_range").unwrap().as_deref(),
        Some("60-120")
    );
    let p = analyze(&s, &[id], false);
    assert!(p.warnings.is_empty(), "{p:?}");
    // 128 BPM folds to 64 inside 60–120.
    assert_bpm(&row(&s, id), 64.0);
}

#[test]
fn invalid_bpm_range_falls_back_and_is_reported() {
    let s = setup();
    let id = add_track(&s, "a.wav", 128.0, 0.25);
    s.lib
        .writer()
        .submit_wait(WriteOp::SetSetting {
            key: "bpm_range".into(),
            value: Some("180-90".into()),
        })
        .unwrap();
    let p = analyze(&s, &[id], false);
    assert_eq!(p.done, 1, "{p:?}");
    assert_eq!(
        p.warnings,
        vec![AnalysisWarning::InvalidBpmRange {
            value: "180-90".into(),
            error: BpmRangeError::NotAscending,
        }]
    );
    assert_bpm(&row(&s, id), 128.0);
}

#[test]
fn settings_round_trip_and_delete() {
    let s = setup();
    assert_eq!(s.lib.setting("key_notation").unwrap(), None);
    let set = |value: Option<&str>| {
        s.lib
            .writer()
            .submit_wait(WriteOp::SetSetting {
                key: "key_notation".into(),
                value: value.map(Into::into),
            })
            .unwrap();
    };
    set(Some("camelot"));
    assert_eq!(
        s.lib.setting("key_notation").unwrap().as_deref(),
        Some("camelot")
    );
    set(Some("open_key"));
    assert_eq!(
        s.lib.setting("key_notation").unwrap().as_deref(),
        Some("open_key")
    );
    set(None);
    assert_eq!(s.lib.setting("key_notation").unwrap(), None);
}

#[test]
fn bpm_range_parsing_is_strict() {
    let r = parse_bpm_range("90-180").unwrap();
    assert_eq!((r.min, r.max), (90.0, 180.0));
    let r = parse_bpm_range(" 70.5 - 150 ").unwrap();
    assert_eq!((r.min, r.max), (70.5, 150.0));
    for bad in [
        "",
        "90",
        "90-",
        "-180",
        "a-b",
        "90-180-200",
        "NaN-180",
        "90-inf",
    ] {
        assert_eq!(parse_bpm_range(bad), Err(BpmRangeError::Format), "{bad:?}");
    }
    assert_eq!(parse_bpm_range("20-180"), Err(BpmRangeError::OutOfBounds));
    assert_eq!(parse_bpm_range("90-500"), Err(BpmRangeError::OutOfBounds));
    assert_eq!(parse_bpm_range("180-90"), Err(BpmRangeError::NotAscending));
    assert_eq!(parse_bpm_range("90-90"), Err(BpmRangeError::NotAscending));
    assert_eq!(parse_bpm_range("100-180"), Err(BpmRangeError::TooNarrow));
}

#[test]
fn low_bpm_ranges_get_a_warning() {
    let (range, warning) = validate_bpm_range("90-180").unwrap();
    assert_eq!((range.min, range.max), (90.0, 180.0));
    assert_eq!(warning, None);
    let (_, warning) = validate_bpm_range("70-140").unwrap();
    assert_eq!(warning, None);
    let (_, warning) = validate_bpm_range("60-120").unwrap();
    assert!(warning.unwrap().contains("half tempo"));
    assert_eq!(validate_bpm_range("60-100"), Err(BpmRangeError::TooNarrow));
}

#[test]
fn duplicate_enqueues_run_once() {
    let s = setup();
    let id = add_track(&s, "a.wav", 128.0, 0.25);
    let analyzer = Analyzer::start(&s.lib, 1).unwrap();
    analyzer.enqueue(&[id, id, id], false);
    let p = wait(&analyzer);
    assert_eq!(p.done + p.skipped, 1, "{p:?}");
}

#[test]
fn drop_stops_without_draining_the_queue() {
    let s = setup();
    let ids: Vec<i64> = (0..4)
        .map(|i| add_track(&s, &format!("{i}.wav"), 120.0 + f64::from(i), 0.1))
        .collect();
    let analyzer = Analyzer::start(&s.lib, 1).unwrap();
    analyzer.enqueue(&ids, false);
    let deadline = Instant::now() + Duration::from_secs(120);
    while analyzer.progress().done == 0 {
        assert!(Instant::now() < deadline, "no track finished");
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(analyzer);
    let analyzed = ids
        .iter()
        .filter(|&&id| s.lib.analysis(id).unwrap().is_some())
        .count();
    assert!(analyzed < ids.len(), "the whole queue was drained on drop");
}

#[test]
fn truncated_file_is_analyzed_with_a_warning() {
    let s = setup();
    let path = s.music.join("cut.wav");
    write_wav(&path, &clicks(128.0, 0.25));
    // Claim twice the audio that is there, as a file cut short in a copy would.
    let mut bytes = fs::read(&path).unwrap();
    let data_len = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
    bytes[40..44].copy_from_slice(&(data_len * 2).to_le_bytes());
    bytes[4..8].copy_from_slice(&(36 + data_len * 2).to_le_bytes());
    fs::write(&path, bytes).unwrap();
    scan_folder(&s.lib, &s.music, &mut |_: ScanProgress| {}).unwrap();
    let id = track_id(&s.lib, &path);
    let p = analyze(&s, &[id], false);
    assert_eq!(p.done, 1, "{p:?}");
    assert!(p.failed.is_empty(), "{p:?}");
    assert_eq!(
        p.warnings,
        vec![AnalysisWarning::Truncated { track_id: id }]
    );
    assert_bpm(&row(&s, id), 128.0);
}

#[test]
fn stale_analysis_is_not_stored() {
    let s = setup();
    let id = add_track(&s, "a.wav", 128.0, 0.25);
    let op = |fingerprint: Vec<u8>, track_id: i64| {
        WriteOp::StoreAnalysis(Box::new(StoreAnalysis {
            track_id,
            fingerprint,
            analyzer_version: ANALYZER_VERSION,
            bpm: Some(120.0),
            first_beat_frames: Some(0.0),
            key: Some(3),
            waveform: Vec::new(),
        }))
    };
    let err = s.lib.writer().submit_wait(op(vec![0; 32], id)).unwrap_err();
    assert!(
        matches!(err, LibraryError::TrackChanged(t) if t == id),
        "{err}"
    );
    assert_eq!(s.lib.analysis(id).unwrap(), None);
    let err = s
        .lib
        .writer()
        .submit_wait(op(vec![0; 32], 4242))
        .unwrap_err();
    assert!(matches!(err, LibraryError::NoSuchTrack(4242)), "{err}");
}

#[test]
fn failures_are_capped_but_counted() {
    let s = setup();
    let analyzer = Analyzer::start(&s.lib, 1).unwrap();
    let extra = 5;
    let ids: Vec<i64> = (0..i64::try_from(REPORT_CAPACITY + extra).unwrap())
        .map(|i| 100_000 + i)
        .collect();
    analyzer.enqueue(&ids, false);
    let p = wait(&analyzer);
    assert_eq!(p.failed_count, ids.len());
    // Progress copies only a short tail: the latest failures.
    assert_eq!(p.failed.len(), PROGRESS_TAIL);
    assert_eq!(p.failed.last().unwrap().0, *ids.last().unwrap());
    // The kept failures are the most recent REPORT_CAPACITY, oldest first.
    let kept = analyzer.take_failures();
    assert_eq!(kept.len(), REPORT_CAPACITY);
    assert_eq!(kept[0].0, ids[extra]);
    assert_eq!(kept.last().unwrap().0, *ids.last().unwrap());
    assert!(analyzer.take_failures().is_empty());
    let p = analyzer.progress();
    assert!(p.failed.is_empty());
    assert_eq!(p.failed_count, ids.len(), "the total survives taking");
}

#[test]
fn warnings_can_be_taken() {
    let s = setup();
    let id = add_track(&s, "a.wav", 128.0, 0.25);
    s.lib
        .writer()
        .submit_wait(WriteOp::SetSetting {
            key: "bpm_range".into(),
            value: Some("nonsense".into()),
        })
        .unwrap();
    let analyzer = Analyzer::start(&s.lib, 1).unwrap();
    analyzer.enqueue(&[id], false);
    let p = wait(&analyzer);
    assert_eq!(p.warning_count, 1, "{p:?}");
    let taken = analyzer.take_warnings();
    assert!(
        matches!(&taken[..], [AnalysisWarning::InvalidBpmRange { value, .. }] if value == "nonsense"),
        "{taken:?}"
    );
    let p = analyzer.progress();
    assert!(p.warnings.is_empty());
    assert_eq!(p.warning_count, 1);
}

#[test]
fn store_analysis_keeps_a_locked_grid() {
    let s = setup();
    let id = add_track(&s, "a.wav", 128.0, 0.25);
    raw(
        &s.lib,
        "INSERT INTO analysis(track_id, analyzer_version, bpm, first_beat_frames, grid_locked) \
         VALUES (?1, 0, 127.5, 1234.0, 1)",
        vec![Value::Integer(id)],
    );
    let fingerprint: Vec<u8> = s
        .lib
        .reader()
        .unwrap()
        .connection()
        .query_row("SELECT fingerprint FROM tracks WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    let waveform = analysis::Waveform {
        frames_per_bucket: 1024,
        buckets: vec![[1, 2, 3, 4]],
    };
    s.lib
        .writer()
        .submit_wait(WriteOp::StoreAnalysis(Box::new(StoreAnalysis {
            track_id: id,
            fingerprint,
            analyzer_version: ANALYZER_VERSION,
            bpm: Some(120.0),
            first_beat_frames: Some(0.0),
            key: Some(3),
            waveform: waveform.to_bytes(),
        })))
        .unwrap();
    let r = row(&s, id);
    assert!(r.grid_locked);
    assert_eq!((r.bpm, r.first_beat_frames), (Some(127.5), Some(1234.0)));
    assert_eq!(r.key.map(analysis::Key::index), Some(3));
    assert_eq!(r.waveform, Some(waveform));
    assert_eq!(r.analyzer_version, i64::from(ANALYZER_VERSION));
}

/// Writes a click track whose header declares `SECS` but holds only the first `keep`
/// fraction of the audio, as a file read while still being copied.
fn write_partial_wav(path: &Path, keep: f64) {
    write_wav(path, &clicks(128.0, 0.25));
    let mut bytes = fs::read(path).unwrap();
    let data_len = bytes.len() - 44;
    let kept = ((data_len as f64 * keep) as usize) / 4 * 4;
    bytes.truncate(44 + kept);
    fs::write(path, bytes).unwrap();
}

#[test]
fn truncated_analysis_is_stored_as_out_of_date() {
    let s = setup();
    let path = s.music.join("copying.wav");
    write_partial_wav(&path, 0.5);
    scan_folder(&s.lib, &s.music, &mut |_: ScanProgress| {}).unwrap();
    let id = track_id(&s.lib, &path);
    let p = analyze(&s, &[id], false);
    assert_eq!(p.done, 1, "{p:?}");
    assert_eq!(
        p.warnings,
        vec![AnalysisWarning::Truncated { track_id: id }]
    );
    assert_eq!(row(&s, id).analyzer_version, 0, "retried next time");
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
fn a_copy_completed_after_its_first_scan_is_reanalyzed_whole() {
    let s = setup();
    let path = s.music.join("copying.wav");
    write_partial_wav(&path, 0.5);
    scan_folder(&s.lib, &s.music, &mut |_: ScanProgress| {}).unwrap();
    let id = track_id(&s.lib, &path);
    analyze(&s, &[id], false);

    // The copy completes: same first seconds (same fingerprint), new size.
    write_wav(&path, &clicks(128.0, 0.25));
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(10))
        .unwrap();
    let report = scan_folder(&s.lib, &s.music, &mut |_: ScanProgress| {}).unwrap();
    assert_eq!((report.updated, report.added), (1, 0), "{report:?}");
    assert_eq!(track_id(&s.lib, &path), id);
    let p = analyze(&s, &[id], false);
    assert_eq!(p.done, 1, "{p:?}");
    assert!(p.warnings.is_empty(), "{p:?}");
    let r = row(&s, id);
    assert_eq!(r.analyzer_version, i64::from(ANALYZER_VERSION));
    let frames = (SECS * f64::from(SR)) as usize;
    assert_eq!(
        r.waveform.expect("a waveform").buckets.len(),
        frames.div_ceil(1024),
        "the waveform covers the whole file"
    );
}

#[test]
fn max_active_limits_and_pauses_the_workers() {
    let s = setup();
    let ids: Vec<i64> = (0..4)
        .map(|i| add_track(&s, &format!("{i}.wav"), 120.0 + f64::from(i), 0.1))
        .collect();
    let analyzer = Analyzer::start(&s.lib, 3).unwrap();
    assert_eq!(analyzer.max_active(), usize::MAX);

    // Paused: nothing starts.
    analyzer.set_max_active(0);
    analyzer.enqueue(&ids, false);
    std::thread::sleep(Duration::from_millis(300));
    let p = analyzer.progress();
    assert_eq!((p.active, p.done, p.queued), (0, 0, 4), "{p:?}");

    // One at a time.
    analyzer.set_max_active(1);
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut seen_active = false;
    loop {
        let p = analyzer.progress();
        assert!(p.active <= 1, "{p:?}");
        seen_active |= p.active == 1;
        if p.done >= 2 {
            break;
        }
        assert!(Instant::now() < deadline, "{p:?}");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(seen_active);

    // Unlimited again: the rest finishes.
    analyzer.set_max_active(usize::MAX);
    let p = wait(&analyzer);
    assert_eq!(p.done, 4, "{p:?}");
}
