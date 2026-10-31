#[path = "support/synth.rs"]
mod synth;

use analysis::{Beatgrid, BpmRange, detect_beatgrid};

const SR: u32 = 44_100;
const SECS: f32 = 30.0;

fn detect(mono: &[f32], sr: u32) -> Option<Beatgrid> {
    detect_beatgrid(&synth::stereo(mono, sr), BpmRange::default())
}

fn assert_bpm(grid: &Beatgrid, expected: f64, tol: f64) {
    assert!(
        (grid.bpm - expected).abs() <= tol,
        "expected {expected} ± {tol} BPM, got {}",
        grid.bpm
    );
}

#[test]
fn default_range_is_90_to_180() {
    let r = BpmRange::default();
    assert_eq!((r.min, r.max), (90.0, 180.0));
}

#[test]
fn detects_128_bpm_and_phase() {
    let grid = detect(&synth::clicks(128.0, SECS, SR, 0.37), SR).expect("a grid");
    assert_bpm(&grid, 128.0, 0.02);
    let first_secs = grid.first_beat_frames / f64::from(SR);
    assert!(
        (first_secs - 0.37).abs() <= 0.010,
        "first beat at {first_secs} s, expected 0.37 s"
    );
}

#[test]
fn detects_174_bpm() {
    // 48 kHz also exercises resampling from a rate other than 44.1 kHz.
    let sr = 48_000;
    let grid = detect(&synth::clicks(174.0, SECS, sr, 0.1), sr).expect("a grid");
    assert_bpm(&grid, 174.0, 0.02);
    let first_secs = grid.first_beat_frames / f64::from(sr);
    assert!(
        (first_secs - 0.1).abs() <= 0.010,
        "first beat at {first_secs} s"
    );
}

#[test]
fn dnb_174_not_reported_as_87() {
    let kicks = synth::clicks(174.0, SECS, SR, 0.2);
    let snare_times = synth::beat_times(174.0, SECS, 0.2, 2.0, 1.0);
    let snares = synth::place(&synth::snare_hit(SR, 7), &snare_times, SECS, SR);
    let grid = detect(&synth::mix(&kicks, &snares), SR).expect("a grid");
    assert_bpm(&grid, 174.0, 0.02);
}

#[test]
fn dubstep_140_half_time_reports_140() {
    let (bpm, first) = (140.0, 0.25);
    let kick = synth::place(
        &synth::clicks(60.0, 0.25, SR, 0.0),
        &synth::beat_times(bpm, SECS, first, 4.0, 0.0),
        SECS,
        SR,
    );
    let snare = synth::place(
        &synth::snare_hit(SR, 11),
        &synth::beat_times(bpm, SECS, first, 4.0, 2.0),
        SECS,
        SR,
    );
    let hats = synth::place(
        &synth::hat_hit(SR, 13),
        &synth::beat_times(bpm, SECS, first, 0.5, 0.0),
        SECS,
        SR,
    );
    let mono = synth::mix(&synth::mix(&kick, &snare), &hats);
    let grid = detect(&mono, SR).expect("a grid");
    assert_bpm(&grid, 140.0, 0.02);
    let first_secs = grid.first_beat_frames / f64::from(SR);
    // Any beat of the grid is acceptable as the first one, so compare modulo one beat.
    let period = 60.0 / bpm;
    let off = (first_secs - first).rem_euclid(period);
    let off = off.min(period - off);
    assert!(off <= 0.010, "grid is {off} s off the beat");
}

#[test]
fn non_integer_bpm_not_snapped() {
    let grid = detect(&synth::clicks(123.5, SECS, SR, 0.05), SR).expect("a grid");
    assert_bpm(&grid, 123.5, 0.03);
}

#[test]
fn silence_returns_none() {
    assert_eq!(detect(&synth::silence(SECS, SR), SR), None);
}

#[test]
fn short_track_returns_none() {
    assert_eq!(detect(&synth::clicks(128.0, 9.5, SR, 0.0), SR), None);
}

#[test]
fn range_folds_out_of_range_candidates() {
    let grid = detect(&synth::clicks(75.0, SECS, SR, 0.0), SR).expect("a grid");
    assert_bpm(&grid, 150.0, 0.02);
}

#[test]
fn noise_does_not_panic() {
    let _ = detect(&synth::noise(SECS, SR, 42, 0.5), SR);
}

#[test]
fn absurd_sample_rates_return_none() {
    let low = 4_000;
    assert_eq!(detect(&synth::clicks(128.0, SECS, low, 0.0), low), None);
    let high = 1_000_000;
    assert_eq!(detect(&synth::silence(11.0, high), high), None);
}

#[test]
fn invalid_ranges_return_none() {
    let t = synth::stereo(&synth::clicks(128.0, SECS, SR, 0.0), SR);
    for (min, max) in [
        (0.0, 180.0),
        (140.0, 120.0),
        (f64::NAN, 180.0),
        (90.0, f64::INFINITY),
        (1.0e6, 2.0e6),
    ] {
        assert_eq!(
            detect_beatgrid(&t, BpmRange { min, max }),
            None,
            "{min}-{max}"
        );
    }
}

#[test]
fn first_beat_is_earliest_non_negative() {
    // First audible beat at 1.3 s: the grid extends back to the beat at 1.3 - 2 * 0.46875 s.
    let grid = detect(&synth::clicks(128.0, SECS, SR, 1.3), SR).expect("a grid");
    let period = 60.0 * f64::from(SR) / grid.bpm;
    assert!(grid.first_beat_frames >= 0.0 && grid.first_beat_frames < period);
    let expected = 1.3 - 2.0 * 60.0 / 128.0;
    let first_secs = grid.first_beat_frames / f64::from(SR);
    assert!(
        (first_secs - expected).abs() <= 0.010,
        "first beat at {first_secs} s"
    );
}

/// Manual check against real files: `DJ_ANALYSIS_DIR=~/Music cargo nextest run -p analysis
/// --release --run-ignored only analyze_real_files_from_env --no-capture`.
#[test]
#[ignore = "needs DJ_ANALYSIS_DIR"]
fn analyze_real_files_from_env() {
    let Some(dir) = std::env::var_os("DJ_ANALYSIS_DIR") else {
        eprintln!("DJ_ANALYSIS_DIR is not set; nothing to analyze");
        return;
    };
    let mut stack = vec![std::path::PathBuf::from(dir)];
    let mut files = Vec::new();
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|x| x.to_str()).is_some_and(|x| {
                ["mp3", "flac", "wav", "aiff", "aif", "m4a", "mp4", "ogg"]
                    .contains(&x.to_ascii_lowercase().as_str())
            }) {
                files.push(p);
            }
        }
    }
    files.sort();
    println!("path bpm first_beat key");
    for p in files {
        match decode::decode_file(&p) {
            Ok(d) => {
                let sr = f64::from(d.track.sample_rate());
                match detect_beatgrid(&d.track, BpmRange::default()) {
                    Some(g) => println!(
                        "{} {:.2} {:.3} -",
                        p.display(),
                        g.bpm,
                        g.first_beat_frames / sr
                    ),
                    None => println!("{} - - -", p.display()),
                }
            }
            Err(e) => println!("{} error: {e}", p.display()),
        }
    }
}
