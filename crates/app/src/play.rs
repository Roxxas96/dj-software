//! The `play` command: decode up to four tracks and play them through an output device,
//! optionally driven by a MIDI controller.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, channel, sync_channel};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use audio_io::{
    AudioError, OutputRouting, OutputStream, RoutingError, StreamError, StreamErrorKind,
};
use controller::builtin::load_mapping;
use controller::{Connection, PresenceChange, Runtime, TimedMidi, clock};
use engine::{
    ChannelCommand, Command, DeckCommand, DeckId, DeckSnapshot, EngineController, engine,
};

const TICK: Duration = Duration::from_millis(250);
/// How long the control loop waits for MIDI before doing its periodic work.
const MIDI_WAIT: Duration = Duration::from_millis(10);
/// How often the controller's LEDs are updated (about 30 Hz).
const LED_TICK: Duration = Duration::from_millis(33);
/// How long deck 1 may take to report that it is playing before we give up.
const START_TIMEOUT: Duration = Duration::from_secs(5);
/// The shortest time between two automatic controller reconnects.
const AUTO_RECONNECT_INTERVAL: Duration = Duration::from_secs(5);
const DEVICES_HINT: &str = "run `dj-software devices` to see the available devices and channels";

#[derive(Debug, clap::Args)]
pub struct PlayArgs {
    /// Track to play on deck 1
    pub track: PathBuf,
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
    let track =
        decode::decode_file(path).with_context(|| format!("decoding {}", path.display()))?;
    Ok(Arc::new(track))
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

fn deck_line(deck: DeckId, snapshot: &DeckSnapshot) -> String {
    let rate = f64::from(snapshot.track_sample_rate.max(1));
    let duration = snapshot.duration_frames as f64 / rate;
    let position = (snapshot.position_frames / rate).min(duration);
    format!(
        "Deck {}  {} / {}",
        deck.index() + 1,
        format_time(position),
        format_time(duration)
    )
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
    // Load the mapping before anything slow, so a bad mapping file fails at once.
    let mapping = args
        .controller
        .as_deref()
        .map(|name| load_mapping(name).context("loading the controller mapping"))
        .transpose()?;
    let track = decode(&args.track)?;
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
        Command::Deck(deck(1)?, DeckCommand::Load(track)),
    )?;
    if mapping.is_none() {
        send(&mut controller, Command::Deck(deck(1)?, DeckCommand::Play))?;
    }
    if let Some(cue) = cue_track {
        let deck2 = deck(2)?;
        send(
            &mut controller,
            Command::Deck(deck2, DeckCommand::Load(cue)),
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
                Command::Deck(deck(n)?, DeckCommand::Load(track)),
            )?;
        }
    }

    println!("Playing on {}", pending.device_name());
    let mut stream = pending.start(renderer).context("starting audio output")?;

    match mapping {
        Some(mapping) => control_loop(mapping, &mut controller, &mut stream, &interrupted),
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

/// Sends `()` for every line typed on stdin, from a helper thread so the control loop
/// never blocks on the terminal. Stops at end of input.
fn spawn_enter_reader() -> Receiver<()> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            match stdin.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) if tx.send(()).is_err() => break,
                Ok(_) => {}
            }
        }
    });
    rx
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
    engine: &mut EngineController,
    stream: &mut OutputStream,
    interrupted: &AtomicBool,
) -> anyhow::Result<()> {
    let name = mapping.controller.name.clone();
    let (tx, rx) = channel::<TimedMidi>();
    let mut connection = Connection::new(&mapping, tx).context("opening the controller")?;
    let mut runtime = Runtime::new(mapping);
    let printer = Printer::spawn();
    let enter = std::io::stdin().is_terminal().then(spawn_enter_reader);
    let keys = if enter.is_some() {
        "press Enter to reconnect the controller, Ctrl-C to quit"
    } else {
        "Ctrl-C to quit; Enter-to-reconnect unavailable, stdin is not a terminal"
    };

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

        let manual = enter.as_ref().is_some_and(|rx| rx.try_iter().count() > 0);
        let mut stream_errors = Vec::new();

        if last_status.is_none_or(|last| now.duration_since(last) >= TICK) {
            last_status = Some(now);
            engine.collect_garbage();
            let (warning, errors) = poll_stream_errors(stream, &mut reported_errors);
            if let Some(warning) = warning {
                printer.event(warning);
            }
            stream_errors = errors;
            let snapshots = DeckId::ALL.map(|id| (id, engine.deck(id)));
            printer.status(decks_line(&snapshots));
        }

        if let Some(why) = policy.decide(now, manual, &stream_errors) {
            let change =
                reconnect_controller(&mut runtime, engine, &mut connection, &rx, &mut batch);
            attached_once |= change == PresenceChange::Back;
            printer.event(reconnect_message(change, why));
        }
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

/// Prints on its own thread, so a slow or blocked stdout never stalls the control loop.
/// Lines that do not fit in the queue are dropped.
struct Printer {
    tx: SyncSender<Line>,
    thread: std::thread::JoinHandle<()>,
}

enum Line {
    /// Replaces the current status line.
    Status(String),
    /// A message on its own line.
    Event(String),
}

impl Printer {
    const QUEUE: usize = 64;

    fn spawn() -> Self {
        let (tx, rx) = sync_channel::<Line>(Self::QUEUE);
        let thread = std::thread::spawn(move || {
            let mut stdout = std::io::stdout();
            for line in rx {
                let _ = match line {
                    Line::Status(text) => write!(stdout, "\r{text}"),
                    Line::Event(text) => write!(stdout, "\n{text}\n"),
                };
                let _ = stdout.flush();
            }
        });
        Self { tx, thread }
    }

    /// Prints what is still queued, then stops the thread.
    fn finish(self) {
        drop(self.tx);
        let _ = self.thread.join();
    }

    fn status(&self, text: String) {
        self.send(Line::Status(text));
    }

    fn event(&self, text: String) {
        self.send(Line::Event(text));
    }

    fn send(&self, line: Line) {
        // A full queue drops the line rather than wait.
        let _ = self.tx.try_send(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(text.contains("dj-software devices"));

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
