use std::path::PathBuf;

use decode::{DecodeError, decode_file, decode_prefix};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn peak(buf: &engine::TrackBuffer) -> f32 {
    (0..buf.frames())
        .flat_map(|i| buf.frame(i))
        .fold(0.0, |m, s| m.max(s.abs()))
}

/// Estimates the frequency from rising zero crossings of the left channel.
fn frequency(buf: &engine::TrackBuffer) -> f64 {
    let left: Vec<f32> = (0..buf.frames()).map(|i| buf.frame(i)[0]).collect();
    let crossings: Vec<usize> = (1..left.len())
        .filter(|&i| left[i - 1] < 0.0 && left[i] >= 0.0)
        .collect();
    let (first, last) = (crossings[0], crossings[crossings.len() - 1]);
    (crossings.len() - 1) as f64 * f64::from(buf.sample_rate()) / (last - first) as f64
}

#[test]
fn decodes_every_supported_format() {
    for (ext, lossless) in [
        ("wav", true),
        ("aiff", true),
        ("flac", true),
        ("mp3", false),
        ("m4a", false),
        ("ogg", false),
    ] {
        let buf = decode_file(&fixture(&format!("sine440.{ext}")))
            .unwrap()
            .track;
        let (frames, p, f) = (buf.frames(), peak(&buf), frequency(&buf));
        println!(
            "{ext}: rate={} frames={frames} peak={p} freq={f}",
            buf.sample_rate()
        );
        assert_eq!(buf.sample_rate(), 44_100, "{ext}");
        if lossless {
            assert_eq!(frames, 44_100, "{ext}");
        } else {
            assert!(frames.abs_diff(44_100) <= 2_048, "{ext}: {frames} frames");
        }
        assert!((0.45..=0.55).contains(&p), "{ext}: peak {p}");
        assert!((f - 440.0).abs() <= 5.0, "{ext}: freq {f}");
    }
}

#[test]
fn mono_file_is_duplicated_to_both_channels() {
    let buf = decode_file(&fixture("sine440_mono.wav")).unwrap().track;
    assert!(buf.frames() > 0);
    for i in 0..buf.frames() {
        let [l, r] = buf.frame(i);
        assert_eq!(l, r, "frame {i}");
    }
    assert!((peak(&buf) - 0.5).abs() < 0.05);
}

#[test]
fn corrupt_file_is_an_error() {
    let err = decode_file(&fixture("corrupt.mp3")).unwrap_err();
    assert!(
        matches!(err, DecodeError::Unsupported(_) | DecodeError::Corrupt(_)),
        "{err:?}"
    );
}

#[test]
fn truncated_file_decodes_what_it_can() {
    let buf = decode_file(&fixture("truncated.mp3")).unwrap().track;
    assert!(
        buf.frames() > 0 && buf.frames() < 44_100,
        "{} frames",
        buf.frames()
    );
    assert!(
        (0..buf.frames())
            .flat_map(|i| buf.frame(i))
            .all(f32::is_finite)
    );
}

#[test]
fn missing_file_is_an_error() {
    let err = decode_file(&fixture("nope.wav")).unwrap_err();
    assert!(matches!(err, DecodeError::Io(_)), "{err:?}");
}

#[test]
fn non_audio_file_is_unsupported() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let err = decode_file(&path).unwrap_err();
    assert!(matches!(err, DecodeError::Unsupported(_)), "{err:?}");
}

#[test]
fn decode_prefix_is_the_start_of_the_full_decode() {
    for ext in ["wav", "aiff", "flac", "mp3", "m4a", "ogg"] {
        let path = fixture(&format!("sine440.{ext}"));
        let full = decode_file(&path).unwrap().track;
        for max in [1, 1_000, 10_000] {
            let prefix = decode_prefix(&path, max).unwrap();
            assert_eq!(prefix.sample_rate(), full.sample_rate(), "{ext}");
            assert_eq!(prefix.frames(), max, "{ext}");
            for i in 0..max {
                assert_eq!(prefix.frame(i), full.frame(i), "{ext} frame {i}");
            }
        }
        // A limit past the end returns the whole file.
        assert_eq!(decode_prefix(&path, 10_000_000).unwrap(), full, "{ext}");
    }
    assert!(decode_prefix(&fixture("corrupt.mp3"), 1_000).is_err());
}

#[test]
fn big_endian_16_bit_aiff_decodes() {
    let buf = decode_file(&fixture("sine440_s16be.aiff")).unwrap().track;
    assert_eq!(buf.sample_rate(), 44_100);
    assert_eq!(buf.frames(), 44_100);
    assert!((peak(&buf) - 0.5).abs() < 0.05);
    assert!((frequency(&buf) - 440.0).abs() <= 5.0);
}

#[test]
fn twenty_four_bit_wav_decodes() {
    let buf = decode_file(&fixture("sine440_s24.wav")).unwrap().track;
    assert_eq!(buf.sample_rate(), 44_100);
    assert_eq!(buf.frames(), 44_100);
    assert!((peak(&buf) - 0.5).abs() < 0.05);
    assert!((frequency(&buf) - 440.0).abs() <= 5.0);
}

#[test]
fn truncated_file_reports_truncated() {
    assert!(decode_file(&fixture("truncated.mp3")).unwrap().truncated);
    assert!(!decode_file(&fixture("sine440.mp3")).unwrap().truncated);
}
