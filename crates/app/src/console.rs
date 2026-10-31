//! The `play` console: lines typed on stdin while playing (only when stdin is a terminal).
//!
//! A blank line reconnects the controller. With `--library`, `grid` edits the beatgrid
//! of a deck's track through the same path as the controller's grid buttons, and `load`
//! loads a library track by id.

use std::sync::mpsc::{Receiver, channel};

use actions::{GRID_SHIFT_MILLIS, GridEditRequest};
use engine::DeckId;

/// What `help` prints.
pub const HELP: &str = "\
console commands:
  <Enter>                      reconnect the controller
  grid <deck> back|forward     move the beatgrid 5 ms earlier or later
  grid <deck> downbeat         put a beat at the playhead
  grid <deck> halve|double     halve or double the BPM
  grid <deck> tap              tap the tempo (4 taps or more set the BPM)
  grid <deck> lock             lock or unlock the grid
  load <deck> <track-id>       load a library track (ids: `dj-software library list`)
  help                         show this help
decks are 1 to 4; grid and load need `play --library`";

/// A parsed console line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleCommand {
    /// A blank line.
    Reconnect,
    Grid {
        deck: DeckId,
        edit: GridEditRequest,
    },
    Load {
        deck: DeckId,
        track: i64,
    },
    Help,
}

/// Parses one console line. `now_micros` is when it was typed (on
/// `controller::clock::now_micros`), the time of a `tap`.
///
/// # Errors
///
/// A message for the user when the line is not a command.
pub fn parse(line: &str, now_micros: u64) -> Result<ConsoleCommand, String> {
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.as_slice() {
        [] => Ok(ConsoleCommand::Reconnect),
        ["help" | "?"] => Ok(ConsoleCommand::Help),
        ["grid", deck, edit] => {
            let deck = parse_deck(deck)?;
            let edit = match *edit {
                "back" => GridEditRequest::Shift {
                    millis: -GRID_SHIFT_MILLIS,
                },
                "forward" => GridEditRequest::Shift {
                    millis: GRID_SHIFT_MILLIS,
                },
                "downbeat" => GridEditRequest::SetDownbeat,
                "halve" => GridEditRequest::Halve,
                "double" => GridEditRequest::Double,
                "tap" => GridEditRequest::Tap {
                    at_micros: now_micros,
                },
                "lock" => GridEditRequest::ToggleLock,
                other => {
                    return Err(format!(
                        "unknown grid edit `{other}`: use back, forward, downbeat, halve, \
                         double, tap or lock"
                    ));
                }
            };
            Ok(ConsoleCommand::Grid { deck, edit })
        }
        ["grid", ..] => {
            Err("usage: grid <deck> back|forward|downbeat|halve|double|tap|lock".into())
        }
        ["load", deck, track] => {
            let deck = parse_deck(deck)?;
            let track = track
                .parse()
                .map_err(|_| format!("`{track}` is not a track id"))?;
            Ok(ConsoleCommand::Load { deck, track })
        }
        ["load", ..] => Err("usage: load <deck> <track-id>".into()),
        [other, ..] => Err(format!("unknown command `{other}`; type `help`")),
    }
}

fn parse_deck(text: &str) -> Result<DeckId, String> {
    text.parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(DeckId::new)
        .ok_or_else(|| format!("`{text}` is not a deck: use 1 to 4"))
}

/// Sends every line typed on stdin, with the time it was read, from a helper thread so
/// the control loop never blocks on the terminal. Stops at end of input.
pub fn spawn_reader() -> Receiver<(String, u64)> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            match stdin.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let at = controller::clock::now_micros();
                    if tx.send((line.trim().to_owned(), at)).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deck(n: usize) -> DeckId {
        DeckId::new(n - 1).unwrap()
    }

    #[test]
    fn console_parses_grid_and_load() {
        assert_eq!(parse("", 0), Ok(ConsoleCommand::Reconnect));
        assert_eq!(parse("   ", 0), Ok(ConsoleCommand::Reconnect));
        assert_eq!(parse("help", 0), Ok(ConsoleCommand::Help));
        let grid = |line: &str| match parse(line, 42).unwrap() {
            ConsoleCommand::Grid { deck: d, edit } => (d, edit),
            other => panic!("{line}: {other:?}"),
        };
        assert_eq!(
            grid("grid 1 back"),
            (deck(1), GridEditRequest::Shift { millis: -5 })
        );
        assert_eq!(
            grid("grid 2 forward"),
            (deck(2), GridEditRequest::Shift { millis: 5 })
        );
        assert_eq!(grid("grid 3 downbeat").1, GridEditRequest::SetDownbeat);
        assert_eq!(grid("grid 4 halve").1, GridEditRequest::Halve);
        assert_eq!(grid("grid 1 double").1, GridEditRequest::Double);
        assert_eq!(
            grid("  grid 1   tap "),
            (deck(1), GridEditRequest::Tap { at_micros: 42 })
        );
        assert_eq!(grid("grid 1 lock").1, GridEditRequest::ToggleLock);
        assert_eq!(
            parse("load 3 17", 0),
            Ok(ConsoleCommand::Load {
                deck: deck(3),
                track: 17
            })
        );
    }

    #[test]
    fn console_rejects_unknown() {
        for line in [
            "play",
            "grid",
            "grid 1",
            "grid 0 tap",
            "grid 5 tap",
            "grid one tap",
            "grid 1 wobble",
            "load 1",
            "load 1 abc",
            "load 9 1",
            "load 1 2 3",
        ] {
            assert!(parse(line, 0).is_err(), "{line} should be rejected");
        }
        assert!(parse("play", 0).unwrap_err().contains("help"));
    }
}
