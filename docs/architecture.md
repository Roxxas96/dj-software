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
  never affect audio. `open-jockey` with no command runs the window (`ui`) on the main
  thread and the control loop on a **session thread** behind it: the same loop as
  `play --library --controller`, which publishes a `ControlFrame` (control state, library
  decks, browser, status lines and events) at most every 10 ms by swapping one pointer, and
  never waits on the window. The window reads the deck and meter snapshots through an
  `EngineMonitor` and never sends the engine anything. The session waits for the audio
  device (retrying every 2 s), and closing the window ends the loop, which saves the last
  cues and hotcues.
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
| `library` | The SQLite library: schema and migrations, startup backups, folder scans and the watcher, fingerprints, tags, cues, playlists, beatgrid edits, the writer thread, the analyzer pool that stores `analysis` results, and the stems cache with its Ogg Vorbis format (encoded in `library/src/stems_cache.rs`, decoded through `decode`). See [library.md](library.md). |
| `stems` | Splits a track into vocals, bass and rhythm (instruments are the residual; kicks and hats are split from rhythm later, in `engine`) with the Demucs `htdemucs` model through ONNX Runtime (`ort`; CoreML on macOS, CPU elsewhere), in chunks, with the STFT in Rust. Also `stems setup` (one-time download of the author's weights, filling the weight-free graph embedded in the crate, verification), and a fixed synthetic separator for tests. No database, no audio thread. See [stems-backend.md](stems-backend.md). |
| `ui` | The window (egui through eframe): the published `ControlFrame` model, the theme and the top bar. It only reads the frame and the engine's monitor. |
| `open-jockey` (`crates/app`) | The application binary. Without a command, the window and the session behind it; also a command-line player and MIDI tools (`devices`, `play`, `library`, `stems` and `midi` with `ports`, `monitor`, `capture` and `led-scan`) that wire the other crates together. |

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

## Stems

Stems are off unless `[stems] enabled = true` ([config.md](config.md)). Nothing leaves the
computer: the only download is `open-jockey stems setup`, which fetches the Demucs author's
weights once. Open Jockey never redistributes them.

**The cache.** `library::StemCache` (`stems_cache.rs`) keeps each track's stems as Ogg Vorbis
files in the `stems` folder next to the library database, with a `stems` table in the database.
An entry is keyed by the track id and its audio fingerprint. Writes go to a temporary folder
that is renamed into place, so a crash leaves no half entry; leftovers are swept at start.
`put` stores an entry and then evicts down to the budget (`stems.cache_gb`), never touching a
track a deck holds. The order is in [library.md](library.md#stems). Maintenance is best-effort
per entry: a folder that cannot be removed is skipped and retried later, and a cache problem
never blocks playing a track. `tracks.last_loaded_at` (set at each library load) and
`tracks.stems_prepared` are the eviction inputs.

**The job thread.** In library mode (`library_mode/stem_jobs.rs`) a `stems` thread owns the
cache and the separator, so the control loop never waits on either. A track load asks it for
the track's stems. One job runs per track, shared by the decks that hold it. It looks in the
cache first (a hit is decoded and sent at once, also between the chunks of a running
separation); on a miss it separates in chunks working outward from the playhead, reporting
progress for the deck's status line, and then stores the result. The ONNX session opens in a
background thread at start (on macOS the first compile after an update takes about a minute),
and decks keep the EQ until it is ready. CPU separation uses half the cores so that playback
wins. A finished set travels to the engine like a track buffer, through the command queue,
and the replaced one comes back through the garbage queue, which the app collects every
tick. Tracks over 20 minutes are refused.

**In the engine** (`crates/engine/src/stems.rs`). A `StemSet` holds four stems as 16-bit samples
at the track's own rate: vocals, bass, rhythm and kicks. Kicks are precomputed when the set is
built (not cached; a cache hit recomputes them): rhythm through a zero-phase low pass at
`stems.kicks_hz` (300 Hz by default, `DEFAULT_KICKS_HZ`), a Butterworth biquad run forward and backward off the audio thread. Instruments are
the residual `mix - (vocals + bass + rhythm)` and hats are `rhythm - kicks`, both computed per
frame from the deck's track. So there are five parts (vocals, instruments, bass, kicks, hats)
and they always sum to the mix. The deck's single source reader is the one place that reads
stems: with a set, it returns the gain-weighted sum of the five. The split holds no filter
state at play time, so loops, rolls, scratching, slip and beat jump work unchanged, and keylock
stretches the stem mix. The sampler is not stemmed.

- **Knob law.** `knob_law(layout, knobs)` turns the EQ knob positions into five part gains.
  Each knob gives its parts `eq_gain`: silent fully left (squared law), 1 at the centre, +6 dB
  fully right. In `StemKnobs::Vdj` LOW is kicks, MID is vocals, instruments and bass, and HI is
  hats. In `StemKnobs::Raw` HI is vocals, MID instruments and LOW bass, with kicks and hats at
  1. A knob only changes its own parts. `pad_gains` turns the seven pad flags into five part
  gains (melody covers vocals, instruments and bass; rhythm covers kicks and hats), so two pads
  covering one part multiply. The knob and pad gains are multiplied. Gains ramp over one block,
  so no click.
- **Stem mode** applies when `EqMode::Stems` is set and the deck has a set. The EQ isolator
  keeps running with its band gains at unity (a flat allpass) instead of being bypassed, so
  the EQ-to-stems changeover with centred knobs is exact. The changeover is a one-block ramp
  of the source and the band gains. Pad flags (`DeckCommand::StemOn`) apply in either
  mode.
- **Keylock alignment.** On the stretched path the stem gains are applied at the stretcher's
  input, but the heard sound is `input latency + output latency` later. So at a changeover
  the mixer's EQ-band retarget is delayed by the stretcher's total latency (a countdown in
  the channel strip, no allocation) and both change in the same audible instant. For the
  same reason a knob move in stem mode under keylock is heard after the stretcher latency
  (about 140 ms at 44.1 kHz).
- **Real-time rules.** Setting, replacing and freeing stems use the command and garbage
  queues; `crates/engine/tests/no_alloc.rs` covers the stem render paths.

**Pads.** The seven `deck.stem_toggle_*` actions flip the deck's stem flags (`DeckControl::stems_on`) and send
`StemOn`; the flags reset to all on when the deck's `load_seq` moves, and a press before
the stems are ready is remembered. A press in the same control-loop iteration as a load
can be lost (press again).

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

**Firmware pad routines.** In its FX FADE, PAD SCRATCH and TRANS modes the SB3 performs
routines itself and sends ordinary-looking messages: fader and FILTER moves, jog touches,
a platter jog (`deck.jog_scratch`, `JogSurface::Platter`, which scratches a touched deck
whatever the vinyl flag says) and loop notes. The engine adds what the SB3 cannot send:
`DeckCommand::Backspin` (a reverse ramp from 3x speed to 0, then forward again; jog
touches and top/platter moves within 50 ms of its start, measured on the deck's clock of
rendered output frames, belong to it, and a scratch begun in the 50 ms before is taken
over) and
`AutoLoopBeats` / `LoopExit` for the fade loops. `deck.loop_exit` and `deck.loop_fade_N`
rely on the engine's guards rather than the snapshot, because a pad note and its loop note
arrive within a millisecond, before the engine has rendered. The resolver ties a side's
fade loop to its deck: the side's next `deck.loop_exit` press goes to the deck its last
`deck.loop_fade_N` started on, even after a DECK switch. The recorded routines are
replayed in time by `crates/controller/tests/sb3_firmware_replay.rs`.

**Keylock.** With keylock on (`DeckCommand::SetKeylock`, kept across loads), a deck at a
tempo other than the original plays through its own Signalsmith Stretch time-stretcher
(the C++ library, through the `signalsmith-stretch` crate; the cheaper preset, stereo, at
the output rate). Each deck's stretcher and its interleaved buffers are built with the
renderer, before it moves to the audio thread. For a block of `n` output frames the deck
reads `n × tempo × (1 + nudge bend)` source frames (rounded, the remainder carried to the
next block) at the original pitch through the same reader as the normal path, so loops,
rolls and the slip ghost wrap as usual, and the playhead advances by exactly those frames.
Scratching, a paused deck and the original tempo (a tempo ratio within 1e-4 of 1) play
resampled, without the stretcher. The nudge bend never decides the path: it bends
whichever one runs (at the original tempo the resampled step, so a nudge there bends the
pitch briefly, as on vinyl), so a nudge never restarts the stretcher. After a discontinuity (a jump of any kind, a load, a slip return, a change
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
`crates/engine/tests/no_alloc.rs` only sees Rust allocations; the C++ Signalsmith
stretcher is trusted on its documented real-time behavior: it allocates only when
constructed, never in `process`, `seek` or `reset`.

**Effects.** Every channel has three effect slots (`ChannelCommand::FxEnabled`) sharing one
amount (`ChannelCommand::FxAmount`); the effect in each slot is a master setting
(`MasterCommand::FxSlot`, an `EffectKind`), which `play` sends at startup from `[fx] slots`
in `config.toml`. Echo and reverb follow a DJ mixer's dry/wet curve (dry
`min(1, 2(1 − a))`, wet `min(1, 2a)`: the dry stays at unity up to mid amount), the
flanger crossfades by the amount, and the filter's amount is its cutoff. The actions
`deck.fx_toggle_1` to `_3` and `deck.fx_level` drive the
side's deck, and `deck.keylock_toggle` flips `DeckCommand::SetKeylock` from the snapshot.
The app tracks the slots it switched in `DeckControl::fx_on` and the amount in
`DeckControl::fx_level` (the soft takeover value).

**config.toml.** `play` reads the per-user `config.toml` ([config.md](config.md)) before the
controller attaches: the crossfader switch, the effect in each slot, the sampler files and volume, and the beat jump
and loop roll size tables, which are written into `ControlState`.

Controllers are documented in [`controllers/`](controllers/): the [mapping reference](controllers/mapping-reference.md) and [adding a controller](controllers/adding-a-controller.md).
