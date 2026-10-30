# DJ software: design

Date: 2026-10-30
Status: draft, awaiting approval (builder check done)

## Intent

The goal is an open-source, cross-platform DJ application written in Rust that looks and works
as much like **VirtualDJ** as possible. The owner uses it for both set preparation and live
performance with a **Pioneer DDJ-SB3**.

**Why:** VirtualDJ is closed. With the SB3, VDJ's deck switching (1/3, 2/4) did not work, the
LEDs were broken, and button mappings could not be made precise. With closed software there is
nothing the owner can do about any of that. Owning the code means any feature or controller
behavior can be added or changed. For that reason the controller layer is the core of this
project, not an add-on.

**Open source from day one:** the project is meant to be shared, so documentation and
contribution guidelines are first-class deliverables, not afterthoughts.

**Success for the first version:** the owner plays a full practice set at home using only this
app and the SB3, without opening VirtualDJ once.

## Goals

1. Feel like VirtualDJ: layout, workflow and terminology.
2. Full DDJ-SB3 support out of the box: every control, every LED, deck switching, pad modes, and
   the SB3's built-in sound card (master + headphone cue).
3. Every control mapping is configurable: a mapping file plus in-app MIDI learn.
4. A customizable interface: resizable, collapsible panels with saved layout presets. Skins come
   later.
5. Hotcue (and, where possible, playlist) import/export with VirtualDJ, Rekordbox and Serato.
6. Runs on macOS, Windows and Linux.
7. Extensible: new controllers, actions, effects and import/export formats can be added without
   touching unrelated parts.
8. Repository standards match the owner's existing projects (see [Project standards](#project-standards)).

## Non-goals (v1)

- Streaming services. Local files only.
- Lighting/DMX control. The action system must not preclude it (see [Actions](#actions)), but no
  lighting code ships in v1. This is the owner's stated "later" feature.
- Fully custom skins that change widget appearance. v1 has layout + color files only.
- Video, karaoke, stems separation, DVS/timecode, broadcasting, and recording a set (recording
  was not requested; easy to add later).
- A scripting language for mappings (my call; see [Mapping](#mapping)).
- Controllers other than the SB3. The architecture must support them, and v1 ships only the SB3.

## Anti-goals

These count as failure even if every feature exists:

- **A freeze, crash or audio dropout during playback.**
- **A control doing something other than what its mapping says**, including after deck switching.
- **Cues disappearing.**

## How the owner plays (the workflow to support)

- Browse playlists and local folders with the SB3's browse knob, press the load button to put a
  track on a deck, and play.
- Mostly 2 physical decks, but with deck switching, so up to **4 tracks can play at once**.
- Hotcues mark transition points.
- Performance pads trigger samples (transition sounds), pad FX, and so on.
- Uses essentially every control on the SB3: pads and pad-mode selection, loops, channel faders,
  tempo sliders, knobs (EQ/filter/trim/FX), deck switch, browse/load.
- **Practice = set preparation:** build a playlist from local files, order the tracks, and set
  hotcues on each.

## Decided by the owner

- Language: **Rust** (the owner will extend it themselves).
- Platforms: **macOS, Windows, Linux**.
- Controller: **DDJ-SB3** first. Fully configurable mappings.
- Default layout **A** (see [Interface](#interface)), with a one-step switch to layout **B**.
- Resizing panels matters more than skins. Both are wanted eventually.
- Cue import **and** export with **VirtualDJ, Rekordbox, Serato**.
- Open source, with strong docs and contribution guidelines.
- Same development standards as the owner's repos `Roxxas96/ha-soundboks-integration` and
  `Roxxas96/uptime-kuma-operator` (mise, prek, Conventional Commits, and so on).

## Architecture

### Threads and real-time rules

The anti-goals drive this section.

- **Audio thread** (owned by the audio backend callback): mixes decks, applies EQ/filter/FX, and
  writes master and cue outputs. It **never** allocates, locks a mutex that another thread can
  hold, does I/O, or logs. It talks to the rest of the app only through lock-free structures:
  single-producer/single-consumer command queues inward, and atomics or SPSC queues outward for
  playhead/level/state. In debug builds and tests, an allocation guard on the callback (e.g.
  `assert_no_alloc`) turns any allocation into a test failure.
- **MIDI thread:** receives controller input, resolves it through the mapping into actions, and
  sends engine commands. LED/feedback output also comes from here, driven by state changes.
  Input-to-engine latency should be a single queue hop.
- **Loader/analysis workers:** decode tracks into memory ahead of playback (no streaming from
  disk on the audio thread), and run BPM/key/waveform analysis in the background.
- **UI thread:** reads published state snapshots. A slow UI frame or a slow folder scan must never
  affect audio.
- **Database writer:** serializes writes to the library DB off the UI and audio threads.

### Crate layout (Cargo workspace)

A suggested split; the builder may adjust names:

| Crate | Responsibility |
|---|---|
| `engine` | Decks (transport, tempo, keylock, loops, hotcues, jog), 4-channel mixer, cue bus, effects, sampler. Real-time safe. No UI or MIDI knowledge. |
| `audio-io` | Audio device enumeration and output (master + headphone cue routing). |
| `actions` | The action registry: named, typed actions (`deck.play`, `deck.hotcue{n}`, `mixer.eq_low`, `ui.layout`, ...) and state that can be observed for feedback. |
| `controller` | MIDI I/O, the mapping engine (mapping file → actions, state → LEDs), MIDI learn, device hot-plug. |
| `controllers/` | Per-device mapping files (TOML) plus optional per-device Rust code (e.g. SB3 jog-wheel math). |
| `library` | Track DB, playlists, folders, analysis cache, cue persistence, backups. |
| `interop` | Import/export modules: `virtualdj`, `rekordbox`, `serato`, all behind one trait. |
| `ui` | egui interface, docking layout, waveforms, browser, settings, MIDI monitor, MIDI-learn UI. |
| `app` | The binary: wires everything together, config/data paths, startup and shutdown. |

`engine` must be testable without an audio device (render N frames into a buffer), and
`controller` must be testable without hardware (feed recorded MIDI and assert on actions/LED
output).

### Actions

All behavior a control can trigger is an **action** in a central registry, whether it comes from
the controller, a keyboard shortcut or a UI widget. Each action has a stable string ID, typed
parameters (button press/release, absolute 0..1 value, relative/encoder delta, and so on), and
optional **observable state** used for LED feedback and UI display.

Adding a capability = registering new actions. This is how lighting gets added later: a pad can
be mapped to a `light.*` action without the mapping layer knowing what lighting is.

### Mapping

- **Format:** one TOML file per controller in `controllers/<device>/`. A user override copy lives
  in the user config dir; edits never touch the shipped file. Entries bind a MIDI message
  (status/channel/note-or-CC, with value interpretation such as button, absolute, 14-bit,
  relative encoder) to an action, with optional **conditions/modifiers**: shift held, active deck
  layer (1/3 vs 2/4), active pad mode. A separate section binds observable state to output MIDI
  (LEDs, pad colors).
- **MIDI learn:** in-app mode where the user picks an action (or clicks an on-screen control),
  then touches the hardware control, and a binding is written to the user mapping.
- **MIDI monitor:** an in-app view showing raw incoming/outgoing MIDI and which action (if any)
  each message resolved to, for debugging "why did that button do that".
- **No scripting language in v1 (my call).** VirtualDJ uses VDJScript, but declarative bindings
  plus conditions cover the SB3, and anything more complex goes into per-device Rust code. If
  declarative mappings prove too limiting, an embedded scripting language (e.g. Rhai) is the
  likely next step. Design the mapping engine so one could be added later.

### DDJ-SB3 specifics

- **Get ground truth from the hardware.** Before writing the default mapping, capture a full MIDI
  dump of every SB3 control (with shift, in each pad mode, on both deck layers) and LED behavior.
  Pioneer publishes a MIDI message list for the SB3; use it as a reference, but the capture is
  authoritative. Commit the capture as a test fixture.
- **Default mapping basis (owner's decision):** reproduce **VirtualDJ's default SB3 behavior**
  so the owner has nothing to relearn, but with deck switching and LEDs working correctly. Where
  VDJ's behavior is unclear, fall back to the printed panel labels.
- **Deck switching:** the SB3 sends decks 3/4 on different MIDI channels when a side is switched.
  LEDs and pad colors for a layer must be (re)sent when the layer becomes active, so the hardware
  always shows the state of the deck it is controlling. This is the behavior VDJ got wrong and it
  gets dedicated tests.
- **The mixer strip switches with the deck (owner's decision):** when a side switches layer, that
  side's channel fader, trim, EQ, filter and tempo slider control the newly active deck too.
- **Soft takeover (owner's decision):** after a layer switch, a physical fader, knob or slider
  whose position doesn't match the new deck's value has no effect until it crosses that value, so
  nothing jumps mid-set. The mapping engine supports soft takeover as a per-binding option, on by
  default for all absolute controls in the SB3 mapping. The UI shows when a control is waiting to
  pick up (e.g. a ghost marker on the on-screen fader). Covered by tests.
- **Capture tool first (my call):** the owner has the only SB3. The first controller work item is
  a small capture tool (CLI or in-app) the owner runs to record every control in every mode/layer
  and observe LED responses, producing the committed fixture files.
- **Pad modes:** hot cue, pad FX 1, pad FX 2, sampler, plus the shifted modes the hardware
  provides (e.g. beat jump / beat loop / key-related modes; confirm from the capture). Each mode
  maps to actions and has LED feedback.
- **Sound card:** the SB3 is a USB audio interface. Master goes on outputs 1/2, headphones (cue)
  on 3/4. The app must open the device as 4 channels and route master and cue independently.
  - **Windows:** Pioneer's SB3 driver is ASIO. The Windows build must support ASIO (e.g. `cpal`
    with its ASIO feature; this needs the Steinberg ASIO SDK at build time, so document it in
    CONTRIBUTING and handle it in CI). WASAPI as a fallback.
  - **macOS:** CoreAudio, class-compliant.
  - **Linux:** ALSA, optionally JACK/PipeWire.
- **Hot-plug:** if the controller disconnects mid-set, playback continues. On reconnect, the
  mapping reattaches and all LEDs are re-sent. Audio device loss should fail over to the system
  default output and be shown clearly in the UI.

### Engine features (v1)

- **Decks (4):** load, play/pause, cue (CDJ-style cue behavior like VDJ's default), sync (beat
  and tempo, using analyzed BPM/beatgrid), tempo slider with selectable range (±6/10/16/100%),
  keylock (pitch-preserving time-stretch), jog wheel nudge (pitch bend) and scratch/vinyl mode,
  needle search where available.
- **Beatgrid editing (owner's decision: relies heavily on sync and beat features):** v1 includes
  manual grid correction: shift grid left/right, set downbeat at the playhead, halve/double BPM,
  type or tap a BPM, and lock the grid so re-analysis doesn't overwrite it. Edits are stored with
  the track like cues (write-through, backed up). Sync, beat jump, auto-loop, loop roll and
  beat-synced FX all use the corrected grid. Constant-tempo grids are fine for v1; variable-tempo
  grids come later.
- **Hotcues:** 8 per track (the SB3 has 8 pads), set/trigger/delete, colored, named.
- **Loops:** loop in/out, auto-loop of N beats with halve/double, loop roll, reloop, beat jump.
- **Mixer:** 4 channels, each with trim, 3-band EQ (with kill), filter (single-knob LPF/HPF),
  channel fader, headphone cue (PFL) toggle. Crossfader with assignable channels (A/B/thru).
  Master level, headphone level and headphone cue/master mix.
- **Beat FX:** a small set in v1: echo, reverb, flanger, filter (my call on the list),
  beat-synced where relevant, with FX select, on/off and level/depth, as on the SB3.
- **Pad FX:** whatever the SB3's pad FX modes need (e.g. roll, echo, trans), built on the same
  effect primitives.
- **Sampler:** sample banks assignable to pads. Samples play through the master (with a
  sampler volume). Samples are configured in the UI.
- **Meters:** per-channel and master levels for the UI and any controller LEDs.

### Library

- **Sources:** local folders (scanned, watched for changes) and playlists created in the app.
  The browse knob navigates both the folder/playlist tree and the track list.
- **Playlists:** create, rename, reorder tracks by drag, remove. A visible indicator (or column)
  shows tracks that have no hotcues, so the owner can see when a set is fully prepared (my guess
  at what helps prep).
- **Analysis:** BPM + beatgrid, musical key (shown in both Camelot and standard notation, with a
  setting), and multi-resolution waveform peaks, computed in background workers on import/first
  load and cached in the DB. Columns: title, artist, BPM, key, length, plus any others the builder
  sees fit. Re-analysis on demand.
- **Formats (my call):** MP3, WAV, AIFF, FLAC, M4A/AAC, OGG/Vorbis. Tags (title/artist/...) are
  read, never written.
- **Storage:** SQLite in the platform data dir. Audio files are never modified by normal use (see
  Serato export for the single opt-in exception).
- **Cue safety:**
  - Every cue change is written immediately in a transaction (no "save on quit").
  - The DB is backed up on startup. Keep the last N (e.g. 10) backups.
  - The DB has a schema version and migrations. A failed migration leaves the original untouched.
  - Tracks are identified robustly enough that moving or renaming a file can be relinked without
    losing cues (e.g. path plus a content fingerprint/hash; the builder chooses).

### Interop (import/export)

One `CueFormat` trait (or similar) per format: import cues (+ playlists where available) and
export them. The builder may adjust the shape. Each import shows a summary and, when a track
already has cues, asks: **keep / replace / merge**. Imports never silently overwrite cues.

- **VirtualDJ:** read/write `database.xml` (cue POIs) and its playlists (`.vdjfolder` / `.m3u`).
  Writing must not corrupt a live VDJ database: write to a copy or require VDJ to be closed, and
  back up first.
- **Rekordbox:** via **rekordbox XML** (the documented collection-export format with
  `POSITION_MARK` cues and playlists), import and export. Not the encrypted `master.db` (my call:
  XML is documented and supported by rekordbox itself).
- **Serato:** Serato stores cues inside the audio files' tags (`Serato Markers2` and related
  GEOB/atoms). **Import** reads the tags (non-destructive). **Export writes into the files**, which
  is the one exception to "never modify audio files". It is therefore an explicit, opt-in action
  with a clear warning, it backs up each file before writing, and it only touches the Serato tag
  frames. This is my call; the alternative is no Serato export.

Cue positions must be converted carefully between formats (sample offsets vs. milliseconds,
encoder delay/gapless padding on MP3/AAC). Each format module needs round-trip tests with real
sample files from each application.

### Interface

UI framework: **egui** (via `eframe`) with **`egui_dock`** for docking/resizing. **This is my
call, not the owner's.** Reasons: Rust-only codebase (the owner works in Rust), immediate-mode
redraw at display refresh rate suits scrolling waveforms, cross-platform, and docking gives
drag-to-resize, collapse and rearrange almost for free. The alternative considered was Tauri with
a web UI (easier skinning, but splits the codebase into Rust + JS).

**Layout A (default):** like VirtualDJ's 2-deck view.
- Top: stacked scrolling waveforms for every playing deck (a deck switched away from stays
  visible while it plays), with beat markers and cue markers, plus a zoomed-out overview per deck.
- Middle: two large deck panels (left = decks 1/3, right = decks 2/4). Each shows a layer tag
  (which deck it currently controls), track info, time elapsed/remaining, BPM/pitch, jog
  display, transport, loop controls, and a pad area reflecting the current pad mode. The mixer
  sits between them.
- Bottom: browser with the folder/playlist tree on the left and the track list on the right.

**Layout B:** four compact decks, with a 4-channel mixer in the middle and four stacked
waveforms. Decks currently driven by the controller are highlighted.

Mockups from brainstorming: `.superpowers/brainstorm/*/content/layout.html` (not committed; A is
the left card, B the right).

- Switching A ⇄ B: one click, a keyboard shortcut, and a mappable action (`ui.layout`), so it can
  be bound to a controller button.
- Every split between panels is draggable. Panels can be collapsed. The user can save the current
  arrangement as a named **layout preset**; A and B are built-in presets. The current layout is
  restored on next launch.
- Layout presets and colors are stored as readable files (TOML) in the config dir, so a setup can
  be shared as a file. Full skins come later.
- Also needed: settings (audio device + output routing, controller/mapping selection, tempo range
  default, key notation, library folders), the MIDI-learn UI and the MIDI monitor.

### Open choices for the builder

These must satisfy the constraints above. The builder picks:

- Audio output: `cpal` is the natural default (supports CoreAudio/WASAPI/ASIO/ALSA/JACK).
- Decoding: `symphonia` is the natural default.
- MIDI: `midir` is the natural default.
- Time-stretch for keylock, BPM/beatgrid detection and key detection libraries. **License
  constraint:** the project is Apache-2.0 (see below), so GPL dependencies (e.g. Rubber Band,
  libKeyFinder, aubio) cannot be linked. Prefer permissive Rust crates or permissive C/C++ libs
  (e.g. Signalsmith Stretch is MIT). `cargo deny` enforces the allowed license list.
- Waveform rendering approach inside egui (CPU meshes vs. custom wgpu paint callback), as long as
  4 scrolling waveforms stay smooth at display refresh rate.
- Exact SQLite crate and schema.
- Config/data directory handling (e.g. the `directories` crate).

## Project standards

Mirrors the owner's repos (`ha-soundboks-integration`, `uptime-kuma-operator`), adapted for Rust.

- **License: Apache-2.0** (matches both repos; my call, the owner can switch to GPL if they want
  forks to stay open).
- **mise** (`mise.toml`): pins the Rust toolchain, `prek`, and Rust dev tools (e.g.
  `cargo-nextest`, `cargo-deny`), with the same explanatory comments style. `[settings]
  experimental = true`. Setup is `mise install` then `prek install`. Use mise tasks instead of a
  Makefile (my call).
- **prek** (`.pre-commit-config.yaml`), with
  `default_install_hook_types: [commit-msg, pre-commit, pre-push]`:
  - `pre-commit-hooks`: trailing-whitespace, end-of-file-fixer, check-merge-conflict,
    check-added-large-files (`--maxkb=1024`; test audio fixtures must stay small), check-yaml,
    check-toml, check-json, mixed-line-ending (`--fix=lf`), check-executables-have-shebangs,
    check-shebang-scripts-are-executable, detect-private-key.
  - editorconfig-checker, typos, gitleaks, actionlint.
  - commitizen (commit-msg) + commitizen-branch (pre-push).
  - local: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo deny check`
    (advisories + licenses; the Rust analogue of govulncheck).
- **Conventional Commits** + **commitizen** (`.cz.toml`: `cz_conventional_commits`, `tag_format =
  "v$version"`, `version_scheme = "semver2"`, `update_changelog_on_bump = true`, version file =
  workspace `Cargo.toml`). `CHANGELOG.md` generated by `cz bump`.
- **`.editorconfig`** in the same style (LF, UTF-8, final newline, 2-space default; Rust 4 spaces;
  markdown and LICENSE exempt from indent rules).
- **GitHub:**
  - `ci.yml`: `lint` job runs prek via `jdx/mise-action` + `j178/prek-action` (single source of
    truth for checks). `test` and `build` jobs run on a **macOS/Windows/Linux matrix**. Third-party
    actions pinned by SHA with a version comment. `concurrency` cancels superseded runs. On `v*`
    tags, a `release` job builds binaries for all three OSes and attaches them to a GitHub Release
    with generated notes (`-rc` tags → prerelease). App bundling/signing per OS is the builder's
    call. Unsigned binaries are acceptable for v1, documented in the README.
  - `dependabot.yml` (cargo + github-actions, weekly).
  - `pull_request_template.md` (what & why + checklist, as in the owner's repos).
  - Issue templates: bug report, feature request, **new controller support** (asks for the
    controller model, OS, and a MIDI capture), and `config.yml`.
- **Docs:** `README.md` (what it is, features, supported controllers table, install, quick
  start, screenshots once available, releasing), `CONTRIBUTING.md` (setup, making a change,
  commit messages, PRs, license agreement line), `SECURITY.md`, `CHANGELOG.md`, and `docs/`:
  user guide, mapping file reference, **"Adding a controller" guide** (the most likely outside
  contribution), architecture overview, and design history in `docs/superpowers/specs/`.
- **Disclaimer** in the README, like the SOUNDBOKS one: not affiliated with Pioneer DJ,
  AlphaTheta, Atomix (VirtualDJ), or Serato; trademarks used only to name compatibility.
- `.superpowers/` goes in `.gitignore`.

## Verification scope for v1 (my call)

v1 is verified by hand on the owner's Mac with the SB3. Windows and Linux must build, pass all
tests in CI and get release binaries (Windows with ASIO), but hands-on hardware testing on those
OSes comes after v1.

The owner has no existing cues they need for the first practice set, so interop can stay near the
end of the build order. Library size wasn't given: design scanning and the browser for at least
tens of thousands of tracks (my call).

Interop fixtures: Rekordbox and Serato are free downloads. The owner (or a contributor) exports a
few small sample files/XMLs from each app when the interop step starts.

## Testing priorities

1. **Engine:** offline rendering tests (deterministic output for transport, loops, cue behavior,
   sync, mixer routing to master vs. cue). Allocation guard on the render path.
2. **Controller:** the SB3 MIDI capture replayed against the default mapping, asserting the
   resulting actions and the LED output. Explicit tests for deck switching (state and LEDs follow
   the active layer, shift/pad-mode per layer).
3. **Library:** cue persistence (write-through, crash safety, backups, migrations), relinking.
4. **Interop:** round-trip tests per format with fixtures exported from each application.
5. **Manual acceptance:** the success criterion, a full practice set on the SB3 on the owner's
   Mac.

## Suggested build order

The builder can adjust this, but each step should produce something runnable:

1. Repo scaffolding and standards (workspace, mise, prek, CI matrix, docs skeleton, license).
2. Engine core: decode → one deck → audio out, offline render tests.
3. Four decks + mixer + cue routing on a 4-channel device (the SB3).
4. SB3 capture tool, then action registry + mapping engine (with soft takeover) + default SB3
   mapping with LEDs and deck switching.
5. Library: folders, playlists, analysis, beatgrid editing, waveforms, cue persistence.
6. UI: layouts A/B with docking, waveforms, browser, settings, MIDI learn and monitor.
7. Loops, sampler, pad modes, FX.
8. Interop: VirtualDJ, Rekordbox, Serato.
9. Release pipeline and docs completion.
