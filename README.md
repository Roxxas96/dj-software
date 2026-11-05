# Open Jockey

Open-source DJ software written in Rust. It aims for a VirtualDJ-like workflow, built around
precise controller mapping: every control on a supported controller is mapped to a named action,
and LEDs follow the app state.

**Status: early development.** The audio engine, file decoding, audio output, the first
controller mapping and a music library with BPM, key and beatgrid analysis work from the
command line; there is no UI yet.

## Library

A SQLite library of your music folders, kept next to your user data and backed up on every
start (the 10 newest backups are kept):

- Folder scans that keep cues and grids when files are moved, renamed or retagged, and
  flag missing files instead of deleting them.
- Background analysis: three-band waveform, BPM and beatgrid, and musical key (Camelot,
  standard or Open Key notation).
- Cue points that are remembered per track, playlists, and beatgrid editing (shift, set
  downbeat, halve, double, set BPM, tap, lock).
- Browsing and loading from the DDJ-SB3 (BROWSE knob and LOAD buttons) with `play --library`.

BPM and key come from detectors written for this project and can be wrong: there is no
confidence value. See [docs/library.md](docs/library.md) for details and known limits.

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
cargo run --release -p open-jockey -- devices

# Play track A to the master output; track B only in the headphones (channels 3 and 4)
cargo run --release -p open-jockey -- play <track-a> --cue-track <track-b> \
  --device "DDJ-SB3" --headphones 3,4

# Play on the default output
cargo run --release -p open-jockey -- play <track-a>

# Drive up to four decks with a DDJ-SB3 (tracks load paused; use the controller to play)
cargo run --release -p open-jockey -- play <track-a> --cue-track <track-b> \
  --deck3 <track-c> --deck4 <track-d> --controller ddj-sb3 --device "DDJ-SB3" --master 3,4

# Build a library: add a folder (scans it), then analyze BPM, key and beatgrids
cargo run --release -p open-jockey -- library add-folder <music-folder>
cargo run --release -p open-jockey -- library analyze
cargo run --release -p open-jockey -- library list --sort bpm --limit 20

# Browse the library and load tracks from the DDJ-SB3
cargo run --release -p open-jockey -- play --library --controller ddj-sb3 \
  --device "DDJ-SB3" --master 3,4

# Check what a controller sends and what the mapping does with it
cargo run --release -p open-jockey -- midi monitor --controller ddj-sb3
```

Channel numbers are 1-based, as printed on the hardware. `--cue-track` needs `--headphones`.
`--library` needs `--controller`. Press Ctrl-C to stop.

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
