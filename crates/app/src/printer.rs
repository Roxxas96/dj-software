//! Prints on its own thread, so a slow or blocked stdout never stalls the control loop;
//! or, for the window, forwards what it would print.

use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};

use crate::format::{sanitize_for_terminal, sanitize_lines};

/// Status lines are cut to this many characters on a terminal, so they do not wrap (a
/// wrapped line would break redrawing them in place).
const STATUS_WIDTH: usize = 110;

/// The printer thread's handle. Status updates replace the status lines in place and at
/// most one waits in the queue (a newer one is dropped while one is pending: the next
/// tick sends a fresh one); events, such as warnings, are never dropped.
pub struct Printer {
    tx: Sender<Line>,
    status_pending: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<()>,
}

/// What the printer is asked to show.
pub enum Line {
    /// Replaces the status lines.
    Status(Vec<String>),
    /// A message on its own line, above the status lines.
    Event(String),
}

impl Printer {
    pub fn spawn() -> Self {
        let (tx, rx) = channel::<Line>();
        let status_pending = Arc::new(AtomicBool::new(false));
        let pending = Arc::clone(&status_pending);
        let thread = std::thread::spawn(move || {
            let mut stdout = std::io::stdout();
            let mut screen = Screen::new(stdout.is_terminal());
            for line in rx {
                let mut out = Vec::new();
                match line {
                    Line::Status(lines) => {
                        pending.store(false, Ordering::Relaxed);
                        screen.status(&mut out, lines);
                    }
                    Line::Event(text) => screen.event(&mut out, &text),
                }
                let _ = stdout.write_all(&out);
                let _ = stdout.flush();
            }
        });
        Self {
            tx,
            status_pending,
            thread,
        }
    }

    /// A printer that forwards what it is asked to show to `out` instead of printing it,
    /// for the window. Status updates are dropped as [`Printer::spawn`]'s are, while one is
    /// still on its way.
    pub fn to_channel(out: Sender<Line>) -> Self {
        let (tx, rx) = channel::<Line>();
        let status_pending = Arc::new(AtomicBool::new(false));
        let pending = Arc::clone(&status_pending);
        let thread = std::thread::spawn(move || {
            for line in rx {
                if let Line::Status(_) = line {
                    pending.store(false, Ordering::Relaxed);
                }
                let _ = out.send(line);
            }
        });
        Self {
            tx,
            status_pending,
            thread,
        }
    }

    /// A printer that keeps its events instead of printing them (status lines are
    /// dropped), for tests.
    #[cfg(test)]
    pub fn capturing() -> (Self, Arc<std::sync::Mutex<Vec<String>>>) {
        let (tx, rx) = channel::<Line>();
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let kept = Arc::clone(&events);
        let status_pending = Arc::new(AtomicBool::new(false));
        let pending = Arc::clone(&status_pending);
        let thread = std::thread::spawn(move || {
            for line in rx {
                match line {
                    Line::Status(_) => pending.store(false, Ordering::Relaxed),
                    Line::Event(text) => kept.lock().unwrap().push(text),
                }
            }
        });
        let printer = Self {
            tx,
            status_pending,
            thread,
        };
        (printer, events)
    }

    /// Prints what is still queued, then stops the thread.
    pub fn finish(self) {
        drop(self.tx);
        let _ = self.thread.join();
    }

    /// Several status lines, top to bottom.
    pub fn status_lines(&self, lines: Vec<String>) {
        if !self.status_pending.swap(true, Ordering::Relaxed) {
            let _ = self.tx.send(Line::Status(lines));
        }
    }

    pub fn event(&self, text: String) {
        let _ = self.tx.send(Line::Event(text));
    }
}

/// What is on screen, and how to redraw it.
struct Screen {
    /// Redraw status lines in place with ANSI escapes; otherwise (a pipe or file) they
    /// are joined on one `\r`-separated line.
    tty: bool,
    /// How many status lines are on screen (the cursor is at the end of the last).
    shown: usize,
    last: Vec<String>,
}

impl Screen {
    fn new(tty: bool) -> Self {
        Self {
            tty,
            shown: 0,
            last: Vec::new(),
        }
    }

    fn status(&mut self, out: &mut Vec<u8>, lines: Vec<String>) {
        // Library text (tags, file names) may hold control characters: strip them before
        // adding our own `\r` and escape sequences.
        let lines: Vec<String> = lines
            .iter()
            .map(|l| sanitize_for_terminal(l).into_owned())
            .collect();
        if self.tty {
            self.erase(out);
            let cut: Vec<String> = lines
                .iter()
                .map(|l| crate::format::truncate(l, STATUS_WIDTH))
                .collect();
            out.extend_from_slice(cut.join("\n").as_bytes());
            self.shown = lines.len();
        } else {
            out.extend_from_slice(format!("\r{}", lines.join("  |  ")).as_bytes());
        }
        self.last = lines;
    }

    fn event(&mut self, out: &mut Vec<u8>, text: &str) {
        let text = &*sanitize_lines(text);
        if self.tty {
            self.erase(out);
            out.extend_from_slice(format!("{text}\n").as_bytes());
            let last = std::mem::take(&mut self.last);
            if !last.is_empty() {
                self.status(out, last);
            }
        } else {
            out.extend_from_slice(format!("\n{text}\n").as_bytes());
        }
    }

    /// Clears the status lines, leaving the cursor where the first one started.
    fn erase(&mut self, out: &mut Vec<u8>) {
        if self.shown > 0 {
            out.extend_from_slice(b"\r\x1b[2K");
            for _ in 1..self.shown {
                out.extend_from_slice(b"\x1b[1A\x1b[2K");
            }
        }
        self.shown = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(out: &[u8]) -> String {
        String::from_utf8(out.to_vec()).unwrap()
    }

    #[test]
    fn terminal_redraws_status_lines_in_place() {
        let mut screen = Screen::new(true);
        let mut out = Vec::new();
        screen.status(&mut out, vec!["browser".into(), "deck 1".into()]);
        assert_eq!(text(&out), "browser\ndeck 1");
        out.clear();
        screen.status(&mut out, vec!["browser 2".into(), "deck 1".into()]);
        assert_eq!(text(&out), "\r\x1b[2K\x1b[1A\x1b[2Kbrowser 2\ndeck 1");
        out.clear();
        screen.event(&mut out, "Deck 1 ◂ A – B");
        assert_eq!(
            text(&out),
            "\r\x1b[2K\x1b[1A\x1b[2KDeck 1 ◂ A – B\nbrowser 2\ndeck 1"
        );
    }

    #[test]
    fn library_text_cannot_inject_escapes() {
        for tty in [true, false] {
            let mut screen = Screen::new(tty);
            let mut out = Vec::new();
            screen.status(&mut out, vec!["Deck 1  A\u{1b}[2J\u{7}".into()]);
            screen.event(&mut out, "Deck 1 ◂ B\u{1b}]0;x\u{7}\nnext line é🎧");
            let text = text(&out);
            assert!(
                !text.contains("[2J\u{1b}") && !text.contains('\u{7}'),
                "{text:?}"
            );
            assert!(
                !text.contains("\u{1b}[2J") && !text.contains("\u{1b}]0"),
                "{text:?}"
            );
            assert!(text.contains("A\u{FFFD}[2J\u{FFFD}"), "{text:?}");
            assert!(text.contains("\nnext line é🎧"), "{text:?}");
        }
    }

    #[test]
    fn pipe_gets_one_line_per_status() {
        let mut screen = Screen::new(false);
        let mut out = Vec::new();
        screen.status(&mut out, vec!["a".into(), "b".into()]);
        screen.event(&mut out, "event");
        assert_eq!(text(&out), "\ra  |  b\nevent\n");
    }
}
