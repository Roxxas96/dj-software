# Plan 12b: Mouse Control Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every control in the window works with the mouse, as in VirtualDJ:
- **Library:** browse, search, sort, load tracks (double-click, drag onto a deck, or right-click), and edit playlists.
- **Decks:** pads, transport, loops, the jog and the waveforms.
- **Mixer:** knobs and faders, including the headphone level and mix.
- **Hotcues:** name and colour them with the mouse.

**Architecture:** Each control the window draws becomes interactive and sends what a controller input bound to it would send, `UiCommand::Action(action, input)`:
- knobs and faders send `Absolute`;
- buttons send `Press`/`Release`;
- the jog and waveform drags send `JogTouch` and `Relative`.

The session already applies those through `Runtime::apply_ui`. Library work (browsing, search, sort, loading by id, playlists and hotcue labels) uses a new `UiCommand::Library(LibraryStep)` that the session routes to `LibraryMode`. It goes through the library's writer, as the controller's requests do.

**Tech Stack:** Rust, egui/eframe 0.37 (`Response` drag/click/secondary-click, `dnd_drag_source`/`dnd_drop_zone`, `context_menu`), egui_kittest for gestures.

**Spec:** `docs/superpowers/specs/2026-11-12-vdj-feel-design.md`. This plan covers goals 1, 2, 3 and 6, the "Decisions after the builder check" section, and the clickable controls 12a left: the search box, the loop-size arrows and the pad-mode arrows. Plan 12a, the compact window, is merged.

## Global Constraints

- Everything from the project still holds:
  - mise, prek, Conventional Commits;
  - `cargo clippy --all-targets -- -D warnings`, `cargo fmt`, `cargo deny check`;
  - builds and tests on macOS, Windows and Linux.
- "The window never acts on the engine directly. A mouse control sends the same `UiCommand::Action(action, input)` a controller input would send … The session applies it on the control thread." New actions go in the `actions` crate.
- **Mouse control must not make the controller worse:**
  - no added latency on MIDI;
  - no lost soft takeover;
  - no knob jumping because the mouse moved it.
- **The playing-deck rule:** "A load onto a deck whose transport is playing is refused, and an event says 'Deck N is playing'". The session's check is authoritative.
- **Misclicks:** "Deleting a hotcue or playlist from a menu is a separate menu item, never a single click on a pad."
- "Library writes go through the library's writer, in one transaction where the change has several parts."
- "Hotcue names and colours use the existing storage: `HotcuePoint`, `WriteOp::PutHotcue` and the memory-sync path". An edit is written at once and shows live, even on a playing deck.
- **Mouse and soft takeover:** a mouse move sets the value at once. Afterwards the hardware must cross the on-screen value before it takes over, and the ghost marker shows where the hardware is. Ghosts exist for trim, EQ, filter, channel fader, crossfader, pitch, master level and headphone level.
- **Rendering:** the plan 9 frame rule applies (no allocation per frame in drawing paths that avoid it). Plan 12a's look stays: no outlines, compact sections, and the mirrored right decks.
- **Tests** never touch the owner's real config, data, music or `/Volumes`.

## Review Focus

1. **A drag that ends outside any target, or a drop onto a deck that starts playing during the drag.** Nothing loads, and the playing deck gets the "Deck N is playing" event. Test: Task 6 `load_onto_a_deck_that_started_playing_is_refused`.
2. **A knob moved with the mouse while the controller's knob sits elsewhere.** The value does not jump back on the next MIDI message; the hardware has to cross the value first. Test: Task 1 `mouse_value_needs_pickup_by_the_hardware`.
3. **A search or sort on a 50,000-track library, typed fast.** The window never waits, and the list shows the latest query's result, never a stale one. Test: Task 7 `only_the_latest_search_result_shows`.
4. **Editing a hotcue name on a track that changes deck or is unloaded meanwhile.** The edit goes to the track that was right-clicked, never to whatever is on the deck now. Test: Task 4 `hotcue_label_goes_to_the_clicked_track`.
5. **Playlist edits with duplicates and reorders.** Moving the second copy of a track moves that entry and not the first, and removing removes exactly that position. Test: Task 8 `moving_a_duplicate_moves_that_entry`.

---

## Decisions this plan makes (my calls)

- **Gestures:**
  - **Knobs:** a vertical drag turns a knob, 150 points for the full travel. Shift gives a fine drag at a quarter of the speed. The wheel moves 2 % per notch.
  - **Faders and sliders:** dragged along their track. A click on the track jumps the handle there.
  - **Double-click resets:**

    | Control | Resets to |
    |---|---|
    | EQ and filter | centre (0.5) |
    | Gain | its unity position, read from the mapping/engine conversion (1 = 0 dB) |
    | Pitch | 0 % (0.5) |
    | Crossfader | centre |
    | Master and headphone levels | their engine defaults (0.8) |

    Channel faders do not reset.
- **Buttons** send `Press` on pointer down and `Release` on pointer up, so holds work (CUE preview, hotcue preview, loop roll). If the pointer is lost, the button sends `Release` when the press ends.
- **Pads** in each mode send the action the hardware pad sends in that mode, found through the same table the controller uses (Task 3).
- **The loop-size arrows** send `LoopHalve` and `LoopDouble`. **The pad-mode arrows** step through the dropdown's list of modes. VirtualDJ's USER block is not built: Open Jockey has no user buttons.
- **Headphone controls:** the mixer's top row gets two small knobs next to MASTER, HEADPHONE (level) and MIX (cue ↔ master). They are drawn only in the 2-deck mixer. The 4-deck mixer shows them as "HP" and "MX" letters, per plan 12a's letter rule.
- **The jog:** dragging along its ring sends `JogTouch` Press when the drag starts, `JogScratch` with `Relative` steps while dragging, and `JogTouch` Release when it ends. Steps follow the angle moved, using the same steps per turn as the SB3 jog (find it in the mapping).
- **The scrolling waveform** behaves the same as the jog. Its steps are scaled so the waveform under the pointer follows it.
- **The overview** sends a new action, `deck.seek` (`Absolute`, the fraction of the track), on click and while dragging.
- **The browser:**
  - **Clicks:** a click selects (tree entry, list row). The tree's arrow, or a double-click on a folder, opens and closes it.
  - **Loading:** double-clicking a track loads it, on the first deck that is not playing, left before right, decks 1–2 only. Two-deck view: 1, 2. Four-deck view: 1, 2, 3, 4.
  - **Selection:** Cmd/Ctrl-click and Shift-click extend the list selection. The selection is the window's own, a set of absolute row indices, cleared when the list changes.
  - **Context menus:**
    - right-click a track: "Load on deck N" for each shown deck, "Add to playlist ▸" (one entry per playlist), and "Remove from playlist" in a playlist;
    - right-click a playlist: "Rename…", then "Delete…", which needs a confirm dialog;
    - right-click the "Playlists" node: "New playlist…".
- **The search box** filters the current list by title or artist as you type, ASCII case-insensitive, using the library's existing `TrackFilter.text`. Clearing it restores the list. It applies to folders and playlists alike.
- **Hotcue menu:** right-click a pad or a waveform flag.
  - On a set hotcue it holds a name field (Enter saves), the 8 colours of the existing pad palette plus "No colour" (the slot default), and "Delete".
  - On an empty pad it holds only "Set hotcue here", which sets it at the playhead.

## File structure

```
crates/actions/src/action.rs        ActionKind::Seek ("deck.seek", Absolute) → DeckCommand::Seek
crates/ui/src/controls.rs (new)     knob_input / fader_input / button_input: gesture → Input
crates/ui/src/mixer.rs              interactive knobs, faders, PFL, crossfader, master, headphone knobs
crates/ui/src/deck.rs               interactive transport, states, FX, pitch, loop, pads, pad-mode arrows, jog, overview
crates/ui/src/hotcue_menu.rs (new)  the right-click hotcue editor
crates/ui/src/waveform/lane.rs      drag to scrub, flag right-click
crates/ui/src/browser.rs            clicks, wheel, double-click, selection, context menus, search box, headers, drag sources, drop zones
crates/ui/src/command.rs            UiCommand::Library(LibraryStep), LibraryStep, BrowserEdit
crates/library/src/browser/mod.rs   Browser::edit(BrowserEdit): select, toggle at, search, sort
crates/library/src/writer.rs        WriteOp::SetHotcueLabel
crates/app/src/session/*            route Library steps; refuse loads on playing decks; hotcue labels; events
crates/app/src/library_mode/*       load by id, browser edits on the worker, playlist ops
docs/ui.md                          the mouse controls; "Known limits" updated
```

---

### Task 1: Mouse gestures, the seek action, and soft takeover

**Files:**
- Create: `crates/ui/src/controls.rs`
- Modify: `crates/actions/src/action.rs` and `apply.rs` (add the `Seek` kind)
- Test:
  - unit tests in `controls.rs`;
  - a test in `crates/controller/src/runtime.rs` (or its tests);
  - `crates/actions` tests for `Seek`.

**Interfaces:**
- Produces:
  - `pub fn knob_input(response: &egui::Response, value: f32, reset: f32, ui: &egui::Ui) -> Option<actions::Input>`. It returns `Input::Absolute(new)` on a drag, a wheel move or a double-click reset, else `None`. The value is clamped to 0..=1. Constants: `KNOB_DRAG_POINTS = 150.0`, `FINE_DIVISOR = 4.0`, `WHEEL_STEP = 0.02`.
  - `pub fn fader_input(response: &egui::Response, track: egui::Rect, vertical: bool, reset: Option<f32>) -> Option<Input>`. It maps the pointer's position along `track` to 0..=1. Vertical faders put the top at 1, as the channel faders do today; check how they are drawn. The pitch slider has its own direction (faster toward the bottom), so its caller inverts the value.
  - `pub fn button_input(response: &egui::Response, held: &mut bool) -> Option<Input>`. It gives `Press` on pointer down over the button and `Release` when that press ends. `held` is the widget's state, kept in egui memory by its id.
  - `Action::Seek(DeckId)`, id `"deck.seek"`, `Absolute` input. Applying it sends `DeckCommand::Seek(fraction × the deck's duration in frames)` when a track is loaded, else nothing.

- [ ] **Step 1: Write the failing tests.**
  - `controls.rs`, with kittest harnesses for a test widget:
    - `dragging_up_turns_a_knob_up`: a 75-point upward drag from 0.5 gives about 1.0.
    - `shift_drag_is_four_times_finer`.
    - `wheel_moves_two_percent`.
    - `double_click_resets_a_knob`.
    - `fader_click_jumps_there`.
    - `button_press_and_release`: pointer down gives `Press`, pointer up gives `Release`, and the press never repeats while held.
  - actions:
    - `seek_kind_round_trips`: `"deck.seek"` parses, and its `InputKind` is `Absolute`.
    - `seek_sends_the_fraction_of_the_track`: 0.25 on a loaded deck of 1,000,000 frames sends `Seek(250000.0)`. On an empty deck it sends nothing.
  - controller, `mouse_value_needs_pickup_by_the_hardware`:
    1. With the controller attached and its trim knob at 0.2, `apply_ui(Trim(deck1), Absolute(0.8))` sets the trim to 0.8.
    2. A MIDI value of 0.25 does not move the trim, and `pickup(trim)` is `Some(0.25)`.
    3. Values rising past 0.8 take over.
- [ ] **Step 2: Run them and see them fail. Step 3: Implement.** If the takeover test passes without any change ("Soft takeover follows as for any value changed elsewhere", runtime.rs), keep the test as the guard and say so in the report.
- [ ] **Step 4: Run `cargo test -p ui -p actions -p controller`; it passes.**
- [ ] **Step 5: Commit** `feat(ui): mouse gestures for knobs, faders and buttons, and a seek action`

---

### Task 2: The mixer and the deck's buttons, sliders and loops work with the mouse

**Files:**
- Modify: `crates/ui/src/mixer.rs` and `crates/ui/src/deck.rs`.
- Modify: the frame model, if the headphone level and mix are not in the frame yet. Find the master level's path from `ControlState` to `ControlFrame`, and mirror it.
- Modify: `crates/app/src/session/frame.rs`.
- Test: `crates/ui/tests/mixer.rs`, `crates/ui/tests/deck.rs` and `crates/ui/tests/four_decks.rs`.

**Interfaces:**
- Consumes: `knob_input`, `fader_input` and `button_input` (Task 1).
- Produces, as the commands each control sends:
  - **Mixer** (the strip's deck; `Global` for crossfader, master and the headphone knobs):
    - `Trim(deck)`, `EqHigh(deck)`, `EqMid(deck)`, `EqLow(deck)`, `Filter(deck)` and `Fader(deck)`, each `Absolute`;
    - in stems mode, the EQ knobs keep sending `EqHigh`, `EqMid` and `EqLow`, as the hardware does;
    - `PflToggle(deck)`, Press then Release;
    - `Crossfader`, `MasterLevel`, `HeadphoneLevel` and `HeadphoneMix`, each `Absolute`.
  - **Deck** (the panel's deck, or its `Side` where the action targets a side; check `Action::from_kind`):
    - `PlayPause` and `Cue`;
    - `Sync`, `VinylToggle`, `SlipToggle` and `KeylockToggle`;
    - `FxToggle(n)`, and `FxLevel` (`Absolute`);
    - `Tempo` (`Absolute`, from the pitch slider);
    - `LoopIn`, `LoopOut`, `AutoLoop` (the size box), `LoopHalve` ("<") and `LoopDouble` (">");
    - the pad-mode arrows, ◀ and ▶ either side of the dropdown, which send the `PadMode` before or after the current one in `PAD_MODES`, wrapping round.

  The mixer's top row gains the HEADPHONE and MIX knobs next to MASTER. Their ghosts show as the other knobs' do.

- [ ] **Step 1: Write the failing kittest tests.** Each one sends commands to a channel and asserts what arrives.
  - `dragging_the_left_high_knob_sends_eq_high`.
  - `clicking_pfl_sends_a_press_and_release`.
  - `dragging_the_crossfader_sends_absolute`.
  - `master_and_headphone_knobs_send_their_actions`: the 2-deck mixer has HEADPHONE and MIX knobs that send `HeadphoneLevel`/`HeadphoneMix`, and the 4-deck mixer shows them as "HP"/"MX".
  - `double_click_resets_pitch_to_zero`: sends `Tempo` `Absolute(0.5)`.
  - `play_cue_sync_and_states_send_their_actions`: covers deck 1 and deck 2. Deck 2 is mirrored: query the buttons by label.
  - `loop_arrows_halve_and_double`.
  - `pad_mode_arrows_step_and_wrap`.
  - `fx_toggle_and_level`.
  - `ghosts_for_every_absolute_control`: trim, EQ, filter, channel fader, crossfader, pitch,
    master level and headphone level each draw a hollow marker when their pickup is set
    (add the frame fields and the session plumbing for any that lack one today).
- [ ] **Step 2: Run them and see them fail. Step 3: Implement.** Draw exactly as 12a does. Only the interaction changes, plus the two new knobs and the arrows. **Step 4: Run** `cargo test -p ui -p open-jockey`. Re-render with `cargo test -p ui --test render -- --ignored` and look at pro.png and pro-4decks.png.
- [ ] **Step 5: Commit** `feat(ui): play the mixer and the deck controls with the mouse`

---

### Task 3: Pads with the mouse in every pad mode

**Files:**
- Modify: `crates/ui/src/deck.rs` (the pads).
- Modify: `crates/actions` (a pure function mapping a pad press to its action), or reuse the controller mapping's lookup if one exists.
- Test: `crates/ui/tests/deck.rs` and `crates/actions` tests.

**Interfaces:**
- Produces: `pub fn pad_action(mode: PadMode, side: Side, deck: DeckId, slot: HotcueSlot, shift: bool) -> Option<Action>` in `actions`, which gives the action a hardware pad sends in that mode. Find the existing per-mode handling (`SideControl`, the mapping's pad rules) and expose it rather than duplicating it.
  - **Hotcues:** `Hotcue(slot)`.
  - **Beat jump:** `BeatJump(slot)` (`BeatJumpShift` with shift).
  - **Loop roll:** `LoopRoll(slot)`.
  - **Slicer:** `Slice(slot)`.
  - **Sampler:** `SamplerPlay(slot)`.
  - **Hardware mode:** the hardware decides what the pads do, so the window sends hotcues. My call: hotcues are what the pads show in that mode.
- The window's Shift is the keyboard's Shift key while clicking.

- [ ] **Step 1: Write the failing tests.**
  - `pad_action_per_mode`: one assertion per mode for slot 3.
  - `clicking_a_hotcue_pad_sends_press_and_release` (kittest).
  - `loop_roll_pad_holds_while_pressed`: a release comes only when the pointer is released.
  - `shift_click_in_beat_jump_mode_sends_beat_jump_shift`.
- [ ] **Step 2: Run them and see them fail. Step 3: Implement. Step 4: Run them; they pass. Step 5: Commit** `feat(ui): play the pads with the mouse`.

---

### Task 4: Name, colour, set and delete hotcues from a right-click menu

**Files:**
- Create: `crates/ui/src/hotcue_menu.rs`.
- Modify: `crates/ui/src/deck.rs` (right-click on a pad), `crates/ui/src/waveform/lane.rs` (right-click on a flag), `crates/ui/src/command.rs`.
- Modify: `crates/library/src/writer.rs` (and wherever hotcues are written).
- Modify: `crates/app/src/session/*` (apply the edit, refresh marks, events).
- Test: `crates/ui/tests/deck.rs`, `crates/library/tests/hotcue_points.rs`, and session tests.

**Interfaces:**
- Produces:
  - `WriteOp::SetHotcueLabel { track: i64, slot: u8, name: Option<String>, color: Option<u32> }`. It updates only the name and colour of an existing slot. An empty slot gives `LibraryError::NoSuchHotcue`; add that variant if it is missing.
  - `UiCommand::Library(LibraryStep::HotcueLabel { deck: DeckId, track: i64, slot: HotcueSlot, name: Option<String>, color: Option<u32> })`. `track` is the id the window saw on that deck when the menu opened.
  - The session ignores the edit, with an event "The track changed before the hotcue was saved", unless deck `deck` still has `track` loaded. Otherwise it writes and then refreshes that deck's marks (`FrameParts::refresh_marks`).
  - "Set hotcue here" sends `Hotcue(slot)` Press then Release. "Delete" sends `HotcueDelete(slot)` Press then Release. Neither is a single click on a pad.
  - Names are cut to 32 characters (my call; plan 11a shows 14 on flags).

- [ ] **Step 1: Write the failing tests.**
  - `set_hotcue_label_updates_only_name_and_colour` (library): frames and the loop length are unchanged.
  - `label_on_an_empty_slot_is_refused`.
  - `right_click_on_a_set_pad_opens_the_editor` (kittest): the editor has a name field, 9 colour swatches and "Delete". Enter in the name field sends `HotcueLabel` with the text. A swatch sends its colour.
  - `right_click_on_an_empty_pad_offers_set_here_only`.
  - `flag_right_click_opens_the_same_editor` (lane).
  - `hotcue_label_goes_to_the_clicked_track` (session): if the deck's track changed after the menu opened, nothing is written and the event shows.
  - `label_edit_shows_live` (session): after the write, the next frame's `hotcue_marks` for that deck carry the new name and colour.
- [ ] **Step 2: Run them and see them fail. Step 3: Implement. Step 4: Run** `cargo test -p ui -p library -p open-jockey`. **Step 5: Commit** `feat(ui): name, colour, set and delete hotcues from a menu`.

---

### Task 5: Jog, waveform and overview with the mouse

**Files:**
- Modify: `crates/ui/src/deck.rs` (the jog and the overview) and `crates/ui/src/waveform/lane.rs`.
- Test: `crates/ui/tests/deck.rs` and `crates/ui/tests/waveform.rs`.

**Interfaces:**
- Consumes: `Action::Seek` (Task 1).
- Produces:
  - **Jog drag:**
    - `JogTouch` Press when the drag starts;
    - `JogScratch` `Relative(steps)` while dragging, where steps = angle moved × the SB3's steps per turn ÷ 2π, rounded, with the remainder carried over;
    - `JogTouch` Release when the drag ends.
  - **Lane drag:** the same three actions. Steps = horizontal points moved × (frames per point at the lane's zoom) ÷ (frames per jog step). A pointer moved left sends forward steps, because the waveform follows the pointer.
  - **Overview:** a click or drag sends `Seek(deck)` `Absolute(x fraction)`.

- [ ] **Step 1: Write the failing tests.**
  - `jog_drag_touches_scratches_and_releases`: the order of the commands, and a quarter turn gives about a quarter of the steps per turn.
  - `lane_drag_follows_the_pointer`: a 100-point left drag sends forward steps equal to 100 × frames per point ÷ frames per step (±1).
  - `overview_click_seeks`: a click at 25 % of the overview's width sends `Seek` `Absolute(0.25)` (±0.01).
  - `drag_on_an_empty_deck_sends_nothing`.
- [ ] **Step 2: Run them and see them fail. Step 3: Implement. Step 4: Run them; they pass. Step 5: Commit** `feat(ui): scratch and seek with the mouse`.

---

### Task 6: The library with the mouse — browse, select, load

**Files:**
- Modify: `crates/ui/src/command.rs`, which gets `LibraryStep` and `BrowserEdit`.
- Modify: `crates/ui/src/browser.rs`.
- Modify: `crates/library/src/browser/mod.rs`, which gets `Browser::edit`.
- Modify: `crates/app/src/library_mode/{mod.rs, worker.rs}`, and `crates/app/src/session/{commands.rs, mod.rs}`. The control hook must reach `LibraryMode`; extend the hook passed to `run_controller` so it receives `Option<&mut LibraryMode>`, as `on_pass` does with `pass.library`.
- Test: `crates/library/tests/browser.rs` (or the browser unit tests), `crates/ui/tests/browser.rs`, and session tests.

**Interfaces:**
- Produces:
  - In `library::browser`, `pub enum BrowserEdit { SelectTree(usize), SelectRow(usize), ToggleTreeAt(usize), Search(Option<String>), Sort { key: TrackSortKey, descending: bool } }`, with `pub fn edit(&mut self, edit: BrowserEdit) -> Result<BrowserView, LibraryError>`. Search and Sort are implemented in Task 7. In this task they may return the view unchanged.
  - `ui::LibraryStep`:
    - `Browse(BrowserEdit)`;
    - `Load { deck: DeckId, track: i64 }`;
    - `HotcueLabel {..}` (Task 4);
    - and the playlist steps of Task 8.
  - `UiCommand::Library(LibraryStep)`.
  - The session applies a `Load` like the console's `load`, which shares its path (`LibraryMode`). It is refused with the event `Deck N is playing` when `engine.deck(deck).playing`.
  - **Window gestures:**
    - click a tree row: `SelectTree(i)`;
    - click its arrow, or double-click a folder: `ToggleTreeAt(i)`;
    - click a list row: `SelectRow(absolute i)`;
    - the wheel over a pane scrolls what the window shows. Pages load as today: the session pages the list around the cursor, and a scroll beyond the loaded page sends `SelectRow`.
  - **Double-click a track:** `Load` on the first deck not playing in the order 1, 2 (then 3, 4 in the 4-deck view). With none free, the window sends nothing and shows its own event "Every deck is playing". It reads "playing" from `DeckReads`; the session still checks.
  - **Right-click a track:** "Load on deck N" for each shown deck.

- [ ] **Step 1: Write the failing tests.**
  - library:
    - `select_tree_reads_that_entry`;
    - `select_row_moves_the_list_cursor` (beyond the loaded page too);
    - `toggle_tree_at_opens_a_folder`.
  - ui:
    - `clicking_a_row_selects_it`;
    - `double_click_loads_on_the_first_free_deck` (deck 1 playing → `Load { deck: 2 }`);
    - `double_click_with_every_deck_playing_sends_nothing`;
    - `context_menu_loads_on_the_chosen_deck`;
    - `wheel_scrolls_the_list`.
  - session:
    - `load_by_id_loads_the_track`;
    - `load_onto_a_deck_that_started_playing_is_refused` (the event text is exact);
    - `browse_steps_reach_the_library_worker`.
- [ ] **Step 2: Run them and see them fail. Step 3: Implement. Step 4: Run** `cargo test -p library -p ui -p open-jockey`. **Step 5: Commit** `feat(ui): browse and load tracks with the mouse`.

---

### Task 7: Search and sort

**Files:**
- Modify: `crates/library/src/browser/mod.rs` (the list's filter and sort).
- Modify: `crates/ui/src/browser.rs` (the search box above the list, and clickable headers with a ▲/▼ on the sorted column).
- Modify: `crates/app/src/library_mode/*`.
- Test: library browser tests, `crates/ui/tests/browser.rs`, and session tests.

**Interfaces:**
- Consumes: `BrowserEdit::Search` and `BrowserEdit::Sort` (Task 6).
- Produces:
  - **Search:**
    - `Search(Some(text))` filters the current list by `TrackFilter.text` and puts the cursor on its first row. In a folder, files that are not in the library are kept when their file name matches.
    - `Search(None)` restores the list.
    - Search and sort stay set while you move between tree entries (VirtualDJ does the same; my call), and the view shows them.
  - **Sort:**
    - `Sort` orders by `TrackSortKey` (Title, Artist, BPM, Key, Added). Remix and Length sort client-side? No: add `Remix` and `Length` keys to `TrackSortKey` if the query can do them. Otherwise those headers are not clickable (my call: Length must sort; Remix may stay unclickable).
    - A playlist's natural order is "#". Clicking the sorted header a third time returns to it.
  - **Fresh results only:** each `Search` and `Sort` carries a sequence number (`BrowserEdit` gains `seq: u64` on those two variants, or the window keeps its latest and ignores older views). Only the latest query's view is shown.
  - **The search box:** an egui `TextEdit` with a "Search" hint above the list. It sends `Search` on every change. Escape clears it.

- [ ] **Step 1: Write the failing tests.**
  - library:
    - `search_filters_title_and_artist` (case-insensitive ASCII);
    - `search_none_restores`;
    - `sort_by_bpm_descending`;
    - `third_click_returns_to_playlist_order`;
    - `search_in_a_folder_keeps_matching_unscanned_files`.
  - ui:
    - `typing_sends_search`;
    - `escape_clears_search`;
    - `header_click_sends_sort_then_reverses`.
  - session, `only_the_latest_search_result_shows`: two searches sent back to back; the published view reflects the second.
  - library, `search_on_fifty_thousand_tracks_is_fast`: under 750 ms in debug, 2000 ms on CI's Windows, matching the existing timing test's bounds in `crates/library/tests/playlists.rs`.
- [ ] **Step 2: Run them and see them fail. Step 3: Implement. Step 4: Run them; they pass. Step 5: Commit** `feat(ui): search and sort the library`.

---

### Task 8: Drag and drop — onto decks, into playlists, and reordering

**Files:**
- Modify: `crates/ui/src/browser.rs`, `crates/ui/src/deck.rs`, `crates/ui/src/waveform/lane.rs`, `crates/ui/src/lib.rs` (drop zones), and `crates/ui/src/command.rs`.
- Modify: `crates/app/src/library_mode/*`, using the playlist `WriteOp`s that already exist.
- Test: `crates/ui/tests/browser.rs` and session or library-mode tests.

**Interfaces:**
- Produces:
  - **Drag payload:** `TrackDrag { tracks: Vec<i64>, from_playlist: Option<(i64, Vec<usize>)> }`. It holds the selected rows' track ids in list order, and their playlist positions when dragged from a playlist.
  - **New `LibraryStep`s:**
    - `AddToPlaylist { playlist: i64, tracks: Vec<i64> }`, which appends;
    - `MoveInPlaylist { playlist: i64, from: Vec<usize>, to: usize }`;
    - `RemoveFromPlaylist { playlist: i64, positions: Vec<usize> }`.

    Each is one writer batch (one transaction). Positions are 0-based here; the CLI's are 1-based.
  - **Drop targets:**
    - a deck panel or its lane: `Load { deck, track: tracks[0] }`, refused if the deck is playing;
    - a playlist in the tree: `AddToPlaylist`;
    - between rows of the open playlist, from the same playlist: `MoveInPlaylist`. A drop indicator line shows where it will land.
  - **Selection:** Cmd/Ctrl-click toggles a row, and Shift-click extends from the anchor. A plain click selects only that row. Dragging an unselected row drags just it.
  - **Context menu:** right-click a track to get "Add to playlist ▸ <each playlist>", and "Remove from playlist" when in a playlist.

- [ ] **Step 1: Write the failing tests.**
  - `drag_track_onto_deck_two_loads_it` (kittest drag from row to deck 2's panel).
  - `drop_on_a_playing_deck_is_refused` (session event).
  - `drag_selection_into_a_playlist_appends_in_order`.
  - `moving_a_duplicate_moves_that_entry`: playlist [a, b, a]; move position 2 to 0 → [a, a, b], where the moved entry is the one that was at 2. Assert by position in the written playlist.
  - `remove_from_playlist_removes_exactly_those_positions`.
  - `shift_and_cmd_click_build_a_selection`.
  - `a_drop_outside_any_target_does_nothing`.
- [ ] **Step 2: Run them and see them fail. Step 3: Implement. Step 4: Run** `cargo test -p ui -p open-jockey`. **Step 5: Commit** `feat(ui): drag tracks onto decks and into playlists`.

---

### Task 9: Playlists from the menus, and docs

**Files:**
- Modify: `crates/ui/src/browser.rs` (the tree's context menus and the dialogs) and `crates/ui/src/command.rs`.
- Modify: `crates/app/src/library_mode/*`.
- Modify: `docs/ui.md`, which gets a "Mouse" section and an updated "Known limits" (the window no longer needs the controller to play), and `README.md`.
- Test: `crates/ui/tests/browser.rs` and session tests.

**Interfaces:**
- Produces `LibraryStep`s:
  - `CreatePlaylist { name: String }`;
  - `RenamePlaylist { playlist: i64, name: String }`;
  - `DeletePlaylist { playlist: i64 }`.

  An empty or duplicate name is refused, with the events "A playlist needs a name" and "There is already a playlist called <name>".
- The dialogs are small egui windows:
  - "New playlist" and "Rename playlist" have a name field, OK and Cancel. Enter means OK, Escape means Cancel.
  - "Delete playlist" shows "Delete <name>? Its tracks stay in the library." with Delete and Cancel. Cancel is focused.

- [ ] **Step 1: Write the failing tests.**
  - `new_playlist_from_the_playlists_node`.
  - `rename_playlist`.
  - `delete_asks_first_and_cancel_keeps_it`.
  - `delete_confirmed_deletes`.
  - `empty_name_is_refused`.
  - `duplicate_name_is_refused`.
- [ ] **Step 2: Run them and see them fail. Step 3: Implement. Step 4: Run** `cargo test --workspace` and `cargo clippy --all-targets -- -D warnings`. Re-render and look.
- [ ] **Step 5: Commit** `feat(ui): create, rename and delete playlists from the browser` and `docs: the window works with the mouse`.

---

## Hands-on (the owner, after the final review, without the controller)

1. **Load and play:**
   - Search for a track and sort by BPM.
   - Load a track by double-clicking it, and another by dragging it onto deck 2.
   - Play both with the mouse, and mix with the faders, EQ and crossfader.
   - Cue in the headphones with PFL, the headphone level and the mix.
2. **Pads, loops and pitch:**
   - Set hotcues by clicking the pads.
   - Right-click a pad to name and colour it, and check the flag on the waveform.
   - Set a saved loop with IN/OUT and a pad.
   - Use the loop arrows, the pad-mode arrows and each pad mode.
   - Drag the pitch slider, then double-click it to reset it.
3. **Scratch and seek:** scratch with the jog and the waveform, and click the overview to seek.
4. **Playlists:**
   - Make a playlist from the Playlists node and drag several tracks into it.
   - Reorder it, remove a track, rename it, then delete it (it asks first).
5. **Safety and the controller:**
   - Try loading onto a playing deck: it refuses.
   - Plug the SB3 in and move a knob the mouse changed: it does not jump until it crosses the value.
6. Record the results in `controllers/ddj-sb3/README.md`.
