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
    CaptureSession, Fixture, LedProbe, Meta, StepKind, StepStatus, parse_checklist, parse_fixture,
};
use controller::{
    MidiMessage, OutputPort, TimedMidi, clock, input_port_names, open_input, open_output,
    output_port_names,
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
/// Velocities sent to lit pads to see whether the color depends on the value.
const PROBE_VELOCITIES: [u8; 5] = [1, 16, 32, 64, 127];
/// How long each probe velocity stays on.
const PROBE_HOLD: Duration = Duration::from_millis(700);
const PAD_COUNT: usize = 8;

/// A line typed between steps or while listening.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    /// Empty line: start listening, or finish the step now while listening.
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

fn save(path: &Path, fixture: &Fixture) -> anyhow::Result<()> {
    write_atomic(path, &fixture.to_toml())
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

/// The output port plus the notes currently lit, so Ctrl-C can switch them off.
struct Leds {
    port: Mutex<OutputPort>,
    lit: Mutex<Vec<(u8, u8)>>,
}

impl Leds {
    fn light(&self, probe: &LedProbe, velocity: u8) -> anyhow::Result<()> {
        let mut port = self.port.lock().unwrap_or_else(|e| e.into_inner());
        *self.lit.lock().unwrap_or_else(|e| e.into_inner()) = probe.notes.clone();
        for msg in probe.messages(velocity) {
            port.send(msg).context("sending LED message")?;
        }
        Ok(())
    }

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

/// `dj-software midi capture`: walks the checklist, recording each control,
/// then probes LEDs. The fixture is saved after every step.
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
            println!("warning: {error}; the LED check will be skipped");
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
    .context("installing the Ctrl-C handler")?;

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
    save(out, &session.fixture())?;

    println!("Capturing from {} into {}", input.name(), out.display());
    println!(
        "Between steps: Enter = listen, s = skip, r = redo the previous step, q = save and quit."
    );
    println!("While listening: Enter = finish the step now, s = skip, q = save and quit.");
    if !run_steps(&mut session, &rx, &lines, out)? {
        println!(
            "\nSaved {}. Run the same command again to continue.",
            out.display()
        );
        return Ok(());
    }

    let mut fixture = session.finish();
    let captured = fixture.controls.len();
    let skipped = fixture.meta.skipped.len();
    println!("\nAll steps done: {captured} captured, {skipped} skipped.");
    drop(input);
    match &leds {
        Some(leds) => probe_leds(&mut fixture, leds, &lines, out)?,
        None => println!("No MIDI output port: LED check skipped."),
    }
    save(out, &fixture)?;
    println!("Saved {}.", out.display());
    Ok(())
}

/// A new session, or one resumed from an existing `out` if the user agrees.
/// `None` means the user chose to stop.
fn open_session(
    steps: Vec<controller::capture::Step>,
    out: &Path,
    lines: &Lines,
) -> anyhow::Result<Option<CaptureSession>> {
    if !out.exists() {
        return Ok(Some(CaptureSession::new(steps)));
    }
    let text = fs::read_to_string(out).with_context(|| format!("reading {}", out.display()))?;
    let prior = parse_fixture(&text).with_context(|| {
        format!(
            "{} exists but is not a capture fixture; move it away or choose another --out",
            out.display()
        )
    })?;
    let step_ids: Vec<String> = steps.iter().map(|s| s.id.clone()).collect();
    let unknown: Vec<String> = prior
        .controls
        .iter()
        .map(|c| c.id.clone())
        .filter(|id| !step_ids.contains(id))
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
    let question = match session.current() {
        Some(step) => format!("Continue from step {n}/{total} ({})? [y/n] ", step.id),
        None => "Every step is done. Continue with the LED check? [y/n] ".to_owned(),
    };
    match lines.ask_yes_no(&question) {
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
) -> anyhow::Result<bool> {
    let mut shown_section: Option<String> = None;
    while let Some(step) = session.current().cloned() {
        if shown_section.as_deref() != Some(step.section.as_str()) {
            println!("\n=== {} ===", step.section);
            if let Some(setup) = &step.setup {
                println!("{setup}");
            }
            shown_section = Some(step.section.clone());
        }
        let (n, total) = session.position();
        println!("\n[step {n}/{total}] {}  ({})", step.id, step.kind.as_str());
        println!("  {}", step.prompt);
        let Some(line) = lines.ask("  Enter = listen, s = skip, r = redo previous, q = quit > ")
        else {
            return Ok(false);
        };
        match parse_command(&line) {
            Command::Enter => match listen(session, &step, rx, lines)? {
                Listened::Done => match session.control(&step.id) {
                    Some(control) => println!("  recorded {}", control.summary()),
                    None => println!("  recorded {}", step.id),
                },
                Listened::Skipped => println!("  skipped {}", step.id),
                Listened::Quit => return Ok(false),
            },
            Command::Skip => {
                session.skip();
                println!("  skipped {}", step.id);
            }
            Command::Redo => {
                session.redo();
                match session.current() {
                    Some(again) if again.id != step.id => println!("  back to {}", again.id),
                    _ => println!("  nothing to redo yet"),
                }
            }
            Command::Quit => {
                save(out, &session.fixture())?;
                return Ok(false);
            }
            Command::Unknown(other) => {
                println!("  unknown command {other:?}");
                continue;
            }
        }
        save(out, &session.fixture())?;
    }
    Ok(true)
}

enum Listened {
    Done,
    Skipped,
    Quit,
}

/// Feeds MIDI into the session until the current step completes, echoing each message.
fn listen(
    session: &mut CaptureSession,
    step: &controller::capture::Step,
    rx: &Receiver<TimedMidi>,
    lines: &Lines,
) -> anyhow::Result<Listened> {
    // Drop anything sent before Enter (mode buttons, SHIFT held beforehand, fiddling).
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
            if step.kind == StepKind::Pads && matches!(timed.message, MidiMessage::NoteOn { .. }) {
                println!(
                    "    {}   ({}/{PAD_COUNT} pads)",
                    timed.message,
                    session.distinct_notes()
                );
            } else {
                println!("    {}", timed.message);
            }
        }
        match lines.try_next() {
            Typed::Nothing => {}
            Typed::Eof => return Ok(Listened::Quit),
            Typed::Line(line) => match parse_command(&line) {
                Command::Enter => {
                    if session.accept() {
                        return Ok(Listened::Done);
                    }
                    println!("  Nothing received yet. Do the action, or type s to skip.");
                }
                Command::Skip => {
                    session.skip();
                    return Ok(Listened::Skipped);
                }
                Command::Quit => return Ok(Listened::Quit),
                Command::Redo | Command::Unknown(_) => {
                    println!("  While listening: Enter = finish now, s = skip, q = quit.");
                }
            },
        }
        if session.poll(clock::now_micros()) == StepStatus::Done {
            return Ok(Listened::Done);
        }
    }
}

/// Lights each unprobed button and pad set and asks whether a light came on.
fn probe_leds(fixture: &mut Fixture, leds: &Leds, lines: &Lines, out: &Path) -> anyhow::Result<()> {
    let probes = fixture.led_probes();
    if probes.is_empty() {
        return Ok(());
    }
    println!("\n=== LED check ({} controls) ===", probes.len());
    println!("Each control's light is switched on in turn. Answer y or n (q = save and stop).");
    let total = probes.len();
    for (k, probe) in probes.iter().enumerate() {
        let lit = asked_with_light(leds, probe, 127, || {
            lines.ask_yes_no(&format!(
                "[{}/{total}] {}: did a light turn on? [y/n/q] ",
                k + 1,
                probe.id
            ))
        })?;
        let Some(lit) = lit else {
            save(out, fixture)?;
            return Ok(());
        };
        fixture.controls[probe.control].led = Some(lit);
        if lit && probe.pads {
            println!("  Showing velocities 1, 16, 32, 64, 127 on the pads...");
            for velocity in PROBE_VELOCITIES {
                leds.light(probe, velocity).inspect_err(|_| leds.off())?;
                println!("    velocity {velocity}");
                thread::sleep(PROBE_HOLD);
            }
            leds.off();
            let Some(vary) = lines.ask_yes_no("  Did the color change? [y/n/q] ") else {
                fixture.controls[probe.control].led = None;
                save(out, fixture)?;
                return Ok(());
            };
            fixture.controls[probe.control].led_values_vary = Some(vary);
        }
        save(out, fixture)?;
    }
    Ok(())
}

/// Lights `probe` at `velocity`, runs `ask`, and always switches the light off.
fn asked_with_light<T>(
    leds: &Leds,
    probe: &LedProbe,
    velocity: u8,
    ask: impl FnOnce() -> T,
) -> anyhow::Result<T> {
    let result = leds.light(probe, velocity).map(|()| ask());
    leds.off();
    result
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
