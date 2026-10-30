# Plan 1: Foundation, Engine Core and Audio Output

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Set up the open-source repo with the owner's standards, and build a real-time-safe 4-deck
engine with mixer and master/headphone routing, plus a command-line player that plays tracks
through the DDJ-SB3's sound card.

**Architecture:** A Cargo workspace. `engine` is pure DSP with no I/O: an `EngineController`
(control side) sends commands over a lock-free SPSC queue to an `EngineRenderer` (audio side)
that fills interleaved stereo master and headphone buffers. `decode` turns files into in-memory
`TrackBuffer`s. `audio-io` opens a multi-channel output device with `cpal` and routes master and
headphones to chosen channel pairs. `dj-software` (the app crate) is a CLI for now. Later plans
add controller, library, UI and interop crates.

**Tech Stack:** Rust 1.99.1 (edition 2024), cpal 0.18, symphonia 0.6, rtrb 0.4, assert_no_alloc
1.1, clap 4.6, anyhow 1, thiserror 2, approx 0.5 (tests), mise, prek, commitizen, cargo-nextest,
cargo-deny.

**Spec:** `docs/superpowers/specs/2026-10-30-dj-software-design.md`

**This is plan 1 of a series.** It covers build-order steps 1 to 3 of the spec. Later plans, each
written after the previous one lands: (2) SB3 capture tool + actions + mapping + controller,
(3) library + analysis + beatgrids, (4) engine beat features (hotcues, loops, sync, keylock,
FX, sampler), (5) UI, (6) interop, (7) release pipeline (including Windows ASIO) and docs
completion.

## Global Constraints

- License **Apache-2.0**. No GPL/LGPL/AGPL dependencies; `cargo deny` enforces the allow-list.
- Must build and pass tests on **macOS, Windows and Linux**.
- **Audio thread never allocates, frees, locks, does I/O or logs.** `EngineRenderer::render` is
  tested under `assert_no_alloc`.
- **Audio files are never modified.**
- Conventional Commits. Every commit passes `prek` hooks.
- Toolchain pinned in `mise.toml`. Third-party GitHub actions pinned by SHA with a version
  comment, except `actions/checkout@v7` and `jdx/mise-action@v4`, which follow the owner's repos.
- Every package is `publish = false` and inherits `version`, `edition`, `rust-version`,
  `license`, `repository` from `[workspace.package]`.
- Workspace version starts at `0.0.0`.

## Review Focus

1. **The device asks for more frames than `max_block_frames`** (common on Windows/Linux: 2048 to 4096). The output callback must render in chunks without allocating. Test: Task 8, `fill_device_buffer_chunks_large_requests`.
2. **Headphone routing on a 2-channel device** (laptop speakers, or the SB3 not plugged in). Starting must fail with a clear message naming the device's channel count, not panic or play silence. Test: Task 8, `routing_rejects_channels_beyond_device`.
3. **The track's sample rate differs from the device's** (48 kHz files on the 44.1 kHz SB3). Playback speed and pitch must be correct. Test: Task 3, `resamples_track_rate_to_output_rate`.
4. **Corrupt, empty or unsupported files.** These must return a `DecodeError`, never panic. A zero-frame track loaded on a deck must render silence. Tests: Task 7, `corrupt_file_is_an_error`, `missing_file_is_an_error`; Task 3, `empty_track_renders_silence`.
5. **Mono and multichannel files** must come out as correct stereo. Test: Task 7, `mono_file_is_duplicated_to_both_channels`.

---

## File Structure

```
Cargo.toml                      workspace + [workspace.package] + [workspace.dependencies]
mise.toml  .pre-commit-config.yaml  .cz.toml  .editorconfig  _typos.toml  deny.toml  .gitignore
LICENSE README.md CONTRIBUTING.md SECURITY.md CHANGELOG.md
.github/workflows/ci.yml  .github/dependabot.yml  .github/pull_request_template.md
.github/ISSUE_TEMPLATE/{bug_report.yml,feature_request.yml,controller_support.yml,config.yml}
docs/architecture.md
scripts/gen-fixtures.sh         regenerates audio test fixtures with ffmpeg
crates/engine/src/lib.rs        re-exports; engine() constructor
crates/engine/src/track.rs      TrackBuffer
crates/engine/src/ids.rs        DeckId
crates/engine/src/command.rs    Command, DeckCommand, ChannelCommand, MasterCommand, TempoRange, XfaderAssign
crates/engine/src/shared.rs     atomics published by the renderer; DeckSnapshot
crates/engine/src/controller.rs EngineController
crates/engine/src/renderer.rs   EngineRenderer (owns decks + mixer, drains commands)
crates/engine/src/deck.rs       Deck: transport, tempo, CDJ cue, interpolated read
crates/engine/src/mixer.rs      Channel strip + master/headphone bus, curves
crates/engine/src/dsp/{mod.rs,biquad.rs,isolator.rs,filter.rs,smooth.rs}
crates/engine/tests/{common/mod.rs,transport.rs,cue.rs,mixer.rs,eq_filter.rs,no_alloc.rs}
crates/decode/src/lib.rs        decode_file
crates/decode/tests/decode.rs   + crates/decode/tests/fixtures/*
crates/audio-io/src/lib.rs      re-exports
crates/audio-io/src/routing.rs  OutputRouting, fill_device_buffer (pure, testable)
crates/audio-io/src/device.rs   list_output_devices, start_output, OutputStream
crates/app/src/main.rs          CLI: `devices`, `play`
```

---

### Task 1: Workspace and tooling standards

**Files:**
- Create: `Cargo.toml`, `crates/{engine,decode,audio-io,app}/Cargo.toml`, a `src/lib.rs` per library crate (`//! <one-line purpose>` only), `crates/app/src/main.rs` (`fn main() {}`), `mise.toml`, `.pre-commit-config.yaml`, `.cz.toml`, `.editorconfig`, `_typos.toml`, `deny.toml`
- Modify: `.gitignore` (add `/target`)

**Interfaces:**
- Produces: workspace with members `crates/*`; package names `engine`, `decode`, `audio-io`, `dj-software` (binary `dj-software`, in `crates/app`). `[workspace.dependencies]` lists every external crate in the Tech Stack line, so member crates use `foo.workspace = true`.

- [ ] **Step 1: Write `mise.toml`**

```toml
[tools]
rust = { version = "1.99.1", components = "rustfmt,clippy" }
prek = "0.5.7"
cargo-binstall = "1.26.1"
"cargo:cargo-nextest" = "0.9.150"
"cargo:cargo-deny" = "0.20.2"

[settings]
experimental = true

# `mise install` pulls every tool above at the pinned version — run it once after
# cloning, then `prek install` to activate the git hooks. cargo-binstall is here so
# the cargo: tools install as prebuilt binaries instead of compiling from source.
# typos, gitleaks, actionlint, editorconfig-checker and commitizen are not here:
# prek fetches them itself from .pre-commit-config.yaml.

[tasks.test]
description = "Run the whole test suite"
run = "cargo nextest run --workspace --no-tests=pass"

[tasks.lint]
description = "Run every check CI runs"
run = "prek run --all-files"
```

- [ ] **Step 2: Write `.pre-commit-config.yaml`**

Copy the structure of `Roxxas96/ha-soundboks-integration`'s file. `default_install_hook_types: [commit-msg, pre-commit, pre-push]`. Use the same `pre-commit-hooks` v6.0.0 hook list (check-added-large-files `--maxkb=1024`, mixed-line-ending `--fix=lf`, and so on), editorconfig-checker 3.11.1 (exclude `\.rs$`: rustfmt is the authority), typos v1.51.1, gitleaks v8.30.1, actionlint v1.7.12, and commitizen v4.19.3 (`commitizen` + `commitizen-branch` at `stages: [pre-push]`). Replace ruff with a `local` repo:

```yaml
  - repo: local
    hooks:
      - id: cargo-fmt
        name: cargo fmt --check
        entry: cargo fmt --all -- --check
        language: system
        pass_filenames: false
        files: '\.rs$'
      - id: cargo-clippy
        name: cargo clippy (warnings are errors)
        entry: cargo clippy --workspace --all-targets --locked -- -D warnings
        language: system
        pass_filenames: false
        files: '(\.rs|Cargo\.(toml|lock))$'
      - id: cargo-deny
        name: cargo deny (advisories, licenses, sources)
        entry: cargo deny check
        language: system
        pass_filenames: false
        files: '(Cargo\.(toml|lock)|deny\.toml)$'
```

Keep the header comment style ("Fast, local checks only...").

- [ ] **Step 3: Write `.cz.toml`, `.editorconfig`, `_typos.toml`, `deny.toml`**

`.cz.toml`: `name = "cz_conventional_commits"`, `version = "0.0.0"`, `tag_format = "v$version"`, `version_scheme = "semver2"`, `major_version_zero = true`, `update_changelog_on_bump = true`, `version_files = ["Cargo.toml:^version"]`.

`.editorconfig`: same as `uptime-kuma-operator`'s (LF, UTF-8, final newline, trim, 2-space default, `[LICENSE]` and `[*.md]` exemptions), plus `[*.rs] indent_size = 4` with the comment "rustfmt is the authority on Rust formatting".

`_typos.toml`: `[files] extend-exclude = ["crates/decode/tests/fixtures/"]`.

`deny.toml`: `[licenses] allow = ["Apache-2.0", "Apache-2.0 WITH LLVM-exception", "MIT", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "Unicode-3.0", "MPL-2.0", "CC0-1.0"]` with a comment that MPL-2.0 is file-level copyleft (symphonia), compatible with Apache-2.0 distribution, and that GPL-family licenses are excluded by the spec. `[advisories]` default (vulnerabilities deny), `[bans] multiple-versions = "warn"`, `[sources] unknown-registry = "deny"`, `unknown-git = "deny"`.

- [ ] **Step 4: Write the workspace `Cargo.toml` and the four crate skeletons**

`[workspace] resolver = "3"`, `members = ["crates/*"]`. `[workspace.package] version = "0.0.0"` (on its own line starting with `version`, which the cz regex relies on), `edition = "2024"`, `rust-version = "1.99"`, `license = "Apache-2.0"`, `repository = "https://github.com/Roxxas96/dj-software"`. `[workspace.lints.rust] unsafe_code = "forbid"`, and each crate sets `[lints] workspace = true`.

- [ ] **Step 5: Install and verify**

Run: `mise install && prek install && cargo build --workspace && mise run test && prek run --all-files`
Expected: build succeeds; nextest reports 0 tests and passes; every prek hook passes (`Cargo.lock` is created by the build and committed).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "build: set up Rust workspace, mise, prek and commitizen"
```

---

### Task 2: Open-source docs, GitHub files and CI

**Files:**
- Create: `LICENSE` (Apache-2.0 verbatim text), `README.md`, `CONTRIBUTING.md`, `SECURITY.md`, `CHANGELOG.md`, `docs/architecture.md`, `.github/workflows/ci.yml`, `.github/dependabot.yml`, `.github/pull_request_template.md`, `.github/ISSUE_TEMPLATE/{bug_report.yml,feature_request.yml,controller_support.yml,config.yml}`

**Interfaces:**
- Consumes: mise tasks `test`, `lint` from Task 1.

- [ ] **Step 1: Write the docs**

- `README.md`: one-paragraph pitch (open-source DJ software in Rust, VirtualDJ-like, built around precise controller mapping), a **status: early development** note, a supported-controllers table (DDJ-SB3: in progress), "Building from source" (mise + `cargo run -p dj-software`), and the disclaimer: *"Not affiliated with Pioneer DJ, AlphaTheta, Atomix Productions (VirtualDJ) or Serato. Product names are trademarks of their owners, used only to describe compatibility."* Leave a `## Releasing` section: tag `vX.Y.Z` after `cz bump`; release binaries come in a later plan.
- `CONTRIBUTING.md`: follow `uptime-kuma-operator`'s sections (Setup, Making a change, Commit messages, Pull requests, license line). Setup adds: Linux needs `libasound2-dev` (ALSA headers) for cpal. Point to `docs/architecture.md` and `docs/superpowers/specs/`. Mention that new controllers are the most welcome contribution and that a guide is coming.
- `SECURITY.md`: report privately via GitHub security advisories; supported version = latest release.
- `CHANGELOG.md`: same header as `ha-soundboks-integration`'s, no entries yet.
- `docs/architecture.md`: the thread model and real-time rules from the spec's "Threads and real-time rules", plus a crate table for the crates that exist now. Each later plan updates this.

- [ ] **Step 2: Write the GitHub files**

- `pull_request_template.md`: "What & why" plus a checklist: `mise run test` passes; `prek run --all-files` passes (or `prek install` is active); docs updated if user-facing behavior changed; for controller changes, the MIDI capture fixture is updated.
- Issue forms: `bug_report.yml` (version, OS, audio device, controller, steps, expected/actual, logs), `feature_request.yml` (problem, proposal, VirtualDJ equivalent if any), `controller_support.yml` (controller model, OS, does it have a built-in sound card, link to the manufacturer MIDI message list, attach a MIDI capture), `config.yml` (`blank_issues_enabled: false`).
- `dependabot.yml`: `cargo` and `github-actions`, weekly.

- [ ] **Step 3: Write `.github/workflows/ci.yml`**

```yaml
name: CI

on:
  push:
    branches: [main]
    tags: [v*.*.*, v*.*.*-rc*]
  pull_request:

concurrency:
  group: ci-${{ github.workflow }}-${{ github.head_ref || github.ref }}
  cancel-in-progress: true

jobs:
  # Reuses .pre-commit-config.yaml as the single source of truth for lint/format
  # checks — the same checks that run locally via `prek install`.
  lint:
    permissions:
      contents: read
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      # cpal links ALSA on Linux; clippy needs the headers to type-check audio-io.
      - run: sudo apt-get update && sudo apt-get install -y libasound2-dev
      - uses: jdx/mise-action@v4
      - uses: j178/prek-action@1382ad72dea72f9d53832f2bcb65b3e87a947204 # v3.0.1

  test:
    permissions:
      contents: read
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v7
      - if: runner.os == 'Linux'
        run: sudo apt-get update && sudo apt-get install -y libasound2-dev
      - uses: jdx/mise-action@v4
      - run: cargo nextest run --workspace --locked --no-tests=pass
      - run: cargo build --workspace --release --locked
```

- [ ] **Step 4: Verify**

Run: `prek run --all-files`
Expected: all hooks pass (actionlint validates `ci.yml`, editorconfig-checker passes the new files).

- [ ] **Step 5: Commit, push and check CI**

```bash
git add -A
git commit -m "docs: add README, contributing guide, community files and CI"
```

Pushing requires a GitHub repo, so ask the owner before creating or pushing to one. Once pushed, `gh run watch` shows all four jobs green.

---

### Task 3: Engine core: tracks, commands and deck transport

**Files:**
- Create: `crates/engine/src/{track.rs,ids.rs,command.rs,shared.rs,controller.rs,renderer.rs,deck.rs}`, `crates/engine/tests/common/mod.rs`, `crates/engine/tests/transport.rs`
- Modify: `crates/engine/src/lib.rs`, `crates/engine/Cargo.toml` (deps: `rtrb`, `thiserror`; dev: `approx`)

**Interfaces:**
- Produces (all re-exported from `engine`):
  - `pub struct TrackBuffer` with `pub fn new(sample_rate: u32, interleaved_stereo: Vec<f32>) -> Result<Self, TrackError>` (error on odd length or `sample_rate == 0`), `pub fn frames(&self) -> usize`, `pub fn sample_rate(&self) -> u32`, `pub fn frame(&self, index: usize) -> [f32; 2]`. Stores `Box<[f32]>`.
  - `pub struct DeckId(u8)` with `pub const ALL: [DeckId; 4]`, `pub fn new(index: usize) -> Option<DeckId>` (0..=3), `pub fn index(self) -> usize`. Deck numbers shown to users are `index + 1`.
  - `pub enum Command { Deck(DeckId, DeckCommand), Channel(DeckId, ChannelCommand), Master(MasterCommand) }`
  - `pub enum DeckCommand { Load(Arc<TrackBuffer>), Unload, Play, Pause, TogglePlay, CuePress, CueRelease, SetTempoSlider(f32), SetTempoRange(TempoRange), Seek(f64) }`. `Seek` is in track frames, clamped to `[0, frames]`.
  - `pub enum TempoRange { Six, Ten, Sixteen, Hundred }` with `pub fn fraction(self) -> f64` (0.06, 0.10, 0.16, 1.00). Default `Ten`.
  - `pub enum ChannelCommand {}` and `pub enum MasterCommand {}` are declared here empty (uninhabited). Task 5 adds their variants.
  - `pub struct EngineConfig { pub sample_rate: u32, pub max_block_frames: usize }`
  - `pub fn engine(config: EngineConfig) -> (EngineController, EngineRenderer)`
  - `EngineController::send(&mut self, cmd: Command) -> Result<(), CommandQueueFull>` (calls `collect_garbage` first), `collect_garbage(&mut self)`, `deck(&self, id: DeckId) -> DeckSnapshot`.
  - `pub struct DeckSnapshot { pub loaded: bool, pub playing: bool, pub position_frames: f64, pub duration_frames: usize, pub track_sample_rate: u32, pub tempo_ratio: f64 }`
  - `EngineRenderer::render(&mut self, master: &mut [f32], headphones: &mut [f32])`: both interleaved stereo with equal length, frames = `len / 2` ≤ `max_block_frames` (`debug_assert`). Drains all queued commands first, then renders, then publishes snapshots. Both buffers are fully overwritten.
- Test helper `tests/common/mod.rs`: `pub fn render(r: &mut EngineRenderer, frames: usize) -> (Vec<f32>, Vec<f32>)`, which renders in `max_block_frames` chunks, and `pub fn ramp_track(sample_rate: u32, frames: usize) -> Arc<TrackBuffer>` (left = right = `i as f32 / frames as f32`).

These decisions are fixed for the implementer:
- Command queue: `rtrb` with capacity **256**. The garbage queue (renderer → controller, carrying replaced `Arc<TrackBuffer>`s) also has capacity **256**. Because `send` collects garbage before pushing, pending garbage can never exceed the commands in flight, so the renderer never has to drop an `Arc` itself (dropping the last `Arc` would free memory on the audio thread).
- Shared state: `Arc<Shared>` holding per-deck atomics (`AtomicBool`; `f64` stored as `AtomicU64` bits; `Relaxed` ordering is fine for display values). The renderer writes them at the end of each `render`.
- Until the mixer exists (Task 5), the renderer sums all decks at unity gain into `master` and writes silence to `headphones`. Task 5 replaces this.
- Deck playback: `rate = tempo_ratio * track_sr / output_sr`. `tempo_ratio = 1 + (slider - 0.5) * 2 * range.fraction()`, slider clamped to `[0, 1]` (1.0 = fastest), and the ratio is clamped to `>= 0`. Read with linear interpolation between `floor(pos)` and `floor(pos) + 1`, then `pos += rate`. When `pos >= frames`: output silence, `playing = false`, `pos = frames`. Load: `pos = 0`, `cue = 0`, paused, slider position kept.

- [ ] **Step 1: Write the failing tests in `crates/engine/tests/transport.rs`**

```rust
#[test] fn silent_when_nothing_loaded()          // all output == 0.0
#[test] fn loaded_but_paused_is_silent()
#[test] fn play_outputs_track_samples_exactly()   // same SR, slider 0.5: master[2*i] == track.frame(i)[0] for i in 0..1000
#[test] fn resamples_track_rate_to_output_rate()  // track 48_000 Hz, output 44_100: after rendering 44_100 frames, position_frames ≈ 48_000 (±1)
#[test] fn tempo_slider_scales_rate()             // range Sixteen, slider 1.0 → tempo_ratio == 1.16; after 1000 frames position ≈ 1160 (±1)
#[test] fn tempo_slider_minus_hundred_stops_advancing() // range Hundred, slider 0.0 → ratio 0.0, position unchanged, no panic
#[test] fn interpolates_between_frames()          // slider gives ratio 0.5 (range Hundred, slider 0.25): output[1] ≈ (f0+f1)/2
#[test] fn stops_at_end_of_track()                // 100-frame track, render 200: playing == false, position == 100, frames 100.. are 0.0
#[test] fn empty_track_renders_silence()          // TrackBuffer::new(44_100, vec![]) loads; play; output all 0.0; no panic
#[test] fn toggle_play_and_pause()
#[test] fn seek_clamps_to_track()                 // Seek(-5.0) → 0; Seek(1e9) → frames
#[test] fn load_replaces_track_and_old_one_is_returned_as_garbage() // Arc::strong_count(old) drops back to 1 after controller.collect_garbage()
#[test] fn four_decks_sum_into_master()           // two decks with constant 0.25 tracks → master ≈ 0.5
#[test] fn track_buffer_rejects_odd_length_and_zero_rate()
```

Write each body with `approx::assert_relative_eq!`/`assert_abs_diff_eq!` and the exact values in its comment.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p engine --test transport`
Expected: compile errors (types not defined).

- [ ] **Step 3: Implement the types and the renderer described in the Interfaces block**

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo nextest run -p engine --test transport`
Expected: 14 passed.

- [ ] **Step 5: Write `crates/engine/tests/no_alloc.rs`**

Install `#[global_allocator] static A: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;` in this test binary. Test `render_does_not_allocate`: send `Load`, `Play`, `SetTempoSlider(0.7)` to two decks, then inside `assert_no_alloc(|| { ... })` render 100 blocks of `max_block_frames`. In the middle of those, send another `Load` before a render (the renderer must push the old track to the garbage queue, not drop it). Add the dev-dependency `assert_no_alloc = { workspace = true, features = ["warn_debug"] }` only if needed for diagnostics. The default (abort on allocation) is what makes the test meaningful. Later tasks extend this test with every new command.

- [ ] **Step 6: Run it**

Run: `cargo nextest run -p engine --test no_alloc`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/engine Cargo.lock
git commit -m "feat(engine): add decks with transport, tempo and lock-free command queue"
```

---

### Task 4: CDJ-style cue

**Files:**
- Modify: `crates/engine/src/deck.rs`
- Test: `crates/engine/tests/cue.rs`

**Interfaces:**
- Consumes: `DeckCommand::{CuePress, CueRelease, Play, Pause}`, `DeckSnapshot` from Task 3.
- Produces: `DeckSnapshot` gains `pub cue_frames: f64`.

Behavior (VirtualDJ's default "CDJ" cue mode):
1. Paused and **not** at the cue point: `CuePress` sets cue = current position (stays paused).
2. Paused **at** the cue point: `CuePress` starts preview playback. `CueRelease` jumps back to the cue point and pauses.
3. During preview, `Play` (press) makes playback continue after `CueRelease`.
4. Playing (not preview): `CuePress` jumps to the cue point and pauses. `CueRelease` does nothing.
5. "At the cue point" means `|pos - cue| < 1.0` frame.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn cue_while_paused_sets_cue_point()         // seek 500, CuePress → cue_frames == 500, playing == false
#[test] fn cue_held_at_cue_point_previews()          // cue at 500: CuePress → playing; render 100; CueRelease → position == 500, paused
#[test] fn play_during_preview_keeps_playing()       // CuePress, Play, CueRelease → playing, position > 500
#[test] fn cue_while_playing_returns_and_pauses()    // play from cue 0 to ~1000; CuePress → position == 0, paused
#[test] fn cue_release_while_playing_is_ignored()
#[test] fn load_resets_cue_to_zero()
```

- [ ] **Step 2: Run them to verify they fail** — `cargo nextest run -p engine --test cue`. Expected: FAIL (`cue_frames` missing).
- [ ] **Step 3: Implement the cue state machine in `Deck`** (a `preview: bool` flag plus `play_latched: bool` is enough).
- [ ] **Step 4: Run them to verify they pass** — Expected: 6 passed. Also run `--test no_alloc` after adding the cue commands to it.
- [ ] **Step 5: Commit** — `git commit -m "feat(engine): add CDJ-style cue behavior"`

---

### Task 5: Mixer: channel strips, crossfader, master and headphone buses

**Files:**
- Create: `crates/engine/src/mixer.rs`, `crates/engine/src/dsp/{mod.rs,smooth.rs}`
- Modify: `crates/engine/src/{command.rs,renderer.rs,shared.rs}`
- Test: `crates/engine/tests/mixer.rs`

**Interfaces:**
- Produces:
  - `pub enum ChannelCommand { Trim(f32), EqLow(f32), EqMid(f32), EqHigh(f32), Filter(f32), Fader(f32), Pfl(bool), XfaderAssign(XfaderAssign) }`. All `f32` values are **knob/fader positions in `[0, 1]`** (clamped). The engine owns the curves, so the controller and the UI share them.
  - `pub enum XfaderAssign { A, Thru, B }`. Default: deck 1 and deck 3 → `A`, decks 2 and 4 → `B`.
  - `pub enum MasterCommand { Crossfader(f32), MasterLevel(f32), HeadphoneLevel(f32), HeadphoneMix(f32) }`. `HeadphoneMix` 0 = cue only, 1 = master only.
  - `pub fn trim_gain(pos: f32) -> f32`, `fader_gain(pos: f32) -> f32`, `crossfader_gains(pos: f32) -> (f32, f32)`, all `pub` in `engine::mixer` so the UI can display dB later.
  - `EngineController::meters(&self) -> Meters { pub channel_peak: [f32; 4], pub master_peak: [f32; 2] }` (peak of the last block, post-fader per channel).
- Defaults: trim 0.5, EQ 0.5, filter 0.5, fader **1.0**, PFL off, crossfader 0.5, master level **0.8**, headphone level 0.8, headphone mix 0.0.

Curves (my calls; the spec leaves them open):
- `trim_gain`: dB = `(pos - 0.5) * 24` (−12 to +12 dB).
- `fader_gain`: `pos * pos`.
- `crossfader_gains`: constant power, `a = cos(pos·π/2)`, `b = sin(pos·π/2)`. `Thru` channels ignore the crossfader.
- `master level` and `headphone level`: `pos * pos * 1.0` (0.8 → 0.64).

Signal flow per channel: deck → trim → EQ → filter (both are pass-through until Task 6) → **PFL tap** → fader → crossfader gain → master sum → master level → clamp to `[-1, 1]`.
Headphones: `lerp(cue_sum, master_pre_clamp, mix) * headphone level`, then clamp. `cue_sum` = sum of PFL taps.
Every gain change is ramped linearly across the block (`dsp::smooth::Ramp { current, target }`) to avoid zipper noise.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn curves_have_expected_values()
//  trim_gain(0.5)==1.0, trim_gain(1.0)≈3.981 (+12 dB), trim_gain(0.0)≈0.2512;
//  fader_gain(0.5)==0.25; crossfader_gains(0.0)==(1.0,0.0), (0.5)≈(0.7071,0.7071), (1.0)≈(0.0,1.0)
#[test] fn fader_zero_mutes_channel_in_master()
#[test] fn crossfader_full_a_mutes_b_channels_but_not_thru()
#[test] fn pfl_sends_prefader_signal_to_headphones()   // deck1 fader 0, PFL on, mix 0 → headphones ≈ signal*0.64, master == 0
#[test] fn headphone_mix_one_is_master()               // mix 1.0 → headphones ≈ master_pre_clamp * 0.64
#[test] fn headphones_silent_without_pfl_at_mix_zero()
#[test] fn master_output_is_clamped()                  // 4 decks at 0.9 constant, trims at 1.0 → every |sample| <= 1.0
#[test] fn gain_changes_are_ramped()                   // fader 1.0 → 0.0 in one block: first sample ≈ unchanged, last ≈ 0, monotonic
#[test] fn meters_report_channel_and_master_peaks()
```

Constant-value tracks (`vec![0.5; n]`) make the expected values exact. With the master level at its default of 0.64, use `MasterLevel(1.0)` in tests where that keeps the arithmetic simple.

- [ ] **Step 2: Run them to verify they fail** — `cargo nextest run -p engine --test mixer`. Expected: FAIL.
- [ ] **Step 3: Implement `mixer.rs` and wire it into `EngineRenderer`**, replacing the Task 3 unity sum. Preallocate any per-channel scratch buffers to `max_block_frames` in `engine()`.
- [ ] **Step 4: Run all engine tests** — `cargo nextest run -p engine`. Expected: all pass, including `transport` (update `four_decks_sum_into_master` and `play_outputs_track_samples_exactly` to set `MasterLevel(1.0)` and `Crossfader` to account for the new defaults, keeping their intent) and `no_alloc` (extended with every channel/master command).
- [ ] **Step 5: Commit** — `git commit -m "feat(engine): add 4-channel mixer with crossfader and headphone cue"`

---

### Task 6: 3-band isolator EQ and single-knob filter

**Files:**
- Create: `crates/engine/src/dsp/{biquad.rs,isolator.rs,filter.rs}`
- Modify: `crates/engine/src/mixer.rs`
- Test: `crates/engine/tests/eq_filter.rs`

**Interfaces:**
- Produces: `pub fn eq_gain(pos: f32) -> f32` in `engine::mixer`. The DSP types stay crate-private.

Decisions (my calls):
- **EQ is an isolator**, so a full-down knob is a true kill: split with **Linkwitz-Riley 4th-order** crossovers (two cascaded 2nd-order Butterworth biquads, Q = 0.7071) at **250 Hz** and **2500 Hz** into low/mid/high, scale each band, and sum. To keep the summed bands phase-coherent, the low band also passes through an allpass at 2500 Hz (standard 3-way LR4 recombination).
- `eq_gain`: `pos <= 0.5` → `(pos / 0.5)^2` (0 = kill, 0.5 = unity). `pos > 0.5` → dB = `(pos - 0.5) / 0.5 * 6` (+6 dB max).
- **Filter:** within a deadzone of `|pos - 0.5| <= 0.02` it is bypassed. Below that it's a low-pass whose cutoff sweeps exponentially from 20 kHz (at the deadzone edge) down to **40 Hz** (at 0). Above it's a high-pass sweeping from **20 Hz** up to **8 kHz** (at 1). Biquad, Q = **1.0**. Coefficients are recomputed at most once per block (no allocation).
- Biquad: transposed direct form II, RBJ cookbook coefficients, per-channel (L/R) state.
- Filter coefficients depend on the sample rate, so they come from `EngineConfig::sample_rate`.

- [ ] **Step 1: Write the failing tests** (helper: `rms_db(signal)` and `sine(freq, sr, frames)` tracks at 44.1 kHz; measure after 4096 frames of settling)

```rust
#[test] fn eq_gain_curve()                       // eq_gain(0.0)==0.0, (0.25)==0.25, (0.5)==1.0, (1.0)≈1.9953
#[test] fn neutral_eq_is_flat()                  // all bands 0.5: |gain| within ±0.5 dB at 50, 1000, 10000 Hz
#[test] fn low_kill_removes_bass()               // EqLow(0.0): 50 Hz attenuated ≥ 30 dB, 5 kHz within ±1 dB
#[test] fn high_kill_removes_treble()            // EqHigh(0.0): 10 kHz attenuated ≥ 30 dB, 100 Hz within ±1 dB
#[test] fn mid_kill_removes_mids()               // EqMid(0.0): 1 kHz attenuated ≥ 20 dB
#[test] fn filter_center_is_bypass()             // Filter(0.5) and Filter(0.51): output == input after settling (±0.1 dB)
#[test] fn filter_low_pass_cuts_highs()          // Filter(0.1): 10 kHz attenuated ≥ 24 dB, 60 Hz within ±3 dB
#[test] fn filter_high_pass_cuts_lows()          // Filter(0.9): 60 Hz attenuated ≥ 24 dB, 10 kHz within ±3 dB
#[test] fn sweeping_filter_never_produces_nan()  // sweep 0→1 across 200 blocks with full-scale noise: all finite, |x| <= 1 at master
```

- [ ] **Step 2: Run them to verify they fail** — `cargo nextest run -p engine --test eq_filter`. Expected: FAIL.
- [ ] **Step 3: Implement the biquad, isolator and filter, and insert them in the channel strip where Task 5 left the pass-throughs.**
- [ ] **Step 4: Run all engine tests** — `cargo nextest run -p engine`. Expected: all pass, `no_alloc` included.
- [ ] **Step 5: Commit** — `git commit -m "feat(engine): add isolator EQ and single-knob filter"`

---

### Task 7: Decoding audio files

**Files:**
- Create: `crates/decode/src/lib.rs`, `crates/decode/tests/decode.rs`, `scripts/gen-fixtures.sh` (executable, `#!/usr/bin/env bash`), and `crates/decode/tests/fixtures/` with `sine440.{wav,aiff,flac,mp3,m4a,ogg}`, `sine440_mono.wav`, `corrupt.mp3`, `truncated.mp3`
- Modify: `crates/decode/Cargo.toml` (deps: `engine`, `symphonia` with the features needed for mp3, aac+isomp4, flac, wav, aiff, vorbis+ogg; `thiserror`)

**Interfaces:**
- Consumes: `engine::TrackBuffer::new`.
- Produces: `pub fn decode_file(path: &Path) -> Result<TrackBuffer, DecodeError>`, and `pub enum DecodeError { Io(std::io::Error), Unsupported(String), Corrupt(String), NoAudioTrack }` (thiserror). Output is at the file's native sample rate (no resampling; the engine handles rate differences). Mono is duplicated to both channels. With more than 2 channels, the first two are kept. Gapless mode is **on**, so MP3/AAC encoder delay and padding are trimmed. This matters for cue positions matching other DJ apps later.

- [ ] **Step 1: Write `scripts/gen-fixtures.sh`**

It generates 1.0 s of 440 Hz sine at amplitude 0.5, 44.1 kHz stereo (and one mono WAV), in each format, with ffmpeg (`-f lavfi -i "sine=frequency=440:sample_rate=44100:duration=1"`, `volume=0.5`, `-ac 2`). `corrupt.mp3` is 4 KiB of deterministic pseudo-random bytes (e.g. `head -c 4096` of a seeded `openssl enc` stream, or a fixed byte pattern), so it isn't decodable. `truncated.mp3` is the first 8 KiB of `sine440.mp3`. Run the script and commit the outputs. Each file must stay under 100 KB.

- [ ] **Step 2: Write the failing tests**

```rust
#[test] fn decodes_every_supported_format()
//  for each of wav, aiff, flac, mp3, m4a, ogg:
//    sample_rate == 44_100;
//    frames within 44_100 ± 2_048 (lossless formats: exactly 44_100);
//    peak within 0.45..=0.55;
//    frequency from zero crossings of the left channel within 440 ± 5 Hz
#[test] fn mono_file_is_duplicated_to_both_channels()  // every frame: left == right, peak ≈ 0.5
#[test] fn corrupt_file_is_an_error()                   // corrupt.mp3 → matches!(err, DecodeError::Unsupported(_) | DecodeError::Corrupt(_))
#[test] fn truncated_file_decodes_what_it_can()         // truncated.mp3 → Ok, 0 < frames < 44_100, every sample finite
#[test] fn missing_file_is_an_error()                   // matches!(err, DecodeError::Io(_))
#[test] fn non_audio_file_is_unsupported()              // decode Cargo.toml → Err(DecodeError::Unsupported(_))
```

- [ ] **Step 3: Run them to verify they fail** — `cargo nextest run -p decode`. Expected: FAIL.
- [ ] **Step 4: Implement `decode_file` with symphonia** (probe by extension hint + content, pick the default audio track, decode all packets into a `Vec<f32>` interleaved stereo; a decode error on an individual packet is skipped, symphonia's recommended behavior; an error before any audio is decoded becomes `Corrupt`). Check symphonia 0.6's API through its docs (context7) rather than from memory: 0.6 changed a lot from 0.5.
- [ ] **Step 5: Run them to verify they pass** — Expected: 6 passed.
- [ ] **Step 6: Commit** — `git commit -m "feat(decode): decode mp3, aac, flac, wav, aiff and ogg into track buffers"`

---

### Task 8: Audio output with master/headphone routing

**Files:**
- Create: `crates/audio-io/src/{routing.rs,device.rs}`
- Modify: `crates/audio-io/src/lib.rs`, `crates/audio-io/Cargo.toml` (deps: `engine`, `cpal`, `thiserror`)
- Test: unit tests in `crates/audio-io/src/routing.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `engine::{EngineRenderer, EngineConfig}`.
- Produces:
  - `pub struct OutputRouting { pub master: [usize; 2], pub headphones: Option<[usize; 2]> }`, 0-based device channel indices. `pub fn validate(&self, device_channels: usize) -> Result<(), RoutingError>`: errors if any index ≥ `device_channels`, or if master and headphones overlap. The message names the device's channel count and the offending index. `OutputRouting::SB3` = master `[0, 1]`, headphones `Some([2, 3])`. `OutputRouting::STEREO` = master `[0, 1]`, headphones `None`.
  - `pub fn fill_device_buffer(renderer: &mut EngineRenderer, scratch: &mut Scratch, out: &mut [f32], device_channels: usize, routing: &OutputRouting)`: renders `out.len() / device_channels` frames in chunks of at most `max_block_frames`, writes master/headphones to their channels, and zeroes every other channel. `pub struct Scratch` holds two preallocated `Vec<f32>` of `2 * max_block_frames`, created by `Scratch::new(max_block_frames)`.
  - `pub struct OutputDeviceInfo { pub name: String, pub max_channels: u16, pub default_sample_rate: u32 }`, and `pub fn list_output_devices() -> Result<Vec<OutputDeviceInfo>, AudioError>`
  - `pub fn open_output(device_name: Option<&str>, routing: OutputRouting) -> Result<PendingOutput, AudioError>`: finds the device (case-insensitive substring match on the name; `None` = system default), picks a config with ≥ the needed channels and an `f32` or `i16` sample format at the default sample rate, and validates the routing. `PendingOutput::engine_config(&self) -> EngineConfig` (`max_block_frames` = **1024**). `PendingOutput::start(self, renderer: EngineRenderer) -> Result<OutputStream, AudioError>` builds and plays the cpal stream.
  - `OutputStream` keeps the stream alive while held, plus `pub fn errors(&self) -> u64`, a count of stream errors reported by cpal's error callback (an `AtomicU64`; the error callback must not allocate or print). Device-loss failover comes in the UI plan.
- The cpal data callback calls `fill_device_buffer`, converting to `i16` with a second preallocated scratch when the format is `i16`. ASIO is **not** in this plan (it's in the release plan). Windows uses WASAPI for now.

- [ ] **Step 1: Write the failing unit tests in `routing.rs`**

```rust
#[test] fn routing_rejects_channels_beyond_device()  // SB3 routing on 2 channels → Err, message contains "2 channels" and "2"
#[test] fn routing_rejects_overlap()                  // master [0,1], headphones Some([1,2]) → Err
#[test] fn fill_writes_master_and_headphones_to_their_channels()
//   engine with deck 1 playing a constant 0.5 track, PFL on, master level 1.0, headphone level 1.0:
//   4-channel buffer → ch0,ch1 ≈ master value; ch2,ch3 ≈ headphone value
#[test] fn fill_zeroes_unrouted_channels()            // 6-channel device, SB3 routing → ch4, ch5 == 0.0
#[test] fn fill_device_buffer_chunks_large_requests() // device buffer of 4096 frames with max_block_frames 1024 → all 4096 frames rendered (no zero tail), runs inside assert_no_alloc
```

(`assert_no_alloc` needs a test-only `#[global_allocator]`. Put the last test in `crates/audio-io/tests/no_alloc.rs` as its own test binary, since the global allocator is per-binary.)

- [ ] **Step 2: Run them to verify they fail** — `cargo nextest run -p audio-io`. Expected: FAIL.
- [ ] **Step 3: Implement `routing.rs`.**
- [ ] **Step 4: Run them to verify they pass** — Expected: 5 passed.
- [ ] **Step 5: Implement `device.rs`** using cpal 0.18 (check its docs through context7: the device/config API changed across 0.15 to 0.18). There are no automated tests for it, since CI has no audio devices. Task 9 exercises it by hand.
- [ ] **Step 6: Verify it builds everywhere it can** — `cargo clippy -p audio-io --all-targets -- -D warnings`. Expected: clean. CI covers Windows/Linux.
- [ ] **Step 7: Commit** — `git commit -m "feat(audio-io): add multi-channel output with master and headphone routing"`

---

### Task 9: Command-line player and hands-on check on the SB3

**Files:**
- Modify: `crates/app/src/main.rs`, `crates/app/Cargo.toml` (deps: `engine`, `decode`, `audio-io`, `clap` with `derive`, `anyhow`), `README.md` (quick start), `docs/architecture.md` (crate table)

**Interfaces:**
- Consumes: everything above.
- Produces the CLI:
  - `dj-software devices`: lists output devices, one per line: `<name>  (<max_channels> ch, <rate> Hz)`.
  - `dj-software play <TRACK> [--cue-track <TRACK2>] [--device <NAME>] [--master <L,R>] [--headphones <L,R>]`. Channel numbers are **1-based** on the command line (as printed on hardware) and converted to 0-based `OutputRouting`. Defaults: `--master 1,2`, and headphones off unless `--headphones` is given. `TRACK` plays on deck 1 to master. `--cue-track` loads on deck 2 with its fader at 0 and PFL on, playing, so it's heard only in the headphones. Crossfader stays at the default 0.5. The command prints `Deck 1  mm:ss / mm:ss` on one line, updated every 250 ms (`\r`), and calls `collect_garbage` on each tick. It exits when deck 1 stops at the end of the track, or on Ctrl-C (exit code 0).
- Decoding happens on the main thread before the stream starts. Errors are reported through `anyhow` with context (`"decoding <path>"`, `"opening audio device"`). A routing error prints the `RoutingError` message plus a hint to run `dj-software devices`.

- [ ] **Step 1: Write a CLI parsing test** (`#[cfg(test)]` in `main.rs`, using `clap::Parser::try_parse_from`)

```rust
#[test] fn parses_one_based_channels()      // ["dj-software","play","a.mp3","--headphones","3,4"] → routing.headphones == Some([2,3]), master [0,1]
#[test] fn rejects_channel_zero()           // "--master","0,1" → Err
#[test] fn rejects_malformed_channel_pair() // "--master","1" → Err
```

- [ ] **Step 2: Run them to verify they fail** — `cargo nextest run -p dj-software`. Expected: FAIL.
- [ ] **Step 3: Implement the CLI.**
- [ ] **Step 4: Run all tests and lints** — `mise run test && prek run --all-files`. Expected: everything passes.
- [ ] **Step 5: Hands-on check with the owner (the SB3 must be plugged in)**

```bash
cargo run --release -p dj-software -- devices
# expect a "DDJ-SB3" entry with 4 ch
cargo run --release -p dj-software -- play <track-a> --cue-track <track-b> --device "DDJ-SB3" --headphones 3,4
```

Expected: track A on the speakers (master out), track B only in the headphones, at the correct pitch and speed, with no clicks or dropouts for a full track. Then run `play <track-a>` with no `--device` to check the default output on the Mac. Ask the owner to confirm what they hear. Don't claim this step passed without their confirmation.

- [ ] **Step 6: Update `README.md`** (quick start with the two commands above) and `docs/architecture.md` (crate table: `engine`, `decode`, `audio-io`, `dj-software`).
- [ ] **Step 7: Commit** — `git commit -m "feat(app): add command-line player with headphone cue routing"`
