//! The `midi` commands: list ports, monitor raw messages and run a guided capture.

use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use controller::capture::{
    CaptureSession, Fixture, LedProbe, LedSink, Meta, PROBE_VELOCITIES, Question, Step, StepKind,
    StepStatus, parse_checklist, parse_fixture, run_led_probe,
};
use controller::{
    MidiMessage, OutputPort, PortError, TimedMidi, clock, input_port_names, open_input,
    open_output, output_port_names,
};

const POLL: Duration = Duration::from_millis(100);

pub fn ports() -> anyhow::Result<()> {
    for name in input_port_names().context("listing MIDI inputs")? {
        println!("in:  {name}");
    }
    for name in output_port_names().context("listing MIDI outputs")? {
        println!("out: {name}");
    }
    Ok(())
}

/// Picks the port to monitor: the requested name, else the first input containing "DDJ".
fn choose_port(requested: Option<&str>, inputs: &[String]) -> anyhow::Result<String> {
    if let Some(name) = requested {
        return Ok(name.to_owned());
    }
    if let Some(name) = inputs.iter().find(|n| n.contains("DDJ")) {
        return Ok(name.clone());
    }
    let list = if inputs.is_empty() {
        "none".to_owned()
    } else {
        inputs.join(", ")
    };
    Err(anyhow!(
        "no MIDI input containing \"DDJ\" found; pass --port <NAME>. Available inputs: {list}"
    ))
}

pub fn monitor(port: Option<&str>) -> anyhow::Result<()> {
    let inputs = input_port_names().context("listing MIDI inputs")?;
    let wanted = choose_port(port, &inputs)?;

    let interrupted = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&interrupted);
    ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed))
        .context("installing the Ctrl-C handler")?;

    let (tx, rx) = channel();
    let started = clock::now_micros();
    let input = open_input(&wanted, tx).context("opening MIDI input")?;
    println!("Monitoring {} (Ctrl-C to stop)", input.name());
    while !interrupted.load(Ordering::Relaxed) {
        match rx.recv_timeout(POLL) {
            Ok(timed) => {
                let ms = timed.micros.saturating_sub(started) / 1000;
                println!("{ms:>8} ms  {}", timed.message);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}

/// How often the capture loop polls the session while listening.
const CAPTURE_POLL: Duration = Duration::from_millis(20);
/// How long each probe velocity stays on.
const PROBE_HOLD: Duration = Duration::from_millis(700);
const PAD_COUNT: usize = 8;

/// A line typed while a step is shown or listening.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    /// Empty line: "ready" after preparing, or "finish now" while listening.
    Enter,
    Skip,
    Redo,
    Quit,
    Unknown(String),
}

fn parse_command(line: &str) -> Command {
    match line.trim().to_lowercase().as_str() {
        "" => Command::Enter,
        "s" | "skip" => Command::Skip,
        "r" | "redo" => Command::Redo,
        "q" | "quit" => Command::Quit,
        other => Command::Unknown(other.to_owned()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Yes,
    No,
    Quit,
    Invalid,
}

fn parse_answer(line: &str) -> Answer {
    match line.trim().to_lowercase().as_str() {
        "y" | "yes" => Answer::Yes,
        "n" | "no" => Answer::No,
        "q" | "quit" => Answer::Quit,
        _ => Answer::Invalid,
    }
}

/// Writes `contents` to a temporary file next to `path`, then renames it over
/// `path`, so a crash never leaves a half-written fixture.
fn write_atomic(path: &Path, contents: &str) -> anyhow::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| anyhow!("{} is not a file path", path.display()))?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    let write = || -> io::Result<()> {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    };
    write().with_context(|| format!("writing {}", path.display()))
}

fn save(path: &Path, session: &CaptureSession) -> anyhow::Result<()> {
    write_atomic(path, &session.fixture().to_toml())
}

/// Stdin lines, read on a helper thread so the capture loop can keep polling MIDI.
struct Lines(Receiver<String>);

enum Typed {
    Line(String),
    Nothing,
    Eof,
}

impl Lines {
    fn spawn() -> Lines {
        let (tx, rx) = channel();
        thread::spawn(move || {
            for line in io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Lines(rx)
    }

    /// Prints `prompt` and waits for a line; `None` on end of input.
    fn ask(&self, prompt: &str) -> Option<String> {
        print!("{prompt}");
        let _ = io::stdout().flush();
        self.0.recv().ok()
    }

    fn try_next(&self) -> Typed {
        match self.0.try_recv() {
            Ok(line) => Typed::Line(line),
            Err(TryRecvError::Empty) => Typed::Nothing,
            Err(TryRecvError::Disconnected) => Typed::Eof,
        }
    }

    /// Drops lines typed ahead (e.g. Enter pressed while lights were cycling).
    fn drain(&self) {
        while self.0.try_recv().is_ok() {}
    }

    /// Asks until the answer is y, n or q; `None` for q or end of input.
    fn ask_yes_no(&self, prompt: &str) -> Option<bool> {
        loop {
            match parse_answer(&self.ask(prompt)?) {
                Answer::Yes => return Some(true),
                Answer::No => return Some(false),
                Answer::Quit => return None,
                Answer::Invalid => println!("  Please answer y, n or q."),
            }
        }
    }
}

/// The output port plus the notes currently lit, shared with the signal
/// handler so Ctrl-C, SIGTERM or SIGHUP switch them off.
struct Leds {
    port: Mutex<OutputPort>,
    lit: Mutex<Vec<(u8, u8)>>,
}

impl Leds {
    /// Switches every lit note off (best effort).
    fn off(&self) {
        let mut port = self.port.lock().unwrap_or_else(|e| e.into_inner());
        let mut lit = self.lit.lock().unwrap_or_else(|e| e.into_inner());
        for (channel, note) in lit.drain(..) {
            let _ = port.send(MidiMessage::NoteOn {
                channel,
                note,
                velocity: 0,
            });
        }
    }
}

/// [`LedSink`] over [`Leds`] that tracks which notes are lit.
struct TrackedLeds<'a>(&'a Leds);

impl LedSink for TrackedLeds<'_> {
    fn send(&mut self, msg: MidiMessage) -> Result<(), PortError> {
        let mut port = self.0.port.lock().unwrap_or_else(|e| e.into_inner());
        let result = port.send(msg);
        if let MidiMessage::NoteOn {
            channel,
            note,
            velocity,
        } = msg
        {
            let mut lit = self.0.lit.lock().unwrap_or_else(|e| e.into_inner());
            lit.retain(|&n| n != (channel, note));
            // Keep a failed "off" in the list so the handler retries it.
            if velocity > 0 || result.is_err() {
                lit.push((channel, note));
            }
        }
        result
    }
}

/// `dj-software midi capture`: walks the checklist, recording each control and
/// probing its LED right after it is captured. The fixture is saved after every step.
pub fn capture(checklist_path: &Path, out: &Path, port: Option<&str>) -> anyhow::Result<()> {
    let text = fs::read_to_string(checklist_path)
        .with_context(|| format!("reading {}", checklist_path.display()))?;
    let checklist = parse_checklist(&text)
        .with_context(|| format!("loading checklist {}", checklist_path.display()))?;
    if checklist.steps.is_empty() {
        bail!("checklist {} has no steps", checklist_path.display());
    }

    let inputs = input_port_names().context("listing MIDI inputs")?;
    let wanted = choose_port(port, &inputs)?;
    let (tx, rx) = channel();
    let input = open_input(&wanted, tx).context("opening MIDI input")?;
    let leds = match open_output(input.name()) {
        Ok(port) => Some(Arc::new(Leds {
            port: Mutex::new(port),
            lit: Mutex::new(Vec::new()),
        })),
        Err(error) => {
            println!("warning: {error}; LED checks will be skipped");
            None
        }
    };
    let handler_leds = leds.clone();
    ctrlc::set_handler(move || {
        if let Some(leds) = &handler_leds {
            leds.off();
        }
        println!("\nInterrupted. Progress up to the last finished step is saved.");
        std::process::exit(130);
    })
    .context("installing the signal handler")?;

    let lines = Lines::spawn();
    let meta = Meta {
        controller: checklist
            .controller
            .clone()
            .unwrap_or_else(|| input.name().to_owned()),
        port: input.name().to_owned(),
        ..Meta::default()
    };
    let Some(mut session) = open_session(checklist.steps, out, &lines)? else {
        return Ok(());
    };
    session.set_meta(meta);
    save(out, &session)?;

    println!("Capturing from {} into {}", input.name(), out.display());
    println!("Each step listens as soon as it is shown: just do what it says.");
    println!(
        "Type then Enter: nothing = finish the step now, s = skip, r = redo the previous step, q = save and quit."
    );
    println!("When a step asks you to prepare something first, do it, then press Enter.");
    if !run_steps(&mut session, &rx, &lines, out, leds.as_deref())? {
        println!(
            "\nSaved {}. Run the same command again to continue.",
            out.display()
        );
        return Ok(());
    }
    let fixture = session.finish();
    println!(
        "\nAll steps done: {} captured, {} skipped. Saved {}.",
        fixture.controls.len(),
        fixture.meta.skipped.len(),
        out.display()
    );
    Ok(())
}

/// A new session, or one resumed from an existing `out` if the user agrees.
/// `None` means the user chose to stop.
fn open_session(
    steps: Vec<Step>,
    out: &Path,
    lines: &Lines,
) -> anyhow::Result<Option<CaptureSession>> {
    if !out.exists() {
        return Ok(Some(CaptureSession::new(steps)));
    }
    let text = fs::read_to_string(out).with_context(|| format!("reading {}", out.display()))?;
    let prior: Fixture = parse_fixture(&text).with_context(|| {
        format!(
            "{} exists but is not a capture fixture; move it away or choose another --out",
            out.display()
        )
    })?;
    let unknown: Vec<String> = prior
        .controls
        .iter()
        .map(|c| c.id.clone())
        .filter(|id| !steps.iter().any(|s| &s.id == id))
        .collect();
    let session = CaptureSession::resume(steps.clone(), prior);
    let (n, total) = session.position();
    println!(
        "{} already has {}/{total} steps done.",
        out.display(),
        session.done_count()
    );
    if !unknown.is_empty() {
        println!(
            "note: kept {} controls that are not in this checklist: {}",
            unknown.len(),
            unknown.join(", ")
        );
    }
    let resume = match session.current() {
        Some(step) => lines.ask_yes_no(&format!(
            "Continue from step {n}/{total} ({})? [y/n] ",
            step.id
        )),
        None => {
            println!("Every step is done.");
            Some(false)
        }
    };
    match resume {
        Some(true) => Ok(Some(session)),
        Some(false) => {
            let again = format!("Start over and overwrite {}? [y/n] ", out.display());
            if lines.ask_yes_no(&again) == Some(true) {
                Ok(Some(CaptureSession::new(steps)))
            } else {
                Ok(None)
            }
        }
        None => Ok(None),
    }
}

/// Runs every remaining step. Returns false if the user quit early.
fn run_steps(
    session: &mut CaptureSession,
    rx: &Receiver<TimedMidi>,
    lines: &Lines,
    out: &Path,
    leds: Option<&Leds>,
) -> anyhow::Result<bool> {
    let mut shown_section: Option<String> = None;
    while let Some(step) = session.current().cloned() {
        // Wait for Enter when the user has something to do first (switch deck,
        // select a pad mode), so that action is not recorded.
        let mut wait = false;
        if shown_section.as_deref() != Some(step.section.as_str()) {
            println!("\n=== {} ===", step.section);
            if let Some(setup) = &step.setup {
                println!("{setup}");
                wait = true;
            }
            shown_section = Some(step.section.clone());
        }
        let (n, total) = session.position();
        println!("\n[step {n}/{total}] {}  ({})", step.id, step.kind.as_str());
        println!("  {}", step.prompt);
        if let Some(prepare) = &step.prepare {
            println!("  First: {prepare}");
            wait = true;
        }
        let outcome = if wait {
            match lines.ask("  Press Enter when ready (s = skip, r = redo previous, q = quit) > ") {
                None => Listened::Quit,
                Some(line) => match parse_command(&line) {
                    Command::Enter => listen(session, &step, rx, lines)?,
                    Command::Skip => {
                        session.skip();
                        Listened::Skipped
                    }
                    Command::Redo => Listened::Redo,
                    Command::Quit => Listened::Quit,
                    Command::Unknown(other) => {
                        println!("  unknown command {other:?}");
                        shown_section = None;
                        continue;
                    }
                },
            }
        } else {
            listen(session, &step, rx, lines)?
        };
        match outcome {
            Listened::Done => {
                let probe = match session.control(&step.id) {
                    Some(control) => {
                        println!("  recorded {}", control.summary());
                        LedProbe::for_step(&step, control)
                    }
                    None => None,
                };
                save(out, session)?;
                if let (Some(probe), Some(leds)) = (probe, leds)
                    && !check_led(session, leds, lines, &probe)
                {
                    save(out, session)?;
                    return Ok(false);
                }
            }
            Listened::Skipped => println!("  skipped {}", step.id),
            Listened::Redo => {
                session.redo();
                match session.current() {
                    Some(again) if again.id != step.id => println!("  back to {}", again.id),
                    _ => println!("  nothing to redo yet"),
                }
            }
            Listened::Quit => {
                save(out, session)?;
                return Ok(false);
            }
        }
        save(out, session)?;
    }
    Ok(true)
}

enum Listened {
    Done,
    Skipped,
    Redo,
    Quit,
}

/// Feeds MIDI into the session until the current step completes, echoing each message.
fn listen(
    session: &mut CaptureSession,
    step: &Step,
    rx: &Receiver<TimedMidi>,
    lines: &Lines,
) -> anyhow::Result<Listened> {
    // Drop anything sent before listening (setup, fiddling between steps).
    while rx.try_recv().is_ok() {}
    match step.kind {
        StepKind::Pads => println!("  Listening: tap all {PAD_COUNT} pads..."),
        _ => println!("  Listening..."),
    }
    loop {
        let first = match rx.recv_timeout(CAPTURE_POLL) {
            Ok(timed) => Some(timed),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => bail!("the MIDI input closed"),
        };
        for timed in first.into_iter().chain(rx.try_iter()) {
            session.feed(timed);
            println!(
                "    {}{}",
                timed.message,
                echo_note(session, step, timed.message)
            );
        }
        match lines.try_next() {
            Typed::Nothing => {}
            Typed::Eof => return Ok(Listened::Quit),
            Typed::Line(line) => match parse_command(&line) {
                Command::Enter => {
                    if session.accept() {
                        return Ok(Listened::Done);
                    }
                    println!("  Nothing recorded yet. Do the action, or type s to skip.");
                }
                Command::Skip => {
                    session.skip();
                    return Ok(Listened::Skipped);
                }
                Command::Redo => return Ok(Listened::Redo),
                Command::Quit => return Ok(Listened::Quit),
                Command::Unknown(other) => {
                    println!(
                        "  unknown command {other:?} (Enter = finish now, s = skip, r = redo previous, q = quit)"
                    );
                }
            },
        }
        if session.poll(clock::now_micros()) == StepStatus::Done {
            return Ok(Listened::Done);
        }
    }
}

/// Extra text after an echoed message: the held SHIFT, or the pad count.
fn echo_note(session: &CaptureSession, step: &Step, message: MidiMessage) -> String {
    let note = match message {
        MidiMessage::NoteOn { channel, note, .. } | MidiMessage::NoteOff { channel, note, .. } => {
            Some((channel, note))
        }
        _ => None,
    };
    if note.is_some() && note == session.modifier() {
        return "   (held SHIFT, not recorded)".to_owned();
    }
    if step.kind == StepKind::Pads && matches!(message, MidiMessage::NoteOn { .. }) {
        return format!("   ({}/{PAD_COUNT} pads)", session.distinct_notes());
    }
    String::new()
}

/// Lights the just-captured control and stores the answer. Returns false if the
/// user quit. A failed send is reported and the capture goes on.
fn check_led(session: &mut CaptureSession, leds: &Leds, lines: &Lines, probe: &LedProbe) -> bool {
    let light_question = if probe.pads {
        "  LED check: did the pads light up? [y/n, q = quit] "
    } else {
        "  LED check: did a light turn on? [y/n, q = quit] "
    };
    let result = run_led_probe(
        &mut TrackedLeds(leds),
        probe,
        |question| {
            lines.drain();
            match question {
                Question::LightOn => lines.ask_yes_no(light_question),
                Question::ColorChanged => {
                    lines.ask_yes_no("  Did the color change? [y/n, q = quit] ")
                }
            }
        },
        |velocity| {
            if velocity == PROBE_VELOCITIES[0] {
                println!("  Showing velocities 1, 16, 32, 64, 127 on the pads...");
            }
            println!("    velocity {velocity}");
            thread::sleep(PROBE_HOLD);
        },
    );
    // run_led_probe already switched everything off; retry anything still lit.
    leds.off();
    match result {
        Ok(Some(answer)) => {
            session.set_led(&probe.id, answer.led, answer.values_vary);
            true
        }
        Ok(None) => false,
        Err(error) => {
            println!("  warning: LED check failed ({error}); continuing without it");
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn explicit_port_wins() {
        let inputs = names(&["DDJ-SB3"]);
        assert_eq!(choose_port(Some("IAC"), &inputs).unwrap(), "IAC");
    }

    #[test]
    fn defaults_to_first_ddj_input() {
        let inputs = names(&["IAC Driver", "DDJ-SB3", "DDJ-400"]);
        assert_eq!(choose_port(None, &inputs).unwrap(), "DDJ-SB3");
    }

    #[test]
    fn no_ddj_lists_available_ports() {
        let error = choose_port(None, &names(&["IAC Driver"])).unwrap_err();
        assert!(error.to_string().contains("IAC Driver"));
    }

    #[test]
    fn parses_capture_commands() {
        assert_eq!(parse_command(""), Command::Enter);
        assert_eq!(parse_command("  \r"), Command::Enter);
        assert_eq!(parse_command("s"), Command::Skip);
        assert_eq!(parse_command("R"), Command::Redo);
        assert_eq!(parse_command("quit"), Command::Quit);
        assert_eq!(parse_command("x"), Command::Unknown("x".to_owned()));
    }

    #[test]
    fn parses_yes_no_answers() {
        assert_eq!(parse_answer("y"), Answer::Yes);
        assert_eq!(parse_answer(" No "), Answer::No);
        assert_eq!(parse_answer("q"), Answer::Quit);
        assert_eq!(parse_answer(""), Answer::Invalid);
    }

    #[test]
    fn write_atomic_replaces_file_and_leaves_no_temp() {
        let dir = std::env::temp_dir().join(format!(
            "dj-software-capture-test-{}-{}",
            std::process::id(),
            clock::now_micros()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("capture.toml");
        write_atomic(&path, "one").unwrap();
        write_atomic(&path, "two").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
        let names: Vec<PathBuf> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(names, std::slice::from_ref(&path));
        fs::remove_dir_all(&dir).unwrap();
    }
}
