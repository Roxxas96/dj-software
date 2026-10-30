//! Command-line entry point: argument parsing and dispatch.

mod midi_cmd;
mod play;

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
    /// Play a track on deck 1 to the master output
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
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Sub::Devices => devices(),
        Sub::Play(args) => {
            if args.cue_track.is_some() && args.headphones.is_none() {
                Cli::command()
                    .error(
                        ErrorKind::MissingRequiredArgument,
                        "--cue-track needs --headphones",
                    )
                    .exit();
            }
            play(&args)
        }
        Sub::Midi(MidiCommand::Ports) => midi_cmd::ports(),
        Sub::Midi(MidiCommand::Monitor { port }) => midi_cmd::monitor(port.as_deref()),
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
            Sub::Midi(MidiCommand::Monitor { port }) => assert_eq!(port.as_deref(), Some("DDJ")),
            _ => panic!("expected midi monitor"),
        }
        assert!(matches!(
            parse(&["dj-software", "midi", "monitor"]).unwrap().command,
            Sub::Midi(MidiCommand::Monitor { port: None })
        ));
    }
}
