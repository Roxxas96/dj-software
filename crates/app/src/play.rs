//! The `play` command: decode up to four tracks and play them through an output device,
//! optionally driven by a MIDI controller.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use actions::{ControlState, PadMode, Side};
use anyhow::{Context, anyhow};
use audio_io::{
    AudioError, OutputRouting, OutputStream, RoutingError, StreamError, StreamErrorKind,
};
use controller::{Connection, PresenceChange, Runtime, TimedMidi, clock};
use engine::{
    ChannelCommand, Command, DeckCommand, DeckId, DeckSnapshot, EngineController, MasterCommand,
    SamplerCommand, SamplerSnapshot, engine,
};

use crate::config::{Config, PadSource};
use crate::console::{self, ConsoleCommand};
use crate::library_mode::LibraryMode;
use crate::printer::Printer;

const TICK: Duration = Duration::from_millis(250);
/// How long the control loop waits for MIDI before doing its periodic work.
const MIDI_WAIT: Duration = Duration::from_millis(10);
/// How often the controller's LEDs are updated (about 30 Hz).
const LED_TICK: Duration = Duration::from_millis(33);
/// How long deck 1 may take to report that it is playing before we give up.
const START_TIMEOUT: Duration = Duration::from_secs(5);
/// The shortest time between two automatic controller reconnects.
const AUTO_RECONNECT_INTERVAL: Duration = Duration::from_secs(5);
const DEVICES_HINT: &str = "run `open-jockey devices` to see the available devices and channels";

#[derive(Debug, clap::Args)]
pub struct PlayArgs {
    /// Track to play on deck 1 (optional with --library)
    #[arg(required_unless_present = "library")]
    pub track: Option<PathBuf>,
    /// Track to play on deck 2, heard only in the headphones (without --controller)
    #[arg(long, value_name = "TRACK2")]
    pub cue_track: Option<PathBuf>,
    /// Output device name (default: the system default device)
    #[arg(long, value_name = "NAME")]
    pub device: Option<String>,
    /// Master output channels, 1-based, as `L,R`
    #[arg(long, value_name = "L,R", default_value = "1,2", value_parser = parse_channel_pair)]
    pub master: [usize; 2],
    /// Headphone output channels, 1-based, as `L,R` (off when omitted)
    #[arg(long, value_name = "L,R", value_parser = parse_channel_pair)]
    pub headphones: Option<[usize; 2]>,
    /// Drive the decks with a MIDI controller: a built-in mapping name (e.g. `ddj-sb3`) or a
    /// mapping file. Every track then loads paused, `--cue-track` only loads deck 2 (no
    /// fader or PFL presets: the controller sets them), only Ctrl-C quits, and Enter
    /// reconnects the controller if it stops responding
    #[arg(long, value_name = "NAME|PATH")]
    pub controller: Option<String>,
    /// Track to load (paused) on deck 3
    #[arg(long, value_name = "TRACK", requires = "controller")]
    pub deck3: Option<PathBuf>,
    /// Track to load (paused) on deck 4
    #[arg(long, value_name = "TRACK", requires = "controller")]
    pub deck4: Option<PathBuf>,
    /// Library mode: browse and load library tracks from the controller (or the
    /// console's `load`), analyze new tracks in the background and remember cue points
    /// and hotcues
    #[arg(long, requires = "controller")]
    pub library: bool,
    /// Library database file for --library (default: library.sqlite3 in the per-user
    /// data directory)
    #[arg(long, value_name = "PATH", requires = "library")]
    pub library_db: Option<PathBuf>,
    /// Configuration file (default: config.toml in the per-user configuration folder)
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,
}

impl PlayArgs {
    /// Why these arguments cannot be used together, if they cannot. Without a controller,
    /// `--cue-track` plays deck 2 in the headphones, so it needs `--headphones`; with one,
    /// headphone cueing comes from the controller and master-only routing is fine.
    pub fn conflict(&self) -> Option<&'static str> {
        (self.cue_track.is_some() && self.headphones.is_none() && self.controller.is_none())
            .then_some("--cue-track needs --headphones (or --controller)")
    }

    /// Routing with 0-based channel indices.
    pub fn routing(&self) -> OutputRouting {
        OutputRouting {
            master: self.master.map(|c| c - 1),
            headphones: self.headphones.map(|pair| pair.map(|c| c - 1)),
        }
    }
}

/// Parses `L,R` with 1-based channel numbers.
fn parse_channel_pair(value: &str) -> Result<[usize; 2], String> {
    let malformed = || format!("expected two channel numbers like `1,2`, got `{value}`");
    let (left, right) = value.split_once(',').ok_or_else(malformed)?;
    let channel = |text: &str| -> Result<usize, String> {
        let number: usize = text.trim().parse().map_err(|_| malformed())?;
        if number == 0 {
            return Err("channel numbers start at 1".to_owned());
        }
        Ok(number)
    };
    Ok([channel(left)?, channel(right)?])
}

/// Describes an audio error for the user, with routing channels in 1-based numbers.
pub fn describe_audio_error(error: &AudioError) -> String {
    match error {
        AudioError::Routing { device, source } => {
            let detail = match source {
                RoutingError::ChannelOutOfRange {
                    index,
                    device_channels,
                } => format!(
                    "routing uses output channel {}, but the device has only {device_channels} {}",
                    index + 1,
                    if *device_channels == 1 {
                        "channel"
                    } else {
                        "channels"
                    }
                ),
                RoutingError::Overlap { index } => {
                    format!("output channel {} is routed more than once", index + 1)
                }
            };
            format!(
                "routing does not fit output device \"{device}\": {detail}\nhint: {DEVICES_HINT}"
            )
        }
        other => other.to_string(),
    }
}

fn decode(path: &Path) -> anyhow::Result<Arc<engine::TrackBuffer>> {
    let decoded =
        decode::decode_file(path).with_context(|| format!("decoding {}", path.display()))?;
    if decoded.truncated {
        eprintln!(
            "warning: {} may be incomplete (decoding stopped early)",
            path.display()
        );
    }
    Ok(Arc::new(decoded.track))
}

/// Decodes the configured sampler pads, reporting each failure (the pad stays empty).
/// Track ids are read through the `--library` library, else the default library when it
/// exists; a library that cannot be opened or read fails only the `track` pads.
fn load_samples(
    config: &Config,
    library: Option<&LibraryMode>,
) -> Vec<(usize, Arc<engine::TrackBuffer>)> {
    let (loaded, lines) = load_samples_from(config, open_sample_library(config, library));
    for line in lines {
        eprintln!("{line}");
    }
    loaded
}

/// The library reader for the `track` pads, with the default library it reads (kept alive
/// while the samples load): `None` when no pad uses a track or there is no library file.
fn open_sample_library(
    config: &Config,
    library: Option<&LibraryMode>,
) -> anyhow::Result<Option<(library::Reader, Option<library::Library>)>> {
    let uses_tracks = config
        .sampler
        .pads
        .iter()
        .any(|p| matches!(p, Some(PadSource::Track(_))));
    if !uses_tracks {
        return Ok(None);
    }
    if let Some(library) = library {
        return Ok(Some((
            library.reader().context("reading the library")?,
            None,
        )));
    }
    let path = crate::library_cmd::db_path(None)?;
    if !path.is_file() {
        return Ok(None);
    }
    let lib = crate::library_cmd::open_library(Some(&path))?;
    let reader = lib.reader().context("reading the library")?;
    Ok(Some((reader, Some(lib))))
}

/// Decodes the pads with the library `opened` gave, returning the samples and the lines to
/// report.
fn load_samples_from(
    config: &Config,
    opened: anyhow::Result<Option<(library::Reader, Option<library::Library>)>>,
) -> (Vec<(usize, Arc<engine::TrackBuffer>)>, Vec<String>) {
    let failure = opened.as_ref().err().map(|e| format!("{e:#}"));
    let reader = match &failure {
        Some(reason) => Err(reason.as_str()),
        None => Ok(opened
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .map(|(reader, _)| reader)),
    };
    let mut loaded = Vec::new();
    let mut lines = Vec::new();
    for (index, source, result) in crate::samples::load(&config.sampler, reader) {
        match result {
            Ok(sample) => {
                if sample.truncated {
                    lines.push(crate::samples::truncated_line(index, source));
                }
                loaded.push((index, sample.buffer));
            }
            Err(error) => lines.push(crate::samples::failure_line(index, source, &error)),
        }
    }
    (loaded, lines)
}

fn send(controller: &mut engine::EngineController, command: Command) -> anyhow::Result<()> {
    controller
        .send(command)
        .map_err(|_| anyhow!("engine command queue is full"))
}

fn format_time(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

/// A deck's time, then `loop N` while it loops, `slip` while slip mode is on, `roll` while a
/// roll or slice is held, and `vinyl off` when its jog only nudges.
pub(crate) fn deck_line(deck: DeckId, snapshot: &DeckSnapshot) -> String {
    let rate = f64::from(snapshot.track_sample_rate.max(1));
    let duration = snapshot.duration_frames as f64 / rate;
    let position = (snapshot.position_frames / rate).min(duration);
    let mut line = format!(
        "Deck {}  {} / {}",
        deck.index() + 1,
        format_time(position),
        format_time(duration)
    );
    if snapshot.loop_active {
        line.push_str(&format!("  loop {}", loop_text(snapshot.loop_beats)));
    }
    if snapshot.slip {
        line.push_str("  slip");
    }
    if snapshot.roll_active {
        line.push_str("  roll");
    }
    if !snapshot.vinyl {
        line.push_str("  vinyl off");
    }
    line
}

/// The pad modes that are not `hardware`, as `left: beat jump   right: slicer`; `None` while
/// both sides are in the hardware mode. The SB3 has no mode lights, so this is where a
/// software pad mode shows.
pub(crate) fn pad_mode_line(state: &ControlState) -> Option<String> {
    let modes: Vec<String> = [(Side::Left, "left"), (Side::Right, "right")]
        .into_iter()
        .filter_map(|(side, name)| {
            let mode = match state.side(side).pad_mode {
                PadMode::Hardware => return None,
                PadMode::BeatJump => "beat jump",
                PadMode::LoopRoll => "loop roll",
                PadMode::Slicer => "slicer",
            };
            Some(format!("{name}: {mode}"))
        })
        .collect();
    (!modes.is_empty()).then(|| modes.join("   "))
}

/// `sampler: 2 5` listing the playing slots (from 1); `None` while none plays.
pub(crate) fn sampler_line(sampler: &SamplerSnapshot) -> Option<String> {
    let slots: Vec<String> = sampler
        .playing
        .iter()
        .enumerate()
        .filter(|(_, playing)| **playing)
        .map(|(index, _)| (index + 1).to_string())
        .collect();
    (!slots.is_empty()).then(|| format!("sampler: {}", slots.join(" ")))
}

/// The status lines a controller session adds after the deck lines: the software pad modes
/// and the playing sampler slots, when there is something to show.
fn extra_status(state: &ControlState, engine: &EngineController) -> Vec<String> {
    pad_mode_line(state)
        .into_iter()
        .chain(sampler_line(&engine.sampler()))
        .collect()
}

/// A loop length in beats: `4`, `1.5`, a fraction of a beat as `1/4` or `3/4` (down to
/// 1/64), else to two decimals; `?` for a length that is not positive and finite.
fn loop_text(beats: f64) -> String {
    if !beats.is_finite() || beats <= 0.0 {
        return "?".to_owned();
    }
    if beats < 1.0 {
        for denominator in [2.0, 4.0, 8.0, 16.0, 32.0, 64.0] {
            let numerator = beats * denominator;
            if (numerator - numerator.round()).abs() < 1e-9 {
                return format!("{}/{denominator}", numerator.round());
            }
        }
    }
    let text = format!("{beats:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// The status line with a controller: every loaded deck, playing ones marked with `>`.
fn decks_line(snapshots: &[(DeckId, DeckSnapshot)]) -> String {
    snapshots
        .iter()
        .filter(|(_, snap)| snap.loaded)
        .map(|(id, snap)| {
            let mark = if snap.playing { '>' } else { ' ' };
            format!("{mark}{}", deck_line(*id, snap))
        })
        .collect::<Vec<_>>()
        .join("   ")
}

pub fn play(args: &PlayArgs) -> anyhow::Result<()> {
    let routing = args.routing();
    // Read the configuration first too: a bad value fails before any device is opened.
    let config = crate::config::load(args.config.as_deref())?;
    // Load the mapping before anything slow, so a bad mapping file fails at once.
    let mapping = args
        .controller
        .as_deref()
        .map(|name| {
            crate::legacy_dirs::load_mapping(name).context("loading the controller mapping")
        })
        .transpose()?;
    // Open the library before the slow decodes too.
    let library = args
        .library
        .then(|| LibraryMode::start(args.library_db.as_deref()))
        .transpose()?;
    let samples = load_samples(&config, library.as_ref());
    let track = args.track.as_deref().map(decode).transpose()?;
    let cue_track = args.cue_track.as_deref().map(decode).transpose()?;
    let deck3_track = args.deck3.as_deref().map(decode).transpose()?;
    let deck4_track = args.deck4.as_deref().map(decode).transpose()?;

    let interrupted = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&interrupted);
    ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed))
        .context("installing the Ctrl-C handler")?;

    let pending =
        audio_io::open_output(args.device.as_deref(), routing).map_err(|error| match error {
            AudioError::Routing { .. } => anyhow!(describe_audio_error(&error)),
            other => anyhow::Error::new(other).context("opening audio device"),
        })?;
    let (mut controller, renderer) = engine(pending.engine_config());
    let deck = |n: usize| DeckId::new(n - 1).with_context(|| format!("deck {n} missing"));
    send(
        &mut controller,
        Command::Master(MasterCommand::CrossfaderEnabled(config.crossfader)),
    )?;
    send(
        &mut controller,
        Command::Sampler(SamplerCommand::Volume(config.sampler.volume)),
    )?;
    for (slot, buffer) in samples {
        send(
            &mut controller,
            Command::Sampler(SamplerCommand::Set(slot, Some(buffer))),
        )?;
    }
    if let Some(track) = track {
        send(
            &mut controller,
            Command::Deck(
                deck(1)?,
                DeckCommand::Load {
                    track,
                    memory: engine::TrackMemory::default(),
                },
            ),
        )?;
        if mapping.is_none() {
            send(&mut controller, Command::Deck(deck(1)?, DeckCommand::Play))?;
        }
    }
    if let Some(cue) = cue_track {
        let deck2 = deck(2)?;
        send(
            &mut controller,
            Command::Deck(
                deck2,
                DeckCommand::Load {
                    track: cue,
                    memory: engine::TrackMemory::default(),
                },
            ),
        )?;
        // With a controller, its faders, PFL buttons and PLAY take over.
        if mapping.is_none() {
            send(
                &mut controller,
                Command::Channel(deck2, ChannelCommand::Fader(0.0)),
            )?;
            send(
                &mut controller,
                Command::Channel(deck2, ChannelCommand::Pfl(true)),
            )?;
            send(&mut controller, Command::Deck(deck2, DeckCommand::Play))?;
        }
    }
    for (n, track) in [(3, deck3_track), (4, deck4_track)] {
        if let Some(track) = track {
            send(
                &mut controller,
                Command::Deck(
                    deck(n)?,
                    DeckCommand::Load {
                        track,
                        memory: engine::TrackMemory::default(),
                    },
                ),
            )?;
        }
    }

    println!("Playing on {}", pending.device_name());
    let mut stream = pending.start(renderer).context("starting audio output")?;

    match mapping {
        Some(mapping) => control_loop(
            mapping,
            &config,
            library,
            &mut controller,
            &mut stream,
            &interrupted,
        ),
        None => playback_loop(deck(1)?, &mut controller, &mut stream, &interrupted),
    }
}

/// Without a controller: show deck 1's time until it stops or Ctrl-C.
fn playback_loop(
    deck1: DeckId,
    controller: &mut EngineController,
    stream: &mut OutputStream,
    interrupted: &AtomicBool,
) -> anyhow::Result<()> {
    let began = Instant::now();
    let mut started = false;
    let mut reported_errors = 0;
    let mut stdout = std::io::stdout();
    while !interrupted.load(Ordering::Relaxed) {
        controller.collect_garbage();
        let snapshot = controller.deck(deck1);
        if snapshot.playing {
            started = true;
        } else if started {
            break;
        } else if began.elapsed() > START_TIMEOUT {
            println!();
            return Err(anyhow!("deck 1 did not start playing"));
        }
        if let (Some(warning), _) = poll_stream_errors(stream, &mut reported_errors) {
            eprintln!("\n{warning}");
        }
        let _ = write!(stdout, "\r{}", deck_line(deck1, &snapshot));
        let _ = stdout.flush();
        std::thread::sleep(TICK);
    }
    println!();
    Ok(())
}

/// The errors the audio stream reported since the last call, with a warning for them
/// (`None` if there are none). `reported` is the count already warned about.
fn poll_stream_errors(
    stream: &mut OutputStream,
    reported: &mut u64,
) -> (Option<String>, Vec<StreamError>) {
    // Errors first: an error is counted before it is queued, so the count read after
    // covers every error taken.
    let taken = stream.take_errors();
    let count = stream.errors();
    if count <= *reported && taken.is_empty() {
        return (None, taken);
    }
    *reported = count;
    (Some(stream_warning_text(count, &taken)), taken)
}

fn stream_warning_text(count: u64, errors: &[StreamError]) -> String {
    let only_xruns = !errors.is_empty() && errors.iter().all(|e| e.kind == StreamErrorKind::Xrun);
    let mut text = if only_xruns {
        format!(
            "warning: audio glitch, the device could not keep up ({count} stream error(s) so far)"
        )
    } else {
        format!("warning: the audio stream reported {count} error(s); playback may glitch")
    };
    for error in errors.iter().filter(|e| e.kind != StreamErrorKind::Xrun) {
        text.push_str("\n  audio error: ");
        text.push_str(&error.message);
    }
    text
}

/// When to drop and reopen the controller's ports.
#[derive(Debug, Default)]
struct ReconnectPolicy {
    last_auto: Option<Instant>,
}

impl ReconnectPolicy {
    /// Why to reconnect now, if at all. A `manual` request (Enter) always reconnects.
    /// Stream errors reconnect only when one may mean the device, and so a link it shares
    /// with the controller, went away (never for an xrun, which macOS reports on every
    /// processor overload), and at most once per [`AUTO_RECONNECT_INTERVAL`].
    fn decide(
        &mut self,
        now: Instant,
        manual: bool,
        errors: &[StreamError],
    ) -> Option<&'static str> {
        if manual {
            return Some("on request");
        }
        if !errors.iter().any(|e| e.kind == StreamErrorKind::DeviceLost) {
            return None;
        }
        if self
            .last_auto
            .is_some_and(|last| now.duration_since(last) < AUTO_RECONNECT_INTERVAL)
        {
            return None;
        }
        self.last_auto = Some(now);
        Some("after an audio/USB hiccup")
    }
}

/// Forces the controller's ports closed and reopened (see [`Runtime::force_reconnect`]).
/// Messages the old ports queued are dropped, so they are not applied to the state after
/// the reconnect; those of the new ports are kept in `batch` for the next round.
fn reconnect_controller(
    runtime: &mut Runtime,
    engine: &mut EngineController,
    connection: &mut Connection,
    rx: &Receiver<TimedMidi>,
    batch: &mut Vec<TimedMidi>,
) -> PresenceChange {
    let change = runtime.force_reconnect(engine, connection);
    // The old input port is closed by now: anything it sent is stamped before this.
    let reopened_at = clock::now_micros();
    batch.retain(|m| m.micros > reopened_at);
    batch.extend(rx.try_iter().filter(|m| m.micros > reopened_at));
    change
}

/// What to print after a forced controller reconnect, `why` saying what caused it.
fn reconnect_message(change: PresenceChange, why: &str) -> String {
    match change {
        PresenceChange::Back => format!("controller reconnected {why}"),
        PresenceChange::Lost => {
            format!("controller lost {why}; waiting for it to come back")
        }
        PresenceChange::None => format!("controller still not found {why}; waiting for it"),
    }
}

/// With a controller: apply its MIDI, drive its LEDs and follow it being unplugged, until
/// Ctrl-C. A finished track does not quit: the DJ may start it again.
///
/// The controller's ports are dropped and reopened when the audio stream reports an error
/// that may mean the device went away (on the DDJ-SB3 audio and MIDI share one USB link,
/// and a dropout shorter than the presence poll can leave the MIDI connection dead), at
/// most every [`AUTO_RECONNECT_INTERVAL`], and when Enter is pressed (stdin a terminal).
fn control_loop(
    mapping: controller::mapping::Mapping,
    config: &Config,
    mut library: Option<LibraryMode>,
    engine: &mut EngineController,
    stream: &mut OutputStream,
    interrupted: &AtomicBool,
) -> anyhow::Result<()> {
    let name = mapping.controller.name.clone();
    let (tx, rx) = channel::<TimedMidi>();
    let mut connection = Connection::new(&mapping, tx).context("opening the controller")?;
    let mut runtime = Runtime::new(mapping);
    // The size tables are not engine values, so there is no soft takeover to arm.
    let state = runtime.control_state_mut();
    if let Some(pads) = config.beat_jump_pads {
        state.beat_jump_pads = pads;
    }
    if let Some(pads) = config.beat_jump_shift_pads {
        state.beat_jump_shift_pads = pads;
    }
    if let Some(pads) = config.loop_roll_pads {
        state.loop_roll_pads = pads;
    }
    let printer = Printer::spawn();
    let lines = std::io::stdin().is_terminal().then(console::spawn_reader);
    let keys = if lines.is_some() {
        "press Enter to reconnect the controller, type `help` for commands, Ctrl-C to quit"
    } else {
        "Ctrl-C to quit; the console (Enter to reconnect) is unavailable, stdin is not a \
         terminal"
    };
    let mut hinted_library = false;

    // `[[init]]` is applied on the first attach only; later ones keep the values set.
    let mut attached_once = false;
    match connection.port_names() {
        Some((input, _)) => {
            printer.event(format!("{name} connected on {input} ({keys})"));
            runtime.attach(engine, &mut connection);
            attached_once = true;
        }
        None => printer.event(format!("{name} not found; waiting for it ({keys})")),
    }

    let mut batch: Vec<TimedMidi> = Vec::new();
    let mut last_led = Instant::now();
    let mut last_status: Option<Instant> = None;
    let mut reported_errors = 0;
    let mut policy = ReconnectPolicy::default();
    while !interrupted.load(Ordering::Relaxed) {
        receive_batch(&rx, &mut batch);
        if !batch.is_empty() {
            let stats = runtime.handle_batch(&batch, engine);
            batch.clear();
            if stats.queue_full > 0 {
                printer.event(format!(
                    "warning: {} control action(s) dropped, the engine queue was full",
                    stats.queue_full
                ));
            }
            for request in stats.requests {
                match library.as_mut() {
                    Some(library) => library.handle_request(request, engine, &printer),
                    None if !hinted_library => {
                        hinted_library = true;
                        printer.event(
                            "library actions (browse, load, beatgrid) need `play --library`"
                                .to_owned(),
                        );
                    }
                    None => {}
                }
            }
        }

        let now = Instant::now();
        if now.duration_since(last_led) >= LED_TICK {
            last_led = now;
            runtime.tick(clock::now_micros(), engine, &mut connection);
        }

        match connection.poll_presence() {
            PresenceChange::None => {}
            PresenceChange::Lost => {
                runtime.on_disconnect(engine);
                printer.event("controller disconnected".to_owned());
            }
            PresenceChange::Back => {
                if attached_once {
                    runtime.on_reconnect(engine, &mut connection);
                    printer.event("controller reconnected".to_owned());
                } else {
                    runtime.attach(engine, &mut connection);
                    attached_once = true;
                    printer.event("controller connected".to_owned());
                }
            }
        }

        let mut manual = false;
        for (line, at) in lines.iter().flat_map(Receiver::try_iter) {
            match console::parse(&line, at) {
                Ok(ConsoleCommand::Reconnect) => manual = true,
                Ok(ConsoleCommand::Help) => printer.event(console::HELP.to_owned()),
                Ok(command) => match library.as_mut() {
                    Some(library) => library.console(command, engine, &printer),
                    None => printer.event("`grid` and `load` need `play --library`".to_owned()),
                },
                Err(message) => printer.event(message),
            }
        }
        let mut stream_errors = Vec::new();

        let library_dirty = match library.as_mut() {
            Some(library) => {
                library.poll(now, engine, &printer);
                library.take_dirty()
            }
            None => false,
        };
        if library_dirty || last_status.is_none_or(|last| now.duration_since(last) >= TICK) {
            last_status = Some(now);
            engine.collect_garbage();
            let (warning, errors) = poll_stream_errors(stream, &mut reported_errors);
            if let Some(warning) = warning {
                printer.event(warning);
            }
            stream_errors = errors;
            match &library {
                Some(library) => {
                    let mut lines = library.status_lines(engine);
                    lines.extend(extra_status(runtime.state(), engine));
                    printer.status_lines(lines);
                }
                None => {
                    let snapshots = DeckId::ALL.map(|id| (id, engine.deck(id)));
                    let mut lines = vec![decks_line(&snapshots)];
                    lines.extend(extra_status(runtime.state(), engine));
                    printer.status_lines(lines);
                }
            }
        }

        if let Some(why) = policy.decide(now, manual, &stream_errors) {
            let change =
                reconnect_controller(&mut runtime, engine, &mut connection, &rx, &mut batch);
            attached_once |= change == PresenceChange::Back;
            printer.event(reconnect_message(change, why));
        }
    }
    if let Some(library) = library {
        library.shutdown(engine, &printer);
    }
    printer.finish();
    println!();
    Ok(())
}

/// Waits up to [`MIDI_WAIT`] for a message, then takes every message already queued.
fn receive_batch(rx: &Receiver<TimedMidi>, batch: &mut Vec<TimedMidi>) {
    match rx.recv_timeout(MIDI_WAIT) {
        Ok(first) => {
            batch.push(first);
            batch.extend(rx.try_iter());
        }
        Err(RecvTimeoutError::Timeout) => {}
        // The connection keeps a sender, so this does not happen; never spin if it does.
        Err(RecvTimeoutError::Disconnected) => std::thread::sleep(MIDI_WAIT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_library_that_cannot_be_read_fails_only_the_track_pads() {
        let mut config = Config::default();
        config.sampler.pads = vec![Some(PadSource::Track(42)), None, Some(PadSource::Track(43))];
        let opened = Err(anyhow!(
            "opening the library /x/library.sqlite3: disk I/O error"
        ));
        let (loaded, lines) = load_samples_from(&config, opened);
        assert!(loaded.is_empty());
        assert_eq!(
            lines,
            [
                "sampler pad 1 (track 42): library error: opening the library \
                 /x/library.sqlite3: disk I/O error",
                "sampler pad 3 (track 43): library error: opening the library \
                 /x/library.sqlite3: disk I/O error",
            ]
        );
    }

    #[test]
    fn routing_errors_use_one_based_channels_and_a_hint() {
        let out_of_range = AudioError::Routing {
            device: "DDJ".to_owned(),
            source: RoutingError::ChannelOutOfRange {
                index: 3,
                device_channels: 2,
            },
        };
        let text = describe_audio_error(&out_of_range);
        assert!(text.contains("output channel 4, but the device has only 2 channels"));
        assert!(text.contains("\"DDJ\""));
        assert!(text.contains("open-jockey devices"));

        let overlap = AudioError::Routing {
            device: "DDJ".to_owned(),
            source: RoutingError::Overlap { index: 0 },
        };
        assert!(describe_audio_error(&overlap).contains("output channel 1 is routed"));
    }

    fn err(kind: StreamErrorKind, message: &str) -> StreamError {
        StreamError {
            kind,
            message: message.to_owned(),
        }
    }

    #[test]
    fn stream_warning_lists_messages() {
        assert_eq!(
            stream_warning_text(1, &[]),
            "warning: the audio stream reported 1 error(s); playback may glitch"
        );
        let text = stream_warning_text(
            2,
            &[
                err(StreamErrorKind::DeviceLost, "device gone"),
                err(StreamErrorKind::Other, "busy"),
            ],
        );
        assert!(text.ends_with("\n  audio error: device gone\n  audio error: busy"));
        let text = stream_warning_text(3, &[err(StreamErrorKind::Xrun, "xrun")]);
        assert!(text.starts_with("warning: audio glitch"), "{text}");
        assert!(!text.contains("audio error"), "{text}");
    }

    #[test]
    fn xrun_never_triggers_a_reconnect() {
        let mut policy = ReconnectPolicy::default();
        let start = Instant::now();
        for i in 0..20 {
            let now = start + AUTO_RECONNECT_INTERVAL * i;
            let errors = [err(StreamErrorKind::Xrun, "xrun")];
            assert_eq!(policy.decide(now, false, &errors), None);
        }
        let other = [err(StreamErrorKind::Other, "busy")];
        assert_eq!(policy.decide(start, false, &other), None);
        assert_eq!(policy.decide(start, false, &[]), None);
    }

    #[test]
    fn device_lost_triggers_a_reconnect_at_most_every_interval() {
        let mut policy = ReconnectPolicy::default();
        let lost = [
            err(StreamErrorKind::Xrun, "xrun"),
            err(StreamErrorKind::DeviceLost, "gone"),
        ];
        let start = Instant::now();
        assert_eq!(
            policy.decide(start, false, &lost),
            Some("after an audio/USB hiccup")
        );
        let soon = start + Duration::from_secs(2);
        assert_eq!(policy.decide(soon, false, &lost), None, "within 5 s");
        let later = start + AUTO_RECONNECT_INTERVAL;
        assert!(policy.decide(later, false, &lost).is_some());
    }

    #[test]
    fn enter_bypasses_the_interval() {
        let mut policy = ReconnectPolicy::default();
        let lost = [err(StreamErrorKind::DeviceLost, "gone")];
        let start = Instant::now();
        assert!(policy.decide(start, false, &lost).is_some());
        let soon = start + Duration::from_millis(100);
        assert_eq!(policy.decide(soon, true, &[]), Some("on request"));
        assert_eq!(policy.decide(soon, true, &lost), Some("on request"));
        assert_eq!(policy.decide(soon, false, &lost), None);
    }

    #[test]
    fn reconnect_messages() {
        assert_eq!(
            reconnect_message(PresenceChange::Back, "after an audio/USB hiccup"),
            "controller reconnected after an audio/USB hiccup"
        );
        assert!(reconnect_message(PresenceChange::Lost, "on request").contains("waiting"));
        assert!(reconnect_message(PresenceChange::None, "on request").contains("not found"));
    }

    #[test]
    fn deck_line_shows_the_loop_and_vinyl_off() {
        let deck = DeckId::new(0).unwrap();
        let snapshot = DeckSnapshot {
            loaded: true,
            position_frames: 44_100.0 * 65.0,
            duration_frames: 44_100 * 200,
            track_sample_rate: 44_100,
            ..DeckSnapshot::default()
        };
        let looping = |beats| DeckSnapshot {
            loop_active: true,
            loop_beats: beats,
            ..snapshot
        };
        assert_eq!(
            deck_line(deck, &looping(4.0)),
            "Deck 1  01:05 / 03:20  loop 4"
        );
        assert_eq!(
            deck_line(deck, &looping(64.0)),
            "Deck 1  01:05 / 03:20  loop 64"
        );
        assert_eq!(
            deck_line(deck, &looping(0.25)),
            "Deck 1  01:05 / 03:20  loop 1/4"
        );
        assert_eq!(
            deck_line(deck, &looping(1.0 / 32.0)),
            "Deck 1  01:05 / 03:20  loop 1/32"
        );
        // A set loop that is not active is not shown.
        let inactive = DeckSnapshot {
            loop_in: Some(0.0),
            loop_out: Some(100.0),
            ..snapshot
        };
        assert_eq!(deck_line(deck, &inactive), "Deck 1  01:05 / 03:20");
        let vinyl_off = DeckSnapshot {
            vinyl: false,
            ..looping(2.0)
        };
        assert_eq!(
            deck_line(deck, &vinyl_off),
            "Deck 1  01:05 / 03:20  loop 2  vinyl off"
        );
    }

    #[test]
    fn deck_line_shows_slip_and_roll() {
        let deck = DeckId::new(0).unwrap();
        let snapshot = DeckSnapshot {
            loaded: true,
            position_frames: 44_100.0 * 65.0,
            duration_frames: 44_100 * 200,
            track_sample_rate: 44_100,
            ..DeckSnapshot::default()
        };
        let slip = DeckSnapshot {
            slip: true,
            ..snapshot
        };
        assert_eq!(deck_line(deck, &slip), "Deck 1  01:05 / 03:20  slip");
        let rolling = DeckSnapshot {
            roll_active: true,
            loop_active: true,
            loop_beats: 2.0,
            ..slip
        };
        assert_eq!(
            deck_line(deck, &rolling),
            "Deck 1  01:05 / 03:20  loop 2  slip  roll"
        );
    }

    #[test]
    fn pad_mode_and_sampler_lines_show_only_when_active() {
        let mut state = ControlState::default();
        assert_eq!(pad_mode_line(&state), None);
        state.side_mut(Side::Left).pad_mode = PadMode::BeatJump;
        assert_eq!(pad_mode_line(&state).as_deref(), Some("left: beat jump"));
        state.side_mut(Side::Right).pad_mode = PadMode::LoopRoll;
        assert_eq!(
            pad_mode_line(&state).as_deref(),
            Some("left: beat jump   right: loop roll")
        );
        state.side_mut(Side::Left).pad_mode = PadMode::Hardware;
        state.side_mut(Side::Right).pad_mode = PadMode::Slicer;
        assert_eq!(pad_mode_line(&state).as_deref(), Some("right: slicer"));

        let mut sampler = SamplerSnapshot::default();
        assert_eq!(sampler_line(&sampler), None);
        sampler.playing[1] = true;
        sampler.playing[4] = true;
        assert_eq!(sampler_line(&sampler).as_deref(), Some("sampler: 2 5"));
    }

    #[test]
    fn loop_lengths_read_sensibly() {
        for (beats, text) in [
            (4.0, "4"),
            (1.0, "1"),
            (64.0, "64"),
            (0.5, "1/2"),
            (1.0 / 32.0, "1/32"),
            (0.75, "3/4"),
            (1.5, "1.5"),
            (0.3, "0.3"),
            (0.0, "?"),
            (-1.0, "?"),
            (f64::NAN, "?"),
            (f64::INFINITY, "?"),
        ] {
            assert_eq!(loop_text(beats), text, "{beats}");
        }
    }

    #[test]
    fn formats_deck_time() {
        let snapshot = DeckSnapshot {
            loaded: true,
            playing: true,
            position_frames: 44_100.0 * 65.0,
            duration_frames: 44_100 * 200,
            track_sample_rate: 44_100,
            tempo_ratio: 1.0,
            cue_frames: 0.0,
            previewing: false,
            ..DeckSnapshot::default()
        };
        assert_eq!(
            deck_line(DeckId::new(0).unwrap(), &snapshot),
            "Deck 1  01:05 / 03:20"
        );
        let idle = DeckSnapshot {
            loaded: false,
            ..snapshot
        };
        let paused = DeckSnapshot {
            playing: false,
            ..snapshot
        };
        let line = decks_line(&[
            (DeckId::new(0).unwrap(), snapshot),
            (DeckId::new(1).unwrap(), idle),
            (DeckId::new(2).unwrap(), paused),
        ]);
        assert_eq!(line, ">Deck 1  01:05 / 03:20    Deck 3  01:05 / 03:20");
    }
}
