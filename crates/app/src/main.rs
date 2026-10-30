//! Command-line entry point: argument parsing and dispatch.

mod led_scan;
mod midi_cmd;
mod play;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};

use play::{PlayArgs, play};

#[derive(Debug, Parser)]
#[command(version, about = "Open-source DJ software")]
struct Cli {
    #[command(subcommand)]
    command: Sub,
}

#[derive(Debug, Subcommand)]
enum Sub {
    /// List audio output devices
    Devices,
    /// Play a track on deck 1 to the master output, or up to four decks with a controller
    Play(PlayArgs),
    /// MIDI controller tools
    #[command(subcommand)]
    Midi(MidiCommand),
}

#[derive(Debug, Subcommand)]
enum MidiCommand {
    /// List MIDI input and output ports
    Ports,
    /// Print raw MIDI messages until Ctrl-C
    Monitor {
        /// Input port name (substring; default: the first port containing "DDJ")
        #[arg(long, value_name = "NAME")]
        port: Option<String>,
        /// Also show what each message does in this mapping (a built-in name such as
        /// `ddj-sb3`, or a mapping file); the port comes from the mapping
        #[arg(long, value_name = "NAME|PATH", conflicts_with = "port")]
        controller: Option<String>,
    },
    /// Guided capture of every control's MIDI messages into a TOML fixture
    Capture {
        /// Checklist of controls to capture (e.g. controllers/ddj-sb3/checklist.toml)
        #[arg(long, value_name = "PATH")]
        checklist: PathBuf,
        /// Fixture to write; if it exists, the capture can continue where it stopped
        #[arg(long, value_name = "PATH")]
        out: PathBuf,
        /// Port name (substring; default: the first input containing "DDJ")
        #[arg(long, value_name = "NAME")]
        port: Option<String>,
        /// Capture only this step of the existing fixture again (repeatable)
        #[arg(long, value_name = "STEP")]
        redo: Vec<String>,
    },
    /// Find which MIDI message lights a controller LED, by binary search
    LedScan {
        /// Port name (substring; default: the first input containing "DDJ")
        #[arg(long, value_name = "NAME")]
        port: Option<String>,
        /// 1-based MIDI channels to search
        #[arg(long, value_delimiter = ',', default_value = "1,2,5,6,7,8,9")]
        channels: Vec<u8>,
        /// Velocity (or CC value) used to light messages
        #[arg(long, default_value_t = 127)]
        velocity: u8,
        /// Message kind to search
        #[arg(long, value_enum, default_value = "note")]
        kind: led_scan::Kind,
        /// File the findings are appended to
        #[arg(
            long,
            value_name = "PATH",
            default_value = "controllers/ddj-sb3/led-scan.toml"
        )]
        out: PathBuf,
        /// Light one message instead of searching: <STATUS> <DATA1> [<VALUE>] in hex
        #[arg(long = "try", num_args = 2..=3, value_names = ["STATUS", "DATA1", "VALUE"])]
        try_message: Option<Vec<String>>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Sub::Devices => devices(),
        Sub::Play(args) => {
            if let Some(problem) = args.conflict() {
                Cli::command()
                    .error(ErrorKind::MissingRequiredArgument, problem)
                    .exit();
            }
            play(&args)
        }
        Sub::Midi(MidiCommand::Ports) => midi_cmd::ports(),
        Sub::Midi(MidiCommand::Monitor { port, controller }) => match controller {
            Some(controller) => midi_cmd::monitor_controller(&controller),
            None => midi_cmd::monitor(port.as_deref()),
        },
        Sub::Midi(MidiCommand::Capture {
            checklist,
            out,
            port,
            redo,
        }) => midi_cmd::capture(&checklist, &out, port.as_deref(), &redo),
        Sub::Midi(MidiCommand::LedScan {
            port,
            channels,
            velocity,
            kind,
            out,
            try_message,
        }) => match try_message {
            Some(args) => led_scan::try_one(port.as_deref(), &args),
            None => led_scan::run(port.as_deref(), &channels, velocity, kind, &out),
        },
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn devices() -> anyhow::Result<()> {
    let devices = audio_io::list_output_devices().context("listing audio devices")?;
    if devices.is_empty() {
        println!("No audio output devices found.");
    }
    for device in devices {
        println!(
            "{}  ({} ch, {} Hz)",
            device.name, device.max_channels, device.default_sample_rate
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(args)
    }

    fn play_args(args: &[&str]) -> PlayArgs {
        match parse(args).unwrap().command {
            Sub::Play(play) => play,
            _ => panic!("expected play"),
        }
    }

    #[test]
    fn parses_one_based_channels() {
        let args = play_args(&["dj-software", "play", "a.mp3", "--headphones", "3,4"]);
        let routing = args.routing();
        assert_eq!(routing.headphones, Some([2, 3]));
        assert_eq!(routing.master, [0, 1]);
    }

    #[test]
    fn rejects_channel_zero() {
        assert!(parse(&["dj-software", "play", "a.mp3", "--master", "0,1"]).is_err());
    }

    #[test]
    fn rejects_malformed_channel_pair() {
        assert!(parse(&["dj-software", "play", "a.mp3", "--master", "1"]).is_err());
        assert!(parse(&["dj-software", "play", "a.mp3", "--master", "a,b"]).is_err());
    }

    #[test]
    fn parses_midi_ports() {
        assert!(matches!(
            parse(&["dj-software", "midi", "ports"]).unwrap().command,
            Sub::Midi(MidiCommand::Ports)
        ));
    }

    #[test]
    fn parses_midi_monitor_port() {
        match parse(&["dj-software", "midi", "monitor", "--port", "DDJ"])
            .unwrap()
            .command
        {
            Sub::Midi(MidiCommand::Monitor { port, controller }) => {
                assert_eq!(port.as_deref(), Some("DDJ"));
                assert_eq!(controller, None);
            }
            _ => panic!("expected midi monitor"),
        }
        assert!(matches!(
            parse(&["dj-software", "midi", "monitor"]).unwrap().command,
            Sub::Midi(MidiCommand::Monitor {
                port: None,
                controller: None
            })
        ));
    }

    #[test]
    fn cue_track_needs_headphones_only_without_controller() {
        let alone = play_args(&["dj-software", "play", "a.mp3", "--cue-track", "b.mp3"]);
        assert!(alone.conflict().unwrap().contains("--headphones"));
        let with_headphones = play_args(&[
            "dj-software",
            "play",
            "a.mp3",
            "--cue-track",
            "b.mp3",
            "--headphones",
            "3,4",
        ]);
        assert_eq!(with_headphones.conflict(), None);
        // With a controller, PFL comes from it: master-only routing is fine.
        let controller = play_args(&[
            "dj-software",
            "play",
            "a.mp3",
            "--cue-track",
            "b.mp3",
            "--controller",
            "ddj-sb3",
        ]);
        assert_eq!(controller.conflict(), None);
    }

    #[test]
    fn parses_monitor_with_controller() {
        match parse(&["dj-software", "midi", "monitor", "--controller", "ddj-sb3"])
            .unwrap()
            .command
        {
            Sub::Midi(MidiCommand::Monitor { port, controller }) => {
                assert_eq!(port, None);
                assert_eq!(controller.as_deref(), Some("ddj-sb3"));
            }
            _ => panic!("expected midi monitor"),
        }
        // The mapping names the port.
        assert!(
            parse(&[
                "dj-software",
                "midi",
                "monitor",
                "--controller",
                "ddj-sb3",
                "--port",
                "DDJ",
            ])
            .is_err()
        );
    }

    #[test]
    fn parses_play_with_controller_and_four_decks() {
        let args = play_args(&[
            "dj-software",
            "play",
            "a.mp3",
            "--cue-track",
            "b.mp3",
            "--headphones",
            "3,4",
            "--controller",
            "ddj-sb3",
            "--deck3",
            "c.mp3",
            "--deck4",
            "d.mp3",
        ]);
        assert_eq!(args.track, PathBuf::from("a.mp3"));
        assert_eq!(args.cue_track, Some(PathBuf::from("b.mp3")));
        assert_eq!(args.controller.as_deref(), Some("ddj-sb3"));
        assert_eq!(args.deck3, Some(PathBuf::from("c.mp3")));
        assert_eq!(args.deck4, Some(PathBuf::from("d.mp3")));

        let plain = play_args(&["dj-software", "play", "a.mp3"]);
        assert_eq!(plain.controller, None);
        assert_eq!(plain.deck3, None);
        assert_eq!(args.conflict(), None);
        // Decks 3 and 4 are only reachable with a controller.
        assert!(parse(&["dj-software", "play", "a.mp3", "--deck3", "c.mp3"]).is_err());
        assert!(parse(&["dj-software", "play", "a.mp3", "--deck4", "d.mp3"]).is_err());
    }

    #[test]
    fn parses_midi_led_scan() {
        match parse(&["dj-software", "midi", "led-scan"]).unwrap().command {
            Sub::Midi(MidiCommand::LedScan {
                channels,
                velocity,
                kind,
                out,
                try_message,
                ..
            }) => {
                assert_eq!(channels, vec![1, 2, 5, 6, 7, 8, 9]);
                assert_eq!(velocity, 127);
                assert_eq!(kind, led_scan::Kind::Note);
                assert_eq!(out, PathBuf::from("controllers/ddj-sb3/led-scan.toml"));
                assert!(try_message.is_none());
            }
            _ => panic!("expected led-scan"),
        }
        match parse(&[
            "dj-software",
            "midi",
            "led-scan",
            "--kind",
            "cc",
            "--channels",
            "3,4",
            "--try",
            "0x90",
            "0B",
            "40",
        ])
        .unwrap()
        .command
        {
            Sub::Midi(MidiCommand::LedScan {
                channels,
                kind,
                try_message,
                ..
            }) => {
                assert_eq!(channels, vec![3, 4]);
                assert_eq!(kind, led_scan::Kind::Cc);
                assert_eq!(try_message.unwrap(), ["0x90", "0B", "40"]);
            }
            _ => panic!("expected led-scan"),
        }
    }

    #[test]
    fn parses_midi_capture() {
        match parse(&[
            "dj-software",
            "midi",
            "capture",
            "--checklist",
            "c.toml",
            "--out",
            "o.toml",
        ])
        .unwrap()
        .command
        {
            Sub::Midi(MidiCommand::Capture {
                checklist,
                out,
                port,
                redo,
            }) => {
                assert_eq!(checklist, PathBuf::from("c.toml"));
                assert_eq!(out, PathBuf::from("o.toml"));
                assert_eq!(port, None);
                assert!(redo.is_empty());
            }
            _ => panic!("expected midi capture"),
        }
        assert!(parse(&["dj-software", "midi", "capture", "--out", "o.toml"]).is_err());
        match parse(&[
            "dj-software",
            "midi",
            "capture",
            "--checklist",
            "c.toml",
            "--out",
            "o.toml",
            "--redo",
            "deck1.play",
            "--redo",
            "deck1.cue",
        ])
        .unwrap()
        .command
        {
            Sub::Midi(MidiCommand::Capture { redo, .. }) => {
                assert_eq!(redo, ["deck1.play", "deck1.cue"]);
            }
            _ => panic!("expected midi capture"),
        }
    }
}
