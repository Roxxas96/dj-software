//! The `midi` commands: list ports, monitor raw messages and run a guided capture.

use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use actions::{Input, Target};
use anyhow::{Context, anyhow, bail};
use controller::builtin::load_mapping;
use controller::capture::{
    CaptureSession, Fixture, LedProbe, LedSink, Meta, PROBE_VELOCITIES, Question, Redo, Source,
    Step, StepKind, StepStatus, parse_checklist, parse_fixture, run_led_probe,
};
use controller::mapping::{Mapping, Resolved};
use controller::{
    Connection, MidiMessage, OutputPort, PortError, PresenceChange, Runtime, TimedMidi, clock,
    input_port_names, open_input, open_output, output_port_names,
};
use engine::{EngineConfig, engine};

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
pub(crate) fn choose_port(requested: Option<&str>, inputs: &[String]) -> anyhow::Result<String> {
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

/// How long the controller monitor waits for MIDI before its periodic work.
const MONITOR_WAIT: Duration = Duration::from_millis(10);
/// How often the controller monitor updates the LEDs.
const MONITOR_LED_TICK: Duration = Duration::from_millis(33);

/// `midi monitor --controller`: prints each message of the mapping's controller with what
/// it does, e.g. `→ deck.play_pause deck 1 Press` or `→ (unbound)`.
///
/// The actions are applied to an engine with no audio device (rendered here, silently,
/// only so its command queue drains), so SHIFT, deck switching and soft takeover behave
/// exactly as when playing.
pub fn monitor_controller(name_or_path: &str) -> anyhow::Result<()> {
    let mapping = load_mapping(name_or_path).context("loading the controller mapping")?;
    let name = mapping.controller.name.clone();

    let interrupted = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&interrupted);
    ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed))
        .context("installing the Ctrl-C handler")?;

    let config = EngineConfig {
        sample_rate: 44_100,
        max_block_frames: 64,
    };
    let (mut engine, mut renderer) = engine(config);
    let mut master = vec![0.0; config.max_block_frames * 2];
    let mut headphones = master.clone();

    let (tx, rx) = channel();
    let mut connection = Connection::new(&mapping, tx).context("opening the controller")?;
    let bound = mapping.clone();
    let mut runtime = Runtime::new(mapping);
    let mut attached_once = false;
    match connection.port_names() {
        Some((input, _)) => {
            println!("Monitoring {name} on {input} (Ctrl-C to stop)");
            runtime.attach(&mut engine, &mut connection);
            attached_once = true;
        }
        None => println!("{name} not found; waiting for it (Ctrl-C to stop)"),
    }

    let started = clock::now_micros();
    let mut batch: Vec<TimedMidi> = Vec::new();
    let mut last_led = std::time::Instant::now();
    while !interrupted.load(Ordering::Relaxed) {
        match rx.recv_timeout(MONITOR_WAIT) {
            Ok(first) => {
                batch.push(first);
                batch.extend(rx.try_iter());
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if !batch.is_empty() {
            runtime.handle_batch(&batch, &mut engine);
            for ((msg, resolved), timed) in runtime.last_resolved().iter().zip(&batch) {
                let ms = timed.micros.saturating_sub(started) / 1000;
                println!("{ms:>8} ms  {msg}  {}", describe(&bound, msg, resolved));
            }
            batch.clear();
        }
        renderer.render(&mut master, &mut headphones);
        engine.collect_garbage();
        if last_led.elapsed() >= MONITOR_LED_TICK {
            last_led = std::time::Instant::now();
            runtime.tick(clock::now_micros(), &engine, &mut connection);
        }
        match connection.poll_presence() {
            PresenceChange::None => {}
            PresenceChange::Lost => {
                runtime.on_disconnect(&mut engine);
                println!("controller disconnected");
            }
            PresenceChange::Back => {
                if attached_once {
                    runtime.on_reconnect(&mut engine, &mut connection);
                } else {
                    runtime.attach(&mut engine, &mut connection);
                    attached_once = true;
                }
                println!("controller reconnected");
            }
        }
    }
    Ok(())
}

/// What `msg` did: each resolved action as `→ <action> <target> <input>`. A message no
/// binding listens to is `(unbound)`; a bound one that did nothing (soft takeover not
/// reached yet, or a SHIFT/deck condition not met) is `(no action)`.
fn describe(mapping: &Mapping, msg: &MidiMessage, resolved: &Resolved) -> String {
    if resolved.is_empty() {
        let bound = mapping.inputs.iter().any(|b| b.midi.matches(msg));
        return if bound {
            "→ (no action)"
        } else {
            "→ (unbound)"
        }
        .to_owned();
    }
    resolved
        .iter()
        .map(|(action, input)| {
            let target = match action.target() {
                Target::Deck(deck) => format!(" deck {}", deck.index() + 1),
                Target::Side(side) => format!(" side {}", format!("{side:?}").to_lowercase()),
                Target::Global => String::new(),
            };
            let input = match input {
                Input::Press => "Press".to_owned(),
                Input::Release => "Release".to_owned(),
                Input::Absolute(v) => format!("{v:.3}"),
                Input::Relative(n) => format!("{n:+}"),
            };
            format!("→ {}{target} {input}", action.id())
        })
        .collect::<Vec<_>>()
        .join("  ")
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
    /// `x`: what was just recorded is noise.
    Noise,
    /// `g <step id>`.
    Goto(String),
    /// `u <status> <data1>`: record an ignored source again.
    Unignore(String),
    Quit,
    Unknown(String),
}

fn parse_command(line: &str) -> Command {
    let line = line.trim();
    if let Some(id) = line
        .strip_prefix("g ")
        .or_else(|| (line == "g").then_some(""))
    {
        return Command::Goto(id.trim().to_owned());
    }
    if let Some(arg) = line
        .strip_prefix("u ")
        .or_else(|| (line == "u").then_some(""))
    {
        return Command::Unignore(arg.trim().to_owned());
    }
    match line.to_lowercase().as_str() {
        "" => Command::Enter,
        "s" | "skip" => Command::Skip,
        "r" | "redo" => Command::Redo,
        "x" => Command::Noise,
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
pub(crate) fn write_atomic(path: &Path, contents: &str) -> anyhow::Result<()> {
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
pub(crate) struct Lines(Receiver<String>);

enum Typed {
    Line(String),
    Nothing,
    Eof,
}

impl Lines {
    pub(crate) fn spawn() -> Lines {
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
    pub(crate) fn ask(&self, prompt: &str) -> Option<String> {
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
    pub(crate) fn drain(&self) {
        while self.0.try_recv().is_ok() {}
    }

    /// Like [`Lines::ask_yes_no`], but an empty line means `default`.
    fn ask_default(&self, prompt: &str, default: bool) -> Option<bool> {
        loop {
            let line = self.ask(prompt)?;
            if line.trim().is_empty() {
                return Some(default);
            }
            match parse_answer(&line) {
                Answer::Yes => return Some(true),
                Answer::No => return Some(false),
                Answer::Quit => return None,
                Answer::Invalid => println!("  Please answer y, n or q."),
            }
        }
    }

    /// Asks until the answer is y, n or q; `None` for q or end of input.
    pub(crate) fn ask_yes_no(&self, prompt: &str) -> Option<bool> {
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

/// One help line, shown at the start and on an unknown command.
const KEYS_HELP: &str = "Keys (then Enter): Enter = finish now / keep, r = redo, x = mark as noise, u <status> <data1> = stop ignoring a source, s = skip, g <step> = go to a step, q = save and quit";

/// `dj-software midi capture`: walks the checklist, recording each control,
/// confirming it, and probing its LED. The fixture is saved after every step.
/// With `redo` ids, only those steps of the existing fixture are captured again.
pub fn capture(
    checklist_path: &Path,
    out: &Path,
    port: Option<&str>,
    redo: &[String],
) -> anyhow::Result<()> {
    let text = fs::read_to_string(checklist_path)
        .with_context(|| format!("reading {}", checklist_path.display()))?;
    let checklist = parse_checklist(&text)
        .with_context(|| format!("loading checklist {}", checklist_path.display()))?;
    if checklist.steps.is_empty() {
        bail!("checklist {} has no steps", checklist_path.display());
    }
    if !redo.is_empty() && !out.exists() {
        bail!(
            "--redo needs an existing fixture, but {} does not exist",
            out.display()
        );
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
    let session = if redo.is_empty() {
        open_session(checklist.steps, &checklist.ignore, out, &lines)?
    } else {
        Some(open_redo_session(
            checklist.steps,
            &checklist.ignore,
            out,
            redo,
            &lines,
        )?)
    };
    let Some(mut session) = session else {
        return Ok(());
    };
    session.set_meta(meta);
    save(out, &session)?;

    println!("Capturing from {} into {}", input.name(), out.display());
    println!("Each step listens as soon as it is shown: just do what it says.");
    println!(
        "After each step, check the summary: Enter keeps it, r records it again, x marks what was recorded as noise."
    );
    println!("When a step asks you to prepare something first, do it, then press Enter.");
    println!("{KEYS_HELP}");
    let seeded = &checklist.ignore;
    if !run_steps(&mut session, &rx, &lines, seeded, out, leds.as_deref())? {
        println!(
            "\nSaved {}. Run the same command again to continue.",
            out.display()
        );
        return Ok(());
    }
    let fixture = session.finish();
    println!(
        "\nDone: {} captured, {} skipped, {} ignored sources. Saved {}.",
        fixture.controls.len(),
        fixture.meta.skipped.len(),
        fixture.meta.ignored.len(),
        out.display()
    );
    Ok(())
}

fn load_prior(out: &Path) -> anyhow::Result<Fixture> {
    let text = fs::read_to_string(out).with_context(|| format!("reading {}", out.display()))?;
    parse_fixture(&text).with_context(|| {
        format!(
            "{} exists but is not a capture fixture; move it away or choose another --out",
            out.display()
        )
    })
}

/// Applies the checklist's ignore list to a loaded fixture, then offers any
/// noise suggestions.
fn apply_ignores(session: &mut CaptureSession, ignore: &[Source], lines: &Lines) {
    print_notices(session);
    let reopened = session.ignore(ignore);
    if !reopened.is_empty() {
        println!(
            "note: only ignored sources were recorded for {}; asking again",
            reopened.join(", ")
        );
    }
    confirm_noise(session, lines);
}

/// Prints the session's notices (sources no longer ignored, steps reopened
/// because they held only knob data).
fn print_notices(session: &mut CaptureSession) {
    for notice in session.take_notices() {
        println!("  note: {notice}");
    }
}

/// Asks about each noise suggestion: yes (the default) ignores it, no means
/// it is never suggested again.
fn confirm_noise(session: &mut CaptureSession, lines: &Lines) {
    for noise in session.take_noise() {
        let sources: Vec<String> = noise.sources.iter().map(Source::to_string).collect();
        let question = format!(
            "  These look like noise: {} (in {}). Ignore them? [Y/n] ",
            sources.join(", "),
            noise.ids.join(", ")
        );
        match lines.ask_default(&question, true) {
            Some(true) => {
                let reopened = session.ignore(&noise.sources);
                print_ignored(&noise.sources, &reopened);
            }
            Some(false) => session.decline_noise(&noise.sources),
            None => {}
        }
    }
}

/// Reports newly ignored sources, with how to undo it.
fn print_ignored(sources: &[Source], reopened: &[String]) {
    let list: Vec<String> = sources.iter().map(Source::to_string).collect();
    println!("  ignoring from now on: {}", list.join(", "));
    if !reopened.is_empty() {
        println!(
            "  only ignored sources were recorded for {}; asking again",
            reopened.join(", ")
        );
    }
    let undo: Vec<String> = sources.iter().map(|s| format!("u {s}")).collect();
    println!(
        "  (to undo: {}; recordings already stripped are not restored, use g <step> to redo them)",
        undo.join(", ")
    );
}

/// Parses `B5 02` (hex, `0x` optional) into a source.
fn parse_source(arg: &str) -> Option<Source> {
    let mut bytes = arg.split_whitespace().map(|token| {
        let token = token.trim_start_matches("0x").trim_start_matches("0X");
        u8::from_str_radix(token, 16).ok()
    });
    let status = bytes.next()??;
    let data1 = bytes.next()??;
    if bytes.next().is_some() || !(0x80..0xF0).contains(&status) || data1 > 0x7F {
        return None;
    }
    Some(Source { status, data1 })
}

/// Handles `u <status> <data1>`.
fn unignore(session: &mut CaptureSession, arg: &str, seeded: &[Source]) {
    let Some(source) = parse_source(arg) else {
        println!("  usage: u <status> <data1> in hex, e.g. u B5 02");
        return;
    };
    if !session.unignore(source) {
        println!("  {source} is not ignored");
        return;
    }
    println!("  {source} ({}) is recorded again", source.describe());
    if seeded.iter().any(|s| s.key() == source.key()) {
        println!(
            "  warning: it is in the checklist's [ignore] list, so the next run ignores it again; remove it there to make this permanent"
        );
    }
}

/// A new session, or one resumed from an existing `out` if the user agrees.
/// `None` means the user chose to stop.
fn open_session(
    steps: Vec<Step>,
    ignore: &[Source],
    out: &Path,
    lines: &Lines,
) -> anyhow::Result<Option<CaptureSession>> {
    let fresh = |steps: Vec<Step>| {
        let mut session = CaptureSession::new(steps);
        session.ignore(ignore);
        session
    };
    if !out.exists() {
        return Ok(Some(fresh(steps)));
    }
    let prior = load_prior(out)?;
    let unknown: Vec<String> = prior
        .controls
        .iter()
        .map(|c| c.id.clone())
        .filter(|id| !steps.iter().any(|s| &s.id == id))
        .collect();
    let mut session = CaptureSession::resume(steps.clone(), prior);
    apply_ignores(&mut session, ignore, lines);
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
            println!("Every step is done (use --redo <step> to capture some again).");
            Some(false)
        }
    };
    match resume {
        Some(true) => Ok(Some(session)),
        Some(false) => {
            let again = format!("Start over and overwrite {}? [y/n] ", out.display());
            if lines.ask_yes_no(&again) == Some(true) {
                Ok(Some(fresh(steps)))
            } else {
                Ok(None)
            }
        }
        None => Ok(None),
    }
}

/// `--redo`: the existing fixture, with only the listed steps to capture.
fn open_redo_session(
    steps: Vec<Step>,
    ignore: &[Source],
    out: &Path,
    redo: &[String],
    lines: &Lines,
) -> anyhow::Result<CaptureSession> {
    let prior = load_prior(out)?;
    let mut session = CaptureSession::resume(steps, prior);
    if let Err(unknown) = session.redo_only(redo) {
        bail!(
            "--redo: no step {unknown:?}{}",
            close_matches(&session, &unknown)
        );
    }
    println!("Capturing again: {}", redo.join(", "));
    // In redo mode, steps emptied by ignores are queued after the listed ones.
    apply_ignores(&mut session, ignore, lines);
    Ok(session)
}

/// `; close matches: a, b` or an empty string.
fn close_matches(session: &CaptureSession, query: &str) -> String {
    let matches = session.find_steps(query);
    if matches.is_empty() {
        return String::new();
    }
    let shown: Vec<&str> = matches.iter().take(8).map(String::as_str).collect();
    let more = if matches.len() > shown.len() {
        ", ..."
    } else {
        ""
    };
    format!("; close matches: {}{more}", shown.join(", "))
}

/// Handles `g <step>`: jumps there, or explains why not.
fn go_to(session: &mut CaptureSession, id: &str) {
    if id.is_empty() {
        println!("  usage: g <step id>, e.g. g deck1.play");
    } else if session.goto(id) {
        println!("  going to {id}; afterwards you come back here");
    } else {
        println!("  unknown step {id:?}{}", close_matches(session, id));
    }
}

/// Runs the steps until none is left. Returns false if the user quit early.
fn run_steps(
    session: &mut CaptureSession,
    rx: &Receiver<TimedMidi>,
    lines: &Lines,
    seeded: &[Source],
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
            match lines.ask(
                "  Press Enter when ready (s = skip, r = back one step, g <step>, q = quit) > ",
            ) {
                None => Listened::Quit,
                Some(line) => match parse_command(&line) {
                    Command::Enter => listen(session, &step, rx, lines, seeded)?,
                    Command::Skip => {
                        session.skip();
                        Listened::Skipped
                    }
                    Command::Redo => Listened::Redo,
                    Command::Goto(id) => Listened::Goto(id),
                    Command::Quit => Listened::Quit,
                    Command::Unignore(arg) => {
                        unignore(session, &arg, seeded);
                        continue;
                    }
                    Command::Noise | Command::Unknown(_) => {
                        println!("  {KEYS_HELP}");
                        shown_section = None;
                        continue;
                    }
                },
            }
        } else {
            listen(session, &step, rx, lines, seeded)?
        };
        match outcome {
            Listened::Done => {
                print_notices(session);
                confirm_noise(session, lines);
                // A capture that turned out to be all noise is asked again.
                let refocused = session.current().is_some_and(|c| c.id == step.id);
                let control = session.control(&step.id).filter(|_| !refocused);
                let Some(control) = control else {
                    println!("  only ignored sources or knob data were recorded; listening again");
                    save(out, session)?;
                    continue;
                };
                println!("  recorded {}", control.summary());
                let probe = LedProbe::for_step(&step, control);
                match review(session, &step, lines, seeded) {
                    Review::Keep => {
                        save(out, session)?;
                        if let (Some(probe), Some(leds)) = (probe, leds)
                            && !check_led(session, leds, lines, &probe)
                        {
                            save(out, session)?;
                            return Ok(false);
                        }
                    }
                    Review::Again => {}
                    Review::Quit => {
                        save(out, session)?;
                        return Ok(false);
                    }
                }
            }
            Listened::Skipped => println!("  skipped {}", step.id),
            Listened::Redo => match session.redo() {
                Redo::Restarted => println!("  starting {} over", step.id),
                Redo::Reopened => {
                    if let Some(again) = session.current() {
                        println!("  back to {}", again.id);
                    }
                }
                Redo::Nothing => println!("  nothing to go back to"),
            },
            Listened::Goto(id) => go_to(session, &id),
            Listened::Quit => {
                save(out, session)?;
                return Ok(false);
            }
        }
        save(out, session)?;
    }
    Ok(true)
}

enum Review {
    /// Keep the recording (then save and probe the LED).
    Keep,
    /// The step (or another, with `g`) is captured again.
    Again,
    Quit,
}

/// Asks whether to keep the step just recorded.
fn review(session: &mut CaptureSession, step: &Step, lines: &Lines, seeded: &[Source]) -> Review {
    loop {
        let Some(line) = lines.ask(
            "  Keep it? Enter = keep, r = record again, x = that was noise, g <step>, u <src>, q = quit > ",
        ) else {
            return Review::Quit;
        };
        match parse_command(&line) {
            Command::Enter => return Review::Keep,
            Command::Redo => {
                // Drop this recording (the previous one, if any, comes back).
                session.rebind(&step.id);
                println!("  recording {} again", step.id);
                return Review::Again;
            }
            Command::Noise => {
                let varies = session
                    .control(&step.id)
                    .is_some_and(|c| c.messages.iter().any(|m| m.min != m.max));
                if varies
                    && lines.ask_default(
                        "  This includes a source that changed value (looks like a real control). Ignore anyway? [y/N] ",
                        false,
                    ) != Some(true)
                {
                    continue;
                }
                let marked = session.mark_noise(&step.id);
                print_ignored(&marked, &[]);
                println!("  recording {} again", step.id);
                return Review::Again;
            }
            Command::Unignore(arg) => unignore(session, &arg, seeded),
            Command::Goto(id) => {
                // Keep this step, then jump; otherwise explain and ask again.
                if !id.is_empty() && session.goto(&id) {
                    println!("  kept {}; going to {id}, then back here", step.id);
                    return Review::Keep;
                }
                go_to(session, &id);
            }
            Command::Quit => return Review::Quit,
            Command::Skip | Command::Unknown(_) => println!("  {KEYS_HELP}"),
        }
    }
}

enum Listened {
    Done,
    Skipped,
    Redo,
    Goto(String),
    Quit,
}

/// Feeds MIDI into the session until the current step completes, echoing each message.
fn listen(
    session: &mut CaptureSession,
    step: &Step,
    rx: &Receiver<TimedMidi>,
    lines: &Lines,
    seeded: &[Source],
) -> anyhow::Result<Listened> {
    // Drop anything sent before listening (setup, fiddling between steps).
    while rx.try_recv().is_ok() {}
    match step.kind {
        StepKind::Pads => println!("  Listening: tap all {PAD_COUNT} pads..."),
        _ => println!("  Listening..."),
    }
    let dim = io::stdout().is_terminal();
    loop {
        let first = match rx.recv_timeout(CAPTURE_POLL) {
            Ok(timed) => Some(timed),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => bail!("the MIDI input closed"),
        };
        for timed in first.into_iter().chain(rx.try_iter()) {
            if session.is_ignored(timed.message) {
                let line = format!("    {}   (ignored)", timed.message);
                if dim {
                    println!("\x1b[2m{line}\x1b[0m");
                } else {
                    println!("{line}");
                }
                continue;
            }
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
                Command::Goto(id) => return Ok(Listened::Goto(id)),
                Command::Quit => return Ok(Listened::Quit),
                Command::Unignore(arg) => unignore(session, &arg, seeded),
                Command::Noise | Command::Unknown(_) => println!("  {KEYS_HELP}"),
            },
        }
        if session.poll(clock::now_micros()) == StepStatus::Done {
            return Ok(Listened::Done);
        }
    }
}

/// Extra text after an echoed message: the held SHIFT, or the pad count.
fn echo_note(session: &CaptureSession, step: &Step, message: MidiMessage) -> String {
    if session.is_modifier(message) {
        return "   (SHIFT, not recorded)".to_owned();
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
    fn describes_what_a_message_did() {
        use actions::{Action, Side};
        use engine::DeckId;

        let mapping = load_mapping("ddj-sb3").unwrap();
        let play = MidiMessage::NoteOn {
            channel: 0,
            note: 0x0B,
            velocity: 127,
        };
        let deck1 = DeckId::new(0).unwrap();
        assert_eq!(
            describe(
                &mapping,
                &play,
                &vec![(Action::PlayPause(deck1), Input::Press)]
            ),
            "→ deck.play_pause deck 1 Press"
        );
        assert_eq!(
            describe(
                &mapping,
                &play,
                &vec![
                    (Action::Shift(Side::Left), Input::Release),
                    (Action::Crossfader, Input::Absolute(0.25)),
                ]
            ),
            "→ modifier.shift side left Release  → master.crossfader 0.250"
        );
        assert_eq!(describe(&mapping, &play, &vec![]), "→ (no action)");
        let sync = MidiMessage::NoteOn {
            channel: 0,
            note: 0x58,
            velocity: 127,
        };
        assert_eq!(describe(&mapping, &sync, &vec![]), "→ (unbound)");
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
        assert_eq!(parse_command("x"), Command::Noise);
        assert_eq!(
            parse_command("g deck1.play"),
            Command::Goto("deck1.play".to_owned())
        );
        assert_eq!(
            parse_command(" g  deck1.play "),
            Command::Goto("deck1.play".to_owned())
        );
        assert_eq!(parse_command("g"), Command::Goto(String::new()));
        assert_eq!(parse_command("go"), Command::Unknown("go".to_owned()));
        assert_eq!(
            parse_command("u B5 02"),
            Command::Unignore("B5 02".to_owned())
        );
        assert_eq!(parse_command("u"), Command::Unignore(String::new()));
    }

    #[test]
    fn parses_sources_in_hex() {
        assert_eq!(
            parse_source("B5 02"),
            Some(Source {
                status: 0xB5,
                data1: 0x02
            })
        );
        assert_eq!(
            parse_source("0xb5 0x22"),
            Some(Source {
                status: 0xB5,
                data1: 0x22
            })
        );
        assert_eq!(parse_source("B5"), None);
        assert_eq!(parse_source("B5 02 03"), None);
        assert_eq!(parse_source("35 02"), None);
        assert_eq!(parse_source("B5 80"), None);
        assert_eq!(parse_source("zz 02"), None);
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
