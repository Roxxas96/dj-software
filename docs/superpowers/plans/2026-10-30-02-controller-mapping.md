# Plan 2: SB3 Capture Tool, Actions, Mapping and Controller Runtime

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Drive the engine from the DDJ-SB3: a capture tool that records what the SB3 really sends, an action registry, a TOML mapping format with conditions, soft takeover and LED feedback, a controller runtime with hot-plug, and a default SB3 mapping, so the owner can mix up to four tracks from the controller through the CLI player.

**Architecture:** A new `actions` crate defines every controllable behavior as a typed `Action`, plus the observable state LEDs read. A new `controller` crate holds MIDI parsing, midir port I/O, the capture tool logic, the mapping file model, the mapping engine (MIDI → actions, with shift/deck-layer conditions and soft takeover), the feedback engine (state → MIDI out, with blink and resync) and the runtime that ties them together on the control thread. Shipped mappings live in `controllers/<name>/` and are embedded in the binary. A user copy in the config dir overrides them. The CLI gains `midi ports|monitor|capture` and `play --controller`.

**Tech Stack:** Rust 1.99.1, midir 0.11.1, toml 1.1.8, serde 1.0.229 (derive), directories 6.0.0, plus the plan 1 crates.

**Spec:** `docs/superpowers/specs/2026-10-30-dj-software-design.md` (sections "Actions", "Mapping", "DDJ-SB3 specifics", "Threads and real-time rules").

**Out of scope here:**
- In-app MIDI learn and the in-app MIDI monitor need the UI, so they come in plan 5. This plan ships the CLI monitor, and the resolver API that learn will build on.
- Pads, jog, loops, sync, key lock, FX and browse/load need engine and library features from plans 3–4. Their captured messages are recorded now so those plans only add mapping lines.

## Global Constraints

- License **Apache-2.0**. No GPL/LGPL/AGPL dependencies. **Never copy code or mapping files from Mixxx or other GPL projects.** SB3 MIDI values come only from our own capture fixture.
- Builds and tests on macOS, Windows and Linux. CI has no MIDI devices, so everything except thin midir wrappers must be testable with fakes.
- The audio thread is untouched by this plan. MIDI handling, mapping and feedback run on the control thread (never inside the cpal callback). The midir input callback only timestamps and forwards messages over a channel. It never touches the engine.
- **A control does exactly what its mapping says, including after deck switching** (spec anti-goal). Every SB3 binding is covered by a replay test.
- MIDI channels are **1-based in mapping files and user-facing output**, and 0-based in code.
- Mapping files are TOML. Unknown keys, unknown action IDs and invalid MIDI numbers are load errors that name the file, the binding index and the problem.
- Conventional Commits. Every commit passes prek. Work happens on branch `feat/plan-2-controller` and lands via PR (`main` is protected).

## Review Focus

1. **The controller is unplugged mid-playback.** Audio keeps playing. On reconnect, the mapping reattaches and every LED is re-sent. Test: Task 8, `reconnect_resends_all_leds`.
2. **Bursts of knob/jog messages** (hundreds per callback). These must not fill the engine queue or panic: absolute values are coalesced per drain batch, and a full queue is counted, not fatal. Test: Task 8, `burst_of_cc_is_coalesced`.
3. **Unmapped, malformed or SysEx messages** are ignored (the monitor shows them as unbound), and nothing panics. Test: Task 1, `parse_rejects_malformed_and_sysex`; Task 6, `unmapped_message_resolves_to_nothing`.
4. **Note-on with velocity 0 used as note-off** (very common) counts as a release. Test: Task 1, `note_on_velocity_zero_is_note_off`.
5. **Deck switch while a button is held** (e.g. CUE held on deck 1, switch to deck 3, release). The release goes to the deck that got the press, so deck 1 isn't left stuck previewing. Test: Task 6, `release_goes_to_deck_that_got_the_press`.

---

## File Structure

```
crates/actions/src/{lib.rs,action.rs,state.rs,apply.rs}
crates/actions/tests/apply.rs
crates/controller/src/lib.rs
crates/controller/src/midi.rs           MidiMessage parse/encode
crates/controller/src/ports.rs          midir wrappers: list/open input+output, presence check
crates/controller/src/capture/{mod.rs,checklist.rs,session.rs,fixture.rs}
crates/controller/src/mapping/{mod.rs,file.rs,resolve.rs,takeover.rs}
crates/controller/src/feedback.rs
crates/controller/src/runtime.rs
crates/controller/src/builtin.rs        include_str! of shipped mappings + user override lookup
crates/controller/tests/{mapping_file.rs,resolve.rs,feedback.rs,runtime.rs,sb3_replay.rs}
controllers/ddj-sb3/{checklist.toml,capture.toml,mapping.toml,README.md}
crates/app/src/{main.rs,midi_cmd.rs,play.rs}   main.rs split: subcommands in their own modules
docs/controllers/{mapping-reference.md,adding-a-controller.md}
```

---

### Task 1: MIDI messages and ports

**Files:** Create `crates/controller/Cargo.toml` and `src/{lib.rs,midi.rs,ports.rs}`. Add `midir` to `[workspace.dependencies]`. Unit tests go in `midi.rs`.

**Interfaces (produces):**
- `pub enum MidiMessage { NoteOn { channel: u8, note: u8, velocity: u8 }, NoteOff { channel: u8, note: u8, velocity: u8 }, ControlChange { channel: u8, controller: u8, value: u8 }, PitchBend { channel: u8, value: u16 } }` (channel 0..=15)
- `MidiMessage::parse(bytes: &[u8]) -> Option<MidiMessage>`: note-on with velocity 0 becomes `NoteOff`. SysEx, realtime, truncated and other messages return `None`.
- `MidiMessage::encode(&self) -> [u8; 3]`, plus `impl Display` showing hex bytes and a decoded form with a **1-based** channel (`"90 0B 7F  NoteOn ch1 note 11 vel 127"`).
- `pub struct TimedMidi { pub micros: u64, pub message: MidiMessage }`
- `ports.rs`:
  - `pub fn input_port_names() -> Result<Vec<String>, PortError>`, `pub fn output_port_names() -> Result<Vec<String>, PortError>`.
  - `pub fn open_input(name_contains: &str, tx: std::sync::mpsc::Sender<TimedMidi>) -> Result<InputPort, PortError>`: case-insensitive substring match. The midir callback parses the bytes and sends them; unparsable messages are dropped.
  - `pub fn open_output(name_contains: &str) -> Result<OutputPort, PortError>`, where `OutputPort::send(&mut self, msg: MidiMessage) -> Result<(), PortError>`.
  - `pub trait MidiOut { fn send(&mut self, msg: MidiMessage); }`, implemented for `OutputPort` (errors counted, not returned) and used by feedback so tests can record output.
  - `PortError` messages list the available ports when no match is found.

- [ ] **Step 1: Write failing tests in `midi.rs`:** `parses_note_on_off_cc_pitchbend` (exact field values, including 14-bit pitch bend `E0 00 40` → 8192), `note_on_velocity_zero_is_note_off`, `parse_rejects_malformed_and_sysex` (empty, `[0x90, 0x10]`, `[0xF0, 1, 2, 0xF7]`, `[0xF8]`, data byte ≥ 0x80 → `None`), `encode_round_trips`, `display_uses_one_based_channel`.
- [ ] **Step 2: Run** `cargo nextest run -p controller`. Expected: FAIL (types missing).
- [ ] **Step 3: Implement** `midi.rs` and the `ports.rs` wrappers (midir 0.11; check its docs via context7). `ports.rs` has no automated tests.
- [ ] **Step 4: Run.** Expected: 5 passed. Then `cargo clippy -p controller --all-targets -- -D warnings`: clean.
- [ ] **Step 5: Commit:** `feat(controller): parse MIDI messages and open midir ports`

---

### Task 2: CLI `midi ports` and raw `midi monitor`; split `main.rs`

**Files:** Create `crates/app/src/{midi_cmd.rs,play.rs}`. Modify `crates/app/src/main.rs` (keep only CLI definition + dispatch), `crates/app/Cargo.toml`.

**Interfaces:**
- `dj-software midi ports` prints `in:  <name>` / `out: <name>` lines.
- `dj-software midi monitor [--port <NAME>]` prints one line per message using `MidiMessage`'s `Display`, with a millisecond timestamp, until Ctrl-C. Without `--port`, it picks the first input port whose name contains "DDJ", or errors with the port list. Task 9 adds `--controller` (shows resolved actions).
- `play` moves to `play.rs` unchanged. The existing app tests keep passing.

- [ ] **Step 1: Write failing tests** (clap `try_parse_from`): `parses_midi_monitor_port`, `parses_midi_ports`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo nextest run -p dj-software`. Expected: all pass. Then `cargo run -p dj-software -- midi ports` lists ports (CoreAudio IAC or none, no error).
- [ ] **Step 5: Commit:** `feat(app): add midi ports and monitor commands`

---

### Task 3: Capture tool

**Files:** Create `crates/controller/src/capture/{mod.rs,checklist.rs,session.rs,fixture.rs}`, `controllers/ddj-sb3/checklist.toml`. Modify `crates/app/src/midi_cmd.rs` (adds `midi capture`). Add `toml`, `serde` to workspace deps.

**Interfaces:**
- **Checklist file** (`checklist.rs`, serde, `deny_unknown_fields`):
  ```toml
  [[section]]
  name = "Left deck"
  decks = [1, 3]                 # optional: section repeats once per deck, with {deck} substituted
  setup = "Switch the LEFT side to deck {deck} with its DECK button."   # optional, shown before the section
  [[section.step]]
  id = "deck{deck}.play"
  prompt = "Press PLAY/PAUSE"
  kind = "button"                # button | pads | continuous | jog
  shift = true                   # optional: also capture the step with SHIFT held (id suffix ".shift")
  ```
  `pub fn load_checklist(text: &str) -> Result<Vec<Step>, ChecklistError>` returns the expanded, ordered steps (`Step { id, prompt, setup: Option<String>, kind, shift: bool }`, where shift variants are separate steps). Duplicate expanded ids are an error.
- **Session** (`session.rs`, pure state machine, no I/O):
  - `pub struct CaptureSession`, with `pub fn new(steps: Vec<Step>)`, `pub fn current(&self) -> Option<&Step>`, `pub fn feed(&mut self, msg: TimedMidi)`, `pub fn poll(&mut self, now_micros: u64) -> StepStatus { Waiting, Recording, Done }`, `pub fn skip(&mut self)`, `pub fn redo(&mut self)`, `pub fn finish(self) -> Fixture`.
  - Step completion: the step starts recording on its first message and is done after a quiet gap of **400 ms** for `button`, **1500 ms** for `continuous`/`jog`/`pads`. A `pads` step is done only after 8 distinct note-on notes plus the quiet gap.
  - The fixture records, per control id: `kind`, and the distinct `(status, data1)` pairs seen with their min/max data2 values. A CC pair `n` / `n+32` seen together is recorded as `cc14 = { msb = n, lsb = n+32 }`.
- **Fixture** (`fixture.rs`, serde TOML, the committed format):
  ```toml
  [meta]
  controller = "Pioneer DDJ-SB3"
  port = "DDJ-SB3"
  tool_version = "0.0.0"
  [[control]]
  id = "deck1.play"
  kind = "button"
  messages = [{ status = 0x90, data1 = 0x0B, min = 0, max = 127 }]
  led = true            # filled by the LED probe; absent if not probed
  led_values_vary = false
  ```
  `pub fn parse_fixture(text) -> Result<Fixture, …>` and `Fixture::to_toml(&self) -> String`.
- **CLI** `dj-software midi capture --checklist <PATH> --out <PATH> [--port <NAME>]`:
  - Shows each step's setup and prompt. Enter starts listening, `s` skips, `r` redoes the previous step, `q` saves and quits.
  - Afterwards, **LED probe**: for each captured button or pad note, it sends note-on velocity 127 and asks "Did a light turn on? [y/n]", then sends velocity 0. For `y` on pads, it sends velocities 1, 16, 32, 64, 127 in turn and asks "Did the color change? [y/n]".
  - It writes the fixture after every step, so a crash loses at most one step.
- **`controllers/ddj-sb3/checklist.toml`** covers every control in the [Mixxx DDJ-SB3 manual](https://manual.mixxx.org/2.6/en/hardware/controllers/pioneer_ddj_sb3)'s control list (that list is facts about the hardware, so referencing it is fine; never copy Mixxx's mapping):
  - **Per deck**, as sections for decks [1,3] on the left and [2,4] on the right:
    - PLAY/PAUSE, CUE, SYNC, KEY LOCK, VINYL, AUTO LOOP, 1/2X and 2X (all `shift = true`).
    - SHIFT and the DECK button.
    - Jog top: touch, then spin both ways. Jog outer: spin both ways (`shift` on the top-spin step).
    - TEMPO slider, moved fully both ways.
    - Pad mode buttons HOT CUE, FX FADE, PAD SCRATCH, SAMPLER (`shift = true`).
    - Pads 1→8 in each of the 4 modes, plus pads with SHIFT in HOT CUE mode.
  - **Mixer** for channels 1–4 (section `decks = [1,2,3,4]`, with setup asking to switch the matching side): TRIM, HI, MID, LOW, FILTER, channel fader (`shift = true`), headphone CUE button.
  - **Global:** crossfader, MASTER knob, HEADPHONES knob (they may send nothing; "skip if nothing happens"), headphone MASTER button, FX buttons 1–3 (`shift = true`), FX LEVEL knob, BROWSE rotate (`shift`), BROWSE press (`shift`), LOAD left/right (`shift`).

- [ ] **Step 1: Write failing tests** (unit tests in each module):
  - `checklist_expands_decks_and_shift`: a 1-section 2-step checklist with `decks = [1,3]` and one `shift` step gives exactly 6 steps with ids `deck1.play`, `deck1.play.shift`, `deck1.cue`, `deck3.play`, … in order.
  - `checklist_rejects_duplicate_ids_and_unknown_keys`.
  - `button_step_done_after_quiet_gap`: messages at 0 ms and 80 ms; `poll(300ms)` gives `Recording`, `poll(481ms)` gives `Done`.
  - `continuous_step_records_min_max`.
  - `cc_pair_recorded_as_cc14`.
  - `pads_step_needs_eight_distinct_notes`.
  - `skip_and_redo`.
  - `fixture_round_trips_toml`.
  - `real_sb3_checklist_loads`: loads `controllers/ddj-sb3/checklist.toml` via `include_str!`, no error, more than 80 steps.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement** the session, checklist, fixture and CLI command (stdin line-reading on the main thread; MIDI over the mpsc channel; `poll` every 20 ms).
- [ ] **Step 4: Run** `cargo nextest run -p controller -p dj-software`. Expected: all pass.
- [ ] **Step 5: Commit:** `feat(controller): add guided MIDI capture tool and SB3 checklist`

---

### Task 4: Owner captures the SB3 (gate, controller-run, not an implementer task)

The controller (the session running this plan) asks the owner to run:
```
cargo run --release -p dj-software -- midi capture --checklist controllers/ddj-sb3/checklist.toml --out controllers/ddj-sb3/capture.toml
```
and then reviews the result together with them: every expected control is present, no step was skipped by accident, and the LED answers are recorded. Commit `controllers/ddj-sb3/capture.toml` (`test(controller): add DDJ-SB3 MIDI capture fixture`). Tasks 5–8 don't depend on the fixture, so they can run while the owner captures. Tasks 9–10 need it.

---

### Task 5: Actions and observable state

**Files:** Create `crates/actions/{Cargo.toml,src/lib.rs,src/action.rs,src/state.rs,src/apply.rs}`, `crates/actions/tests/apply.rs`.

**Interfaces (produces):**
- `pub enum Side { Left, Right }`. `Side::of(deck: DeckId) -> Side` (decks 1 and 3 are left, 2 and 4 are right).
- `pub enum Target { Deck(DeckId), Side(Side), Global }`, plus `pub type ValueTarget = (ActionKind, Option<DeckId>)` (the key for soft takeover and coalescing).
- `pub enum Input { Press, Release, Absolute(f32), Relative(i32) }`
- `pub enum Action` with stable string ids (`Action::id(&self) -> &'static str`, `Action::from_id(&str) -> Option<ActionKind>`). A kind needs a target to become an `Action`:

| id | target | accepts | engine effect |
|---|---|---|---|
| `deck.play_pause` | deck | Press | `TogglePlay` |
| `deck.cue` | deck | Press / Release | `CuePress` / `CueRelease` |
| `deck.tempo` | deck | Absolute | `SetTempoSlider` |
| `deck.tempo_range_next` | deck | Press | cycle Six→Ten→Sixteen→Hundred→Six via `SetTempoRange` |
| `deck.return_to_start` | deck | Press | `Seek(0.0)` |
| `channel.trim`, `channel.eq_high`, `channel.eq_mid`, `channel.eq_low`, `channel.filter`, `channel.fader` | deck | Absolute | the matching `ChannelCommand` |
| `channel.pfl_toggle` | deck | Press | `Pfl(!current)` |
| `master.crossfader` | — | Absolute | `Crossfader` |
| `master.level`, `master.headphone_level`, `master.headphone_mix` | — | Absolute | matching `MasterCommand` |
| `master.headphone_master_toggle` | — | Press | `HeadphoneMix(0.0 ↔ 1.0)` |
| `side.deck_toggle` | side | Press | switches the side's active deck (handled by the mapping engine, not the engine) |
| `modifier.shift` | side | Press / Release | handled by the mapping engine |

  An `Input` the action doesn't accept is ignored, and so is any `Release` for press-only actions.
- `pub struct ControlState`: the app-side model, updated from what was sent plus engine snapshots.
  - Per deck it stores: `tempo_range`, `pfl`, and last-sent absolute values for trim/eq/filter/fader/tempo. It also stores the crossfader, headphone-master flag, and per side the active deck and shift.
  - `pub fn value(&self, target: ValueTarget) -> Option<f32>` is used by soft takeover.
- `pub fn apply(action: &Action, input: Input, state: &mut ControlState, engine: &mut EngineController) -> ApplyResult { Sent(u8), Ignored, QueueFull }`. It updates `ControlState` only when the command was accepted.
- **Observable state** (`state.rs`): `pub enum StateKey { DeckPlaying, DeckPlayLed, DeckCueLed, ChannelPfl, SideAltDeck, HeadphoneMaster, Shift }` and `pub enum Led { Off, On, Blink }`. `pub fn led(key: StateKey, target: Option<Target>, state: &ControlState, snap: Option<&DeckSnapshot>) -> Led`. VirtualDJ-like rules:
  - Play LED: `On` while playing, `Blink` when loaded and paused, `Off` when unloaded.
  - Cue LED: `On` when previewing or paused at cue (|pos − cue| < 1 frame), `Blink` when paused elsewhere, `Off` while playing (not previewing) or unloaded.
  - PFL: `On` when enabled.
  - `SideAltDeck`: `On` when the side shows deck 3/4.
  - Shift: `On` while held.

- [ ] **Step 1: Write failing tests in `tests/apply.rs`** (a real engine, rendering a block after each apply so commands take effect):
  - `ids_round_trip_for_every_action`.
  - `play_pause_toggles_engine_playing`.
  - `cue_press_release_previews`.
  - `tempo_absolute_sets_ratio` (Absolute(1.0) at range Ten gives ratio 1.10).
  - `tempo_range_cycles` (four presses: Sixteen, Hundred, Six, Ten).
  - `pfl_toggle_flips_state`.
  - `headphone_master_toggle_flips_mix`.
  - `unaccepted_input_is_ignored`.
  - `led_rules` (one assertion per bullet above, using constructed snapshots).
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo nextest run -p actions`. Expected: all pass.
- [ ] **Step 5: Commit:** `feat(actions): add action registry, control state and LED rules`

---

### Task 6: Mapping file and resolution (conditions, value kinds, soft takeover)

**Files:** Create `crates/controller/src/mapping/{mod.rs,file.rs,resolve.rs,takeover.rs}`, and `crates/controller/tests/{mapping_file.rs,resolve.rs}`.

**Interfaces:**
- **File format** (`file.rs`, serde, `deny_unknown_fields`):
  ```toml
  [controller]
  name = "Pioneer DDJ-SB3"
  port = "DDJ-SB3"                      # substring for both input and output ports

  [[init]]                              # actions applied when the controller attaches
  action = "master.level"
  value = 1.0

  [[input]]
  midi = { note = 11, channel = 1 }     # or { cc = 19, channel = 1 } | { cc14 = [0, 32], channel = 1 } | { pitchbend = true, channel = 1 }
  action = "deck.play_pause"
  deck = 1                              # or side = "left"  (resolves to the side's active deck), or omitted for global actions
  shift = false                         # optional condition: binding's side SHIFT state (global bindings: any side)
  active_deck = 1                       # optional condition: the binding's side currently shows this deck
  value = "button"                      # button | absolute | relative_twos | relative_offset; default from midi kind (note→button, cc/cc14/pitchbend→absolute)
  invert = false
  soft_takeover = true                  # default: true for absolute, ignored otherwise

  [[output]]
  state = "deck.play_led"               # snake_case of StateKey
  deck = 1                              # or side = "left"
  midi = { note = 11, channel = 1 }     # or { cc = 5, channel = 1 }
  on = 127
  off = 0
  ```
  `pub fn parse_mapping(name: &str, text: &str) -> Result<Mapping, MappingError>`. Errors name the file, `input[N]`/`output[N]`, and the problem: unknown action id, channel outside 1–16, number above 127, `deck` and `side` both set, target missing for a deck action, cc14 msb == lsb.
- **Resolver** (`resolve.rs`): `pub struct Resolver`, built from `&Mapping`, holding the per-side active deck (initially 1 and 2), shift state, held-press table and takeover state.
  - `pub fn resolve(&mut self, msg: &MidiMessage, state: &ControlState) -> Resolved`, where `Resolved` is `SmallVec<[(Action, Input); 2]>` or a `Vec` (control thread, so allocation is fine).
  - Rules:
    - Bindings are matched in file order. **Every** matching binding whose conditions hold fires, so one message can drive several actions.
    - `modifier.shift` and `side.deck_toggle` update the resolver state and are also returned, so feedback and the monitor see them.
    - A **button press is remembered with the deck it resolved to**. The matching release goes to that same deck, even if the side switched in between.
    - 14-bit CC: the MSB is stored, and the value is emitted when the LSB arrives (`(msb << 7 | lsb) / 16383`). An MSB with no LSB within the same batch is emitted alone as `msb / 127`.
    - Relative values: two's complement (1..63 means +, 65..127 means −) or offset 64.
    - `invert` maps `v` to `1 - v` for absolute values.
    - `Resolved` is empty for unmapped messages.
- **Soft takeover** (`takeover.rs`): `pub struct Takeover`. A target is keyed by `(ActionKind, DeckId or global)`.
  - **Armed** when: the target's deck becomes active on its side, the controller (re)attaches, or another source changed the value (`ControlState` value differs from the last value this resolver sent).
  - While armed, a physical value is dropped unless `|phys − current| ≤ 0.02` **or** the previous physical value for this target and the new one lie on opposite sides of `current` (crossing). Then it disarms and passes the value through.
  - When the previous physical value is unknown, only the tolerance check applies.
  - `pub fn waiting(&self) -> impl Iterator<Item = (target, current, phys)>` is for the UI later.

- [ ] **Step 1: Write failing tests:**
  - `mapping_file.rs`: `parses_full_example` (the example above round-trips into the expected structs), plus one test per error case listed, each asserting the message contains `input[0]` and the specific problem.
  - `resolve.rs`:
    - `note_binding_press_and_release`.
    - `side_binding_follows_active_deck`.
    - `shift_condition_selects_binding`.
    - `active_deck_condition`.
    - `one_message_can_fire_two_bindings`.
    - `unmapped_message_resolves_to_nothing`.
    - `release_goes_to_deck_that_got_the_press`.
    - `cc14_combines_msb_lsb` (0x7F/0x7F → 1.0; 0x40/0x00 → 8192/16383).
    - `relative_twos_and_offset`.
    - `invert_absolute`.
    - Takeover:
      - `takeover_blocks_until_within_tolerance`.
      - `takeover_picks_up_on_crossing`.
      - `takeover_arms_on_deck_switch` (fader at 1.0 on deck 1; switch to deck 3, whose fader is 0.3; moves to 0.9 and 0.6 are dropped, 0.31 passes, then 0.5 passes).
      - `takeover_arms_when_value_changed_elsewhere`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo nextest run -p controller`. Expected: all pass.
- [ ] **Step 5: Commit:** `feat(controller): add mapping file format, resolver and soft takeover`

---

### Task 7: Feedback (LEDs)

**Files:** Create `crates/controller/src/feedback.rs`, `crates/controller/tests/feedback.rs`.

**Interfaces:**
- `pub struct Feedback`, built from `&Mapping`. `pub fn update(&mut self, now_micros: u64, state: &ControlState, snaps: &[DeckSnapshot; 4], out: &mut dyn MidiOut)` evaluates every `[[output]]` via `actions::led`.
  - `Blink` alternates on/off with a **500 ms period** (on during the first 250 ms), from a shared clock so all blinking LEDs are in phase.
  - Only changed values are sent (per-output cache).
  - Outputs with `side = …` resolve through the side's active deck.
  - `pub fn resync(&mut self)` clears the cache, so the next `update` sends every output.
- [ ] **Step 1: Write failing tests** (a recording `MidiOut`):
  - `sends_only_changes`.
  - `blink_alternates_in_phase`.
  - `resync_resends_everything`.
  - `side_output_follows_active_deck` (after switching left to deck 3, the left play LED reflects deck 3's state).
  - `play_and_cue_leds_follow_engine` (a real engine: load, play, pause away from cue).
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: all pass.
- [ ] **Step 5: Commit:** `feat(controller): add LED feedback with blink and resync`

---

### Task 8: Controller runtime, hot-plug and built-in mappings

**Files:** Create `crates/controller/src/{runtime.rs,builtin.rs}`, `crates/controller/tests/runtime.rs`. Add `directories` to workspace deps.

**Interfaces:**
- `pub struct Runtime`, built with `Runtime::new(mapping: Mapping)`, owns a `Resolver`, `ControlState` and `Feedback`.
  - `pub fn attach(&mut self, engine: &mut EngineController, out: &mut dyn MidiOut)` applies the `[[init]]` actions, arms takeover for all targets, and calls `feedback.resync()`.
  - `pub fn handle_batch(&mut self, msgs: &[TimedMidi], engine: &mut EngineController) -> BatchStats { applied: u32, coalesced: u32, queue_full: u32 }` resolves in order. **Absolute** inputs for the same target within the batch are coalesced to the last value; presses and releases are never coalesced and keep their order.
  - `pub fn tick(&mut self, now_micros: u64, engine: &EngineController, out: &mut dyn MidiOut)` runs feedback.
  - `pub fn on_disconnect(&mut self)` and `on_reconnect(&mut self, engine, out)`: reconnect calls `attach` but does **not** re-run `[[init]]`; it only resyncs LEDs and arms takeover.
  - `pub fn last_resolved(&self) -> &[(MidiMessage, Vec<(Action, Input)>)]` holds the last batch, for the monitor.
- `pub struct Connection` (used by the app, not tested in CI) opens input and output by `mapping.controller.port`. `poll_presence(&mut self) -> PresenceChange { None, Lost, Back }` is checked **once per second** via `ports::input_port_names()`. On `Back` it reopens both ports.
- `builtin.rs`: `pub fn builtin_mapping(name: &str) -> Option<&'static str>`, using `include_str!("../../../controllers/ddj-sb3/mapping.toml")` (a placeholder `[controller]`-only file until Task 9). `pub fn load_mapping(name_or_path: &str) -> Result<Mapping, MappingError>`: a path that exists is loaded as a file. Otherwise the user override at `directories::ProjectDirs::from("", "", "dj-software").config_dir()/controllers/<name>/mapping.toml` wins over the built-in. Unknown names error with the list of built-ins.
- [ ] **Step 1: Write failing tests:**
  - `attach_applies_init_and_resyncs`.
  - `burst_of_cc_is_coalesced`: 1000 CC messages for one fader in one batch give `applied == 1` and `coalesced == 999`, and the engine fader equals the last value.
  - `buttons_keep_order_in_batch`: press, release, press gives 3 applied, in order.
  - `queue_full_is_counted_not_fatal`: fill the queue without rendering; `queue_full > 0` and no panic.
  - `reconnect_resends_all_leds`.
  - `reconnect_does_not_rerun_init`.
  - `load_mapping_prefers_user_override`: a temp dir passed through an internal `load_mapping_from(name, config_dir)` variant.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo nextest run --workspace`. Expected: all pass.
- [ ] **Step 5: Commit:** `feat(controller): add controller runtime with hot-plug and built-in mappings`

---

### Task 9: Default DDJ-SB3 mapping and replay tests (needs Task 4's fixture)

**Files:** Write `controllers/ddj-sb3/mapping.toml`, `controllers/ddj-sb3/README.md`, `crates/controller/tests/sb3_replay.rs`. Modify `crates/app/src/{play.rs,midi_cmd.rs}`.

**Mapping rules.** Values come **only** from `controllers/ddj-sb3/capture.toml`. Behavior follows VirtualDJ's default SB3 mapping, limited to the actions that exist:

| SB3 control | action |
|---|---|
| PLAY/PAUSE | `deck.play_pause` |
| CUE | `deck.cue` |
| SHIFT + CUE | `deck.return_to_start` |
| TEMPO slider | `deck.tempo` (check the direction in the capture: slider toward the player must be faster, so set `invert` accordingly) |
| SHIFT + KEY LOCK | `deck.tempo_range_next` |
| SHIFT | `modifier.shift` (side) |
| DECK button | `side.deck_toggle` |
| TRIM / HI / MID / LOW / FILTER / channel fader | `channel.*` |
| headphone CUE | `channel.pfl_toggle` |
| crossfader | `master.crossfader` |
| headphone MASTER button | `master.headphone_master_toggle` |

`[[init]]`: `master.level = 1.0` and `master.headphone_level = 1.0`, because the SB3 applies its MASTER and HEADPHONES knobs in its own audio interface. If the capture shows those knobs send MIDI, leave them unbound with a comment.

- **Outputs:** play LED, cue LED, PFL LED, DECK button LED (`side_alt_deck`), and SHIFT if the probe says it lights.
- **Every other captured control is listed in a comment block** `# Not mapped yet (needs plan 4): …`, with its MIDI message, so later plans only add lines.
- **Layering decision:** the fixture shows whether the SB3 changes MIDI channel for decks 3/4.
  - If deck-3 captures use a different channel: **hardware layering**. Deck 3 bindings use `deck = 3` on their own channel; outputs go to the deck-3 channel; `side.deck_toggle` still tracks the side for its LED and for takeover arming.
  - If the captures are identical: **software layering**. Bindings use `side = "left"`.
  - Write the decision and the evidence at the top of the mapping file.

**Replay tests** (`sb3_replay.rs`) load the fixture and the built-in mapping, run messages through `Runtime` with a real engine, and assert results:
- `every_mapped_control_has_capture_and_effect`: for each mapped control id in a table inside the test, replaying its captured messages produces the table's expected action and target. A control in the table with no capture fails the test.
- `deck_switch_routes_controls_to_deck_3_and_back`.
- `deck_switch_resyncs_side_leds_to_new_deck` (the recorded `MidiOut` gets deck-3 LED values on the correct channel after the switch).
- `soft_takeover_after_deck_switch_on_fader`.
- `shift_cue_returns_to_start_but_cue_alone_previews`.
- `tempo_slider_direction_matches_vdj` (slider at the "+" end gives tempo_ratio > 1).

**App:**
- `play` gains `--controller <NAME|PATH>` and `--deck3 <TRACK>`, `--deck4 <TRACK>` (loaded paused). With `--controller`, the main loop becomes the control loop:
  - `recv_timeout(10 ms)` on the MIDI channel, drain the batch, `runtime.handle_batch`.
  - `runtime.tick` every 33 ms, and `Connection::poll_presence` every 1 s. It prints `controller disconnected` / `controller reconnected` and keeps playing.
  - The exit rule changes with a controller: it exits only on Ctrl-C (the DJ may restart a finished track).
- `midi monitor --controller <NAME|PATH>` prints each message followed by `→ deck.play_pause deck 1 Press` or `→ (unbound)`.

- [ ] **Step 1: Write the replay tests** against the fixture and a mapping file with only `[controller]` (they fail).
- [ ] **Step 2: Run** `cargo nextest run -p controller --test sb3_replay`. Expected: FAIL.
- [ ] **Step 3: Write `mapping.toml`** from the fixture, per the rules, and the README: supported controls, unmapped controls, and the layering decision.
- [ ] **Step 4: Run.** Expected: all pass.
- [ ] **Step 5: Implement the app changes**, with clap tests `parses_play_with_controller_and_four_decks` and `parses_monitor_with_controller`.
- [ ] **Step 6: Run** `mise run test && prek run --all-files`. Expected: all pass.
- [ ] **Step 7: Commit:** `feat(controller): add default DDJ-SB3 mapping with replay tests`

---

### Task 10: Docs and hands-on check

**Files:** Create `docs/controllers/mapping-reference.md` (every key of the file format, every action id with what it accepts, every state key and LED rule, conditions, value kinds, soft takeover, user override location per OS) and `docs/controllers/adding-a-controller.md` (write a checklist → run `midi capture` → write the mapping → add replay tests → open a PR using the controller issue form). Modify `README.md` (controllers table: DDJ-SB3 "transport, mixer, deck switching, LEDs"; quick start with `--controller ddj-sb3`), `docs/architecture.md` (crates and the control thread), and `CONTRIBUTING.md` (link the guide).

- [ ] **Step 1: Write the docs.** Every action id and file key in `mapping-reference.md` must exist in code. Add a test `docs_list_every_action_id` in `crates/actions/tests/apply.rs` that reads `docs/controllers/mapping-reference.md` and asserts each `Action` id appears in it.
- [ ] **Step 2: Run** `mise run test && prek run --all-files`. Expected: pass.
- [ ] **Step 3: Commit:** `docs: add mapping reference and adding-a-controller guide`
- [ ] **Step 4: Hands-on check with the owner** (controller-run). Owner setup is the laptop speakers for master; nothing is plugged into the SB3 master. So use `--device "MacBook" --master 1,2` and no headphone routing, or SB3 headphones with `--headphones 3,4` when on the SB3 device:
  ```
  cargo run --release -p dj-software -- play <a> --cue-track <b> --deck3 <c> --deck4 <d> --controller ddj-sb3 --device "DDJ-SB3" --master 3,4
  ```
  Check on the hardware:
  - Play/cue on both sides and on decks 3/4 after DECK switching.
  - Faders, EQ and filter, including soft takeover after a switch.
  - The tempo slider direction.
  - The crossfader.
  - LEDs follow the deck the side shows.
  - Unplugging and replugging USB keeps audio playing and restores the LEDs. (With master on the SB3 itself, audio stops when it's unplugged; to test this, run master on the MacBook speakers with no headphone routing.)
  Record the owner's confirmation. Don't claim the check passed without it.
