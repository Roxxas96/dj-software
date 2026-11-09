//! Where each app shows a position that Open Jockey stores in frames.

use super::{ExportTrack, FileKind};

/// A DJ app an export is written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum App {
    Serato,
    Rekordbox,
    VirtualDj,
}

/// Frames Serato adds to an MP3 that has an Info frame: one MP3 frame (1152 samples).
pub const SERATO_INFO_FRAME_FRAMES: f64 = 1152.0;

/// Seconds `app` shows for Open Jockey's `frames` in `t`. Open Jockey's frame 0 is the first
/// sample after the gapless trim (an MP3's Info frame and encoder delay are skipped).
/// Starting values, calibrated by the plan 11c hands-on:
/// - VirtualDJ: the same seconds (the 11a import lined up with no offset);
/// - Rekordbox: the same seconds;
/// - Serato: an MP3 with an Info frame is one MP3 frame later (`SERATO_INFO_FRAME_FRAMES`),
///   the 26 ms shift users report between Serato and other apps.
#[must_use]
pub fn app_seconds(app: App, frames: f64, t: &ExportTrack) -> f64 {
    let rate = f64::from(t.sample_rate);
    let shift = match app {
        App::Serato if t.kind == FileKind::Mp3 && t.lead_in.gapless_header => {
            SERATO_INFO_FRAME_FRAMES
        }
        _ => 0.0,
    };
    (frames + shift) / rate
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use decode::LeadIn;

    use super::*;

    fn track(kind: FileKind, gapless_header: bool) -> ExportTrack {
        ExportTrack {
            id: 1,
            path: PathBuf::from("a"),
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
            kind,
            lead_in: LeadIn {
                gapless_header,
                delay_frames: 0,
            },
        }
    }

    #[test]
    fn virtualdj_and_rekordbox_are_plain_seconds() {
        let t = track(FileKind::Mp3, true);
        for app in [App::VirtualDj, App::Rekordbox] {
            assert!((app_seconds(app, 44_100.0, &t) - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn serato_mp3_with_header_is_one_frame_later() {
        let t = track(FileKind::Mp3, true);
        let want = 1.0 + 1152.0 / 44_100.0;
        assert!((app_seconds(App::Serato, 44_100.0, &t) - want).abs() < 1e-9);
    }

    #[test]
    fn serato_wav_has_no_offset() {
        let t = track(FileKind::Wav, false);
        assert!((app_seconds(App::Serato, 44_100.0, &t) - 1.0).abs() < 1e-9);
    }
}
