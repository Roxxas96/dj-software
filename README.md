# dj-software

Open-source DJ software written in Rust. It aims for a VirtualDJ-like workflow, built around
precise controller mapping: every control on a supported controller is mapped to a named action,
and LEDs follow the app state.

**Status: early development.** The audio engine, file decoding, audio output and the first
controller mapping work from a command-line player; there is no UI yet.

## Supported controllers

| Controller | Status |
|---|---|
| Pioneer DDJ-SB3 | Transport (play, cue), tempo, mixer and deck switching (software layering). Headphone MASTER LED; PLAY and CUE LEDs are not driven yet. Jog wheels, pads, loops and FX are not mapped. See [controllers/ddj-sb3](controllers/ddj-sb3/README.md). |

New controllers are welcome, see [CONTRIBUTING.md](CONTRIBUTING.md) and
[Adding a controller](docs/controllers/adding-a-controller.md). The mapping file format is in the
[mapping reference](docs/controllers/mapping-reference.md).

## Building from source

Install [mise](https://mise.jdx.dev), then:

```bash
mise install                # installs the pinned Rust toolchain and dev tools
```

On Linux you also need the ALSA headers (`libasound2-dev` on Debian and Ubuntu).

## Quick start

```bash
# List output devices and their channel counts
cargo run --release -p dj-software -- devices

# Play track A to the master output; track B only in the headphones (channels 3 and 4)
cargo run --release -p dj-software -- play <track-a> --cue-track <track-b> \
  --device "DDJ-SB3" --headphones 3,4

# Play on the default output
cargo run --release -p dj-software -- play <track-a>

# Drive up to four decks with a DDJ-SB3 (tracks load paused; use the controller to play)
cargo run --release -p dj-software -- play <track-a> --cue-track <track-b> \
  --deck3 <track-c> --deck4 <track-d> --controller ddj-sb3 --device "DDJ-SB3" --master 3,4

# Check what a controller sends and what the mapping does with it
cargo run --release -p dj-software -- midi monitor --controller ddj-sb3
```

Channel numbers are 1-based, as printed on the hardware. `--cue-track` needs `--headphones`.
Press Ctrl-C to stop.

## Architecture

See [docs/architecture.md](docs/architecture.md). Design specs live in
[docs/superpowers/specs/](docs/superpowers/specs/).

## Releasing

Run `cz bump` to update the version and changelog, then push the tag `vX.Y.Z`. Release binaries
come in a later plan.

## Disclaimer

Not affiliated with Pioneer DJ, AlphaTheta, Atomix Productions (VirtualDJ) or Serato. Product
names are trademarks of their owners, used only to describe compatibility.

## License

[Apache 2.0](LICENSE)
