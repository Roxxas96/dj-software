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

## Stems

Open Jockey can split a track into four stems (vocals, instruments, bass and rhythm) and let the
EQ knobs control them: HI is vocals, MID is instruments, LOW is bass. Stem pads switch each stem
on and off. It is off by default.

- Stems are separated on your computer, in the background, with the open-source Demucs model.
  No audio ever leaves the computer.
- `open-jockey stems setup` is a one-time download of the model's weights from the Demucs
  author's own source. Open Jockey never redistributes them. Their license is unclear (the
  author trained them partly on research-only data), so each user fetches them themselves.
  Nothing else ever downloads. Without the model, stems are unavailable and the knobs stay EQ.
- Turn it on in `config.toml`: `[stems] enabled = true` and `eq_mode = "stems"`.
- Separated stems are cached; `open-jockey library stems --playlist "<name>"` prepares them ahead
  (see [docs/library.md](docs/library.md#stems)).

How it works:

- A track loaded from the library (`play --library`) starts separating when it loads, from
  the playhead outward, and plays normally meanwhile. Tracks played with plain `play <file>`
  get no stems. Until a deck's stems are ready the knobs are the normal EQ; the changeover is
  smooth. The deck's status line shows `stems: preparing model` while the model opens, then
  `stems 40%` while separating, then `stems` once they are active. A track longer than 20
  minutes gets `no stems (too long)`.
- Knob law, with 12 o'clock as the middle. Turning a knob left from the middle fades its stem
  from normal down to silent. Turning it right from the middle fades every stem that no knob
  is turned right on, rhythm included, so fully right on one knob leaves only that stem. Every
  knob turned right keeps its own stem: HI and LOW fully right give vocals and bass only.
  Moves never click.
- Stem pads: the actions `deck.stem_toggle_vocals`, `deck.stem_toggle_instruments`,
  `deck.stem_toggle_bass` and `deck.stem_toggle_rhythm` turn a stem on or off. They are bound to
  no pad by default; the [mapping reference](docs/controllers/mapping-reference.md#extending-a-mapping)
  has an example that puts them on the SAMPLER pads. They work whatever `eq_mode` is, and all
  four are on after a load.
- On macOS the first session after an update takes about a minute to prepare the model for
  the GPU and Neural Engine; it runs in the background. On Windows and Linux separation runs on
  the CPU, a whole track takes one to two minutes, and playback always wins over it.

BPM and key come from detectors written for this project and can be wrong: there is no
confidence value. See [docs/library.md](docs/library.md) for details and known limits.

## Supported controllers

| Controller | Status |
|---|---|
| Pioneer DDJ-SB3 | Transport (play, cue), tempo, mixer and deck switching (software layering). Headphone MASTER LED; PLAY and CUE LEDs are not driven yet. Jog wheels, pads, loops and FX are not mapped. See [controllers/ddj-sb3](controllers/ddj-sb3/README.md). |

New controllers are welcome, see [CONTRIBUTING.md](CONTRIBUTING.md) and
[Adding a controller](docs/controllers/adding-a-controller.md). The mapping file format is in the
[mapping reference](docs/controllers/mapping-reference.md).
Mixer, sampler, pad-size and stems settings are in [docs/config.md](docs/config.md).

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
