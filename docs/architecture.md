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
- **MIDI thread:** receives controller input, resolves it through the mapping into actions, and
  sends engine commands. LED/feedback output also comes from here, driven by state changes.
  Input-to-engine latency should be a single queue hop.
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
| `dj-software` (`crates/app`) | The application binary. Currently a command-line player (`devices`, `play`) that wires the other crates together. |
