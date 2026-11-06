# Plan 5: Config File, Sampler and Pad Modes

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The sound features the owner uses, working on the DDJ-SB3:
- a global `config.toml`;
- the sampler (own files on the SAMPLER pads, SHIFT + pad stops);
- beat jump on SHIFT + HOT CUE, with configurable sizes;
- loop roll on SHIFT + FX FADE and slicer on SHIFT + PAD SCRATCH;
- slip mode on SHIFT + VINYL;
- an option that disables the crossfader.

**Architecture:**
- **Engine:**
  - a deck-level slip "ghost" playhead, which loop roll, slicer and slip mode share;
  - beat jump;
  - a shared 8-slot sampler mixed into the master bus;
  - a crossfader-enabled flag in the mixer.
- **Pad modes:** the SB3 does not switch its pads for the SHIFT modes (see the design's capture findings), so each side's pad mode lives in `ControlState`. Mapping bindings get a `pad_mode` condition.
- **App:** it reads `config.toml` at startup. The config fills the pad size tables in `ControlState`, loads the samples off the audio thread and sets the crossfader flag.

**Tech Stack:** Rust 1.99.1, existing crates (serde + toml already in the workspace for mappings), no new dependencies.

**Spec:** `docs/superpowers/specs/2026-11-06-sound-and-pad-modes-design.md` (read its "What the capture found" and "Settled after the builder check" sections first). It extends `docs/superpowers/specs/2026-10-30-dj-software-design.md`.

**Out of scope (plan 6):** keylock, the FX section, and the firmware pad modes (FX FADE, PAD SCRATCH and TRANS routines). The FX FADE and PAD SCRATCH pad notes stay unbound. LEDs stay as they are.

**Plan decision (deviates from the design's wording):** the design says samples are decoded and resampled to the output rate at startup. Instead they are decoded at startup and played at their native rate through the same interpolating read the decks use (rate step = sample rate / output rate). The audible result is the same and it needs no resampler. The 60 s cap and the startup reports stay as designed.

## Global Constraints

- License **Apache-2.0**; no new dependencies.
- Builds and tests on macOS, Windows and Linux. CI has no audio or MIDI devices.
- **The audio thread never allocates, frees, locks or blocks.**
  - Samples are decoded off the audio thread.
  - Replaced sample buffers go back to the control thread through the garbage queue, as tracks do.
  - Every new engine command and render path is covered by `crates/engine/tests/no_alloc.rs`.
- **Anti-goals (main spec and design):**
  - no freeze, crash or dropout;
  - a control never does something other than its mapping says, including after deck switching;
  - cues never disappear;
  - a sample that does not play must say why at startup;
  - a config mistake is reported with the file, key and problem.
- **Config errors:**
  - a missing `config.toml` means defaults;
  - an invalid value stops startup with `<file>: <key>: <problem>`;
  - an unreadable sample, or one longer than **60 s**, is reported at startup and its pad stays silent, and the app still starts.
- **Defaults** (the owner's, from the design):
  - **beat jump pads 1-8:** −4, +4, −8, +8, −16, +16, −32, +32 beats;
  - **SHIFT + beat jump pads 1-8:** −1, +1, −2, +2, −4, +4, −64, +64 beats;
  - **loop roll pads 1-8:** 1/16, 1/8, 1/4, 1/2, 1, 2, 4, 8 beats;
  - **sampler volume:** 1.0;
  - **crossfader:** enabled.
- **SB3 MIDI values** come from `controllers/ddj-sb3/capture.toml`, which must not be modified.
- Conventional Commits, prek on every commit, branch `feat/plan-5-sound`, landing via PR.

## Review Focus

1. **A sample file is missing, unreadable or longer than 60 s.** The app still starts, prints which pad and file and why, and that pad does nothing. Test: Task 7, `bad_samples_are_reported_and_silent`.
2. **A SHIFT pad mode chosen while the hardware pads are in the SAMPLER bank.** The pads then send `0x30`-`0x37`; they act as the chosen SHIFT mode, not as samples. Test: Task 8, `shift_mode_reads_the_sampler_bank_too`.
3. **DECK pressed while a loop roll or slicer pad is held.** The release still reaches the deck that got the press, and that deck slips back. Deck 3 is untouched. Test: Task 8, `held_roll_and_slice_release_on_the_pressed_deck`.
4. **Loading a track while rolling, slicing or slipping.** The new track starts clean: no roll, no slice, no ghost playhead, paused at 0. Tests: Tasks 2 and 3, `load_resets_*`.
5. **Beat jump past the track's start or end, or on a deck without a grid.** The playhead is clamped to `[0, frames]`; without a grid nothing happens. Test: Task 2, `beat_jump_clamps_and_needs_a_grid`.

---

## File Structure

```
crates/engine/src/deck/{slip.rs,beat_jump.rs,roll.rs}   (new; slip ghost, beat jump, roll and slicer)
crates/engine/src/deck/{mod.rs,looping.rs,hotcue.rs,jog.rs}
crates/engine/src/sampler.rs                            (new; 8 sample voices)
crates/engine/src/{command.rs,renderer.rs,mixer.rs,shared.rs,lib.rs}
crates/engine/tests/{slip.rs,beat_jump.rs,roll.rs,sampler.rs,mixer.rs,no_alloc.rs}
crates/actions/src/{action.rs,apply.rs,state.rs}
crates/actions/tests/apply.rs
crates/controller/src/mapping/{file.rs,resolve.rs}
crates/controller/tests/{runtime.rs,sb3_replay.rs}
crates/app/src/config.rs                                (new; config.toml schema, loading, errors)
crates/app/src/samples.rs                               (new; decode the configured samples)
crates/app/src/{main.rs,play.rs,library_mode/mod.rs}
controllers/ddj-sb3/{mapping.toml,README.md}
docs/config.md (new), docs/controllers/mapping-reference.md, docs/architecture.md, README.md
```

---

### Task 1: Slip mode and the ghost playhead

**Files:** Create `crates/engine/src/deck/slip.rs` and `crates/engine/tests/slip.rs`. Modify `command.rs`, `deck/mod.rs`, `deck/looping.rs`, `deck/hotcue.rs`, `deck/jog.rs`, `shared.rs` and `tests/no_alloc.rs`.

**Interfaces:**
- `DeckCommand::SetSlip(bool)` turns a deck's slip mode on or off. It is off by default and kept across loads.
- `DeckSnapshot` gains `slip: bool` and `ghost_frames: Option<f64>`.
- Inside the deck (`slip.rs`), for Tasks 3 and 4:
  - `fn slip_begin(&mut self, forced: bool)` starts the ghost at the playhead when slip is on or `forced`, unless a ghost is already running.
  - `fn slip_end(&mut self)` moves the playhead to the ghost and drops the ghost.
- **The ghost** advances every output frame by the deck's normal speed: the tempo ratio × rate step × (1 + nudge bend). It wraps inside an active normal loop exactly as the playhead would, and pauses when the deck pauses.
- **What slip covers while it is on:**
  - **a scratch:** a touch with vinyl on begins the ghost and the touch release ends it;
  - **a normal loop:** activating it begins the ghost and exiting it ends it;
  - **a hotcue jump while playing:** the pad press begins the ghost and the pad release ends it, so with slip on a hotcue becomes momentary.
- **Turning slip off** while a ghost runs drops the ghost; the playhead stays where it is.
- `Load` drops the ghost.

- [ ] **Step 1: Write the failing tests** in `crates/engine/tests/slip.rs`:
  - `slip_scratch_returns_where_the_track_would_be` (touch, scratch back 0.5 turn, render 1 s, release: the playhead is at start + 1 s of normal playback, within 1 frame)
  - `slip_loop_exit_returns_to_the_ghost`
  - `slip_hotcue_is_momentary`
  - `ghost_follows_tempo_and_nudge`
  - `slip_off_drops_the_ghost`
  - `without_slip_nothing_changes` (the plan 4 behaviors are untouched)
  - `load_resets_slip_ghost`
  - no_alloc: slip on with a scratch, a loop and a hotcue on a playing deck.
- [ ] **Step 2: Run** `cargo test -p engine`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p engine`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(engine): add slip mode`

---

### Task 2: Beat jump

**Files:** Create `crates/engine/src/deck/beat_jump.rs` and `crates/engine/tests/beat_jump.rs`. Modify `command.rs`, `deck/mod.rs` and `tests/no_alloc.rs`.

**Interfaces:**
- `DeckCommand::BeatJump { beats: f64 }`. A negative value jumps back.
- **Behavior:**
  - It needs a grid. It moves the playhead by `beats × period`, clamped to `[0, frames]`.
  - **On an active normal loop**, both loop points and the playhead move by the same amount (VirtualDJ behavior), as long as the moved loop stays inside the track. If it would not, the loop exits and the playhead jumps alone.
  - A running slip ghost moves by the same amount.
  - Non-finite `beats`, or no grid: nothing happens.
  - It is a deliberate jump, so a pending phase sync is dropped (plan 4 rule R3).

- [ ] **Step 1: Write the failing tests:**
  - `beat_jump_moves_by_whole_beats` (120 BPM at 44.1 kHz: +4 beats = +88200 frames)
  - `beat_jump_moves_an_active_loop_with_it`
  - `beat_jump_clamps_and_needs_a_grid`
  - `beat_jump_drops_a_pending_sync`
  - no_alloc: jumps with and without a loop.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p engine`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(engine): add beat jump`

---

### Task 3: Loop roll and slicer

**Files:** Create `crates/engine/src/deck/roll.rs` and `crates/engine/tests/roll.rs`. Modify `command.rs`, `deck/mod.rs`, `shared.rs` and `tests/no_alloc.rs`.

**Interfaces:**
- **New `DeckCommand`s:**
  - `RollPress { beats: f64 }` and `RollRelease`;
  - `SlicePress(u8)` and `SliceRelease(u8)`, with slice 0-7.
- `DeckSnapshot` gains `roll_active: bool` and `slicer_block_beat: Option<f64>`, the first beat of the current 8-beat block.
- **Both always slip** (`slip_begin(forced = true)`) and use a *roll loop* that is separate from the normal loop:
  - The normal loop is never changed by a roll or a slice.
  - The ghost keeps wrapping in the normal loop if one is active.
  - While a roll loop is active, the playhead wraps in it instead.
- **Roll:**
  - It needs a grid and a playing deck.
  - The press starts a roll loop of `beats × period` at the playhead.
  - A press while another roll is held replaces that roll; the ghost keeps running.
  - The release ends the roll and slips back, but only if the roll was started by a press of the same size.
- **Slicer:**
  - It needs a grid and a playing deck.
  - The 8-beat block is `floor((beats_at(ghost or playhead) + 1e-6) / 8) × 8`, counted from the grid's first beat. It follows playback while no slice is held.
  - Pressing slice `n` plays the 1-beat roll loop `[block + n, block + n + 1)` in beats.
  - Releasing that same slice slips back. A press of another slice while one is held moves the loop to the new slice.
- `Load`, `Seek` and a hotcue jump end a roll or slice and drop the ghost.

- [ ] **Step 1: Write the failing tests:**
  - `roll_loops_then_slips_back`
  - `roll_keeps_a_normal_loop_intact`
  - `roll_needs_grid_and_playing`
  - `release_of_another_roll_size_is_ignored`
  - `slicer_block_follows_playback`
  - `slice_press_repeats_its_beat_then_slips_back`
  - `slice_to_slice_moves_the_loop`
  - `load_resets_roll_and_slice`
  - no_alloc: roll press/release, and slice press/move/release.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p engine`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(engine): add loop roll and slicer`

---

### Task 4: Sampler and crossfader disable in the engine

**Files:** Create `crates/engine/src/sampler.rs` and `crates/engine/tests/sampler.rs`. Modify `command.rs`, `renderer.rs`, `mixer.rs`, `shared.rs`, `controller.rs`, `lib.rs`, `crates/engine/tests/mixer.rs` and `tests/no_alloc.rs`.

**Interfaces:**
- `pub const SAMPLER_SLOTS: usize = 8;`
- `Command::Sampler(SamplerCommand)` with `pub enum SamplerCommand`:
  - `Set(usize, Option<Arc<TrackBuffer>>)`: a replaced buffer goes to the garbage queue, like a track;
  - `Play(usize)`: from the start, restarting if it is playing;
  - `Stop(usize)`;
  - `Volume(f32)`: 0-1, clamped, non-finite ignored.

  An out-of-range slot is ignored.
- **Samples are `TrackBuffer`s,** so a replaced one goes through the existing `Arc<TrackBuffer>` garbage queue.
- **Rendering:**
  - Each playing slot reads its buffer at rate step `sample_rate / output_rate` with the deck's linear interpolation and stops at its end.
  - Slots are summed × volume (`level_gain` curve) into the master sum after the channels and before the master level and clamp, so the headphone master mix hears them.
- `EngineController::sampler() -> SamplerSnapshot { playing: [bool; 8] }`, published like the meters.
- `MasterCommand::CrossfaderEnabled(bool)`, default `true`. When disabled, every channel's crossfader gain is 1.0, whatever its assignment or the crossfader position. Re-enabling restores the gains from the current position.

- [ ] **Step 1: Write the failing tests:**
  - `sample_plays_once_then_stops`
  - `play_again_restarts`
  - `stop_silences`
  - `samples_play_at_their_own_rate` (a 22.05 kHz sample at a 44.1 kHz output lasts twice as many output frames)
  - `samples_reach_master_and_headphone_master_mix`
  - `replaced_sample_goes_to_garbage`
  - `out_of_range_slot_is_ignored`
  - `crossfader_disabled_plays_every_channel_at_full_level` (mixer test)
  - no_alloc: set, play, restart, stop, volume, and a render with 8 slots playing.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p engine`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(engine): add the sampler and a crossfader switch`

---

### Task 5: Actions for pad modes, beat jump, roll, slicer, slip and the sampler

**Files:** Modify `crates/actions/src/{action.rs,apply.rs,state.rs}`, `crates/actions/tests/apply.rs` and `docs/controllers/mapping-reference.md`.

**Interfaces:**
- `pub enum PadMode { Hardware, BeatJump, LoopRoll, Slicer }`, re-exported, with ids `"hardware"`, `"beat_jump"`, `"loop_roll"`, `"slicer"`.
  - `SideControl` gains `pad_mode: PadMode`, default `Hardware`.
- `ControlState` gains:
  - `beat_jump_pads: [f64; 8]` and `beat_jump_shift_pads: [f64; 8]`;
  - `loop_roll_pads: [f64; 8]`.

  Their defaults are the Global Constraints values.
- **New action kinds**, with "N" being slots 1-8 through static id tables as for hotcues. Each row lists its target, input and what `apply` sends:

  | Id | Target | Input | Sends |
  |---|---|---|---|
  | `pads.mode_hardware`, `pads.mode_beat_jump`, `pads.mode_loop_roll`, `pads.mode_slicer` | side | button | sets that side's `pad_mode`; no engine command |
  | `deck.beat_jump_N` | deck | button | `BeatJump { beats: beat_jump_pads[N-1] }` |
  | `deck.beat_jump_shift_N` | deck | button | `BeatJump { beats: beat_jump_shift_pads[N-1] }` |
  | `deck.loop_roll_N` | deck | button, press and release | `RollPress { beats: loop_roll_pads[N-1] }` / `RollRelease` |
  | `deck.slice_N` | deck | button, press and release | `SlicePress(N-1)` / `SliceRelease(N-1)` |
  | `deck.slip_toggle` | deck | button | `SetSlip(!snapshot.slip)` |
  | `sampler.play_N` | none | button | `Sampler(Play(N-1))` |
  | `sampler.stop_N` | none | button | `Sampler(Stop(N-1))` |

- `ActionKind::ALL` grows by these 53 kinds, from 57 to 110.
  - `takes_release` adds `LoopRoll` and `Slice`.
  - `hold_partner` (plan 4) is unchanged. The SB3 binds the roll and slice actions to both the unshifted and the shifted pad notes (Task 8), so a release that arrives with SHIFT held still finds its own binding.

- [ ] **Step 1: Write the failing tests** in `crates/actions/tests/apply.rs`:
  - `pad_mode_actions_set_the_side_mode`
  - `beat_jump_uses_the_configured_sizes` (change `beat_jump_pads[0]` to −2, then press → −2 beats)
  - `roll_and_slice_send_press_and_release`
  - `slip_toggle_reads_the_snapshot`
  - `sampler_actions_send_slot_commands`
  - Update `ids_round_trip_for_every_action` and `docs_list_every_action_id` to cover the 110 ids.
- [ ] **Step 2: Run** `cargo test -p actions`. Expected: FAIL.
- [ ] **Step 3: Implement**, and add the rows to the action table in the mapping reference.
- [ ] **Step 4: Run** `mise run test`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(actions): add pad modes, beat jump, roll, slicer, slip and sampler actions`

---

### Task 6: The `pad_mode` mapping condition

**Files:** Modify `crates/controller/src/mapping/{file.rs,resolve.rs}`, `crates/controller/tests/runtime.rs` and `docs/controllers/mapping-reference.md` (the `[[input]]` keys table).

**Interfaces:**
- `[[input]]` gains `pad_mode = "hardware" | "beat_jump" | "loop_roll" | "slicer"`. The binding fires only while its side's `pad_mode` matches.
  - It needs a binding with a side: a `side`, or a `deck` (taken as that deck's side).
  - On a master action it is a load error: `pad_mode needs a side`.
  - An unknown value is a load error that lists the four ids.
- `InputBinding.pad_mode: Option<PadMode>`, checked in `conditions_hold`.
- A held press keeps its release routing even if the pad mode changes while it is held. This is already true through `held`; pin it with a test.

- [ ] **Step 1: Write the failing tests:**
  - `pad_mode_condition_selects_the_binding`
  - `pad_mode_needs_a_side`
  - `unknown_pad_mode_lists_the_ids`
  - `release_after_a_pad_mode_change_reaches_the_pressed_action`
- [ ] **Step 2: Run** `cargo test -p controller`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `mise run test`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(controller): add the pad_mode mapping condition`

---

### Task 7: `config.toml` and sample loading

**Files:** Create `crates/app/src/config.rs`, `crates/app/src/samples.rs` and `docs/config.md`. Modify `crates/app/src/{main.rs,play.rs}` and `README.md` (one line pointing to docs/config.md).

**Interfaces:**
- **Location:** `controller::builtin::config_dir()` / `config.toml`. `play` gains `--config <PATH>` to use another file (for tests). A missing default file means `Config::default()`.
- **Schema** (serde with `deny_unknown_fields`; every section and key is optional):

  ```toml
  [mixer]
  crossfader = true            # false: the crossfader is ignored, every channel at full level

  [sampler]
  volume = 1.0                 # 0 to 1
  # Pad 1-8: a file (absolute, or starting with ~/) or a library track id
  pads = [
    { file = "~/Samples/airhorn.wav" },
    { track = 1234 },
  ]                            # up to 8 entries; fewer leaves the rest empty

  [beat_jump]
  pads = [-4, 4, -8, 8, -16, 16, -32, 32]          # beats, exactly 8, each non-zero and |beats| <= 512
  shift_pads = [-1, 1, -2, 2, -4, 4, -64, 64]

  [loop_roll]
  pads = [0.0625, 0.125, 0.25, 0.5, 1, 2, 4, 8]    # beats, exactly 8, each in [1/32, 64]
  ```

- **Functions:**
  - `pub fn load(path: Option<&Path>) -> anyhow::Result<Config>`. An invalid value errors as `<file>: <key>: <problem>`: unknown keys, wrong types, wrong lengths, out-of-range values, a pad with both or neither of `file`/`track`, more than 8 pads.
  - `samples::load(config: &SamplerConfig, library: Option<&Reader>) -> Vec<(usize, Result<Arc<TrackBuffer>, SampleError>)>`. It decodes each pad off the audio thread with `decode::decode_file`; a `track` id is read through the library.
  - Failures are reported at startup and the pad stays empty:
    - a missing file;
    - a decode error;
    - an unknown track id;
    - a track id with no library;
    - a sample longer than 60 s.

    Each is printed as `sampler pad N (<file or track>): <reason>`, through `format::safe` for paths.
- **`play` at startup:**
  - it loads the config;
  - it sends `CrossfaderEnabled`, the volume and one `Sampler(Set(..))` per loaded pad;
  - it copies the size tables into the runtime's `ControlState`. Add `Runtime::control_state_mut()` if no setter exists. A size table edited through it is not an engine value, so soft takeover is not involved.

- [ ] **Step 1: Write the failing tests** (unit tests in `config.rs` and `samples.rs`, plus a CLI test in `crates/app/tests/library_cli.rs` style for `play --config` with a bad file):
  - `missing_config_is_default`
  - `full_example_parses` (the docs example exactly)
  - `bad_values_name_the_key` (one case per rule above, checking the message)
  - `bad_samples_are_reported_and_silent` (a missing file, a 61 s WAV fixture written in the test, an unknown track id: three reports, three empty pads, one good pad loaded)
  - `tilde_paths_expand`
- [ ] **Step 2: Run** `cargo test -p open-jockey`. Expected: FAIL.
- [ ] **Step 3: Implement**, and write `docs/config.md`: location per OS, every key with its default, the example, and what happens on errors.
- [ ] **Step 4: Run** `mise run test`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(app): read config.toml and load the sampler pads`

---

### Task 8: SB3 bindings, status lines, docs and hands-on check

**Files:** Modify `controllers/ddj-sb3/mapping.toml`, `controllers/ddj-sb3/README.md`, `crates/controller/tests/sb3_replay.rs`, `crates/app/src/{play.rs,library_mode/mod.rs}` (status lines), `docs/controllers/mapping-reference.md` and `docs/architecture.md` (slip ghost, sampler, software pad modes).

**Bindings:** values from the capture. Left side shown: mode buttons on channel 1 and pads on channel 8. The right side is the same on channels 2 and 9. Every deck binding uses `side`.

| Control | MIDI | Action / condition |
|---|---|---|
| HOT CUE, FX FADE, PAD SCRATCH, SAMPLER, SHIFT + SAMPLER (TRANS) | ch1 notes 0x1B, 0x1E, 0x20, 0x22, 0x6E | `pads.mode_hardware` |
| SHIFT + HOT CUE | ch1 note 0x69 | `pads.mode_beat_jump` |
| SHIFT + FX FADE | ch1 note 0x6B | `pads.mode_loop_roll` |
| SHIFT + PAD SCRATCH | ch1 note 0x6D | `pads.mode_slicer` |
| HOT CUE pads 0x00-0x07 / SHIFT 0x08-0x0F | ch8 | existing hotcue bindings, now with `pad_mode = "hardware"` |
| SAMPLER pads 0x30-0x37 / SHIFT 0x38-0x3F | ch8 | `sampler.play_N` / `sampler.stop_N` with `pad_mode = "hardware"` |
| both banks above, pad N / SHIFT + pad N | ch8 | `deck.beat_jump_N` / `deck.beat_jump_shift_N` with `pad_mode = "beat_jump"`; `deck.loop_roll_N` (both unshifted and shifted notes) with `pad_mode = "loop_roll"`; `deck.slice_N` (both) with `pad_mode = "slicer"` |
| SHIFT + VINYL | ch1 note 0x40 | `deck.slip_toggle` |

- The SAMPLER pads are mirrored: left and right pad N both play sample N.
- Add a comment block above the pad bindings explaining software pad modes, and the caveat about FX FADE / PAD SCRATCH.
- **Status lines:**
  - each side shows its pad mode when it is not `hardware` (`left: beat jump`);
  - a deck shows `slip` while slip is on and `roll` while a roll or slice is held;
  - a line `sampler: 2 5` lists the playing slots while any play.

- [ ] **Step 1: Write the failing replay tests:**
  - `mode_buttons_set_the_side_pad_mode`
  - `beat_jump_pads_jump_by_the_configured_beats` (including SHIFT + pad)
  - `shift_mode_reads_the_sampler_bank_too`
  - `roll_pads_roll_and_slip_back`
  - `slicer_pads_repeat_and_slip_back`
  - `held_roll_and_slice_release_on_the_pressed_deck` (DECK pressed while held, checked at engine level: deck 1 slipped back, deck 3 unchanged)
  - `sampler_pads_play_and_shift_stops`
  - `shift_vinyl_toggles_slip`
- [ ] **Step 2: Run** `cargo test -p controller --test sb3_replay`. Expected: FAIL.
- [ ] **Step 3: Write the bindings, status lines and docs.** The docs tests must pass.
- [ ] **Step 4: Run** `mise run test && prek run --all-files`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(controllers): map the SB3 sampler, beat jump, roll, slicer and slip`
- [ ] **Step 6: Hands-on check with the owner**, with a `config.toml` holding 2-3 of their samples. Run `play --library --controller ddj-sb3` (master on the MacBook speakers).
  1. **Sampler:** in SAMPLER mode, the pads play the samples from start to end, pressing again restarts, and SHIFT + pad stops. A missing file is reported at startup.
  2. **Beat jump:** in SHIFT + HOT CUE mode, the pads jump ±4/8/16/32 and SHIFT + pads jump ±1/2/4/64. Change a size in `config.toml`, restart, and check it.
  3. **Loop roll:** in SHIFT + FX FADE, after HOT CUE, the pads roll 1/16 to 8 beats and the release lands in time.
  4. **Slicer:** in SHIFT + PAD SCRATCH, after HOT CUE, the pads repeat beats of the current bar and the release lands in time.
  5. **Slip:** SHIFT + VINYL. With slip on, scratch, loop or hotcue, then let go: playback lands where the track would have been.
  6. **Crossfader:** with `crossfader = false`, moving the crossfader changes nothing.

  Record the results in `controllers/ddj-sb3/README.md`, and fix misses before the PR merges.
