//! `play --config` with a bad file fails with the key named, before any audio device or
//! MIDI port is opened.

use std::process::Command;

#[test]
fn play_rejects_a_bad_config_before_opening_devices() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "[beat_jump]\npads = [1, 2, 3]\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_open-jockey"))
        .args(["play", "track-that-does-not-exist.wav", "--config"])
        .arg(&config)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("config.toml: beat_jump.pads: expected exactly 8 numbers, got 3"),
        "{stderr}"
    );
    assert!(!stderr.contains("decoding"), "{stderr}");
}
