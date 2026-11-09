//! Serato's cue tags (`Serato Markers2`, `Serato BeatGrid`) and its colour palette.

use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD;

use super::super::ExportTrack;
use super::super::timing::{App, app_seconds};

/// The colours Serato offers for a cue.
pub const SERATO_CUE_COLORS: [u32; 18] = [
    0xCC0000, 0xCC4400, 0xCC8800, 0xCCCC00, 0x88CC00, 0x44CC00, 0x00CC00, 0x00CC44, 0x00CC88,
    0x00CCCC, 0x0088CC, 0x0044CC, 0x0000CC, 0x4400CC, 0x8800CC, 0xCC00CC, 0xCC0088, 0xCC0044,
];

/// The colour Serato gives a hotcue slot (1 to 8) that has none of its own.
pub const SLOT_DEFAULT_COLORS: [u32; 8] = [
    0xCC0000, 0xCC8800, 0x0000CC, 0xCCCC00, 0x00CC00, 0xCC00CC, 0x00CCCC, 0x8800CC,
];

/// The colour Serato draws a saved loop in.
pub const LOOP_COLOR: [u8; 4] = [0x00, 0x27, 0xAA, 0xE1];

/// Longest cue name, in bytes without the terminator (the field holds 0x33 with it).
const MAX_NAME_BYTES: usize = 50;

/// Hue in degrees and saturation of `rgb` (0xRRGGBB).
fn hue_sat(rgb: u32) -> (f64, f64) {
    let r = f64::from((rgb >> 16) & 0xFF) / 255.0;
    let g = f64::from((rgb >> 8) & 0xFF) / 255.0;
    let b = f64::from(rgb & 0xFF) / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    if max <= 0.0 || d <= 0.0 {
        return (0.0, 0.0);
    }
    let h = if (max - r).abs() < f64::EPSILON {
        ((g - b) / d).rem_euclid(6.0)
    } else if (max - g).abs() < f64::EPSILON {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, d / max)
}

/// The Serato colour nearest `color` by hue; grey, black or no colour gives the slot's default.
#[must_use]
pub fn serato_color(color: Option<u32>, slot: u8) -> u32 {
    let default = SLOT_DEFAULT_COLORS[usize::from(slot.clamp(1, 8) - 1)];
    let Some(c) = color else {
        return default;
    };
    let (hue, sat) = hue_sat(c);
    if sat < 0.15 {
        return default;
    }
    let mut best = SERATO_CUE_COLORS[0];
    let mut best_dist = f64::MAX;
    for p in SERATO_CUE_COLORS {
        let diff = (hue - hue_sat(p).0).abs();
        let dist = diff.min(360.0 - diff);
        // Strictly smaller, so a tie keeps the earlier entry.
        if dist < best_dist {
            best_dist = dist;
            best = p;
        }
    }
    best
}

fn entry(name: &str, data: &[u8]) -> Vec<u8> {
    let mut out = name.as_bytes().to_vec();
    out.push(0);
    out.extend_from_slice(&u32::try_from(data.len()).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(data);
    out
}

fn ms(t: &ExportTrack, frames: f64) -> u32 {
    let v = (app_seconds(App::Serato, frames, t) * 1000.0)
        .round()
        .max(0.0);
    // Saturating float-to-int conversion is what we want for absurd positions.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let v = v as u32;
    v
}

fn name_bytes(name: Option<&str>) -> Vec<u8> {
    let name = name.unwrap_or("");
    let mut end = name.len().min(MAX_NAME_BYTES);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = name.as_bytes()[..end].to_vec();
    out.push(0);
    out
}

fn cue_entry(
    t: &ExportTrack,
    slot: u8,
    frames: f64,
    color: Option<u32>,
    name: Option<&str>,
) -> Vec<u8> {
    let rgb = serato_color(color, slot).to_be_bytes();
    let mut d = vec![0, slot - 1];
    d.extend_from_slice(&ms(t, frames).to_be_bytes());
    d.push(0);
    d.extend_from_slice(&rgb[1..]);
    d.extend_from_slice(&[0, 0]);
    d.extend_from_slice(&name_bytes(name));
    entry("CUE", &d)
}

fn loop_entry(t: &ExportTrack, slot: u8, start: f64, end: f64, name: Option<&str>) -> Vec<u8> {
    let mut d = vec![0, slot - 1];
    d.extend_from_slice(&ms(t, start).to_be_bytes());
    d.extend_from_slice(&ms(t, end).to_be_bytes());
    d.extend_from_slice(&[0xFF; 4]);
    d.extend_from_slice(&LOOP_COLOR);
    d.extend_from_slice(&[0, 0]);
    d.extend_from_slice(&name_bytes(name));
    entry("LOOP", &d)
}

/// The decoded content of the `Serato Markers2` tag for `t`.
///
/// A slot holds a loop or a cue, never both: a hotcue with a loop length becomes a `LOOP`.
#[must_use]
pub fn markers2_payload(t: &ExportTrack) -> Vec<u8> {
    let mut out = vec![0x01, 0x01];
    out.extend(entry("COLOR", &[0x00, 0xFF, 0xFF, 0xFF]));
    for (i, hc) in t.hotcues.iter().enumerate() {
        if let Some(h) = hc.as_ref().filter(|h| h.loop_frames.is_none()) {
            let slot = u8::try_from(i + 1).unwrap_or(8);
            out.extend(cue_entry(t, slot, h.frames, h.color, h.name.as_deref()));
        }
    }
    for (i, hc) in t.hotcues.iter().enumerate() {
        if let Some((h, len)) = hc.as_ref().and_then(|h| Some((h, h.loop_frames?))) {
            let slot = u8::try_from(i + 1).unwrap_or(8);
            out.extend(loop_entry(
                t,
                slot,
                h.frames,
                h.frames + len,
                h.name.as_deref(),
            ));
        }
    }
    out.extend(entry("BPMLOCK", &[0x00]));
    out.push(0);
    out
}

/// The data of the `Serato Markers2` GEOB frame: base64 in 72-character lines, zero padded.
#[must_use]
pub fn markers2_geob(t: &ExportTrack) -> Vec<u8> {
    let b64 = STANDARD_NO_PAD.encode(markers2_payload(t));
    let mut out = vec![0x01, 0x01];
    for (i, chunk) in b64.as_bytes().chunks(72).enumerate() {
        if i > 0 {
            out.push(b'\n');
        }
        out.extend_from_slice(chunk);
    }
    if out.len() < 470 {
        out.resize(470, 0);
    }
    out
}

/// The data of the `Serato BeatGrid` GEOB frame; `None` without a BPM or a first beat.
#[must_use]
pub fn beatgrid_geob(t: &ExportTrack) -> Option<Vec<u8>> {
    let bpm = t.bpm?;
    let first = t.first_beat_frames?;
    let mut out = vec![0x01, 0x00, 0, 0, 0, 1];
    #[allow(clippy::cast_possible_truncation)]
    {
        out.extend_from_slice(&(app_seconds(App::Serato, first, t) as f32).to_be_bytes());
        out.extend_from_slice(&(bpm as f32).to_be_bytes());
    }
    out.push(0);
    Some(out)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use decode::LeadIn;
    use library::HotcuePoint;

    use super::super::super::FileKind;
    use super::*;

    fn track() -> ExportTrack {
        ExportTrack {
            id: 1,
            path: PathBuf::from("a.wav"),
            fingerprint: [0; 32],
            size: 0,
            title: "a".into(),
            artist: None,
            album: None,
            genre: None,
            sample_rate: 44_100,
            duration_frames: None,
            bpm: None,
            first_beat_frames: None,
            key: None,
            cue_frames: None,
            hotcues: [const { None }; 8],
            kind: FileKind::Wav,
            lead_in: LeadIn {
                gapless_header: false,
                delay_frames: 0,
            },
        }
    }

    fn point(frames: f64, loop_frames: Option<f64>, name: Option<&str>) -> HotcuePoint {
        HotcuePoint {
            frames,
            loop_frames,
            color: Some(0xCC0000),
            name: name.map(str::to_string),
        }
    }

    #[test]
    fn cue_entry_bytes() {
        let t = track();
        let got = cue_entry(&t, 1, 44_100.0, Some(0xCC0000), Some("Drop"));
        let mut want = b"CUE\0".to_vec();
        want.extend([0, 0, 0, 0x11]);
        want.extend([
            0, 0, 0, 0, 0x03, 0xE8, 0, 0xCC, 0, 0, 0, 0, 0x44, 0x72, 0x6F, 0x70, 0,
        ]);
        assert_eq!(got, want);
    }

    #[test]
    fn loop_entry_bytes() {
        let t = track();
        let got = loop_entry(&t, 1, 88_200.0, 176_400.0, None);
        let mut want = b"LOOP\0".to_vec();
        want.extend([0, 0, 0, 0x15]);
        want.extend([
            0, 0, 0, 0, 0x07, 0xD0, 0, 0, 0x0F, 0xA0, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x27, 0xAA,
            0xE1, 0, 0, 0,
        ]);
        assert_eq!(got, want);
    }

    #[test]
    fn payload_order_and_terminator() {
        let mut t = track();
        t.hotcues[0] = Some(point(0.0, None, None));
        t.hotcues[1] = Some(point(44_100.0, Some(44_100.0), None));
        let p = markers2_payload(&t);
        assert_eq!(
            p[..16],
            [
                0x01, 0x01, 0x43, 0x4F, 0x4C, 0x4F, 0x52, 0, 0, 0, 0, 4, 0, 0xFF, 0xFF, 0xFF
            ]
        );
        assert_eq!(
            p[p.len() - 14..],
            [
                0x42, 0x50, 0x4D, 0x4C, 0x4F, 0x43, 0x4B, 0, 0, 0, 0, 1, 0, 0
            ]
        );
        let cue = p.windows(4).position(|w| w == b"CUE\0").unwrap();
        let lp = p.windows(5).position(|w| w == b"LOOP\0").unwrap();
        assert!(cue < lp);
    }

    #[test]
    fn geob_is_padded_and_wrapped() {
        let mut t = track();
        for i in 0..8 {
            t.hotcues[i] = Some(point(
                f64::from(i as u32) * 44_100.0,
                None,
                Some("A cue name"),
            ));
        }
        let g = markers2_geob(&t);
        assert!(g.len() >= 470);
        assert_eq!(g[..2], [1, 1]);
        let body: Vec<u8> = g[2..].iter().copied().take_while(|b| *b != 0).collect();
        let text = String::from_utf8(body).unwrap();
        assert!(!text.contains('='));
        assert!(text.split('\n').all(|l| l.len() <= 72));
        assert!(text.contains('\n'));
        let mut joined: String = text.split('\n').collect();
        while !joined.len().is_multiple_of(4) {
            joined.push('A');
        }
        let back = base64::engine::general_purpose::STANDARD
            .decode(joined)
            .unwrap();
        assert_eq!(
            &back[..markers2_payload(&t).len()],
            markers2_payload(&t).as_slice()
        );
    }

    #[test]
    fn beatgrid_bytes() {
        let mut t = track();
        assert_eq!(beatgrid_geob(&t), None);
        t.bpm = Some(120.0);
        t.first_beat_frames = Some(22_050.0);
        let mut want = vec![1, 0, 0, 0, 0, 1];
        want.extend(0.5f32.to_be_bytes());
        want.extend(120f32.to_be_bytes());
        want.push(0);
        assert_eq!(beatgrid_geob(&t), Some(want));
    }

    #[test]
    fn serato_color_snaps() {
        assert_eq!(serato_color(Some(0xFF0000), 1), 0xCC0000);
        assert_eq!(serato_color(Some(0x0000FF), 1), 0x0000CC);
        assert_eq!(serato_color(None, 3), 0x0000CC);
        assert_eq!(serato_color(Some(0x808080), 2), 0xCC8800);
    }

    #[test]
    fn long_names_are_cut_on_a_char_boundary() {
        let name = "é".repeat(40);
        let b = name_bytes(Some(&name));
        assert_eq!(b.len(), 51);
        assert!(std::str::from_utf8(&b[..50]).is_ok());
        assert_eq!(b[50], 0);
    }
}
