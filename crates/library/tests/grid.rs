//! Beatgrid editing: shift, downbeat, halve/double, set BPM, tap tempo and locking.

use library::{GridEdit, Library, RawStatement, StoreAnalysis, WriteOp};
use rusqlite::types::Value;

/// 120 BPM at 44.1 kHz: one beat every 22050 frames.
const SR: i64 = 44_100;
const PERIOD: f64 = 22_050.0;

struct Setup {
    _dir: tempfile::TempDir,
    lib: Library,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(&dir.path().join("library.sqlite3")).unwrap();
    Setup { _dir: dir, lib }
}

fn raw(lib: &Library, sql: &str, params: Vec<Value>) {
    lib.writer()
        .submit_wait(WriteOp::Raw(vec![RawStatement {
            sql: sql.into(),
            params,
        }]))
        .unwrap();
}

/// Adds track `id` at 44.1 kHz with fingerprint `[id; 32]`.
fn add_track(lib: &Library, id: i64) {
    raw(
        lib,
        "INSERT INTO tracks(id, path, fingerprint, sample_rate, added_at) \
         VALUES (?1, ?2, ?3, ?4, 0)",
        vec![
            Value::Integer(id),
            Value::Text(format!("/music/{id}.wav")),
            Value::Blob(vec![u8::try_from(id).unwrap(); 32]),
            Value::Integer(SR),
        ],
    );
}

/// Stores an unlocked analysis with this grid for track `id`.
fn analyze(lib: &Library, id: i64, bpm: f64, first_beat_frames: f64) {
    lib.writer()
        .submit_wait(WriteOp::StoreAnalysis(Box::new(StoreAnalysis {
            track_id: id,
            fingerprint: vec![u8::try_from(id).unwrap(); 32],
            analyzer_version: library::ANALYZER_VERSION,
            bpm: Some(bpm),
            first_beat_frames: Some(first_beat_frames),
            key: Some(3),
            waveform: analysis::Waveform {
                frames_per_bucket: 1024,
                buckets: vec![[1, 2, 3, 4]],
            }
            .to_bytes(),
        })))
        .unwrap();
}

/// A library with track 1 analyzed at 120 BPM, first beat at `first`.
fn analyzed(first: f64) -> Setup {
    let s = setup();
    add_track(&s.lib, 1);
    analyze(&s.lib, 1, 120.0, first);
    s
}

fn grid(lib: &Library, id: i64) -> (Option<f64>, Option<f64>, bool) {
    let row = lib.analysis(id).unwrap().unwrap();
    (row.bpm, row.first_beat_frames, row.grid_locked)
}

#[test]
fn shift_wraps_into_one_period() {
    let s = analyzed(1000.0);
    let row = s
        .lib
        .edit_grid(1, GridEdit::Shift { frames: -2000.0 }, None)
        .unwrap();
    assert_eq!(row.first_beat_frames, Some(21_050.0));
    assert_eq!(row.bpm, Some(120.0));
    let row = s
        .lib
        .edit_grid(1, GridEdit::Shift { frames: 1450.0 }, None)
        .unwrap();
    // 21050 + 1450 = 22500, one period past 450.
    assert_eq!(row.first_beat_frames, Some(450.0));
    assert_eq!(grid(&s.lib, 1), (Some(120.0), Some(450.0), true));
}

#[test]
fn set_downbeat_puts_a_beat_exactly_there() {
    let s = analyzed(1000.0);
    let row = s
        .lib
        .edit_grid(
            1,
            GridEdit::SetDownbeat {
                at_frames: 100_000.0,
            },
            None,
        )
        .unwrap();
    // 100000 - 4 * 22050.
    assert_eq!(row.first_beat_frames, Some(11_800.0));
    assert_eq!(row.bpm, Some(120.0));
}

#[test]
fn halve_keeps_the_beat_at_the_playhead() {
    let s = analyzed(1000.0);
    // The beat nearest the playhead is 1000 + 22050 = 23050.
    let row = s.lib.edit_grid(1, GridEdit::Halve, Some(23_000.0)).unwrap();
    assert_eq!(row.bpm, Some(60.0));
    assert_eq!(row.first_beat_frames, Some(23_050.0));
}

#[test]
fn halve_without_playhead_keeps_the_first_beat() {
    let s = analyzed(1000.0);
    let row = s.lib.edit_grid(1, GridEdit::Halve, None).unwrap();
    assert_eq!(row.bpm, Some(60.0));
    assert_eq!(row.first_beat_frames, Some(1000.0));
}

#[test]
fn double_keeps_the_first_beat() {
    let s = analyzed(12_000.0);
    let row = s.lib.edit_grid(1, GridEdit::Double, None).unwrap();
    assert_eq!(row.bpm, Some(240.0));
    // 12000 wrapped into the new 11025-frame period: still a beat at 12000.
    assert_eq!(row.first_beat_frames, Some(975.0));
}

#[test]
fn double_keeps_the_beat_at_the_playhead() {
    let s = analyzed(1000.0);
    let row = s
        .lib
        .edit_grid(1, GridEdit::Double, Some(45_000.0))
        .unwrap();
    assert_eq!(row.bpm, Some(240.0));
    // The beat nearest the playhead (45100) stays a beat: 45100 - 4 * 11025 = 1000.
    let first = row.first_beat_frames.unwrap();
    assert_eq!(first, 1000.0);
    assert_eq!((45_100.0 - first) % (PERIOD / 2.0), 0.0);
}

#[test]
fn set_bpm_keeps_the_first_beat_and_checks_the_range() {
    let s = analyzed(1000.0);
    let row = s.lib.edit_grid(1, GridEdit::SetBpm(128.0), None).unwrap();
    assert_eq!(row.bpm, Some(128.0));
    assert_eq!(row.first_beat_frames, Some(1000.0));
    for ok in [40.0, 300.0] {
        assert_eq!(
            s.lib.edit_grid(1, GridEdit::SetBpm(ok), None).unwrap().bpm,
            Some(ok)
        );
    }
    for bad in [39.99, 300.01, 0.0, -120.0, f64::NAN, f64::INFINITY] {
        let err = s.lib.edit_grid(1, GridEdit::SetBpm(bad), None).unwrap_err();
        assert!(err.to_string().contains("40"), "{bad}: {err}");
    }
    // Rejected edits change nothing.
    assert_eq!(grid(&s.lib, 1), (Some(300.0), Some(1000.0), true));
}

#[test]
fn set_bpm_creates_the_analysis_row() {
    let s = setup();
    add_track(&s.lib, 1);
    let row = s.lib.edit_grid(1, GridEdit::SetBpm(124.0), None).unwrap();
    assert_eq!(row.bpm, Some(124.0));
    assert_eq!(row.first_beat_frames, Some(0.0));
    assert!(row.grid_locked);
    assert_eq!(row.analyzer_version, 0, "never analyzed");
    assert_eq!(row.key, None);
    assert_eq!(row.waveform, None);
}

#[test]
fn tap_tempo_from_four_taps() {
    let s = analyzed(1000.0);
    let tap = |at_micros| {
        s.lib
            .edit_grid(1, GridEdit::Tap { at_micros }, None)
            .unwrap()
    };
    // Three taps are not enough: nothing changes, the grid is not locked.
    for at in [0, 480_000, 960_000] {
        assert_eq!(tap(at).bpm, Some(120.0));
    }
    assert_eq!(grid(&s.lib, 1), (Some(120.0), Some(1000.0), false));
    // The fourth tap: mean interval 480 ms = 125 BPM.
    let row = tap(1_440_000);
    assert_eq!(row.bpm, Some(125.0));
    assert_eq!(row.first_beat_frames, Some(1000.0));
    assert!(row.grid_locked);
    // A fifth tap 470 ms later: (1910000 - 0) / 4 = 477.5 ms = 125.65445... -> 125.65.
    assert_eq!(tap(1_910_000).bpm, Some(125.65));
}

#[test]
fn tap_restarts_after_a_two_second_gap() {
    let s = analyzed(1000.0);
    let tap = |at_micros| {
        s.lib
            .edit_grid(1, GridEdit::Tap { at_micros }, None)
            .unwrap()
            .bpm
    };
    for at in [0, 500_000, 1_000_000] {
        tap(at);
    }
    // 2.1 s later: a new sequence starts, so this and the next two taps are not enough.
    for at in [3_100_000, 3_700_000, 4_300_000] {
        assert_eq!(tap(at), Some(120.0));
    }
    // Four taps 600 ms apart: 100 BPM (the old taps at 500 ms would give more).
    assert_eq!(tap(4_900_000), Some(100.0));
    // Exactly 2 s is not a gap: (6.9 - 3.1) / 4 = 950 ms = 63.157... -> 63.16 BPM.
    assert_eq!(tap(6_900_000), Some(63.16));
}

#[test]
fn taps_are_per_track() {
    let s = analyzed(1000.0);
    add_track(&s.lib, 2);
    analyze(&s.lib, 2, 120.0, 0.0);
    for (i, at) in [0, 500_000, 1_000_000].into_iter().enumerate() {
        s.lib
            .edit_grid(1, GridEdit::Tap { at_micros: at }, None)
            .unwrap();
        s.lib
            .edit_grid(
                2,
                GridEdit::Tap {
                    at_micros: at + 250_000,
                },
                None,
            )
            .unwrap();
        assert_eq!(grid(&s.lib, 2).0, Some(120.0), "tap {i}");
    }
    let row = s
        .lib
        .edit_grid(
            1,
            GridEdit::Tap {
                at_micros: 1_500_000,
            },
            None,
        )
        .unwrap();
    assert_eq!(row.bpm, Some(120.0));
    assert!(row.grid_locked);
    assert!(!grid(&s.lib, 2).2, "track 2 has had only three taps");
}

#[test]
fn edits_lock_the_grid() {
    let edits = [
        GridEdit::Shift { frames: 10.0 },
        GridEdit::SetDownbeat { at_frames: 5.0 },
        GridEdit::Halve,
        GridEdit::Double,
        GridEdit::SetBpm(126.0),
    ];
    for edit in edits {
        let s = analyzed(1000.0);
        assert!(!grid(&s.lib, 1).2);
        let row = s.lib.edit_grid(1, edit, None).unwrap();
        assert!(row.grid_locked, "{edit:?}");
        // Re-analysis keeps the edited grid.
        analyze(&s.lib, 1, 90.0, 7.0);
        let after = s.lib.analysis(1).unwrap().unwrap();
        assert_eq!(
            (after.bpm, after.first_beat_frames, after.grid_locked),
            (row.bpm, row.first_beat_frames, true),
            "{edit:?}"
        );
    }
}

#[test]
fn toggle_lock_flips() {
    let s = analyzed(1000.0);
    let row = s.lib.edit_grid(1, GridEdit::ToggleLock, None).unwrap();
    assert!(row.grid_locked);
    assert_eq!(row.bpm, Some(120.0));
    assert_eq!(row.first_beat_frames, Some(1000.0));
    let row = s.lib.edit_grid(1, GridEdit::ToggleLock, None).unwrap();
    assert!(!row.grid_locked);
    // Unlocked again: re-analysis replaces the grid.
    analyze(&s.lib, 1, 90.0, 7.0);
    assert_eq!(grid(&s.lib, 1), (Some(90.0), Some(7.0), false));
}

#[test]
fn shift_without_bpm_errors() {
    let s = setup();
    add_track(&s.lib, 1);
    for edit in [
        GridEdit::Shift { frames: 10.0 },
        GridEdit::SetDownbeat { at_frames: 10.0 },
        GridEdit::Halve,
        GridEdit::Double,
    ] {
        let err = s.lib.edit_grid(1, edit, None).unwrap_err();
        assert!(
            err.to_string().contains("analyze or set a BPM first"),
            "{edit:?}: {err}"
        );
    }
    // Nothing was stored.
    assert_eq!(s.lib.analysis(1).unwrap(), None);
}

#[test]
fn edits_on_a_missing_track_error() {
    let s = setup();
    let err = s
        .lib
        .edit_grid(9, GridEdit::SetBpm(120.0), None)
        .unwrap_err();
    assert!(
        matches!(err, library::LibraryError::NoSuchTrack(9)),
        "{err}"
    );
}
