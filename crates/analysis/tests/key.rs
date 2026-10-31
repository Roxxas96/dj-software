#[path = "support/synth.rs"]
mod synth;

use analysis::{Key, KeyNotation, detect_key};

const SR: u32 = 44_100;
const CHORD_SECS: f32 = 4.0;
const PARTIALS: u32 = 6;

/// One harmonic triad per MIDI-note list, `CHORD_SECS` each, concatenated.
fn progression(chords: &[[f64; 3]], sr: u32) -> Vec<f32> {
    let parts: Vec<Vec<f32>> = chords
        .iter()
        .map(|notes| {
            let hz: Vec<f64> = notes.iter().map(|&m| synth::note_hz(m)).collect();
            synth::chord_harmonic(&hz, CHORD_SECS, sr, PARTIALS)
        })
        .collect();
    let refs: Vec<&[f32]> = parts.iter().map(Vec::as_slice).collect();
    synth::concat(&refs)
}

/// I–IV–V–I in C major (C4 = MIDI 60), shifted by `semitones`.
fn c_major_cadence(semitones: f64) -> Vec<[f64; 3]> {
    [
        [60.0, 64.0, 67.0], // C
        [60.0, 65.0, 69.0], // F (second inversion)
        [59.0, 62.0, 67.0], // G (first inversion)
        [60.0, 64.0, 67.0], // C
    ]
    .iter()
    .map(|c| c.map(|m| m + semitones))
    .collect()
}

/// i–iv–V–i in a harmonic minor key whose tonic is MIDI `tonic` (V is a major triad).
fn minor_cadence(tonic: f64) -> Vec<[f64; 3]> {
    [
        [0.0, 3.0, 7.0],  // i
        [0.0, 5.0, 8.0],  // iv (second inversion)
        [-1.0, 2.0, 7.0], // V (first inversion, leading tone in the bass)
        [0.0, 3.0, 7.0],  // i
    ]
    .iter()
    .map(|c| c.map(|m| m + tonic))
    .collect()
}

fn detect(mono: &[f32], sr: u32) -> Option<Key> {
    detect_key(&synth::stereo(mono, sr))
}

fn assert_camelot(mono: &[f32], sr: u32, expected: &str) {
    let key = detect(mono, sr).expect("a key");
    assert_eq!(key.camelot(), expected, "detected {}", key.standard());
}

#[test]
fn notation_tables() {
    // (index, standard, Camelot, Open Key), from the published Camelot and Open Key wheels.
    let rows = [
        (0, "C", "8B", "1d"),
        (1, "C♯", "3B", "8d"),
        (2, "D", "10B", "3d"),
        (3, "D♯", "5B", "10d"),
        (4, "E", "12B", "5d"),
        (5, "F", "7B", "12d"),
        (6, "F♯", "2B", "7d"),
        (7, "G", "9B", "2d"),
        (8, "G♯", "4B", "9d"),
        (9, "A", "11B", "4d"),
        (10, "A♯", "6B", "11d"),
        (11, "B", "1B", "6d"),
        (12, "Cm", "5A", "10m"),
        (13, "C♯m", "12A", "5m"),
        (14, "Dm", "7A", "12m"),
        (15, "D♯m", "2A", "7m"),
        (16, "Em", "9A", "2m"),
        (17, "Fm", "4A", "9m"),
        (18, "F♯m", "11A", "4m"),
        (19, "Gm", "6A", "11m"),
        (20, "G♯m", "1A", "6m"),
        (21, "Am", "8A", "1m"),
        (22, "A♯m", "3A", "8m"),
        (23, "Bm", "10A", "3m"),
    ];
    for (i, standard, camelot, open_key) in rows {
        let key = Key::new(i).expect("valid index");
        assert_eq!(key.index(), i);
        assert_eq!(key.is_minor(), i >= 12, "{standard}");
        assert_eq!(key.standard(), standard);
        assert_eq!(key.camelot(), camelot, "{standard}");
        assert_eq!(key.open_key(), open_key, "{standard}");
        assert_eq!(key.to_notation(KeyNotation::Standard), standard);
        assert_eq!(key.to_notation(KeyNotation::Camelot), camelot);
        assert_eq!(key.to_notation(KeyNotation::OpenKey), open_key);
    }
    assert_eq!(Key::new(24), None);
    assert_eq!(KeyNotation::default(), KeyNotation::Camelot);
}

#[test]
fn c_major_progression_is_8b() {
    assert_camelot(&progression(&c_major_cadence(0.0), SR), SR, "8B");
}

#[test]
fn a_minor_progression_is_8a() {
    assert_camelot(&progression(&minor_cadence(57.0), SR), SR, "8A");
}

#[test]
fn f_sharp_minor_is_11a() {
    // 48 kHz also exercises resampling from a rate other than 44.1 kHz.
    let sr = 48_000;
    assert_camelot(&progression(&minor_cadence(54.0), sr), sr, "11A");
}

#[test]
fn silence_returns_none() {
    assert_eq!(detect(&synth::silence(10.0, SR), SR), None);
    // Below the -60 dBFS frame gate.
    let quiet = synth::gain(&progression(&c_major_cadence(0.0), SR), 1.0e-4);
    assert_eq!(detect(&quiet, SR), None);
}

#[test]
fn transposition_moves_key() {
    // The C-major cadence up a fifth is G major.
    assert_camelot(&progression(&c_major_cadence(7.0), SR), SR, "9B");
}

#[test]
fn absurd_sample_rates_return_none() {
    let low = 4_000;
    assert_eq!(detect(&progression(&c_major_cadence(0.0), low), low), None);
    let high = 1_000_000;
    assert_eq!(detect(&synth::silence(1.0, high), high), None);
}

#[test]
fn noise_does_not_panic() {
    let _ = detect(&synth::noise(10.0, SR, 42, 0.5), SR);
}

#[test]
fn very_short_track_does_not_panic() {
    let _ = detect(&synth::sine(440.0, 0.01, SR), SR);
    assert_eq!(detect(&[], SR), None);
}
