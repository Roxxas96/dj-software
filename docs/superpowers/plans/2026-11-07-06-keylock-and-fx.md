# Plan 6: Keylock and the FX Section

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two features on the DDJ-SB3:
- **Keylock** on KEY LOCK: tempo changes keep the pitch.
- **The FX section:**
  - FX 1/2/3 switch an effect on or off for the deck the side shows.
  - The side's LEVEL knob sets how much effect is heard.
  - The defaults are echo, reverb and flanger, synced to the beat where it applies. The slot effects are set in `config.toml`.

This plan also adds pad ranges to mapping files. They shrink the 2,160-line SB3 mapping before more bindings arrive.

**Architecture:**
- **Mapping:** a `notes = [first, last]` range with `{n}` in the action id expands to one binding per note when the file loads.
- **Keylock:** each deck gets a Signalsmith Stretch instance, created when the engine is built. With keylock on, the deck reads its track at the original pitch, and the stretcher turns that into the tempo-changed output.
- **Effects:** written in-house in `engine::dsp::fx`. Each channel strip runs 3 effect slots after the filter and before the PFL tap and fader. Slot types are shared by all channels and set from the config. The beat length comes from each deck's grid and tempo every block.

**Tech Stack:** Rust 1.99.1, plus one new dependency, `signalsmith-stretch = "0.1.3"` (MIT; it builds the MIT Signalsmith Stretch C++ library with `cc` and `bindgen`, so a C++ compiler and libclang are needed at build time).

**Spec:** `docs/superpowers/specs/2026-11-06-sound-and-pad-modes-design.md` (goals 5 and 6, constraints) and the main spec `docs/superpowers/specs/2026-10-30-dj-software-design.md`.

**Scope change from the design:** the SB3's firmware pad modes (FX FADE, PAD SCRATCH, TRANS) move to **plan 7**. Their routines send the same filter and fader messages as the physical knobs (see `controllers/ddj-sb3/firmware-modes.txt`), so they need their own analysis and a hands-on session. UI, interop and release become plans 8, 9 and 10. Slip mode, listed in the design, shipped in plan 5.

## Global Constraints

- **License Apache-2.0; no GPL.** The only new dependency is `signalsmith-stretch` 0.1.3 (MIT); `cargo deny` must pass.
- Builds and tests on macOS, Windows and Linux. CI has no audio or MIDI devices. If a CI runner lacks libclang or a C++ compiler for the stretcher's build, install it in `.github/workflows/ci.yml`; never drop the dependency to make CI green.
- **The audio thread never allocates, frees, locks or blocks.**
  - Stretchers and effect buffers (echo delay lines sized for 2 s at the output rate, reverb state) are allocated when the engine is built.
  - Every new render path is covered by `crates/engine/tests/no_alloc.rs`.
  - `assert_no_alloc` only sees Rust allocations. The C++ stretcher is trusted on Signalsmith's documented real-time behavior (no allocation in `process` after construction), and this is stated in the engine docs.
- **Anti-goals:**
  - no freeze, crash or dropout;
  - a control never does something other than its mapping says, including after deck switching;
  - keylock does not audibly smear or drop out at normal tempo ranges (±16 %).
- **Keylock defaults to off.** SHIFT + KEY LOCK keeps its plan-2 function (next tempo range).
- **Effects:**
  - The list is echo, reverb, flanger and filter.
  - The defaults are slots 1/2/3 = echo, reverb, flanger.
  - They sit after the EQ and filter and before the channel fader, so the headphone cue hears them.
  - An effect switched on stays with its deck when the side switches layers.
  - Several can be on at once.
  - The side's LEVEL knob sets the dry/wet for the deck the side shows.
- **Config errors** are reported as `<file>: <key>: <problem>` (plan 5 rules).
- **SB3 MIDI values** come from `controllers/ddj-sb3/capture.toml`, which must not be modified.
- Conventional Commits, prek on every commit, branch `feat/plan-6-fx-keylock`, landing via PR.

## Review Focus

1. **Keylock switched on or off while a deck plays, and big jumps with keylock on** (hotcue, CUE, seek, load, beat jump). No click longer than one short fade, no burst of old audio, and the jump is heard at once. Test: Task 2, `keylock_jump_is_heard_at_once`.
2. **Scratching or a held record with keylock on.** Scratch and nudge-free motion play the raw track (keylock bypassed while scratching), as in VirtualDJ, and keylock resumes after the touch is released. Test: Task 2, `scratch_bypasses_keylock`.
3. **An effect turned on, then DECK pressed.** The effect stays on the first deck. The FX buttons and LEVEL now drive the deck the side shows, and LEVEL needs pickup there (soft takeover). Test: Task 5, `fx_stays_with_its_deck_after_deck_switch`.
4. **Echo or flanger on a deck without a beatgrid, or at tempo extremes.** They fall back to 120 BPM, and the echo time is clamped to its 2 s buffer, with no out-of-range reads. Test: Task 3, `beat_fx_without_grid_use_120_bpm` and `echo_time_is_clamped`.
5. **A mapping range with a bad span.** A reversed range, a range of more than 128 notes, `{n}` without a range, or `notes` alongside `note` are load errors naming the binding. Test: Task 1, `bad_note_ranges_are_errors`.

---

## File Structure

```
crates/controller/src/mapping/file.rs                 (note ranges)
crates/controller/tests/{mapping_file.rs,sb3_replay.rs}
controllers/ddj-sb3/mapping.toml                      (rewritten with ranges)
crates/engine/src/deck/keylock.rs                     (new)
crates/engine/src/dsp/fx/{mod.rs,echo.rs,reverb.rs,flanger.rs,filter.rs}  (new)
crates/engine/src/{command.rs,deck/mod.rs,mixer.rs,renderer.rs,shared.rs,lib.rs}
crates/engine/tests/{keylock.rs,fx.rs,no_alloc.rs}
crates/actions/src/{action.rs,apply.rs,state.rs}, crates/actions/tests/apply.rs
crates/app/src/{config.rs,play.rs}
docs/config.md, docs/controllers/mapping-reference.md, docs/architecture.md, controllers/ddj-sb3/README.md
.github/workflows/ci.yml (only if a runner needs libclang)
```

---

### Task 1: Note ranges in mapping files, and a shorter SB3 mapping

**Files:** Modify `crates/controller/src/mapping/file.rs`, `crates/controller/tests/mapping_file.rs`, `controllers/ddj-sb3/mapping.toml` and `docs/controllers/mapping-reference.md`.

**Interfaces:**
- **New `midi` form:** `{ channel = 8, notes = [0x00, 0x07] }`, inclusive first to last. It expands, when the file loads, into one `[[input]]` per note in order, with `n` running from 1 to the count.
- **`{n}` in the action id** is replaced by `n`, as in `action = "deck.loop_roll_{n}"`. Every other key is copied unchanged to each expanded binding.
- **Errors** name the binding, `input[i]`:
  - `notes` given together with `note`/`cc`/`cc14`;
  - a reversed range;
  - more than 128 notes;
  - a value outside 0-127;
  - `{n}` in an action of a binding without `notes`;
  - an expanded action id that is unknown (the message shows the expanded id).
- **Expanded bindings keep file order.** Errors on an expanded binding are reported as `input[i] (note 0x03)`.
- **The SB3 mapping** is rewritten with ranges for every pad block (both banks, both sides, every pad mode). The controller replay tests must pass unchanged; they define the behavior. The file must end up well under 600 lines.

- [ ] **Step 1: Write the failing tests:**
  - `note_range_expands_with_n`
  - `range_keeps_other_keys_and_order`
  - `bad_note_ranges_are_errors` (one case per error above)
  - `sb3_mapping_expands_to_the_same_bindings`: the expanded SB3 mapping has the same `(midi, action, side, pad_mode, shift)` set as before the rewrite. Snapshot the old set into the test from the pre-rewrite file before changing it.
- [ ] **Step 2: Run** `cargo test -p controller`. Expected: FAIL.
- [ ] **Step 3: Implement**, rewrite `controllers/ddj-sb3/mapping.toml` with ranges, and document the form in the mapping reference.
- [ ] **Step 4: Run** `mise run test`. Expected: pass, including all SB3 replay tests unchanged.
- [ ] **Step 5: Commit:** `feat(controller): add note ranges to mapping files`

---

### Task 2: Keylock in the engine

**Files:** Create `crates/engine/src/deck/keylock.rs` and `crates/engine/tests/keylock.rs`. Modify `crates/engine/Cargo.toml` (`signalsmith-stretch`), the workspace `Cargo.toml`, `deny.toml` if a license needs listing, `command.rs`, `deck/mod.rs`, `renderer.rs` (create the stretchers at build time), `shared.rs` and `tests/no_alloc.rs`. Modify `.github/workflows/ci.yml` only if a runner needs libclang.

**Interfaces:**
- `DeckCommand::SetKeylock(bool)` turns keylock on or off. It is off by default and kept across loads. `DeckSnapshot.keylock: bool`.
- **Stretcher setup:**
  - Each deck owns a `signalsmith_stretch::Stretch::preset_cheaper(2, output_rate)`. It is created in `EngineRenderer::new` (not on the audio thread), with preallocated interleaved input and output buffers sized for the largest block × the 2× tempo limit (100 % range) + the stretcher's input latency.
  - `transpose_factor` stays at 1.0; the tempo change is what is stretched.
- **Rendering with keylock on**, a tempo ratio ≠ 1, the deck playing and not scratching:
  - For an output block of `n` frames, the deck reads `round_carry(n × tempo_ratio × (1 + bend))` source frames. It reads them at the original pitch, with step `track_rate / output_rate`, through the same reader the normal path uses (loops, roll and slip wrap as usual).
  - It then calls `process(input, output[n])` and adds the output to the channel buffer.
  - `round_carry` keeps the fractional remainder between blocks, so no drift builds up.
  - The playhead advances by exactly the source frames read.
- **Discontinuities:**
  - After a jump (Seek, CUE, hotcue, beat jump, Load, a slip return, the end of a roll, keylock switched on, or a scratch release), the stretcher is `reset()`.
  - It is then fed `seek()` with `input_latency()` frames starting at the new playhead, so the output starts at the jump without a delay or a tail of old audio.
  - Loop and roll wraps are not discontinuities: the input stream wraps and the stretcher smooths it.
- **Bypass:** keylock is bypassed (normal resampled path) while scratching, while the tempo ratio × (1 + bend) is within 1e-4 of 1.0, and when the deck is paused. Switching between bypass and stretch counts as a discontinuity.
- **Latency is not reported** in the snapshot. The `seek` preroll keeps what is heard aligned with the playhead to within the stretcher's output latency, which is documented in `docs/architecture.md`.

- [ ] **Step 1: Write the failing tests** in `crates/engine/tests/keylock.rs`:
  - `keylock_keeps_pitch_at_plus_8_percent`: a 440 Hz sine with keylock on at ratio 1.08. The output's dominant frequency, by zero crossings over 0.5 s after 0.2 s, is within 2 % of 440 Hz. Without keylock it is within 2 % of 475 Hz.
  - `keylock_keeps_tempo`: the playhead advances 1.08 × the output frames, within 1 frame per second.
  - `keylock_jump_is_heard_at_once`: a track whose two halves are different sines. A hotcue jump into the second half gives the second half's frequency within 30 ms of output.
  - `scratch_bypasses_keylock`
  - `ratio_one_bypasses_keylock`
  - `keylock_off_by_default_and_kept_across_load`
  - no_alloc: keylock on with a playing deck, with a jump, a loop wrap and a toggle.
- [ ] **Step 2: Run** `cargo test -p engine --test keylock`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `mise run test`. Expected: pass on macOS locally. Push the branch early to confirm CI builds the C++ stretcher on all three OSes; fix CI if a runner lacks libclang.
- [ ] **Step 5: Commit:** `feat(engine): add keylock with Signalsmith Stretch`

---

### Task 3: Effects DSP

**Files:** Create `crates/engine/src/dsp/fx/{mod.rs,echo.rs,reverb.rs,flanger.rs,filter.rs}` and `crates/engine/tests/fx.rs`. Modify `crates/engine/src/dsp/mod.rs`.

**Interfaces:**
- `pub enum EffectKind { Echo, Reverb, Flanger, Filter }`, re-exported, with ids `"echo"`, `"reverb"`, `"flanger"` and `"filter"` (`id()` and `from_id()`).
- `pub(crate) struct Effect` holds the state of every kind, so changing the kind never allocates:
  - `new(sample_rate) -> Effect` preallocates everything: a 2 s stereo echo delay, Freeverb-style reverb buffers and a 20 ms flanger delay.
  - `set_kind(kind)` clears the state.
  - `process(ch: usize, x: f32, beat_frames: f64) -> f32` returns the **wet** signal only. The channel mixes it: `out = x + (wet − x) × amount`.
  - `flush_denormals()`.
- **Algorithms** (in-house, no ported GPL code):
  - **Echo:** a delay of ½ beat, feedback 0.5, the delay time clamped to the 2 s buffer.
  - **Reverb:** 8 comb filters and 4 all-passes per channel, using Freeverb's public tunings. Room 0.8, damping 0.5.
  - **Flanger:** a delay swept 1-7 ms by a triangle LFO with a period of 4 beats, feedback 0.3.
  - **Filter:** a resonant low-pass whose cutoff sweeps from 20 kHz down to 200 Hz with the effect amount. It is the one kind where the amount drives the cutoff, not the mix. Reuse `dsp::biquad`.
- **Beat length:** `beat_frames` is the deck's beat period in output frames. When it is 0 or not finite, 120 BPM is used.

- [ ] **Step 1: Write the failing tests** in `crates/engine/tests/fx.rs`, through a small test hook (`#[doc(hidden)] pub fn effect_for_tests`):
  - `echo_repeats_after_half_a_beat` (an impulse at 120 BPM and 48 kHz gives an echo at 12000 frames at 0.5 gain)
  - `echo_time_is_clamped`
  - `beat_fx_without_grid_use_120_bpm`
  - `reverb_tail_decays`
  - `flanger_sweeps_within_1_to_7_ms`
  - `filter_amount_lowers_the_cutoff`
  - `set_kind_clears_state`
  - `effects_stay_finite_on_full_scale_noise` (10 s of ±1 noise, every output finite and |y| < 4)
- [ ] **Step 2: Run** `cargo test -p engine --test fx`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p engine`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(engine): add echo, reverb, flanger and filter effects`

---

### Task 4: Effects in the mixer

**Files:** Modify `crates/engine/src/{command.rs,mixer.rs,renderer.rs,shared.rs}` and `crates/engine/tests/{fx.rs,no_alloc.rs,mixer.rs}`.

**Interfaces:**
- **New `ChannelCommand`s:**
  - `FxEnabled(usize, bool)` for slots 0-2; an out-of-range slot is ignored.
  - `FxAmount(f32)`, 0-1 clamped, non-finite ignored. It is the channel's dry/wet for all of its effects.
- `MasterCommand::FxSlot(usize, EffectKind)` sets the kind of slot 0-2 for every channel and clears its state.
- **Defaults:** slots Echo, Reverb, Flanger; every slot off; amount 0.5.
- **Mixing:** each channel strip owns 3 `Effect`s, built in `Mixer::new`.
  - They run in slot order after the filter and before the PFL tap and fader, each on the previous one's output.
  - Each enabled slot mixes its wet part in at the amount. The Filter kind applies the amount as its cutoff, at full wet.
  - Turning a slot on or off crossfades over one block (`Ramp`), so there is no click.
- **Beat length:** the renderer passes each channel's beat length in output frames each block, as `period × output_rate / track_rate / (tempo_ratio)`, or 0 without a grid. It goes through `Mixer::set_beat_frames(deck, f64)`.
- **Snapshots:** `ChannelSnapshot { fx_enabled: [bool; 3], fx_amount: f32 }` is published with the deck snapshot (seqlock) for the status line.

- [ ] **Step 1: Write the failing tests:**
  - `enabled_echo_is_heard_after_the_filter_and_before_the_fader` (the cue (PFL) bus hears the echo with the fader closed)
  - `fx_off_is_bit_identical_to_plan_5_output`
  - `toggling_fx_does_not_click` (no sample jump above 0.05 at the toggle with a constant input)
  - `fx_slot_kind_applies_to_every_channel`
  - `several_slots_stack`
  - no_alloc: every command plus a render with all 3 slots on, on 4 channels.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `mise run test`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(engine): run three effect slots per channel`

---

### Task 5: Actions, config and SB3 bindings for keylock and FX

**Files:** Modify `crates/actions/src/{action.rs,apply.rs,state.rs}`, `crates/actions/tests/apply.rs`, `crates/app/src/{config.rs,play.rs}`, `docs/config.md`, `controllers/ddj-sb3/{mapping.toml,README.md}`, `crates/controller/tests/sb3_replay.rs`, `docs/controllers/mapping-reference.md` and `docs/architecture.md`.

**Interfaces:**
- **New action kinds:**
  - `deck.keylock_toggle` (deck, button) sends `SetKeylock(!snapshot.keylock)`.
  - `deck.fx_toggle_1`..`_3` (deck, button) flip `DeckControl.fx_on[N-1]` and send `FxEnabled(N-1, on)`.
  - `deck.fx_level` (deck, **absolute**) sends `FxAmount(v)`. It is a soft-takeover value target (`DeckControl.fx_level`, default 0.5).

  The `ActionKind::ALL` count grows by 5.
- **Config:** `[fx] slots = ["echo", "reverb", "flanger"]`, exactly 3 ids from the effect list. `play` sends `FxSlot` for each slot at startup. An unknown id is a config error. `docs/config.md` documents it.
- **SB3 bindings** (capture values; left on channel 5, right on channel 6; all `side`):
  - FX 1/2/3 are notes `0x47`/`0x48`/`0x49` → `deck.fx_toggle_1`..`_3`.
  - LEVEL is cc14 `[0x02, 0x22]` → `deck.fx_level`. The knob also sends `0x04/0x24` and `0x06/0x26` with the same value; leave those unbound, with a comment.
  - KEY LOCK is ch1/ch2 note `0x1A` → `deck.keylock_toggle`.
  - SHIFT + FX buttons (`0x63`-`0x65`) stay unbound; effect parameters are a non-goal.
- **Status line:** a deck shows `keylock` when it is on, and `fx: echo reverb` listing the enabled slots' kinds.

- [ ] **Step 1: Write the failing tests:**
  - actions: `keylock_toggle_reads_the_snapshot`, `fx_toggles_track_state_and_send`, `fx_level_is_a_takeover_value`, and the ids and docs tests updated.
  - config: `fx_slots_parse_and_reject_unknown_ids`.
  - replay: `fx_buttons_toggle_the_side_deck_effects`, `fx_level_drives_the_side_deck_amount`, `fx_stays_with_its_deck_after_deck_switch`, and `key_lock_toggles_keylock_and_shift_key_lock_still_changes_range`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement**, and update the docs.
- [ ] **Step 4: Run** `mise run test && prek run --all-files`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(controllers): map the SB3 FX section and key lock`
- [ ] **Step 6: Hands-on check with the owner** (`play --library --controller ddj-sb3`):
  1. **Keylock:** KEY LOCK on, move the tempo ±8 % and ±16 %. The pitch stays put, with no smearing or dropouts on the owner's music. Hotcues, CUE and the jog still feel instant, and scratching sounds raw.
  2. **FX:** FX 1/2/3 switch echo, reverb and flanger on the side's deck. LEVEL sets how much is heard. The echo follows the beat. An effect stays on its deck after DECK.
  3. **Config:** change `[fx] slots` (for example a filter in slot 3), restart, and hear it.

  Record the results in `controllers/ddj-sb3/README.md`, and fix misses before the PR merges.
