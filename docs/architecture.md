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
- **Deck snapshots:** the audio thread publishes each deck's state (position, cue, hotcues,
  grid, loop, vinyl, jog) for the control thread and UI seqlock-style: a per-deck sequence
  number goes odd before the fields are stored and even after, and readers retry until they
  see the same even number before and after. The writer never waits and nothing allocates.
  `EngineController::deck_consistent` returns a snapshot only when it was read that way, and
  the library mode saves cues and hotcues only from such snapshots, so a torn read
  can never write one track's cues onto another. A per-deck `load_seq` counts loads, so the
  app can tell when a `Load` has been applied.
- **Loader/analysis workers:** decode tracks into memory ahead of playback (no streaming from
  disk on the audio thread), and run BPM/key/waveform analysis in the background.
  In library mode (`play --library`) the control loop never waits on the database or a
  decode: a **library worker** thread does browser moves, track lookups and grid edits, and
  a **loader** thread decodes the selected track, handing the buffer back to the control
  loop, which sends it to the engine.
- **Analyzer pool:** a pool of worker threads (CPU cores minus one, at least one) that
  decodes and analyzes tracks (waveform, BPM and grid, key) and sends results to the
  writer. Analysis never blocks the control loop or audio.
- **Folder watcher:** watches the library folders, debounces events (2 s) and applies the
  same per-file logic as a scan; when the OS watch fails it rescans every 10 minutes.
- **UI thread:** reads published state snapshots. A slow UI frame or a slow folder scan must
  never affect audio.
- **Database writer:** one thread owns the write connection to the library DB. Every change
  (scan results, cues, grid edits, analysis, playlists, settings) is a write operation sent
  to it and applied in one transaction, so writes are serialized and never run on the UI or
  audio threads. Readers use their own read-only connections.

## Crates

| Crate | Responsibility |
|---|---|
| `engine` | Audio engine: decks, mixing and effects. Real-time safe, no I/O, no UI or MIDI knowledge. Testable without an audio device by rendering frames into a buffer. |
| `decode` | Decodes audio files into memory. |
| `audio-io` | Audio device enumeration and output. |
| `actions` | The named actions controls can trigger (ids such as `deck.play_pause`), the app-side control state (which deck each side shows, SHIFT, last set values) and the LED rules. `apply` turns an action into engine commands. No MIDI knowledge. |
| `controller` | MIDI ports and messages, the mapping file format, the resolver that turns messages into actions (conditions, soft takeover), LED feedback, the capture tools' logic, and the built-in mappings (`controllers/<name>/mapping.toml`). Tested with fake ports. |
| `analysis` | Offline track analysis written for this project: three-band waveform, tempo and constant beatgrid, and musical key with Camelot, standard and Open Key notation. Pure functions on decoded audio; no database. |
| `library` | The SQLite library: schema and migrations, startup backups, folder scans and the watcher, fingerprints, tags, cues, playlists, beatgrid edits, the writer thread and the analyzer pool that stores `analysis` results. See [library.md](library.md). |
| `open-jockey` (`crates/app`) | The application binary. A command-line player and MIDI tools (`devices`, `play`, `library` and `midi` with `ports`, `monitor`, `capture` and `led-scan`) that wire the other crates together. |

## Track memory, grids and sync

A track's remembered state (`TrackMemory`: the cue point, up to eight hotcues and the
beatgrid) travels with the `Load` command, and the engine applies it on the audio thread
when the track starts, clamped to the track length. The beatgrid lives in the engine as
well as in the library, so the audio thread can use it for loops (`AutoLoop` snaps to the
beat before the playhead and sizes the loop in beats) and beat sync. The library mode
(`MemorySync` in `crates/app`) watches the deck snapshots after a load and writes any
change of cue, hotcues or grid back to the library; grid edits go the other way as
`SetGrid`.

SYNC is planned in `actions` (`deck.sync`): it picks the playing deck to follow, computes
the tempo slider and range that match its BPM (also at half or double time), and sends
the tempo commands plus one `PhaseSync` command. The engine then moves the playhead by at
most half a beat so the beats line up, at once for a playing deck, or at the first block
where a paused deck plays. The jog wheels are engine commands as well (`JogTouch`, `Jog`,
`JogSearch`): scratching, nudging and search run on the audio thread, and the control side
only turns controller steps into platter turns using the mapping's `jog_steps_per_turn`.

## Slip, the sampler and software pad modes

**Slip.** With slip on, a scratch, a normal loop or a hotcue jump while playing keeps a
silent *ghost* playhead moving at the deck's tempo; when the gesture ends the playhead
returns to the ghost, so the track lands where it would have been. Loop rolls and slicer
pads always slip. The ghost lives in the deck (`DeckSnapshot::ghost_frames`), and a
deliberate jump (seek, cue, search) drops it. A roll or slice is a separate roll loop: the
normal loop commands are ignored while one is held.

**Sampler.** The engine has eight one-shot sampler slots (`SamplerCommand`) mixed into the
master bus after the channels, so the headphone master mix hears them too. A slot holds a
shared decoded buffer; playing it restarts it from the start and it stops at the end.
Samples are decoded by the app before the controller attaches (up to 60 s each) and sent
with `Set`, so the audio thread never allocates or frees. The sampler is global, not per
deck: both sides' SAMPLER pads play the same eight samples. `EngineController::sampler()`
shows which slots play, for the status line.

**Software pad modes.** The SB3 does not switch its pads for the SHIFT pad modes (beat
jump, loop roll, slicer): its pads keep sending the notes of the last hardware mode. So
each side's pad mode lives in `ControlState` (`SideControl::pad_mode`), set by `pads.mode_*`
actions, and a mapping binding's `pad_mode` condition only fires while the binding's side
is in that mode. A master action such as `sampler.play_N` may carry a `side` that only
scopes the condition. A held roll or slice keeps its release on the deck that got the
press, even after a DECK switch, and a release on the shifted sibling note pairs with the
press (`ActionKind::hold_partner`).

**Keylock.** With keylock on (`DeckCommand::SetKeylock`, kept across loads), a deck at a
tempo other than the original plays through its own Signalsmith Stretch time-stretcher
(the C++ library, through the `signalsmith-stretch` crate; the cheaper preset, stereo, at
the output rate). Each deck's stretcher and its interleaved buffers are built with the
renderer, before it moves to the audio thread. For a block of `n` output frames the deck
reads `n × tempo × (1 + nudge bend)` source frames (rounded, the remainder carried to the
next block) at the original pitch through the same reader as the normal path, so loops,
rolls and the slip ghost wrap as usual, and the playhead advances by exactly those frames.
Scratching, a paused deck and the original tempo (within 1e-4) play resampled, without
the stretcher. After a discontinuity (a jump of any kind, a load, a slip return, a change
of the loop or roll the playhead wraps in, keylock switched on, or a switch from the
resampled path) the stretcher is reset, fed `input_latency` frames from the playhead with
`seek`, and its output latency is rendered and discarded, so the next block starts at the
playhead at full level, with no tail of the old audio. Loop and roll wraps are not
discontinuities: the input stream wraps and the stretcher smooths it. The stretcher holds
about its input latency plus its output latency of source ahead of what is heard (2205 +
3969 frames, about 140 ms, at 44.1 kHz); latency is not reported in the snapshot. After a restart
what is heard is at the playhead; a later tempo change lets them drift apart by up to the
output latency (about 90 ms) times the change in tempo ratio, about 9 ms for a 10 % move.
A restart costs about one millisecond of the audio callback.

**config.toml.** `play` reads the per-user `config.toml` ([config.md](config.md)) before the
controller attaches: the crossfader switch, the sampler files and volume, and the beat jump
and loop roll size tables, which are written into `ControlState`.

Controllers are documented in [`controllers/`](controllers/): the [mapping reference](controllers/mapping-reference.md) and [adding a controller](controllers/adding-a-controller.md).
