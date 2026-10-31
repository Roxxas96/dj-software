# Architecture

Each plan that adds or changes a crate updates this page. The full design is in
[`superpowers/specs/`](superpowers/specs/).

## Threads and real-time rules

- **Audio thread** (owned by the audio backend callback): mixes decks, applies EQ/filter/FX, and
  writes master and cue outputs. It never allocates, locks a mutex that another thread can hold,
  does I/O, or logs. It talks to the rest of the app only through lock-free structures:
  single-producer/single-consumer command queues inward, and atomics or SPSC queues outward for
  playhead, level and state. In debug builds and tests, an allocation guard on the callback
  (e.g. `assert_no_alloc`) turns any allocation into a test failure.
- **MIDI input callback and control thread:** the `midir` input callback only timestamps each
  message and forwards it over a channel; it never touches the engine. The control thread (the
  `play` loop) drains the channel, resolves messages through the mapping into actions, applies
  them (engine commands plus the app-side control state) and sends LED updates (about 30 Hz,
  only values that changed). None of this runs on the audio thread. It also notices when the
  controller is unplugged or plugged back in (polled once a second): audio keeps playing, held
  buttons are released, and on return soft takeover is armed and every LED is sent again.
- **Loader/analysis workers:** decode tracks into memory ahead of playback (no streaming from
  disk on the audio thread), and run BPM/key/waveform analysis in the background.
- **UI thread:** reads published state snapshots. A slow UI frame or a slow folder scan must
  never affect audio.
- **Database writer:** serializes writes to the library DB off the UI and audio threads.

## Crates

| Crate | Responsibility |
|---|---|
| `engine` | Audio engine: decks, mixing and effects. Real-time safe, no I/O, no UI or MIDI knowledge. Testable without an audio device by rendering frames into a buffer. |
| `decode` | Decodes audio files into memory. |
| `audio-io` | Audio device enumeration and output. |
| `actions` | The named actions controls can trigger (ids such as `deck.play_pause`), the app-side control state (which deck each side shows, SHIFT, last set values) and the LED rules. `apply` turns an action into engine commands. No MIDI knowledge. |
| `controller` | MIDI ports and messages, the mapping file format, the resolver that turns messages into actions (conditions, soft takeover), LED feedback, the capture tools' logic, and the built-in mappings (`controllers/<name>/mapping.toml`). Tested with fake ports. |
| `dj-software` (`crates/app`) | The application binary. A command-line player and MIDI tools (`devices`, `play`, `midi` with `ports`, `monitor`, `capture` and `led-scan`) that wire the other crates together. |

Controllers are documented in [`controllers/`](controllers/): the [mapping reference](controllers/mapping-reference.md) and [adding a controller](controllers/adding-a-controller.md).
