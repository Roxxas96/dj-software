use std::path::{Path, PathBuf};
use std::process::Command;

use decode::{LeadIn, lead_in};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Writes a 16-bit mono 44.1 kHz WAV of silence, `frames` long.
fn write_wav(path: &Path, frames: u32) {
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
    bytes.resize(bytes.len() + data_len as usize, 0);
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn wav_has_no_lead_in() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("a.wav");
    write_wav(&wav, 4_410);
    assert_eq!(lead_in(&wav).unwrap(), LeadIn::default());
    assert_eq!(lead_in(&fixture("sine440.wav")).unwrap(), LeadIn::default());
}

#[test]
fn lame_mp3_reports_its_delay() {
    let dir = tempfile::tempdir().unwrap();
    let mp3 = dir.path().join("tone.mp3");
    let made = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2",
        ])
        .args(["-c:a", "libmp3lame", "-y"])
        .arg(&mp3)
        .status();
    if !matches!(made, Ok(s) if s.success()) {
        println!("skipped: ffmpeg with libmp3lame is not available");
        return;
    }
    let lead = lead_in(&mp3).unwrap();
    assert!(lead.gapless_header);
    assert!(lead.delay_frames > 0);
}
