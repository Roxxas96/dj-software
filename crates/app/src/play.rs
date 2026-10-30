//! The `play` command: decode one or two tracks and play them through an output device.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use audio_io::{AudioError, OutputRouting, RoutingError};
use engine::{ChannelCommand, Command, DeckCommand, DeckId, DeckSnapshot, engine};

const TICK: Duration = Duration::from_millis(250);
/// How long deck 1 may take to report that it is playing before we give up.
const START_TIMEOUT: Duration = Duration::from_secs(5);
const DEVICES_HINT: &str = "run `dj-software devices` to see the available devices and channels";

#[derive(Debug, clap::Args)]
pub struct PlayArgs {
    /// Track to play on deck 1
    pub track: PathBuf,
    /// Track to play on deck 2, heard only in the headphones
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
}

impl PlayArgs {
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

fn deck_line(snapshot: &DeckSnapshot) -> String {
    let rate = f64::from(snapshot.track_sample_rate.max(1));
    let duration = snapshot.duration_frames as f64 / rate;
    let position = (snapshot.position_frames / rate).min(duration);
    format!(
        "Deck 1  {} / {}",
        format_time(position),
        format_time(duration)
    )
}

pub fn play(args: &PlayArgs) -> anyhow::Result<()> {
    let routing = args.routing();
    let track = decode(&args.track)?;
    let cue_track = args.cue_track.as_deref().map(decode).transpose()?;

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
    let deck1 = DeckId::new(0).context("deck 1 missing")?;
    send(
        &mut controller,
        Command::Deck(deck1, DeckCommand::Load(track)),
    )?;
    send(&mut controller, Command::Deck(deck1, DeckCommand::Play))?;
    if let Some(cue) = cue_track {
        let deck2 = DeckId::new(1).context("deck 2 missing")?;
        send(
            &mut controller,
            Command::Deck(deck2, DeckCommand::Load(cue)),
        )?;
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

    println!("Playing on {}", pending.device_name());
    let stream = pending.start(renderer).context("starting audio output")?;

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
        let errors = stream.errors();
        if errors > reported_errors {
            reported_errors = errors;
            eprintln!(
                "\nwarning: the audio stream reported {errors} error(s); playback may glitch"
            );
        }
        let _ = write!(stdout, "\r{}", deck_line(&snapshot));
        let _ = stdout.flush();
        std::thread::sleep(TICK);
    }
    println!();
    Ok(())
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
        assert_eq!(deck_line(&snapshot), "Deck 1  01:05 / 03:20");
    }
}
