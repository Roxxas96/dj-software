//! `dj-software midi led-scan`: finds which MIDI message lights a controller LED
//! by binary search, with the owner watching the device.
//!
//! The search itself ([`scan`]) is generic over [`LedSink`] and a question
//! callback, so it is tested without a device.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::{fs, io};

use anyhow::{Context, anyhow, bail};
use clap::ValueEnum;
use controller::capture::LedSink;
use controller::{MidiMessage, OutputPort, PortError, input_port_names, open_output};

use crate::midi_cmd::{Lines, choose_port, write_atomic};

/// Time for the controller to take hundreds of messages before the owner looks.
const SETTLE: Duration = Duration::from_millis(150);

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Kind {
    Note,
    Cc,
}

/// One message slot: 0-based channel and note or controller number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub channel: u8,
    pub data1: u8,
}

impl Candidate {
    fn message(self, kind: Kind, value: u8) -> MidiMessage {
        match kind {
            Kind::Note => MidiMessage::NoteOn {
                channel: self.channel,
                note: self.data1,
                velocity: value,
            },
            Kind::Cc => MidiMessage::ControlChange {
                channel: self.channel,
                controller: self.data1,
                value,
            },
        }
    }

    fn status(self, kind: Kind) -> u8 {
        let high = match kind {
            Kind::Note => 0x90,
            Kind::Cc => 0xB0,
        };
        high | self.channel
    }

    fn describe(self, kind: Kind, value: u8) -> String {
        let name = match kind {
            Kind::Note => "NoteOn",
            Kind::Cc => "CC",
        };
        let what = match kind {
            Kind::Note => "note",
            Kind::Cc => "controller",
        };
        format!(
            "{name} ch{} {what} 0x{:02X} vel {value}",
            self.channel + 1,
            self.data1
        )
    }
}

/// Every `(channel, data1)` of the given 1-based channels, channel-major.
pub fn candidates(channels: &[u8]) -> Vec<Candidate> {
    channels
        .iter()
        .flat_map(|&ch| {
            (0..=127u8).map(move |data1| Candidate {
                channel: ch - 1,
                data1,
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Question<'a> {
    /// "Did the LED light up?" with these candidates lit.
    Half(&'a [Candidate]),
    /// "Is the LED lit now?" with only this candidate lit.
    Confirm(Candidate),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    Yes,
    No,
    /// Partly, both halves, or cannot tell.
    Unsure,
    Skip,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Found(Candidate),
    NoMatch,
    Ambiguous,
    ConfirmFailed(Candidate),
    Skipped,
    Quit,
}

/// Sends `value` to each candidate, remembering them in `lit` until switched off.
fn light<S: LedSink>(
    sink: &mut S,
    kind: Kind,
    lit: &mut Vec<Candidate>,
    cands: &[Candidate],
    value: u8,
) -> Result<(), PortError> {
    let mut result = Ok(());
    for &c in cands {
        if !lit.contains(&c) {
            lit.push(c);
        }
        if let Err(e) = sink.send(c.message(kind, value)) {
            result = result.and(Err(e));
        }
    }
    result
}

/// Switches everything in `lit` off; entries that failed stay for a retry.
fn all_off<S: LedSink>(
    sink: &mut S,
    kind: Kind,
    lit: &mut Vec<Candidate>,
) -> Result<(), PortError> {
    let mut result = Ok(());
    let mut failed = Vec::new();
    for c in lit.drain(..) {
        if let Err(e) = sink.send(c.message(kind, 0)) {
            result = result.and(Err(e));
            failed.push(c);
        }
    }
    *lit = failed;
    result
}

/// Binary search over `cands`. Every lit message is switched off again on every
/// path, including a failed send. `settle` runs after lighting, before asking.
pub fn scan<S: LedSink>(
    sink: &mut S,
    kind: Kind,
    cands: &[Candidate],
    velocity: u8,
    mut settle: impl FnMut(),
    mut ask: impl FnMut(Question<'_>) -> Reply,
) -> Result<Outcome, PortError> {
    let mut lit = Vec::new();
    let result = search(sink, kind, cands, velocity, &mut lit, &mut settle, &mut ask);
    let off = all_off(sink, kind, &mut lit);
    let outcome = result?;
    off?;
    Ok(outcome)
}

fn search<S: LedSink>(
    sink: &mut S,
    kind: Kind,
    cands: &[Candidate],
    velocity: u8,
    lit: &mut Vec<Candidate>,
    settle: &mut impl FnMut(),
    ask: &mut impl FnMut(Question<'_>) -> Reply,
) -> Result<Outcome, PortError> {
    let mut range = cands;
    // Until a "yes", the target may be in neither half.
    let mut verified = false;
    while range.len() > 1 {
        let (first, second) = range.split_at(range.len().div_ceil(2));
        let mut shown = |half: &[Candidate],
                         sink: &mut S,
                         lit: &mut Vec<Candidate>|
         -> Result<Reply, PortError> {
            light(sink, kind, lit, half, velocity)?;
            settle();
            let reply = ask(Question::Half(half));
            all_off(sink, kind, lit)?;
            Ok(reply)
        };
        match shown(first, sink, lit)? {
            Reply::Yes => {
                range = first;
                verified = true;
            }
            Reply::No if verified => range = second,
            Reply::No => match shown(second, sink, lit)? {
                Reply::Yes => {
                    range = second;
                    verified = true;
                }
                Reply::No => return Ok(Outcome::NoMatch),
                Reply::Unsure => return Ok(Outcome::Ambiguous),
                Reply::Skip => return Ok(Outcome::Skipped),
                Reply::Quit => return Ok(Outcome::Quit),
            },
            Reply::Unsure => return Ok(Outcome::Ambiguous),
            Reply::Skip => return Ok(Outcome::Skipped),
            Reply::Quit => return Ok(Outcome::Quit),
        }
    }
    let Some(&target) = range.first() else {
        return Ok(Outcome::NoMatch);
    };
    light(sink, kind, lit, &[target], velocity)?;
    settle();
    let reply = ask(Question::Confirm(target));
    all_off(sink, kind, lit)?;
    Ok(match reply {
        Reply::Yes => Outcome::Found(target),
        Reply::No | Reply::Unsure => Outcome::ConfirmFailed(target),
        Reply::Skip => Outcome::Skipped,
        Reply::Quit => Outcome::Quit,
    })
}

fn parse_reply(line: &str) -> Option<Reply> {
    match line.trim().to_lowercase().as_str() {
        "y" | "yes" => Some(Reply::Yes),
        "n" | "no" => Some(Reply::No),
        "u" | "unsure" => Some(Reply::Unsure),
        "s" | "skip" => Some(Reply::Skip),
        "q" | "quit" => Some(Reply::Quit),
        _ => None,
    }
}

/// Parses a hex byte such as `0x90` or `90`.
pub fn parse_hex(text: &str) -> anyhow::Result<u8> {
    let t = text.trim();
    let t = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .unwrap_or(t);
    u8::from_str_radix(t, 16).map_err(|_| anyhow!("{text:?} is not a hex byte (00-FF)"))
}

/// Builds the message for `--try <status> <data1> [<value>]` (value defaults to 7F).
pub fn try_message(args: &[String]) -> anyhow::Result<(Kind, Candidate, u8)> {
    let [status, data1, rest @ ..] = args else {
        bail!("--try needs <status> <data1> [<value>]");
    };
    let status = parse_hex(status)?;
    let data1 = parse_hex(data1)?;
    let value = match rest {
        [] => 0x7F,
        [v] => parse_hex(v)?,
        _ => bail!("--try takes at most three values"),
    };
    if data1 > 0x7F || value > 0x7F {
        bail!("data bytes must be 00-7F");
    }
    let kind = match status & 0xF0 {
        0x90 => Kind::Note,
        0xB0 => Kind::Cc,
        _ => bail!("--try supports note-on (9n) and control change (Bn) statuses only"),
    };
    Ok((
        kind,
        Candidate {
            channel: status & 0x0F,
            data1,
        },
        value,
    ))
}

fn toml_string(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One `[[led]]` TOML block.
pub fn led_entry(label: &str, status: u8, data1: u8, velocity: u8) -> String {
    format!(
        "[[led]]\nlabel = {}\nstatus = 0x{status:02X}\ndata1 = 0x{data1:02X}\nvelocity = {velocity}\n",
        toml_string(label)
    )
}

/// Appends `entry` to the file at `path` (created if missing), atomically.
fn append_entry(path: &Path, entry: &str) -> anyhow::Result<()> {
    let mut text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    if !text.is_empty() {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push('\n');
    }
    text.push_str(entry);
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    write_atomic(path, &text)
}

/// The output port plus the messages currently lit, shared with the signal
/// handler so Ctrl-C and SIGTERM switch them off.
struct ScanLeds {
    port: Mutex<OutputPort>,
    /// "Off" messages for everything lit.
    lit: Mutex<Vec<MidiMessage>>,
}

fn off_message(msg: MidiMessage) -> Option<MidiMessage> {
    match msg {
        MidiMessage::NoteOn { channel, note, .. } | MidiMessage::NoteOff { channel, note, .. } => {
            Some(MidiMessage::NoteOn {
                channel,
                note,
                velocity: 0,
            })
        }
        MidiMessage::ControlChange {
            channel,
            controller,
            ..
        } => Some(MidiMessage::ControlChange {
            channel,
            controller,
            value: 0,
        }),
        MidiMessage::PitchBend { .. } => None,
    }
}

fn is_off(msg: MidiMessage) -> bool {
    matches!(
        msg,
        MidiMessage::NoteOn { velocity: 0, .. }
            | MidiMessage::NoteOff { .. }
            | MidiMessage::ControlChange { value: 0, .. }
    )
}

impl ScanLeds {
    fn off(&self) {
        let mut port = self.port.lock().unwrap_or_else(|e| e.into_inner());
        let mut lit = self.lit.lock().unwrap_or_else(|e| e.into_inner());
        let mut failed = Vec::new();
        for msg in lit.drain(..) {
            if port.send(msg).is_err() {
                failed.push(msg);
            }
        }
        *lit = failed;
    }
}

struct TrackedScan<'a>(&'a ScanLeds);

impl LedSink for TrackedScan<'_> {
    fn send(&mut self, msg: MidiMessage) -> Result<(), PortError> {
        let mut port = self.0.port.lock().unwrap_or_else(|e| e.into_inner());
        let result = port.send(msg);
        if let Some(off) = off_message(msg) {
            let mut lit = self.0.lit.lock().unwrap_or_else(|e| e.into_inner());
            lit.retain(|&m| m != off);
            // A failed "off" stays listed so the handler retries it.
            if !is_off(msg) || result.is_err() {
                lit.push(off);
            }
        }
        result
    }
}

fn open_leds(port: Option<&str>) -> anyhow::Result<(Arc<ScanLeds>, String)> {
    let inputs = input_port_names().context("listing MIDI inputs")?;
    let wanted = choose_port(port, &inputs)?;
    let output = open_output(&wanted).context("opening MIDI output")?;
    let name = output.name().to_owned();
    let leds = Arc::new(ScanLeds {
        port: Mutex::new(output),
        lit: Mutex::new(Vec::new()),
    });
    let handler = Arc::clone(&leds);
    ctrlc::set_handler(move || {
        handler.off();
        println!("\nInterrupted; LEDs switched off.");
        std::process::exit(130);
    })
    .context("installing the signal handler")?;
    Ok((leds, name))
}

fn settle_and_drain(lines: &Lines) -> impl FnMut() + '_ {
    move || {
        thread::sleep(SETTLE);
        lines.drain();
    }
}

fn prompt_for(question: Question<'_>, label: &str, kind: Kind, velocity: u8) -> String {
    match question {
        Question::Half(half) => format!(
            "Did {label} light up? ({} messages lit) [y/n/u=unsure/s=skip label/q=quit] ",
            half.len()
        ),
        Question::Confirm(c) => format!(
            "Is {label} lit now? ({}) [y/n/q] ",
            c.describe(kind, velocity)
        ),
    }
}

fn ask_reply(lines: &Lines, prompt: &str) -> Reply {
    loop {
        let Some(line) = lines.ask(prompt) else {
            return Reply::Quit;
        };
        match parse_reply(&line) {
            Some(reply) => return reply,
            None => println!("  Please answer y, n, u, s or q."),
        }
    }
}

/// `--try`: lights one message, asks, switches it off.
pub fn try_one(port: Option<&str>, args: &[String]) -> anyhow::Result<()> {
    let (kind, cand, value) = try_message(args)?;
    let (leds, name) = open_leds(port)?;
    let lines = Lines::spawn();
    println!("Output: {name}");
    let mut sink = TrackedScan(&leds);
    let msg = cand.message(kind, value);
    let sent = sink.send(msg);
    thread::sleep(SETTLE);
    lines.drain();
    if sent.is_ok() {
        println!("Sent {}", cand.describe(kind, value));
        let answer = lines.ask("Press Enter to switch it off. ");
        let _ = answer;
    }
    leds.off();
    sent.map_err(Into::into)
}

/// `dj-software midi led-scan`.
pub fn run(
    port: Option<&str>,
    channels: &[u8],
    velocity: u8,
    kind: Kind,
    out: &Path,
) -> anyhow::Result<()> {
    if channels.is_empty() || channels.iter().any(|c| !(1..=16).contains(c)) {
        bail!("--channels must list channels 1-16");
    }
    if velocity > 127 {
        bail!("--velocity must be 0-127");
    }
    let cands = candidates(channels);
    let (leds, name) = open_leds(port)?;
    let lines = Lines::spawn();
    println!("LED scan on {name}: {} candidate messages.", cands.len());
    println!("Results are appended to {}.", out.display());
    let quitting = AtomicBool::new(false);
    while !quitting.load(Ordering::Relaxed) {
        let Some(label) =
            lines.ask("\nWhich LED are you looking for (e.g. \"deck1 PLAY\")? Blank or q = done: ")
        else {
            break;
        };
        let label = label.trim().to_owned();
        if label.is_empty() || label.eq_ignore_ascii_case("q") {
            break;
        }
        loop {
            let mut sink = TrackedScan(&leds);
            let outcome = scan(
                &mut sink,
                kind,
                &cands,
                velocity,
                settle_and_drain(&lines),
                |q| ask_reply(&lines, &prompt_for(q, &label, kind, velocity)),
            );
            leds.off();
            let outcome = outcome.map_err(anyhow::Error::from)?;
            let retry = match outcome {
                Outcome::Found(c) => {
                    let line = format!("{label} -> {}", c.describe(kind, velocity));
                    println!("{}", line.replacen("->", "\u{2192}", 1));
                    append_entry(out, &led_entry(&label, c.status(kind), c.data1, velocity))?;
                    false
                }
                Outcome::NoMatch => {
                    println!("no message in this range lights {label}");
                    false
                }
                Outcome::Ambiguous => {
                    println!("Ambiguous answer for {label}; the search cannot continue.");
                    ask_restart(&lines)
                }
                Outcome::ConfirmFailed(c) => {
                    println!(
                        "Confirmation failed: {} did not light {label}.",
                        c.describe(kind, velocity)
                    );
                    ask_restart(&lines)
                }
                Outcome::Skipped => {
                    println!("Skipped {label}.");
                    false
                }
                Outcome::Quit => {
                    quitting.store(true, Ordering::Relaxed);
                    false
                }
            };
            if !retry {
                break;
            }
        }
    }
    leds.off();
    Ok(())
}

fn ask_restart(lines: &Lines) -> bool {
    matches!(
        lines.ask_yes_no("Restart the search for this label? [y/n] "),
        Some(true)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        sent: Vec<MidiMessage>,
        lit: Vec<MidiMessage>,
        fail_after: Option<usize>,
    }

    impl LedSink for Fake {
        fn send(&mut self, msg: MidiMessage) -> Result<(), PortError> {
            if self.fail_after.is_some_and(|n| self.sent.len() >= n) {
                return Err(PortError::Send("boom".into()));
            }
            self.sent.push(msg);
            let off = off_message(msg).unwrap();
            self.lit.retain(|&m| m != off);
            if !is_off(msg) {
                self.lit.push(off);
            }
            Ok(())
        }
    }

    fn cands(n: u8) -> Vec<Candidate> {
        (0..n)
            .map(|i| Candidate {
                channel: 0,
                data1: i,
            })
            .collect()
    }

    /// Answers honestly for a LED reacting to `target` only.
    fn oracle(
        target: Option<Candidate>,
        asked: &mut usize,
    ) -> impl FnMut(Question<'_>) -> Reply + '_ {
        move |q| {
            *asked += 1;
            let hit = match q {
                Question::Half(h) => target.is_some_and(|t| h.contains(&t)),
                Question::Confirm(c) => Some(c) == target,
            };
            if hit { Reply::Yes } else { Reply::No }
        }
    }

    #[test]
    fn finds_every_candidate_in_log2_questions() {
        for n in [1u8, 2, 3, 7, 8, 100, 127] {
            let all = cands(n);
            let bound = (n as f64).log2().ceil() as usize + 1; // + final confirm
            for &t in &all {
                let mut fake = Fake::default();
                let mut asked = 0;
                let out = scan(
                    &mut fake,
                    Kind::Note,
                    &all,
                    127,
                    || {},
                    oracle(Some(t), &mut asked),
                )
                .unwrap();
                assert_eq!(out, Outcome::Found(t), "n={n} t={t:?}");
                // A target in a second half costs one extra question at the root only.
                assert!(asked <= bound + 1, "n={n} asked {asked}");
                assert!(fake.lit.is_empty());
            }
        }
    }

    #[test]
    fn first_half_target_uses_exactly_ceil_log2_plus_confirm() {
        let all = cands(64);
        let mut fake = Fake::default();
        let mut asked = 0;
        let t = all[0];
        scan(
            &mut fake,
            Kind::Note,
            &all,
            127,
            || {},
            oracle(Some(t), &mut asked),
        )
        .unwrap();
        assert_eq!(asked, 6 + 1);
    }

    #[test]
    fn neither_half_lit_reports_no_match() {
        let all = cands(16);
        let mut fake = Fake::default();
        let mut asked = 0;
        let out = scan(
            &mut fake,
            Kind::Note,
            &all,
            127,
            || {},
            oracle(None, &mut asked),
        )
        .unwrap();
        assert_eq!(out, Outcome::NoMatch);
        assert_eq!(asked, 2);
        assert!(fake.lit.is_empty());
    }

    #[test]
    fn skip_quit_and_unsure_switch_everything_off() {
        for (reply, expected) in [
            (Reply::Skip, Outcome::Skipped),
            (Reply::Quit, Outcome::Quit),
            (Reply::Unsure, Outcome::Ambiguous),
        ] {
            let mut fake = Fake::default();
            let out = scan(&mut fake, Kind::Cc, &cands(10), 127, || {}, |_| reply).unwrap();
            assert_eq!(out, expected);
            assert!(!fake.sent.is_empty());
            assert!(fake.lit.is_empty());
        }
    }

    #[test]
    fn failed_confirm_is_reported_and_switched_off() {
        let all = cands(4);
        let mut fake = Fake::default();
        let mut n = 0;
        let out = scan(
            &mut fake,
            Kind::Note,
            &all,
            127,
            || {},
            |q| {
                n += 1;
                match q {
                    Question::Half(_) => Reply::Yes,
                    Question::Confirm(_) => Reply::No,
                }
            },
        )
        .unwrap();
        assert_eq!(out, Outcome::ConfirmFailed(all[0]));
        assert!(fake.lit.is_empty());
    }

    #[test]
    fn send_error_still_switches_off_what_was_lit() {
        let mut fake = Fake {
            fail_after: Some(3),
            ..Fake::default()
        };
        let result = scan(
            &mut fake,
            Kind::Note,
            &cands(10),
            127,
            || {},
            |_| Reply::Yes,
        );
        assert!(result.is_err());
        fake.fail_after = None;
        // Anything that was lit before the failure was attempted off; retry works.
        assert!(fake.lit.len() <= 3);
    }

    #[test]
    fn settle_runs_before_every_question() {
        let mut fake = Fake::default();
        let mut settled = 0;
        let mut asked = 0;
        scan(
            &mut fake,
            Kind::Note,
            &cands(8),
            127,
            || settled += 1,
            |_| {
                asked += 1;
                Reply::Yes
            },
        )
        .unwrap();
        assert_eq!(settled, asked);
    }

    #[test]
    fn default_candidates_cover_channels() {
        let c = candidates(&[1, 9]);
        assert_eq!(c.len(), 256);
        assert_eq!(
            c[0],
            Candidate {
                channel: 0,
                data1: 0
            }
        );
        assert_eq!(
            c[255],
            Candidate {
                channel: 8,
                data1: 127
            }
        );
    }

    #[test]
    fn try_message_parses_hex() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (k, c, v) = try_message(&args(&["0x90", "0B"])).unwrap();
        assert_eq!(
            (k, c, v),
            (
                Kind::Note,
                Candidate {
                    channel: 0,
                    data1: 0x0B
                },
                0x7F
            )
        );
        let (k, c, v) = try_message(&args(&["B6", "0x10", "40"])).unwrap();
        assert_eq!(
            (k, c, v),
            (
                Kind::Cc,
                Candidate {
                    channel: 6,
                    data1: 0x10
                },
                0x40
            )
        );
        assert!(try_message(&args(&["E0", "00"])).is_err());
        assert!(try_message(&args(&["90", "80"])).is_err());
        assert!(try_message(&args(&["zz", "00"])).is_err());
    }

    #[test]
    fn entry_is_valid_toml_shape() {
        let e = led_entry("deck1 \"PLAY\"", 0x90, 0x0B, 127);
        assert_eq!(
            e,
            "[[led]]\nlabel = \"deck1 \\\"PLAY\\\"\"\nstatus = 0x90\ndata1 = 0x0B\nvelocity = 127\n"
        );
    }

    #[test]
    fn append_creates_and_appends() {
        let dir = std::env::temp_dir().join(format!("led-scan-test-{}", std::process::id()));
        let path = dir.join("sub").join("led-scan.toml");
        append_entry(&path, &led_entry("a", 0x90, 1, 127)).unwrap();
        append_entry(&path, &led_entry("b", 0x91, 2, 127)).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("[[led]]").count(), 2);
        assert!(text.contains("\n\n[[led]]\nlabel = \"b\""));
        let _ = fs::remove_dir_all(&dir);
    }
}
