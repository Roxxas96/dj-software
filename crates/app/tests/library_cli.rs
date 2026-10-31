//! End to end: `library add-folder`, `analyze` and `list` on the built binary against a
//! temporary library, with a synthetic 128 BPM track.

use std::f64::consts::TAU;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const SR: u32 = 44_100;
const SECS: f64 = 30.0;

/// Kick-like clicks (a decaying 60 Hz burst plus a short transient) at `bpm`, as 16-bit
/// mono samples.
fn clicks(bpm: f64) -> Vec<i16> {
    let sr = f64::from(SR);
    let n = (SECS * sr) as usize;
    let mut out = vec![0.0f64; n];
    let period = 60.0 / bpm * sr;
    let burst = (0.25 * sr) as usize;
    let transient = (0.003 * sr) as usize;
    let mut beat = 0.0;
    loop {
        let start = (0.25 * sr + beat * period).round() as usize;
        if start >= n {
            break;
        }
        for k in 0..burst.min(n - start) {
            let t = k as f64 / sr;
            let mut v = (TAU * 60.0 * t).sin() * (-t / 0.05).exp();
            if k < transient {
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

fn run(db: &Path, args: &[&str]) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_dj-software"))
        .arg("library")
        .args(args)
        .arg("--library-db")
        .arg(db)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "library {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn library_cli_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("music");
    fs::create_dir_all(&music).unwrap();
    write_wav(&music.join("click.wav"), &clicks(128.0));
    let db = dir.path().join("db").join("library.sqlite3");
    let music_arg = music.to_str().unwrap();

    let added = stdout(&run(&db, &["add-folder", music_arg]));
    assert!(added.contains("added 1"), "{added}");

    let analyzed = stdout(&run(&db, &["analyze"]));
    assert!(analyzed.contains("analyzed 1"), "{analyzed}");

    let listed = stdout(&run(&db, &["list"]));
    assert!(listed.contains("128.00"), "{listed}");
    assert!(listed.contains("click"), "{listed}");

    // A second analyze finds nothing left to do.
    let again = stdout(&run(&db, &["analyze"]));
    assert!(again.contains("nothing to analyze"), "{again}");
}
