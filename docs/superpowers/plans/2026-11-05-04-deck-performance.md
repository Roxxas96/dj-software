# Plan 4: Deck Performance (Jog Wheels, Hotcues, Loops, Sync)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The SB3's deck controls work in Open Jockey. That covers the jog wheels (nudge, scratch in VINYL mode, moving through a paused track, SHIFT search), 8 hotcues per track that are remembered in the library, loops (auto loop with halve/double, loop in/out, reloop/exit) and SYNC (tempo plus beat phase from the beatgrid).

**Architecture:**
- **Engine:** the engine learns each deck's beatgrid. `DeckCommand::Load` now carries a `TrackMemory` (cue, hotcues, grid), so a track arrives on the deck complete in one command. The deck gains hotcue, loop and jog state, all handled on the audio thread so jumps and loop wraps are sample-exact. The deck renderer learns to play backwards (scratching). Beat-phase sync is a renderer-level command because it reads two decks.
- **Actions and controller:** new actions turn controller input into those commands. The sync tempo match is computed on the control thread from snapshots, so the tempo slider's soft takeover arms the usual way.
- **Library and app:** the library stores hotcues. The app's cue sync grows into a memory sync for the cue point and the hotcues, and the app sends grid updates when a grid is edited or analyzed.

**Tech Stack:** Rust 1.99.1, existing crates only (no new dependencies).

**Spec:** `docs/superpowers/specs/2026-10-30-dj-software-design.md`, sections "Engine features (v1)" (decks, beatgrid, hotcues, loops), "DDJ-SB3 specifics", "Threads and real-time rules" and "Anti-goals".

**Out of scope (plan 5: sound):** keylock, beat FX, pad FX (the FX FADE and PAD SCRATCH pad modes), the sampler (SAMPLER pad mode), loop roll and beat jump, and the shifted pad modes. These need audio processing (time-stretch, effects, slip mode) or a capture of the shifted pad modes. KEY LOCK, the FX section and the non-hotcue pads stay unmapped. Pad and button LEDs stay out until the LED handshake is found: the owner said LEDs are optional for now. Hotcue colors and names exist in the database but are only set by the UI (plan 6) and interop (plan 7).

**Roadmap change:** the plan series becomes 8 plans: (4) deck performance, (5) sound: keylock, FX, sampler and pad modes, (6) UI, (7) interop, (8) release.

## Global Constraints

- License **Apache-2.0**; no new dependencies.
- Builds and tests on macOS, Windows and Linux. CI has no audio or MIDI devices.
- **The audio thread never allocates, frees, locks or blocks.** Every new engine command and render path is covered by the `assert_no_alloc` test in `crates/engine/tests/no_alloc.rs`.
- **Anti-goals:**
  - no freeze, crash or dropout;
  - a control never does something other than its mapping says, including after deck switching;
  - **cues (cue point and hotcues) never disappear**. They are written through to the library on every change, and only an explicit delete removes a hotcue.
- Engine commands with non-finite values are ignored, as today. Positions are clamped to `[0, frames]`.
- Every SB3 MIDI value comes from `controllers/ddj-sb3/capture.toml`, which must not be modified.
- Conventional Commits, prek on every commit, branch `feat/plan-4-deck-performance`, landing via PR.

## Review Focus

1. **Switching decks while a jog wheel is touched or a hotcue is held.** The release must reach the deck that got the press, so no deck is left scratching or previewing. Test: Task 8, `held_jog_touch_and_hotcue_release_on_the_pressed_deck`.
2. **Loading a track while the deck loops, scratches or previews a hotcue.** The new track starts clean: no loop, no scratch, no preview, paused at 0. Tests: Tasks 2, 3 and 4, `load_resets_*`.
3. **SYNC without a usable grid** (no analysis on either deck, nothing else playing, or a tempo that would need more than ±100 %). Nothing changes and the tempo does not jump. Test: Task 7, `sync_without_a_master_or_grid_is_ignored`.
4. **The app quits right after a hotcue is set or deleted.** The change is already in the library. On the next load the hotcue is there, and a deleted one stays deleted. Tests: Task 9, `hotcue_upsert_delete_and_reopen`; Task 10, `memory_sync_saves_set_and_deleted_hotcues`.
5. **Very short loops and halving past the playhead.** A 1/32-beat loop at 180 BPM (about 10 ms, shorter than one audio block) loops cleanly, and halving a loop when the playhead sits beyond the new end brings it back inside, with no runaway playhead. Test: Task 3, `tiny_loop_stays_inside` and `halving_past_the_playhead_wraps_inside`.

---

## File Structure

```
crates/engine/src/{command.rs,deck.rs,shared.rs,renderer.rs,controller.rs,lib.rs}
crates/engine/src/memory.rs           (TrackMemory, Grid, HotcueSlot: new)
crates/engine/src/deck/{hotcue.rs,looping.rs,jog.rs}   (new: deck.rs becomes deck/mod.rs)
crates/engine/tests/{hotcue.rs,looping.rs,jog.rs,sync.rs,no_alloc.rs,transport.rs}
crates/actions/src/{action.rs,apply.rs,state.rs}
crates/actions/src/sync.rs            (new: master choice and tempo match)
crates/actions/tests/apply.rs
crates/controller/src/{mapping/file.rs,runtime.rs}
crates/controller/tests/{runtime.rs,sb3_replay.rs}
crates/library/src/{cues.rs,writer.rs,playlists.rs}
crates/library/tests/cues.rs
crates/app/src/library_mode/{mod.rs,cue_sync.rs → memory_sync.rs}
crates/app/src/play.rs
controllers/ddj-sb3/{mapping.toml,README.md}
docs/controllers/mapping-reference.md, docs/library.md, docs/architecture.md
```

---

### Task 1: Track memory on load, beatgrids in the engine

**Files:** Create `crates/engine/src/memory.rs`. Move `crates/engine/src/deck.rs` to `crates/engine/src/deck/mod.rs`. Modify `command.rs`, `shared.rs`, `controller.rs`, `lib.rs`, and every `DeckCommand::Load` caller: `crates/app/src/play.rs`, `crates/app/src/library_mode/mod.rs`, `crates/audio-io/src/routing.rs`, and the tests in `crates/{actions,audio-io,controller}/tests`. Test: `crates/engine/tests/transport.rs`.

**Interfaces:**
- Produces, in `engine::memory` and re-exported from `engine`:
  - `pub struct HotcueSlot(u8)`: 1-8.
    - `HotcueSlot::new(n: u8) -> Option<HotcueSlot>`
    - `HotcueSlot::index(self) -> usize` (0-7)
    - `HotcueSlot::number(self) -> u8`
    - `HotcueSlot::ALL: [HotcueSlot; 8]`

    Derives `Copy, Eq, Hash, Debug`.
  - `pub const HOTCUES: usize = 8;`
  - `pub struct Grid { pub beat_period_frames: f64, pub first_beat_frames: f64 }` (`Copy, PartialEq, Debug`).
    - `Grid::from_bpm(bpm: f64, first_beat_frames: f64, sample_rate: u32) -> Option<Grid>`: period = `60 * sample_rate / bpm`. `None` unless bpm and first beat are finite, bpm > 0 and the rate > 0.
    - `Grid::beats_at(&self, frames: f64) -> f64` = `(frames - first) / period`.
  - `pub struct TrackMemory { pub cue: f64, pub hotcues: [Option<f64>; HOTCUES], pub grid: Option<Grid> }` (`Copy, PartialEq, Debug, Default`).
    - `TrackMemory::clamped(self, frames: usize) -> TrackMemory`: the cue and every hotcue clamped to `[0, frames]`; a non-finite cue becomes 0 and a non-finite hotcue becomes `None`; the grid unchanged. The engine applies exactly this on `Load`, and the app uses it to predict what the snapshot will show.
- `DeckCommand::Load(Arc<TrackBuffer>)` becomes `DeckCommand::Load { track: Arc<TrackBuffer>, memory: TrackMemory }`. A load resets transport as today, then applies `memory.clamped(track.frames())`.
- New `DeckCommand::SetGrid(Option<Grid>)`. A grid with a non-finite or non-positive period is ignored.
- `DeckSnapshot` gains:
  - `grid: Option<Grid>`
  - `hotcues: [Option<f64>; HOTCUES]`
  - `loop_in: Option<f64>`, `loop_out: Option<f64>`, `loop_active: bool`, `loop_beats: f64`
  - `vinyl: bool`, `jog_touched: bool`

  The atomics store `None` as NaN. Tasks 2-4 fill the loop and jog fields; until then they hold their defaults: `loop_beats` 4.0, `vinyl` true.
- `EngineController::send_all(&mut self, cmds: &[Command]) -> Result<(), CommandQueueFull>`. It sends every command, or none when the queue has fewer free slots than `cmds.len()`. Use `rtrb::Producer::slots`, and collect garbage first as `send` does.

- [ ] **Step 1: Write the failing tests** in `crates/engine/tests/transport.rs`:
  - `load_applies_track_memory`: load with cue 500, hotcue 3 at 900 and a 120 BPM grid at 44.1 kHz. The snapshot then shows cue 500, `hotcues[2] == Some(900.0)`, `grid.beat_period_frames == 22050.0`, paused at 0.
  - `load_clamps_memory_like_clamped`: a 1000-frame track with cue 5000, hotcue 1 at −5 and hotcue 2 at NaN. The snapshot equals `memory.clamped(1000)`: cue 1000, `Some(0.0)`, `None`.
  - `set_grid_replaces_and_clears`: `SetGrid(Some(g))` shows; `SetGrid(None)` clears; a grid with period 0 is ignored.
  - `send_all_is_all_or_nothing`: fill the queue to 254 of 256, then `send_all` with 3 commands is `Err`, and after one render 3 go through.
  - `grid_from_bpm_validates`: 0, negative, NaN BPM and a zero rate all give `None`.
- [ ] **Step 2: Run** `cargo test -p engine`. Expected: FAIL (types missing).
- [ ] **Step 3: Implement** the interfaces above, and update every `Load` caller to pass `TrackMemory::default()` for now (Task 10 passes the library's memory).
- [ ] **Step 4: Run** `mise run test`. Expected: pass, including the existing no_alloc test.
- [ ] **Step 5: Commit:** `feat(engine): load tracks with their cue, hotcues and beatgrid`

---

### Task 2: Hotcues in the engine

**Files:** Create `crates/engine/src/deck/hotcue.rs` and `crates/engine/tests/hotcue.rs`. Modify `command.rs`, `deck/mod.rs` and `tests/no_alloc.rs`.

**Interfaces:**
- New `DeckCommand`s:
  - `HotcuePress(HotcueSlot)`
  - `HotcueRelease(HotcueSlot)`
  - `HotcueDelete(HotcueSlot)`
  - `SetHotcue(HotcueSlot, Option<f64>)`: restores or clears without moving or playing. Clamped like `TrackMemory::clamped`.
- Behavior (CDJ/VirtualDJ-style). All of these do nothing with no track loaded.
  - **Press on an empty slot:** stores the playhead there exactly. Nothing moves.
  - **Press on a set slot while playing:** jumps there and keeps playing.
  - **Press on a set slot while paused:** jumps there and previews while held, like the CUE preview. Release returns to the hotcue and pauses, unless PLAY was pressed during the hold, which latches playing (reuse the `preview`/`play_latched` fields). The cue point is not changed.
  - **Release** of a slot that is not the one previewing does nothing.
  - **Delete** clears the slot. If that slot was previewing, the preview ends as a release would.
  - A jump to a position outside an active loop exits the loop (Task 3 adds loops: write this rule there, with its test).

- [ ] **Step 1: Write the failing tests** in `crates/engine/tests/hotcue.rs`:
  - `press_on_empty_slot_sets_at_playhead`
  - `press_while_playing_jumps_and_keeps_playing`
  - `press_while_paused_previews_until_release`
  - `play_during_hotcue_preview_latches`
  - `delete_clears_and_ends_its_preview`
  - `release_of_another_slot_is_ignored`
  - `set_hotcue_restores_and_clamps`
  - `load_resets_hotcue_preview`
  - In no_alloc: press, release, delete and set on a playing deck.
- [ ] **Step 2: Run** `cargo test -p engine`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p engine`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(engine): add 8 hotcues per deck`

---

### Task 3: Loops in the engine

**Files:** Create `crates/engine/src/deck/looping.rs` and `crates/engine/tests/looping.rs`. Modify `command.rs`, `deck/mod.rs`, `deck/hotcue.rs` and `tests/no_alloc.rs`.

**Interfaces:**
- New `DeckCommand`s: `LoopIn`, `LoopOut`, `AutoLoop`, `LoopHalve`, `LoopDouble`, `ReloopExit`.
- Constants in `looping.rs`: `DEFAULT_LOOP_BEATS: f64 = 4.0`, `MIN_LOOP_BEATS: f64 = 1.0 / 32.0`, `MAX_LOOP_BEATS: f64 = 64.0`.
- Behavior:
  - **`LoopIn`:** sets the loop-in point at the playhead and clears loop-out. The loop goes inactive.
  - **`LoopOut`:** with an in point before the playhead, sets loop-out at the playhead and activates the loop. Otherwise it does nothing.
  - **`AutoLoop` while inactive:** needs a grid. Loop-in is the last beat at or before the playhead (`floor` of `grid.beats_at`); loop-out is in + `loop_beats` × period; the loop activates. Without a grid, nothing happens.
  - **`AutoLoop` while active:** exits the loop and keeps playing.
  - **`LoopHalve`/`LoopDouble`:**
    - They change `loop_beats` (clamped to the min/max), whether or not a loop is active.
    - On an active loop, out becomes in + new length. With a grid the length is `loop_beats` × period; without a grid it is the current length halved or doubled.
    - Track frames must stay ≥ 1 per loop.
  - **`ReloopExit`:** an active loop exits. Otherwise, when both points are set, the playhead jumps to loop-in and the loop activates.
  - **Render:** while active and moving forward, a playhead at or past loop-out wraps to `in + (pos - in) mod (out - in)`, within the frame loop, so loops shorter than a block work. Moving backwards (Task 4) never wraps.
  - A hotcue jump outside `[in, out)` exits an active loop.
  - Load clears the loop and keeps `loop_beats`.

- [ ] **Step 1: Write the failing tests** in `crates/engine/tests/looping.rs`, using a 120 BPM grid (period 22050 at 44.1 kHz):
  - `auto_loop_starts_on_the_last_beat`: playhead at 30000 gives in 22050 and out 110250.
  - `loop_wraps_sample_exactly`: render across out and check the rendered samples continue from in.
  - `auto_loop_without_grid_does_nothing`
  - `auto_loop_again_exits_and_keeps_playing`
  - `manual_loop_in_out_and_reloop`
  - `loop_out_before_in_is_ignored`
  - `halve_and_double_clamp`: 1/32 to 64 beats.
  - `tiny_loop_stays_inside`: 1/32 beat at 180 BPM, render 8192 frames in one block, every position inside `[in, out)`.
  - `halving_past_the_playhead_wraps_inside`
  - `hotcue_outside_loop_exits_it`
  - `load_resets_loop_keeps_size`
  - In no_alloc: auto loop, halve and a wrapping render.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p engine`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(engine): add auto loops, manual loops and reloop`

---

### Task 4: Jog wheels in the engine: nudge, scratch, seek, search

**Files:** Create `crates/engine/src/deck/jog.rs` and `crates/engine/tests/jog.rs`. Modify `command.rs`, `deck/mod.rs` (render) and `tests/no_alloc.rs`.

**Interfaces:**
- New `DeckCommand`s:
  - `SetVinyl(bool)`. Vinyl starts **on** (VirtualDJ's SB3 default) and is kept across loads.
  - `JogTouch(bool)`
  - `Jog { turns: f64, surface: JogSurface }` with `pub enum JogSurface { Top, Ring }` (re-exported)
  - `JogSearch { turns: f64 }`

  Non-finite `turns` are ignored.
- Constants in `jog.rs`:
  - `PLATTER_SECONDS_PER_TURN: f64 = 1.8` (33⅓ rpm)
  - `NUDGE_SECONDS_PER_TURN: f64 = 0.5`
  - `NUDGE_MAX_BEND: f64 = 0.25`
  - `SEARCH_SECONDS_PER_TURN: f64 = 10.0`
  - `SCRATCH_LAG_SECONDS: f64 = 0.01`
  - `SCRATCH_MAX_SPEED: f64 = 8.0`

  Seconds are at the track's own rate and the original tempo.
- Behavior of `Jog`:
  - **Scratch** (surface `Top`, vinyl on, touched): adds `turns × PLATTER_SECONDS_PER_TURN × track rate` frames to a scratch target.
    - Touching (vinyl on) sets the target to the playhead and stops normal motion. The deck renders only the scratch motion, audibly, even when paused.
    - Each output frame, the speed in track frames per output frame is `(target − pos) / (SCRATCH_LAG_SECONDS × output rate)`, clamped to ±`SCRATCH_MAX_SPEED`.
    - Releasing the touch drops the target and resumes normal playback if the deck was playing.
  - **Nudge** (any other `Jog` while playing): adds `turns × NUDGE_SECONDS_PER_TURN × track rate` frames to a pending nudge. Each frame applies up to `NUDGE_MAX_BEND × tempo ratio × rate step` of it on top of normal motion, until it is used up.
  - **Seek** (any other `Jog` while paused): moves the playhead by `turns × PLATTER_SECONDS_PER_TURN × track rate`, silently.
  - **`JogSearch`:** moves the playhead by `turns × SEARCH_SECONDS_PER_TURN × track rate`, playing or paused.
- **Render:** `render_add` handles negative speeds.
  - The playhead is clamped to `[0, end]`.
  - Reaching the end while moving forward pauses, as today, except while scratching: then it only clamps.
  - Interpolation works for both directions.
- Load clears touch, scratch target and pending nudge.

- [ ] **Step 1: Write the failing tests** in `crates/engine/tests/jog.rs`:
  - `scratch_follows_the_platter`: touched, `Jog { turns: 1.0, Top }` moves the playhead to +1.8 s within 1 frame after 0.2 s of render.
  - `scratch_backwards_renders_reversed_samples`: ramp track; the rendered values decrease.
  - `scratch_back_past_zero_clamps`
  - `touch_stops_and_release_resumes_playing`
  - `paused_touch_scratch_is_audible_then_stays_paused`
  - `vinyl_off_top_nudges`
  - `nudge_shifts_by_the_turn_then_returns_to_tempo`: +0.25 turn adds 0.125 s over the render, and afterwards the speed equals the tempo ratio.
  - `paused_ring_seeks_silently`
  - `search_moves_ten_seconds_per_turn`
  - `scratch_at_end_does_not_pause`
  - `scratch_inside_loop_does_not_wrap`
  - `load_resets_jog_state`
  - In no_alloc: touch, scratch both directions, nudge and search on a playing deck.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p engine`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(engine): add jog wheel nudge, scratch, seek and search`

---

### Task 5: Beat-phase sync in the renderer

**Files:** Create `crates/engine/tests/sync.rs`. Modify `command.rs`, `renderer.rs`, `deck/mod.rs` and `tests/no_alloc.rs`.

**Interfaces:**
- New `Command::PhaseSync { deck: DeckId, master: DeckId, beat_ratio: f64 }`. `beat_ratio` is the deck's beats per master beat: 1, 2 or 0.5 (see Task 7).
- Behavior:
  - With both grids present, `deck != master`, both loaded and the master playing, the deck's playhead moves by the smallest amount, at most half of its beat, so that `frac(deck.beats_at(pos)) == frac(master.beats_at(master_pos) × beat_ratio)`.
  - On a paused deck, the alignment is remembered and done in the first block where the deck plays: same rule, against the master's position at that block.
  - A pending alignment is dropped by `Load`, `Seek`, a hotcue jump, `SetGrid`, or when the master stops.
  - A finite `beat_ratio` > 0 is required; anything else does nothing.

- [ ] **Step 1: Write the failing tests** in `crates/engine/tests/sync.rs`:
  - `phase_sync_aligns_beats`: master at beat phase 0.25 and deck at 0.75 give the deck phase 0.25 after the command, moved by −0.5 or +0.5 beat at most.
  - `phase_sync_with_double_ratio`
  - `phase_sync_on_paused_deck_happens_on_play`: the master keeps playing between SYNC and PLAY, and the alignment uses the master's phase at PLAY.
  - `phase_sync_without_grid_or_playing_master_does_nothing`
  - `pending_sync_dropped_by_seek`
  - In no_alloc: phase sync between two playing decks.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p engine`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(engine): align beat phase between decks`

---

### Task 6: Actions for hotcues, loops and jog wheels

**Files:** Modify `crates/actions/src/{action.rs,apply.rs,state.rs}`, `crates/actions/tests/apply.rs` and `docs/controllers/mapping-reference.md` (action table).

**Interfaces:**
- New `ActionKind`s, each with a deck target, listed with its id:

  | Kind | Id | Takes |
  |---|---|---|
  | `Hotcue(HotcueSlot)` | `deck.hotcue_1` … `deck.hotcue_8` | button, press and release |
  | `HotcueDelete(HotcueSlot)` | `deck.hotcue_delete_1` … `deck.hotcue_delete_8` | button |
  | `LoopIn` | `deck.loop_in` | button |
  | `LoopOut` | `deck.loop_out` | button |
  | `AutoLoop` | `deck.auto_loop` | button |
  | `LoopHalve` | `deck.loop_halve` | button |
  | `LoopDouble` | `deck.loop_double` | button |
  | `ReloopExit` | `deck.reloop_exit` | button |
  | `Sync` | `deck.sync` | button (Task 7 implements it; until then `apply` returns `Ignored`) |
  | `VinylToggle` | `deck.vinyl_toggle` | button |
  | `JogTouch` | `deck.jog_touch` | button, press and release |
  | `JogTop` | `deck.jog_top` | relative |
  | `JogRing` | `deck.jog_ring` | relative |
  | `JogSearch` | `deck.jog_search` | relative |

  Matching `Action` variants carry `(DeckId)`, plus the slot for the hotcue kinds.
- `ActionKind::ALL` becomes a `const` array of all 57 kinds, both hotcue families included for slots 1-8. The ids for slots come from static tables (`id()` stays `&'static str`).
- `takes_release` adds `Hotcue` and `JogTouch`; `accepts(Relative)` adds the three jog kinds.
- `ControlState` gains `pub jog_steps_per_turn: u16`, default 720. The controller runtime sets it from the mapping (Task 8).
- `apply` turns these into commands:
  - Hotcue press and release become `HotcuePress`/`HotcueRelease`; delete becomes `HotcueDelete`.
  - The loop kinds map one-to-one.
  - `VinylToggle` sends `SetVinyl(!snapshot.vinyl)`, reading `engine.deck(d)`.
  - Jog press and release become `JogTouch(true/false)`.
  - `JogTop`/`JogRing` steps become `Jog { turns: steps / jog_steps_per_turn, surface }`.
  - `JogSearch` steps become `JogSearch { turns }`.

- [ ] **Step 1: Write the failing tests** in `crates/actions/tests/apply.rs`:
  - `hotcue_actions_send_slot_commands` (slots 1 and 8)
  - `loop_actions_send_commands`
  - `vinyl_toggle_reads_the_snapshot`
  - `jog_steps_become_turns` (720 steps = 1 turn; 360 with a 360 setting)
  - `ids_round_trip_for_every_action` and `docs_list_every_action_id`, both updated to all 57 ids
  - `hotcue_ids_outside_1_to_8_are_unknown` (`deck.hotcue_0`, `deck.hotcue_9`)
- [ ] **Step 2: Run** `cargo test -p actions`. Expected: FAIL.
- [ ] **Step 3: Implement**, and add the new ids to the action table in the mapping reference.
- [ ] **Step 4: Run** `mise run test`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(actions): add hotcue, loop and jog wheel actions`

---

### Task 7: SYNC: master choice and tempo match

**Files:** Create `crates/actions/src/sync.rs`. Modify `apply.rs` and `crates/actions/tests/apply.rs`.

**Interfaces:**
- `pub(crate) fn plan_sync(deck: DeckId, state: &ControlState, snaps: &[DeckSnapshot; 4]) -> Option<SyncPlan>`, with `struct SyncPlan { master: DeckId, slider: f32, range: TempoRange, beat_ratio: f64 }`.
- **Master choice:** among the other decks that are loaded, playing and have a grid, pick the active deck of the other side first, otherwise the lowest deck number. With no candidate, or no grid on `deck`, the result is `None`.
- **Tempo:**
  - Each deck's BPM is `60 × track_sample_rate / beat_period_frames`; the master's effective BPM is its BPM × its `tempo_ratio`.
  - The target is the master's effective BPM × 2^k, with k in {−1, 0, 1} chosen so the target is closest to the deck's own BPM on a log scale (half/double time).
  - `ratio = target / own BPM`; `beat_ratio = 2^k`.
  - The range is the deck's current range if `|ratio − 1|` fits in it, otherwise the smallest of 6/10/16/100 % that fits; a ratio needing more than 100 % gives `None`.
  - `slider = 0.5 + (ratio − 1) / (2 × range.fraction())`.
- `apply(Sync(d), Press)` builds the plan from `engine.deck(id)` for all four decks, then calls `send_all` with `SetTempoRange` (only when the range changes), `SetTempoSlider` and `PhaseSync`.
  - On success, `ControlState` records the slider and range like a moved slider would (`Update::Value`), so soft takeover arms the physical tempo slider ("changed elsewhere").
  - Without a plan the result is `Ignored`; with a full queue it is `QueueFull` and nothing changes.

- [ ] **Step 1: Write the failing tests:**
  - `sync_matches_the_other_sides_playing_deck` (deck 1 at 128 BPM, deck 2 at 125 BPM playing at ratio 1.0 → slider for 125/128 at ±10 %, then `PhaseSync { master: 2, beat_ratio: 1.0 }`)
  - `sync_prefers_the_other_sides_active_deck`
  - `sync_picks_half_time`: 87 BPM syncing to 174 → k = −1, ratio 1.0
  - `sync_widens_the_range_when_needed`: a 14 % difference → 16 %
  - `sync_without_a_master_or_grid_is_ignored`
  - `sync_updates_control_state_tempo`
- Controller test in `crates/controller/tests/runtime.rs`: `tempo_slider_is_armed_after_sync` (a slider move away from the new value does not change the tempo until it is picked up).
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `mise run test`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(actions): sync tempo and beat phase to the playing deck`

---

### Task 8: Mapping support for jog wheels

**Files:** Modify `crates/controller/src/mapping/file.rs`, `crates/controller/src/runtime.rs`, `crates/controller/tests/runtime.rs` and `docs/controllers/mapping-reference.md`.

**Interfaces:**
- `[controller]` gains an optional `jog_steps_per_turn` (integer 1-10000).
  - Loading fails with `jog_steps_per_turn is required by jog bindings` when any binding uses `deck.jog_top`, `deck.jog_ring` or `deck.jog_search` and the key is missing.
  - `Mapping.controller.jog_steps_per_turn: Option<u16>`.
- `Runtime::new` copies it into `ControlState::jog_steps_per_turn`.
- Relative value kinds are accepted for the jog actions (the `accepts` table already says so after Task 6; check that file validation uses it rather than a list of its own).

- [ ] **Step 1: Write the failing tests:**
  - `jog_binding_requires_steps_per_turn`
  - `steps_per_turn_reaches_control_state`
  - `jog_steps_are_summed_per_run` (three ring messages in one batch give one `Jog` of the summed turns)
  - **`held_jog_touch_and_hotcue_release_on_the_pressed_deck`:** on the left side, press jog touch and hotcue 1 on deck 1, press DECK, then release both. The releases go to deck 1 (`JogTouch(false)` and `HotcueRelease(1)` on deck 1); deck 3 gets nothing.
- [ ] **Step 2: Run** `cargo test -p controller`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `mise run test`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(controller): configure jog wheel resolution per controller`

---

### Task 9: Hotcues in the library

**Files:** Modify `crates/library/src/{cues.rs,writer.rs,playlists.rs}` and `crates/library/tests/cues.rs`.

**Interfaces:**
- `WriteOp::SetHotcue { track: i64, slot: u8, frames: f64 }`: an upsert of `frames` that keeps an existing `color` and `name`.
- `WriteOp::DeleteHotcue { track: i64, slot: u8 }`.
- Both fail with `LibraryError::InvalidCue` for a slot outside 1-8 or for non-finite or negative frames, and with `NoSuchTrack` for an unknown track, changing nothing.
- `Reader::hotcues(&self, track: i64) -> Result<[Option<f64>; 8], LibraryError>`, indexed by slot − 1.
- The track list's `has_cues` (the `•` column) is true when the track has a cue point above 0 **or** any hotcue. The spec's "tracks without hotcues" indicator relies on it.

- [ ] **Step 1: Write the failing tests** in `crates/library/tests/cues.rs`:
  - `hotcue_upsert_delete_and_reopen`: set slots 1 and 8, move slot 1, delete slot 8, reopen the library; slot 1 has the new value and slot 8 is `None`.
  - `hotcue_update_keeps_color_and_name` (seed color and name with `WriteOp::Raw`)
  - `hotcue_rejects_bad_slot_and_frames`
  - `has_cues_counts_hotcues`
- [ ] **Step 2: Run** `cargo test -p library`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p library`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(library): store hotcues`

---

### Task 10: App: remember hotcues and keep grids in step

**Files:** Rename `crates/app/src/library_mode/cue_sync.rs` to `memory_sync.rs`. Modify `crates/app/src/library_mode/{mod.rs,worker.rs}` and `crates/app/src/play.rs`.

**Interfaces:**
- `MemorySync` replaces `CueSync`.
  - `MemorySync::new(frames: usize, rate: u32, memory: TrackMemory) -> MemorySync` takes the memory the `Load` carried.
  - `step(&mut self, snap: &DeckSnapshot) -> Vec<MemoryAction>`, with `enum MemoryAction { SaveCue(f64), SaveHotcue(HotcueSlot, f64), DeleteHotcue(HotcueSlot) }`.
  - **The load counts as applied** when the snapshot shows the track (length and rate) with exactly `memory.clamped(frames)`'s cue and hotcues. From then on, each change is reported once against the last seen values.
  - The `SetCue` restore path, `AwaitCue` and `restore_failed` go away: the memory arrives with the `Load`.
- Loading a library track reads `Reader::cue`, `Reader::hotcues` and the beatgrid (`Grid::from_bpm` with the decoded rate), and sends `Load { track, memory }`.
- **Grid sync:** whenever the library reports a new grid for a track that is loaded on a deck, the app sends `SetGrid` to every deck holding it. That covers a finished grid edit and a finished analysis of a track that was loaded before it was analyzed.
- Status lines: a deck's line adds `loop N` while looping (N as a fraction for short loops: `1/4`), and `vinyl off` when vinyl is off.

- [ ] **Step 1: Write the failing tests** (unit tests in `memory_sync.rs`):
  - `load_applies_when_snapshot_shows_the_memory`
  - `memory_sync_saves_set_and_deleted_hotcues`: set slot 2 gives `SaveHotcue(2, x)`; deleting slot 2 gives `DeleteHotcue(2)`; each is reported once.
  - `clamped_memory_is_not_saved_back`
  - The cue tests from `cue_sync.rs`, adapted.
- `grid_commands_target_every_deck_holding_the_track`: a unit test of `fn grid_commands(track: i64, grid: Option<Grid>, loaded: &[Option<i64>; 4]) -> Vec<Command>`, the pure helper that grid sync uses. With the track on decks 1 and 3 it returns `SetGrid` for those two decks only.
- [ ] **Step 2: Run** `cargo test -p open-jockey`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `mise run test`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(app): remember hotcues and keep deck grids in step with the library`

---

### Task 11: SB3 mapping, docs and hands-on check

**Files:** Modify `controllers/ddj-sb3/mapping.toml`, `controllers/ddj-sb3/README.md`, `crates/controller/tests/sb3_replay.rs`, `docs/controllers/mapping-reference.md`, `docs/library.md` (hotcues are remembered like the cue point) and `docs/architecture.md` (track memory, engine grids, sync).

**Bindings:** values from the capture, left side shown; the right side is the same on channel 2 and pads on channel 9. Every deck binding uses `side`. Add `jog_steps_per_turn = 720` to `[controller]`.

| Control | MIDI | Action |
|---|---|---|
| SYNC | ch1 note 0x58 | `deck.sync` |
| VINYL | ch1 note 0x17 | `deck.vinyl_toggle` |
| AUTO LOOP | ch1 note 0x14 | `deck.auto_loop` |
| 1/2X | ch1 note 0x12 | `deck.loop_halve` |
| 2X | ch1 note 0x13 | `deck.loop_double` |
| SHIFT+1/2X | ch1 note 0x61 | `deck.loop_in` |
| SHIFT+2X | ch1 note 0x62 | `deck.loop_out` |
| SHIFT+AUTO LOOP | ch1 note 0x50 | `deck.reloop_exit` |
| jog touch | ch1 note 0x36 | `deck.jog_touch` |
| SHIFT+jog touch | ch1 note 0x67 | `deck.jog_touch` (so a touch that starts or ends with SHIFT held still pairs) |
| jog top | ch1 cc 0x23, `relative_offset` | `deck.jog_top` |
| jog outer ring | ch1 cc 0x21, `relative_offset` | `deck.jog_ring` |
| SHIFT+jog top | ch1 cc 0x1F, `relative_offset` | `deck.jog_search` |
| HOT CUE pads 1-8 | ch8 notes 0x00-0x07 | `deck.hotcue_1` … `_8` |
| SHIFT+HOT CUE pads 1-8 | ch8 notes 0x08-0x0F | `deck.hotcue_delete_1` … `_8` |

The SHIFT loop functions follow the SB3's printed labels per Pioneer's manual (VirtualDJ's default too). The owner confirms them in the hands-on check. Remove the newly bound controls from the "Not mapped yet" block.

- [ ] **Step 1: Write the failing replay tests** in `sb3_replay.rs`:
  - `pads_set_trigger_and_delete_hotcues_on_the_side_deck` (including deck 3 after DECK)
  - `jog_ring_nudges_and_top_scratches_when_touched`
  - `loop_buttons_drive_the_loop`
  - `sync_button_syncs_to_the_other_side`
  - `shift_jog_searches`
- [ ] **Step 2: Run** `cargo test -p controller --test sb3_replay`. Expected: FAIL.
- [ ] **Step 3: Write the bindings and the docs.** Every action id in the docs must exist; `docs_list_every_action_id` checks it.
- [ ] **Step 4: Run** `mise run test && prek run --all-files`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(controllers): map the SB3 jog wheels, hotcues, loops and sync`
- [ ] **Step 6: Hands-on check with the owner** (controller-run), with `play --library --controller ddj-sb3 --device "DDJ-SB3" --master 3,4`:
  1. **Jog wheels:**
     - Playing, turn the outer ring: the track nudges and then returns to tempo.
     - With VINYL on, touch the top and scratch both ways, then release: playback resumes.
     - Paused, turn the jog: the playhead moves.
     - SHIFT + top: fast search.

     Record the feel. If one turn does not match a real platter, adjust `jog_steps_per_turn` (and `NUDGE_SECONDS_PER_TURN` if nudging is too weak or strong) and note it.
  2. **Hotcues:**
     - Set pads 1-4 on a track, trigger them playing and paused (paused previews while held), and delete one with SHIFT + pad.
     - Load another track, then reload: the hotcues are back and the deleted one stays deleted.
     - Repeat on deck 3 after DECK.
  3. **Loops:**
     - AUTO LOOP on a beat; halve down to 1/8 and double back.
     - AUTO LOOP again to exit.
     - SHIFT+1/2X then SHIFT+2X for a manual loop, and SHIFT+AUTO LOOP to reloop and exit.

     Confirm the SHIFT labels match the panel.
  4. **Sync:** two analyzed tracks of different BPM. Press SYNC on one while the other plays:
     - The BPM matches and the beats line up by ear.
     - Move the tempo slider: nothing jumps until it is picked up.
     - SYNC on a paused deck, then PLAY: it starts in phase.
  5. Record results and misses in `controllers/ddj-sb3/README.md`. Fix misses before the PR merges.
