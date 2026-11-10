//! How library tracks, keys and beatgrids are shown, shared by `library` and `play
//! --library`.

use std::borrow::Cow;

use analysis::{Key, KeyNotation};
use library::{Beatgrid, TrackRow};

/// `text` with every control character (C0 such as ESC and BEL, DEL, and C1
/// U+0080–U+009F) replaced by U+FFFD, so tags and file names from downloaded files cannot
/// send escape sequences to the terminal. Other Unicode is kept.
pub fn sanitize_for_terminal(text: &str) -> Cow<'_, str> {
    if text.chars().any(char::is_control) {
        Cow::Owned(
            text.chars()
                .map(|c| if c.is_control() { '\u{FFFD}' } else { c })
                .collect(),
        )
    } else {
        Cow::Borrowed(text)
    }
}

/// `value` as text with every control character replaced, line breaks included: for
/// untrusted text (tags, paths, error messages naming them) inside a message.
pub fn safe(value: impl std::fmt::Display) -> String {
    sanitize_for_terminal(&value.to_string()).into_owned()
}

/// [`sanitize_for_terminal`] on each line of `text`, keeping the line breaks. Only for
/// text whose line breaks are ours; untrusted parts go through [`safe`] first.
pub fn sanitize_lines(text: &str) -> Cow<'_, str> {
    if !text.chars().any(|c| c.is_control() && c != '\n') {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.split('\n')
            .map(sanitize_for_terminal)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// The `key_notation` setting value for each notation.
pub fn notation_setting(notation: KeyNotation) -> &'static str {
    match notation {
        KeyNotation::Camelot => "camelot",
        KeyNotation::Standard => "standard",
        KeyNotation::OpenKey => "open_key",
    }
}

/// The notation a `key_notation` setting names (Camelot when unset or unknown).
pub fn notation_from_setting(value: Option<&str>) -> KeyNotation {
    match value {
        Some("standard") => KeyNotation::Standard,
        Some("open_key" | "open-key") => KeyNotation::OpenKey,
        _ => KeyNotation::Camelot,
    }
}

/// A stored key (0..=23) in `notation`, or empty when unknown.
pub fn key_text(key: Option<u8>, notation: KeyNotation) -> String {
    key.and_then(Key::new)
        .map(|k| k.to_notation(notation))
        .unwrap_or_default()
}

/// "Artist – Title", or just the title without an artist.
pub fn track_name(row: &TrackRow) -> String {
    let title = row.title.as_deref().unwrap_or("(untitled)");
    let name = match row.artist.as_deref().filter(|a| !a.is_empty()) {
        Some(artist) => format!("{artist} – {title}"),
        None => title.to_owned(),
    };
    sanitize_for_terminal(&name).into_owned()
}

/// A BPM with two decimals, such as "128.00".
pub fn bpm_text(bpm: f64) -> String {
    format!("{bpm:.2}")
}

/// A track's length as "m:ss" (or "h:mm:ss"), or empty when unknown.
pub fn length_text(row: &TrackRow) -> String {
    match (row.duration_frames, row.sample_rate) {
        (Some(frames), Some(rate)) if rate > 0 => duration_text(frames / u64::from(rate)),
        _ => String::new(),
    }
}

fn duration_text(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Whether a track is long enough (over 30 minutes) to be a DJ mix, whose tempo drifts
/// away from one constant beatgrid.
pub fn is_long_mix(row: &TrackRow) -> bool {
    match (row.duration_frames, row.sample_rate) {
        (Some(frames), Some(rate)) if rate > 0 => frames / u64::from(rate) > 30 * 60,
        _ => false,
    }
}

/// A beatgrid as "128.00 BPM, first beat at 0.250 s, locked". `sample_rate` turns the
/// first beat into seconds; without it the frame is shown.
pub fn grid_text(grid: &Beatgrid, sample_rate: Option<u32>) -> String {
    let bpm = grid
        .bpm
        .map_or_else(|| "no BPM".to_owned(), |b| format!("{} BPM", bpm_text(b)));
    let first = match (grid.first_beat_frames, sample_rate) {
        (Some(frames), Some(rate)) if rate > 0 => {
            format!(", first beat at {:.3} s", frames / f64::from(rate))
        }
        (Some(frames), _) => format!(", first beat at frame {frames:.0}"),
        (None, _) => String::new(),
    };
    let lock = if grid.grid_locked {
        "locked"
    } else {
        "unlocked"
    };
    format!("{bpm}{first}, {lock}")
}

/// A size in bytes as megabytes of 1,000,000 bytes, like "12.3 MB".
pub fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

/// Cuts `text` to at most `max` characters, ending with "…" when cut.
pub fn truncate(text: &str, max: usize) -> String {
    let text = sanitize_for_terminal(text);
    if text.chars().count() <= max {
        return text.into_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> TrackRow {
        TrackRow {
            id: 1,
            title: Some("Title".into()),
            artist: Some("Artist".into()),
            bpm: Some(128.0),
            key: Some(21),
            duration_frames: Some(44_100 * 200),
            sample_rate: Some(44_100),
            has_cues: false,
            missing: false,
        }
    }

    #[test]
    fn formats_tracks_keys_and_grids() {
        assert_eq!(track_name(&row()), "Artist – Title");
        let untagged = TrackRow {
            artist: None,
            ..row()
        };
        assert_eq!(track_name(&untagged), "Title");
        assert_eq!(length_text(&row()), "3:20");
        assert_eq!(key_text(Some(21), KeyNotation::Camelot), "8A");
        assert_eq!(key_text(Some(21), KeyNotation::Standard), "Am");
        assert_eq!(key_text(Some(21), KeyNotation::OpenKey), "1m");
        assert_eq!(key_text(None, KeyNotation::Camelot), "");
        for n in [
            KeyNotation::Camelot,
            KeyNotation::Standard,
            KeyNotation::OpenKey,
        ] {
            assert_eq!(notation_from_setting(Some(notation_setting(n))), n);
        }
        assert_eq!(notation_from_setting(None), KeyNotation::Camelot);
        let grid = Beatgrid {
            bpm: Some(128.0),
            first_beat_frames: Some(11_025.0),
            grid_locked: true,
        };
        assert_eq!(
            grid_text(&grid, Some(44_100)),
            "128.00 BPM, first beat at 0.250 s, locked"
        );
        let mix = TrackRow {
            duration_frames: Some(44_100 * 3600),
            ..row()
        };
        assert!(is_long_mix(&mix) && !is_long_mix(&row()));
        assert_eq!(length_text(&mix), "1:00:00");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }

    #[test]
    fn control_characters_are_replaced() {
        let evil = "Ti\u{1b}]0;pwned\u{7}tle\u{1b}[2J\u{9b}31m";
        let clean = sanitize_for_terminal(evil);
        assert!(!clean.chars().any(char::is_control), "{clean:?}");
        assert_eq!(clean, "Ti\u{FFFD}]0;pwned\u{FFFD}tle\u{FFFD}[2J\u{FFFD}31m");
        let fine = "Café – Señorita 🎧 日本";
        assert!(matches!(sanitize_for_terminal(fine), Cow::Borrowed(t) if t == fine));
        assert_eq!(sanitize_lines("a\u{1b}b\nc\u{7}"), "a\u{FFFD}b\nc\u{FFFD}");
        assert_eq!(sanitize_lines("line 1\nline 2"), "line 1\nline 2");
        let row = TrackRow {
            artist: Some("A\u{1b}[31m".into()),
            ..row()
        };
        assert_eq!(track_name(&row), "A\u{FFFD}[31m – Title");
        assert_eq!(truncate("x\u{7}y", 10), "x\u{FFFD}y");
        // Untrusted line breaks would break the status line count: replaced too.
        assert_eq!(safe("a\nb\rc"), "a\u{FFFD}b\u{FFFD}c");
        let row = TrackRow {
            title: Some("two\nlines\r".into()),
            artist: None,
            ..row
        };
        assert_eq!(track_name(&row), "two\u{FFFD}lines\u{FFFD}");
    }
}
