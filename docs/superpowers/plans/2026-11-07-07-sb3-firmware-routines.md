# Plan 7: The SB3's Firmware Pad Routines

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The DDJ-SB3's own pad routines sound as they do in VirtualDJ:
- **FX FADE:** filter sweeps, a 1-beat fade loop, backspins and a shrinking loop.
- **PAD SCRATCH:** scratch patterns from hotcue 1.
- **TRANS:** fader cuts.

Each routine leaves the deck where the owner's physical controls are.

**Architecture:** the SB3 runs these routines itself and sends ordinary control messages. Two owner recordings (`controllers/ddj-sb3/firmware-modes.txt` and `firmware-modes-2.txt`) and VirtualDJ's activity names established the following:
- **Fader and filter:** the SB3 sends the physical fader and filter positions at the start and end of every routine, and moves them in between. These already reach `channel.fader` and `channel.filter`, so they need nothing new, and Task 3 pins them with replay tests.
- **What is new:**
  - **A scratch jog, `JOG_VINYL`** (`B0 22`, relative, offset 64). It scratches while touched, whatever the VINYL button says.
  - **A backspin** performed by the engine, because the SB3 only sends a tiny backward nudge.
  - **The loop parts of FX FADE:** `LOOP_FADE_SEL_1` (`0x4C`) starts a 1-beat loop and `LOOP_FADE_SEL_4` (`0x4A`) starts a 4-beat loop. Pressing pad 3 or 7 a second time ends the loop. The SB3's own 1/2X presses (`0x12`, already bound) shrink pad 7's loop.
  - **`0x4D` (`LOOP_FADE_LOOP`)** stays unbound. The SB3 also sends it when TRANS is selected and during the backspin, so binding it would start loops nobody asked for.
- **Config:** the backspin lengths come from `config.toml`, so other users can tune them.

**Tech Stack:** Rust 1.99.1, existing crates.

**Spec:** `docs/superpowers/specs/2026-11-06-sound-and-pad-modes-design.md` (goal 4; anti-goal: a firmware routine must not leave the deck filtered, faded, looping or stopped once it has ended).

## Global Constraints

- License Apache-2.0; no new dependencies.
- Builds and tests on macOS, Windows and Linux. CI has no audio or MIDI devices.
- **The audio thread never allocates, frees, locks or blocks.** Every new engine command and render path is covered by `crates/engine/tests/no_alloc.rs`.
- **Anti-goals:**
  - no freeze, crash or dropout;
  - a control never does something other than its mapping says, including after deck switching;
  - a firmware routine never leaves the deck filtered, faded, looping or stopped after it has ended.
- **SB3 MIDI values** come from `controllers/ddj-sb3/capture.toml` and the two firmware recordings. None of these files may be modified (the recordings are test fixtures).
- **Config errors** are reported as `<file>: <key>: <problem>`.
- **Defaults:** backspin `short = 1.0` s (pad 4) and `long = 2.0` s (pad 8), each 0.2-10 s.
- Conventional Commits, prek on every commit, branch `feat/plan-7-sb3-routines`, landing via PR.

## Review Focus

1. **A routine on a deck the side no longer shows.** DECK is pressed while an FX FADE loop or backspin runs. Pressing the pad again ends the routine on the deck it started on: the release follows the press (plan 4/5 hold rules), and pad toggles are tied to the deck that holds the routine. Test: Task 3, `fx_fade_loop_ends_on_its_deck_after_deck_switch`.
2. **The second press of pad 3 or 7 when no loop is active** (the DJ already exited it with AUTO LOOP). Nothing happens. Test: Task 2, `loop_exit_without_a_loop_does_nothing`.
3. **A backspin on a paused deck, or near the start of the track.** On a paused deck nothing happens. Near the start, the spin stops at 0 and playback resumes from 0. Test: Task 1, `backspin_clamps_at_the_start` and `backspin_on_a_paused_deck_does_nothing`.
4. **`JOG_VINYL` steps with no touch** (a stray message). They nudge like the outer ring and never scratch. Test: Task 1, `platter_without_touch_nudges`.
5. **The full recorded routines replayed.** After each routine the deck's fader and filter equal the physical values the recording started with (`0x58` and `0x15`), and no loop or backspin is left running. Test: Task 3, `recorded_routines_leave_the_deck_as_the_controls_are`.

---

### Task 1: Engine: forced-scratch jog and backspin

**Files:** Modify `crates/engine/src/{command.rs,deck/jog.rs,deck/mod.rs,shared.rs}`, `crates/engine/tests/{jog.rs,no_alloc.rs}`. Create `crates/engine/src/deck/backspin.rs`.

**Interfaces:**
- **`JogSurface::Platter`**, a new variant. A `Jog { turns, surface: Platter }` scratches while the deck is touched, as `Top` does with vinyl on, but whatever the vinyl flag says. Untouched, it nudges like `Ring`. Touch and release keep using `JogTouch`.
  - Implementation note: let the scratch start on `Platter` input even with vinyl off. A scratch begun this way ends on the touch release, as today.
- **`DeckCommand::Backspin { seconds: f64 }`**, with `seconds` clamped to 0.2-10. On a playing deck:
  - the playhead runs backwards, starting at `−3 ×` the deck's normal speed and slowing linearly to 0 over `seconds`;
  - normal playback then resumes forward from where the spin ended;
  - the spin stops at frame 0, and playback resumes from 0;
  - on a paused deck, or with no track, nothing happens.
- **Interactions:** a backspin is a deliberate jump for plan 5's rule R6, so it drops a slip ghost and ends a roll or slice at its start. Keylock is bypassed while spinning, as for scratching, and restarts after it. A `Seek`, hotcue jump, `Load` or `JogTouch(true)` cancels a running backspin.
- **Snapshot:** `DeckSnapshot.backspinning: bool`.

- [ ] **Step 1: Write the failing tests:**
  - `platter_scratches_with_vinyl_off`
  - `platter_without_touch_nudges`
  - `backspin_reverses_then_resumes`: the playhead goes back by `≈ 1.5 × seconds × normal speed` frames (the area under the linear ramp), then advances again.
  - `backspin_clamps_at_the_start`
  - `backspin_on_a_paused_deck_does_nothing`
  - `backspin_is_cancelled_by_a_touch`
  - no_alloc: a platter scratch, and a backspin with keylock on.
- [ ] **Step 2: Run** `cargo test -p engine`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `mise run test`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(engine): add a forced-scratch jog surface and backspin`

---

### Task 2: Actions and config

**Files:** Modify `crates/actions/src/{action.rs,apply.rs,state.rs}`, `crates/actions/tests/apply.rs`, `crates/app/src/{config.rs,play.rs}`, `docs/config.md` and `docs/controllers/mapping-reference.md`.

**Interfaces:**
- **New action kinds**, each with a deck target:

  | Id | Takes | Sends |
  |---|---|---|
  | `deck.jog_scratch` | relative | `Jog { turns: steps / jog_steps_per_turn, surface: Platter }` (it needs `jog_steps_per_turn`, like the other jog actions) |
  | `deck.backspin_short` | button | `Backspin { seconds: backspin_short }` |
  | `deck.backspin_long` | button | `Backspin { seconds: backspin_long }` |
  | `deck.loop_fade_1` | button | if no normal loop is active: an auto loop of 1 beat |
  | `deck.loop_fade_4` | button | if no normal loop is active: an auto loop of 4 beats |
  | `deck.loop_exit` | button | if a normal loop is active: exit it; otherwise nothing |

  - `loop_fade_N` needs one new `DeckCommand::AutoLoopBeats(f64)`. It starts a loop of exactly that size at the last beat, like `AutoLoop`, and changes `loop_beats`. Add it to Task 2's engine edits, with its no_alloc line.
  - `loop_exit` needs `DeckCommand::LoopExit` (exit only, never reloop).
- **`ControlState`** gains `backspin_short: f64` and `backspin_long: f64`, defaulting to 1.0 and 2.0.
- **`config.toml`:** `[backspin] short = 1.0` and `long = 2.0`, each in 0.2-10 s. They are applied like the beat jump tables. Document them in `docs/config.md`.
- The jog-binding check in `mapping/file.rs` (`jog_steps_per_turn` required) includes `deck.jog_scratch`.

- [ ] **Step 1: Write the failing tests:**
  - `jog_scratch_steps_become_platter_turns`
  - `backspin_uses_the_configured_lengths`
  - `loop_fade_starts_a_loop_only_when_none_is_active`
  - `loop_exit_without_a_loop_does_nothing`
  - `backspin_config_parses_and_rejects_out_of_range`
  - the ids and docs tests, updated
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `mise run test`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(actions): add scratch jog, backspin and fade-loop actions`

---

### Task 3: SB3 bindings, replay of the recorded routines, docs and hands-on

**Files:** Modify `controllers/ddj-sb3/mapping.toml`, `controllers/ddj-sb3/README.md`, `crates/controller/tests/sb3_replay.rs` and `docs/architecture.md`. Create `crates/controller/tests/sb3_firmware_replay.rs`.

**Bindings** (left side shown, right side on channel 2 / pads on channel 9; all with `side`):

| SB3 message | Meaning (VirtualDJ name) | Action |
|---|---|---|
| ch1 cc `0x22`, `relative_offset` | `JOG_VINYL` | `deck.jog_scratch` |
| ch1 note `0x4C` | `LOOP_FADE_SEL_1` | `deck.loop_fade_1` |
| ch1 note `0x4A` | `LOOP_FADE_SEL_4` | `deck.loop_fade_4` |
| ch8 note `0x12` and `0x16` (FX FADE pads 3 and 7), `pad_mode = "hardware"` | `FXFADE_PAD3`/`PAD7` | `deck.loop_exit` |
| ch8 note `0x13` (FX FADE pad 4), `pad_mode = "hardware"` | `FXFADE_PAD4` | `deck.backspin_short` |
| ch8 note `0x17` (FX FADE pad 8), `pad_mode = "hardware"` | `FXFADE_PAD8` | `deck.backspin_long` |

- Pad 3's first press arrives before its `0x4C`, so `loop_exit` finds no loop and does nothing, then `loop_fade_1` starts the loop. The second press ends it. Pad 7 behaves the same with `0x4A`, and its `0x12` 1/2X presses (already bound) shrink the loop.
- `0x4D` (`LOOP_FADE_LOOP`) stays unbound, with a comment explaining why (it is also sent on TRANS select and during the backspin).
- The jog touch `0x36`, the HOT CUE pad 1 note that PAD SCRATCH sends, and the fader and filter controls are already bound.
- Remove the bound controls from the "Not mapped yet" block. The README explains each routine and the leave-the-deck-as-the-controls-are behavior.

**Replay tests** (`sb3_firmware_replay.rs`) feed the recording through the real runtime and the SB3 mapping into an engine with a loaded, analyzed (gridded) playing track:
- Parse `controllers/ddj-sb3/firmware-modes-2.txt`: lines `<ms> ms  <hex> <hex> <hex> ...`; skip headers.
- Deliver each message at its recorded time (render the engine between messages by the elapsed frames).
- Test each routine's segment separately (the times are in the header comments of the parsed segments), plus the whole file:
  - `recorded_routines_leave_the_deck_as_the_controls_are`: after the file, deck 1's fader and filter control values equal `0x58` and `0x15` (14-bit with their LSBs), no loop is active, and nothing is backspinning or touched.
  - `fx_fade_pad_3_loops_and_fades_then_restores`: during pad 3 a 1-beat loop is active and the fader falls to 0; after the second press the loop is off and the fader is back at `0x58`.
  - `fx_fade_pad_4_backspins`
  - `pad_scratch_scratches_from_hotcue_1`: hotcue 1 is set at the first PAD SCRATCH message, the playhead keeps returning near it, and after release the deck plays on.
  - `trans_cuts_and_restores_the_fader`
  - `fx_fade_loop_ends_on_its_deck_after_deck_switch`: a synthetic sequence (pad 3 on, DECK, pad 3 again) ends the loop on deck 1.

- [ ] **Step 1: Write the failing tests** (the replay tests and `sb3_replay.rs` binding rows).
- [ ] **Step 2: Run** `cargo test -p controller`. Expected: FAIL.
- [ ] **Step 3: Write the bindings and docs.**
- [ ] **Step 4: Run** `mise run test && prek run --all-files`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(controllers): support the SB3's FX FADE, PAD SCRATCH and TRANS routines`
- [ ] **Step 6: Hands-on check with the owner.** With the fader below full and the filter off center, run each FX FADE pad (1-8), TRANS pads and PAD SCRATCH pads. Each sounds right, ends where the controls are, and works on deck 3 after DECK. Record the results in the README.
