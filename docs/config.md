# config.toml

Settings for the mixer, the effects, the sampler, the performance pad sizes and the stems. Every section and key is
optional; a key left out keeps its default, and so does a missing file.

## Location

`open-jockey play` reads `config.toml` from the per-user configuration folder:

| OS      | Path                                                     |
| ------- | -------------------------------------------------------- |
| macOS   | `~/Library/Application Support/open-jockey/config.toml`  |
| Linux   | `~/.config/open-jockey/config.toml`                      |
| Windows | `%APPDATA%\open-jockey\config.toml`                      |

`play --config <PATH>` reads another file instead (and then the file must exist).

## Keys

| Key                  | Default                                      | Meaning |
| -------------------- | -------------------------------------------- | ------- |
| `mixer.crossfader`   | `true`                                       | `false`: the crossfader is ignored and every channel plays at full level |
| `fx.slots`           | `["echo", "reverb", "flanger"]`            | The effect in each of the 3 effect slots (FX 1, 2 and 3 on the controller); exactly 3 of `echo`, `reverb`, `flanger`, `filter`. Every deck's channel has the same 3 slots |
| `sampler.volume`     | `1.0`                                        | Sampler level, 0 to 1, on the same curve as the master level (0.5 is about −12 dB) |
| `sampler.pads`       | none (all pads empty)                        | Up to 8 entries for pads 1 to 8, each `{ file = "..." }` (an absolute path, or one starting with `~/`) or `{ track = <library track id> }` or `{}` (an empty pad, so a later pad can be set alone); fewer entries leave the other pads empty |
| `beat_jump.pads`     | `[-4, 4, -8, 8, -16, 16, -32, 32]`           | Beats the beat jump pads move the playhead; exactly 8, each non-zero with `abs <= 512` |
| `beat_jump.shift_pads` | `[-1, 1, -2, 2, -4, 4, -64, 64]`           | The same with SHIFT held |
| `loop_roll.pads`     | `[0.0625, 0.125, 0.25, 0.5, 1, 2, 4, 8]`     | Beats the loop roll pads roll; exactly 8, each from 1/32 to 64 |
| `backspin.short`     | `1.0`                                        | Seconds of the short backspin (`deck.backspin_short`), from 0.2 to 10 |
| `backspin.long`      | `2.0`                                        | Seconds of the long backspin (`deck.backspin_long`), from 0.2 to 10 |
| `stems.enabled`      | `false`                                      | `true`: tracks loaded from the library are separated into stems (and cached); `false`: no separation and no cache, nothing is paid |
| `stems.eq_mode`      | `"eq"`                                       | What the EQ knobs control: `"eq"` (the frequency bands) or `"stems"` (the stems, laid out by `stems.knobs`, on a deck whose stems are ready). `"stems"` without `enabled = true` is not an error: `play` starts with the plain EQ |
| `stems.knobs`        | `"vdj"`                                      | What the knobs control in stems mode: `"vdj"` (LOW the kicks, MID the melody (vocals, instruments and bass), HI the hats) or `"raw"` (HI the vocals, MID the instruments, LOW the bass) |
| `stems.cache_gb`     | `5`                                          | The most disk the stems cache may use, in gigabytes, from 0.5 to 1000 |

A sample is at most 60 seconds long. A `track` id is looked up in the library: the one opened
by `--library`, otherwise the default library if it exists.

## Example

```toml
[mixer]
crossfader = true            # false: the crossfader is ignored, every channel at full level

[fx]
slots = ["echo", "reverb", "flanger"]   # also "filter"; exactly 3

[sampler]
volume = 1.0                 # 0 to 1
pads = [
  { file = "~/Samples/airhorn.wav" },
  { track = 1234 },
]

[beat_jump]
pads = [-4, 4, -8, 8, -16, 16, -32, 32]
shift_pads = [-1, 1, -2, 2, -4, 4, -64, 64]

[loop_roll]
pads = [0.0625, 0.125, 0.25, 0.5, 1, 2, 4, 8]

[backspin]
short = 1.0                  # seconds, 0.2 to 10
long = 2.0

[stems]
enabled = false              # true: separate library tracks into stems (needs `stems setup` once)
eq_mode = "eq"               # "eq" or "stems"; "stems" only acts when enabled
knobs = "vdj"                # "vdj" or "raw"; what the knobs control in stems mode
cache_gb = 5                 # 0.5 to 1000
```

## Errors

An invalid file stops `play` before any audio or MIDI device is opened, with a message of the
form `<file>: <key>: <problem>`, for example
`config.toml: beat_jump.pads: expected exactly 8 numbers, got 3`. Unknown keys, wrong types,
wrong lengths, out-of-range values, a pad with both `file` and `track` and more than 8 pads
are all errors.

A sample that cannot be loaded does not stop `play`: it is reported at startup as
`sampler pad N (<file or track>): <reason>` and that pad stays empty. The reasons are a
missing file, a decode error, an unknown track id, a track id with no library, a library that
cannot be opened or read (`library error: <reason>`, reported for each `track` pad), and a
sample longer than 60 seconds.
