# UI: Making It Yours (Plan 10) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the window configurable and rearrangeable the VirtualDJ way: the LAYOUT menu (four layouts, colour scheme, waveform display, jog display, beat counter, 4-Decks), draggable and foldable panels, the pad-mode and per-deck FX dropdowns, and a settings window whose Options page edits `config.toml`.

**Architecture:**
- **What the window may change.** Plan 9's window only displayed. Now it may change view state and a few controls, but always by asking the session thread. It sends `ui::UiCommand`s over a channel, never blocks, and never touches the engine.
- **Who owns the state.**
  - The session owns the view state (layout, styles, panel sizes) and saves it in `ui-state.toml`.
  - It applies control changes (pad mode, FX slot) through the controller runtime and the engine.
  - It applies settings by rewriting `config.toml` (comments kept) and re-parsing it.
- **Publishing.** Everything the window shows still arrives in the published `ControlFrame`.

**Tech Stack:**
- Rust 2024, `eframe`/`egui` 0.37 with `egui_kittest`;
- `toml_edit` (new, MIT/Apache) to edit `config.toml` without losing comments;
- the existing crates.

**Spec:** `docs/superpowers/specs/2026-11-08-ui-design.md` ("Goals: plan 10", the non-goals, anti-goals and constraints). Plan 9's plan, `docs/superpowers/plans/2026-11-08-09-ui.md`, describes the code this plan builds on.

## Global Constraints

- License Apache-2.0, no GPL; `cargo deny` passes. A new crate's licence goes in `deny.toml` only if needed, with a comment.
- **Drawing never slows down or interrupts playback.**
  - The window sends commands with a non-blocking `Sender::send` and reads only the published frame and the `EngineMonitor`.
  - The control thread never waits on the window.
  - Disk writes (`ui-state.toml`, `config.toml`) never run on the control loop: they run on a short-lived thread, or at shutdown.
- **The screen never shows a state the SB3 or the app doesn't have.**
  - The pad-mode dropdown offers only the modes the app switches itself: beat jump, loop roll, slicer, pad scratch, and "hardware". It never offers the SB3's own modes, since the host cannot set them.
  - SYNC stays one-shot, with no badge.
- **Not mouse-driven DJing.** The dropdowns and settings are for setting up. No clickable transport, jog or faders.
- **Look:** VirtualDJ's layout, terminology and dark style; Open Jockey's own artwork. No VirtualDJ logo, icons, images or fonts.
- **Window state** lives in `ui-state.toml` next to `config.toml` (plan 9's zoom file, extended), never in `config.toml`.
- **Settings** written back to `config.toml` keep the user's comments and key order. They are validated by the existing `config::parse` rules, and errors read `<file>: <key>: <problem>`.
- **Live and next-launch settings.**
  - Applied at once: `ui.*`, `stems.knobs`, `stems.eq_mode` (when stems are enabled), `fx.slots` and `fx.deck1`–`fx.deck4`, `mixer.crossfader`, `sampler.volume`, `beat_jump.*`, `loop_roll.pads` and `backspin.*`.
  - Everything else is marked "applies at next launch" in the window.
- **One audio device,** as today.
- **Tests never touch the user's real config or data folders:** use temp paths, and `controller::builtin::load_mapping_from(name, None)` for mappings.
- **Conventional Commits.** Every commit message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never stage `controllers/ddj-sb3/capture.toml`, and never touch anything under `/Volumes`.

## Review Focus

1. **A broken or hand-edited `ui-state.toml`** (unknown keys, a removed layout name, sizes beyond the window): the window opens with defaults for the bad values, keeps the good ones, and never refuses to start. Tested in Task 1.
2. **An Options edit that makes `config.toml` invalid** (out-of-range value, wrong type): the file is not written, the row shows the error, and the running app keeps its current settings. Tested in Task 9.
3. **A hand-written `config.toml` with comments edited from the window:** the comments and the other keys survive byte for byte. Tested in Task 9.
4. **The 4-Decks view while decks 3 and 4 are empty or the SB3 shows deck 3:** the right panels and mixer channels follow the decks, and no panel shows another deck's state. Tested in Task 4.
5. **A panel dragged to zero or past the window, or a window resized smaller:** sizes are clamped to a minimum, so every area stays reachable. Tested in Task 7.

---

## File Structure

- `crates/ui/src/command.rs` (new): `UiCommand` and `ViewChange`, what the window asks the session to do.
- `crates/ui/src/view.rs` (new): `ViewState` and its enums (layout, colour scheme, waveform style and position, jog display, time mode, panel sizes and folds), with defaults and clamping.
- `crates/ui/src/menu.rs` (new): the LAYOUT menu in the top bar.
- `crates/ui/src/layout.rs` (new): per-layout section visibility and proportions; the 2- and 4-deck arrangements.
- `crates/ui/src/theme.rs`: the dark and light `Palette`s.
- `crates/ui/src/waveform/{lane.rs,styles.rs}`: the waveform styles.
- `crates/ui/src/{deck.rs,mixer.rs,browser.rs}`: jog display, beat counter, time cycling, dropdowns, sideview.
- `crates/ui/src/settings/{mod.rs,options.rs,interface.rs}` (new): the settings window.
- `crates/app/src/session/{view.rs (replaces zoom.rs),commands.rs,settings.rs}`: saving view state, applying commands, the settings registry and `config.toml` write-back.
- `crates/app/src/play.rs`: a control hook in `run_controller`.
- `crates/engine/src/{command.rs,mixer.rs}`: `ChannelCommand::FxSlot`.
- `crates/actions`: the `ui.layout` action.
- `crates/app/src/config.rs`: `fx.deck1`–`fx.deck4`.
- Docs: `docs/ui.md`, `docs/config.md`, `docs/controllers/mapping-reference.md`, `README.md`, `controllers/ddj-sb3/README.md`.

---

### Task 1: Window commands and saved view state

**Files:**
- Create: `crates/ui/src/{command.rs,view.rs}`, `crates/app/src/session/{view.rs,commands.rs}`
- Delete: `crates/app/src/session/zoom.rs` (its logic moves to `view.rs`)
- Modify:
  - `crates/ui/src/{lib.rs,model.rs,waveform/lane.rs}`;
  - `crates/app/src/session/{mod.rs,frame.rs}`;
  - `crates/app/src/play.rs`.
- Test: unit tests in the new files; `crates/ui/tests/smoke.rs`

**Interfaces:**
- Produces:
  - **`ui::UiCommand`** (`Debug, Clone, PartialEq`), with these variants:
    - `View(ViewChange)`;
    - `Zoom(i32)`, positive zooms in;
    - `Action(actions::Action, actions::Input)`;
    - `FxSlot { deck: DeckId, slot: usize, kind: engine::EffectKind }`;
    - `SetSetting { key: String, value: String }`;
    - `ResetSettings`.
    `Action` lets a dropdown press an existing action, such as `pads.mode_beat_jump`. Tasks 8 and 9 handle the variants after `Action`; this task routes them to `todo` handlers that log and ignore.
  - **`ui::ViewState`** (`Debug, Clone, PartialEq`, `Default`):
    - `layout: Layout { Starter, Essentials, Pro (default), Performance }`;
    - `colour_scheme: ColourScheme { Dark (default), Light }`;
    - `waveform_style: WaveformStyle { Frequency (default), DeckColour, Bands, Overlaid }`;
    - `waveform_position: WaveformPosition { Top (default), Middle }`;
    - `jog_display: JogDisplay { Plain (default), Info }`;
    - `beat_counter: bool` (default `false`);
    - `four_decks: bool` (default `false`);
    - `overview: bool` (default `true`);
    - `info_panel: bool` (default `true`);
    - `sideview: bool` (default `false`);
    - `time_mode: [TimeMode; 4]`, with `TimeMode { Elapsed, Remaining (default), Total }`;
    - `sizes: PanelSizes { lanes: f32 = 0.22, decks: f32 = 0.38, tree: f32 = 0.20, info: f32 = 0.18, sideview: f32 = 0.22 }` (shares of the window);
    - `waveform_beats: f64`.
    It also has `ViewState::clamped(self, zoom_range: (f64, f64)) -> Self`, which brings each share into 0.05–0.7 and the beats into the zoom range.
  - **`ui::ViewChange`**: one variant per `ViewState` field, carrying its new value (`SetLayout(Layout)`, …, `SetSizes(PanelSizes)`, `CycleTime(DeckId)`).
  - **`ViewState::apply(&mut self, change: ViewChange)`.**
  - **`ControlFrame` changes:** the field `view: ViewState` replaces `waveform_beats`.
  - **`ui::run(frame, monitor, quit, commands: std::sync::mpsc::Sender<UiCommand>)`.** It replaces the `Arc<AtomicI32>` zoom: the lanes send `UiCommand::Zoom(n)`.
  - **`app::session::view::ViewStore`:**
    - `load(path: Option<PathBuf>, defaults: ViewState, zoom_range) -> Self`;
    - `state(&self) -> &ViewState`;
    - `apply(&mut self, change: ViewChange, now: Instant) -> bool`;
    - `zoom(&mut self, steps: i32, actions_count: Option<i32>, now: Instant) -> bool`, which keeps plan 9's semantics;
    - `save_if_due(now)` (2 s after the last change, on a thread, atomic rename);
    - `Drop` saves.
  - **The file format** is TOML: `[view]` with the snake_case fields above, enums as lowercase strings (`layout = "pro"`). An existing plan 9 file holding only `waveform_beats` keeps loading.
  - **`play::run_controller`** gains `control: &mut dyn FnMut(&mut controller::Runtime, &mut EngineController)`, called once per loop pass before `on_pass`. `play` passes a no-op. The session drains its `Receiver<UiCommand>` there and applies `Action` commands with `actions::apply` on the runtime's state, which is what a MIDI input would do.

- [ ] **Step 1: Write the failing tests:**
  - **`view_state_round_trips`:** a non-default `ViewState` saved by `ViewStore` and loaded back is equal.
  - **`a_bad_view_file_keeps_the_good_values`:** `[view]` with `layout = "weird"`, `sizes = { lanes = 5.0 }` and `four_decks = true` gives layout Pro, the lanes share clamped to 0.7, and `four_decks` true. Unknown keys are ignored.
  - **`a_plan_9_file_still_loads`:** `waveform_beats = 8` gives beats 8 and the other fields at default.
  - **`ui_actions_apply_like_midi`:** the control hook, given `UiCommand::Action(PadMode(Left, BeatJump), Press)`, sets the left side's `pad_mode` to `BeatJump`.
  - **Kittest `the_zoom_buttons_ask_for_steps`:** updated so "+" sends `UiCommand::Zoom(1)` on the channel.
- [ ] **Step 2:** `cargo test -p ui -p open-jockey`. Expected: FAIL.
- [ ] **Step 3:** Implement. Plan 9's zoom keeps working: scroll, buttons, `ui.waveform_zoom_in`/`out`, saving and `ui.waveform_min_beats`/`max_beats`.
- [ ] **Step 4:** `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(ui): send window commands to the session and save the view state`.

### Task 2: Colour schemes and the LAYOUT menu

**Files:**
- Create: `crates/ui/src/menu.rs`
- Modify: `crates/ui/src/theme.rs` and every `crates/ui/src` file that uses theme colours; `crates/ui/src/topbar.rs`
- Test: `crates/ui/src/theme.rs` tests, `crates/ui/tests/menu.rs`

**Interfaces:**
- Consumes: Task 1's `ViewState`, `ViewChange` and `UiCommand`.
- Produces:
  - **`theme::Palette`**, a struct holding every colour now in `theme.rs` as a `const`:
    - `background`, `panel`, `lane`, `text`, `text_dim`;
    - `accent_left`, `accent_right`;
    - `beat_tick`, `bar_tick`, `cue`, `error`, `highlight`, and so on.
  - **`Palette::DARK`** has today's values. **`Palette::LIGHT`** is a light-grey VirtualDJ-style scheme: background `#e6e6e6`, panel `#f4f4f4`, lane `#d8d8d8`, text `#1e1f22`, dim text `#5f6166`. The accents and ticks keep their hues. Waveform colours stay as they are in both schemes.
  - **`theme::palette(ctx: &egui::Context) -> Palette`** reads the scheme the root stored in the context's temp data each frame. Every draw uses it instead of the old consts.
  - **`theme::apply(ctx, scheme)`** sets egui's visuals (dark or light) and stores the palette.
  - **`menu::show(ui: &mut Ui, view: &ViewState, commands: &Sender<UiCommand>)`** draws a "LAYOUT" menu button with the current layout name in the top bar. Its entries:
    - the four layouts (checked = current);
    - a separator;
    - submenus **Colour Scheme** (Dark, Light), **Waveform Display** (Frequency, Deck colour, Bands, Overlaid, a separator, Top, Middle, Overview on/off), **Jogwheel Display** (Plain, Info) and **Beat Counter** (on/off);
    - a separator;
    - **4-Decks** (checked when on);
    - **Panels:** Info and Sideview, both on/off.
    Each entry sends `UiCommand::View(..)`. Tasks 3 to 7 make each one change the screen; this task only sends them and switches the colour scheme.

- [ ] **Step 1: Write the failing tests:**
  - **`palettes_differ_and_keep_accents`:** `DARK.background != LIGHT.background` and `DARK.accent_left == LIGHT.accent_left`.
  - **Kittest `the_layout_menu_sends_view_changes`:** click "LAYOUT" then "Light", and the channel receives `View(SetColourScheme(Light))`. Click "LAYOUT" then "Starter", and it receives `View(SetLayout(Starter))`.
  - **Kittest `the_light_scheme_draws`:** the root renders a frame whose view is Light, without panicking.
- [ ] **Step 2:** Run. Expected: FAIL.
- [ ] **Step 3:** Implement. No colour literal may remain outside `theme.rs` (grep `Color32::from_rgb` in `crates/ui/src`; only `theme.rs` and the waveform colour anchors keep them).
- [ ] **Step 4:** `cargo test -p ui && cargo clippy -p ui --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(ui): add the light scheme and the LAYOUT menu`.

### Task 3: The four layouts and the `ui.layout` action

**Files:**
- Create: `crates/ui/src/layout.rs`
- Modify:
  - `crates/ui/src/{lib.rs,deck.rs,mixer.rs}`;
  - `crates/actions/src/{action.rs,apply.rs,state.rs}`;
  - `crates/app/src/session/commands.rs`;
  - `docs/controllers/mapping-reference.md`.
- Test: `crates/ui/src/layout.rs` tests, `crates/actions/tests/apply.rs`, `crates/ui/tests/layout.rs`

**Interfaces:**
- Produces:
  - **`ui::layout::Sections`** (`Debug, Clone, Copy, PartialEq`), the flags for what a deck panel and the screen show: `fx`, `pads`, `jog`, `pitch`, `loop_controls`, `overview`, `mixer_eq`, `mixer_meters`, `browser_share: f32`, `decks_share: f32`.
  - **`ui::layout::sections(layout: Layout) -> Sections`**, VirtualDJ's four as the guide (my calls, adjust only if the owner asks):
    - **Starter:** jog, pitch, loop and mixer EQ; no FX, no pads, no meters; decks 0.42, browser rest.
    - **Essentials:** jog, pads, FX (names and on/off only), loop, mixer EQ and meters; decks 0.40.
    - **Pro:** everything (plan 9's screen); decks 0.38.
    - **Performance:** pads, FX, loop and mixer, but no jog and a compact pitch; decks 0.26, so the browser gets the room.
    `PanelSizes.decks` overrides `decks_share` once the user has dragged it (Task 7).
  - **`actions`:** `ActionKind::LayoutNext`, id `ui.layout`, `Scope::Global`. A press adds 1 to the new `ControlState.layout_steps: i32` (wrapping, like `waveform_zoom`).
  - **The session** turns a change in `layout_steps` into `ViewChange::SetLayout(layout.next())`, cycling Starter → Essentials → Pro → Performance → Starter.

- [ ] **Step 1: Write the failing tests:**
  - **`sections_match_the_layouts`:** Starter has `!fx && !pads && jog`. Performance has `!jog && pads`. Pro has every flag true. Performance's `decks_share` is less than Pro's.
  - **`ui_layout_counts_presses`:** in `crates/actions/tests/apply.rs`, two presses give `layout_steps == 2`, a release is `Ignored`, and the ALL count and id list are updated.
  - **`layout_next_cycles`:** `Layout::Performance.next() == Layout::Starter`.
  - **Kittest `starter_hides_the_pads`:** a Starter frame renders without the pad-mode label, and a Pro frame renders with it.
- [ ] **Step 2:** Run. Expected: FAIL.
- [ ] **Step 3:** Implement. The deck panel and mixer read `Sections` instead of always drawing everything. Add `ui.layout` to the mapping reference's action table (unmapped by default).
- [ ] **Step 4:** `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(ui): add the four layouts and the ui.layout action`.

### Task 4: 4-Decks view

**Files:**
- Modify: `crates/ui/src/{lib.rs,deck.rs,mixer.rs,layout.rs,waveform/lane.rs}`
- Test: `crates/ui/tests/four_decks.rs`, unit tests in `layout.rs`

**Interfaces:**
- Produces:
  - **`ui::layout::deck_slots(four_decks: bool, active: [DeckId; 2]) -> Vec<DeckId>`:**
    - two decks: the side decks `[left, right]`, as in plan 9;
    - four decks: always `[1, 3, 2, 4]` in panel order, so left top is 1, left bottom 3, right top 2 and right bottom 4. The panel of each side's active deck is highlighted with its accent border.
  - **Four-deck screen:** four compact deck panels (Performance-like sections whatever the layout, two per side stacked) and a 4-channel mixer: four strips in deck order 3, 1, 2, 4, like VirtualDJ's 4-deck mixer.
  - **Lanes:** with `four_decks`, all four decks get a lane, in order 1, 2, 3, 4, whether or not they play.

- [ ] **Step 1: Write the failing tests:**
  - **`four_deck_slots_are_fixed`:** `deck_slots(true, [3, 2])` is `[1, 3, 2, 4]`. `deck_slots(false, [3, 2])` is `[3, 2]`.
  - **Kittest `four_decks_show_every_deck`:** a frame with `four_decks` and decks 3 and 4 empty renders the labels "1", "2", "3" and "4" and four mixer strips, without panicking.
  - **Kittest `four_deck_panels_follow_their_decks`:** deck 3 loaded with title "Three", deck 1 with "One". The panel labelled 3 shows "Three" and never "One".
- [ ] **Step 2:** Run. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test -p ui`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(ui): add the 4-Decks view`.

### Task 5: Waveform display styles and position

**Files:**
- Create: `crates/ui/src/waveform/styles.rs`
- Modify: `crates/ui/src/waveform/{lane.rs,mod.rs}`, `crates/ui/src/{lib.rs,deck.rs}`
- Test: unit tests in `styles.rs`, `crates/ui/tests/waveform.rs`

**Interfaces:**
- Produces:
  - **`ui::waveform::column(style: WaveformStyle, bucket: [u8; 4], deck: DeckId, palette: &Palette) -> Vec<Bar>`**, the coloured bars one column draws: `Bar { colour: Color32, from: f32, to: f32 }` with `from` and `to` as fractions of the lane height, 0 at the top and 1 at the bottom.
    - **Frequency:** plan 9's single mirrored bar, coloured by `colour(bucket)`.
    - **DeckColour:** one mirrored bar in the deck's accent, height from the peak.
    - **Bands:** three stacked lanes (highs on top, mids in the middle, bass at the bottom), each mirrored within its third. Colours: highs `#ffffff`, mids `#ff9ad5`, bass `#ff3a1f`, the plan 9 anchors.
  - **`WaveformStyle::Overlaid`:** the two side decks share one lane, each in its accent at 55 % alpha, mirrored. Other playing decks keep their own lanes below it.
  - **`WaveformPosition::Middle`** draws the lanes between the deck panels and the browser instead of at the top.
  - **`ViewState.overview == false`** hides the deck panels' overview waveforms.
  - Ticks, cue and hotcue markers, the loop band, the playhead and the zoom behave the same in every style.

- [ ] **Step 1: Write the failing tests:**
  - **`bands_split_the_lane_in_three`:** for a bucket with only bass, every `Bands` bar lies in `0.66..=1.0`. For only highs, in `0.0..=0.34`.
  - **`deck_colour_uses_the_accent`:** a `DeckColour` bar for deck 1 has `palette.accent_left`.
  - **`frequency_matches_plan_9`:** the `Frequency` bar colour equals `colour(bucket)`.
  - **Kittest `every_style_draws`:** render each of the four styles in both positions, without panicking.
- [ ] **Step 2:** Run. Expected: FAIL.
- [ ] **Step 3:** Implement. Keep the mesh reuse from plan 9 (no new per-frame allocation for columns).
- [ ] **Step 4:** `cargo test -p ui`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(ui): add the waveform display styles and position`.

### Task 6: Jog display, beat counter and time display

**Files:**
- Modify: `crates/ui/src/deck.rs`
- Test: `crates/ui/src/deck.rs` tests, `crates/ui/tests/deck.rs`

**Interfaces:**
- Produces:
  - **`JogDisplay::Info`** draws inside the jog circle the BPM (two decimals), the pitch as `+0.0%`, and the elapsed and remaining times (VirtualDJ's info jog). `Plain` is plan 9's jog.
  - **`ui::deck::beat_counter(grid: Option<Grid>, position_frames: f64) -> Option<String>`** gives `"<bar>.<beat>"`, both 1-based, counted from the grid's first beat with 4 beats a bar. Before the first beat, bar 0 counts backwards: 3 beats before gives `"0.2"`. Without a grid it gives `None`. It is shown in the deck header when `ViewState.beat_counter`.
  - **Clicking the time display** sends `View(CycleTime(deck))`, cycling Elapsed → Remaining → Total → Elapsed. The display shows `TimeMode`: Total is the track length.

- [ ] **Step 1: Write the failing tests:**
  - **`beat_counter_counts_bars`:** with period 100 and first beat 0: at 0 it gives "1.1", at 150 "1.2", at 400 "2.1". With first beat 300, at 0 it gives "0.2". No grid gives `None`.
  - **`time_mode_cycles`:** `TimeMode::Total.next() == Elapsed`.
  - **Kittest `info_jog_shows_the_bpm`:** with `JogDisplay::Info` and a 126 BPM deck, the label "126.00" appears twice (header and jog).
  - **Kittest `clicking_the_time_cycles_it`:** clicking the time label sends `View(CycleTime(deck 1))`.
- [ ] **Step 2:** Run. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test -p ui`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(ui): add the info jog, beat counter and time display modes`.

### Task 7: Resizable, foldable panels and the sideview

**Files:**
- Modify: `crates/ui/src/{lib.rs,browser.rs,layout.rs}`
- Create: `crates/ui/src/sideview.rs`
- Test: `crates/ui/src/layout.rs` tests, `crates/ui/tests/panels.rs`

**Interfaces:**
- Produces:
  - **Dividers:** the lanes, decks, tree, info and sideview areas sit in resizable egui panels.
    - When a drag ends (`response.drag_stopped()`), the root sends `View(SetSizes(sizes))` with each area's new share of the window.
    - Shares come from the stored `PanelSizes` each frame, so the session's copy is the one source.
  - **`ui::layout::clamp_sizes(sizes: PanelSizes, window: egui::Vec2) -> PanelSizes`:** each share is at least 0.05 and at least 80 points; lanes plus decks is at most 0.85; tree plus info plus sideview is at most 0.8.
  - **Folding:**
    - `ViewState.info_panel == false` hides the browser's info panel. A small "info" tab on the browser's right edge brings it back by sending `View(SetInfoPanel(true))`, like VirtualDJ's side tabs.
    - The sideview works the same way, with a "sideview" tab.
  - **`ui::sideview::show(ui, frame, monitor)`:** the sampler bank as 8 slots in a 2×4 grid. Each slot shows its number, lit when loaded (`ControlFrame.samples_loaded`) and brighter while playing (`EngineMonitor::sampler()`). It is display only.
  - A minimum window size of 1024×700 (`ViewportBuilder::with_min_inner_size`).

- [ ] **Step 1: Write the failing tests:**
  - **`sizes_are_clamped`:** shares of 0.0 and 2.0 come back within the limits above for a 1440×900 window and a 1024×700 window.
  - **Kittest `folded_info_shows_its_tab`:** with `info_panel` false, the label "info" is present and the info panel's track title is absent.
  - **Kittest `sideview_shows_the_sampler`:** with `sideview` true and slot 3 loaded, the labels "1" to "8" render inside the sideview.
- [ ] **Step 2:** Run. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test -p ui`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(ui): make the panels resizable and foldable and add the sideview`.

### Task 8: Pad-mode and per-deck FX dropdowns

**Files:**
- Modify:
  - `crates/engine/src/{command.rs,mixer.rs}`;
  - `crates/app/src/{config.rs,play.rs,session/commands.rs,session/frame.rs}`;
  - `crates/ui/src/{deck.rs,model.rs}`;
  - `docs/config.md`.
- Test: `crates/engine/tests/fx.rs` (or the existing FX test file), `crates/app/src/config.rs` tests, session tests, `crates/ui/tests/deck.rs`

**Interfaces:**
- Produces:
  - **`engine::ChannelCommand::FxSlot(slot: usize, kind: EffectKind)`** sets one channel's slot. `MasterCommand::FxSlot` stays and sets every channel, as today. Both are no-alloc: add a `no_alloc.rs` case.
  - **Config:**
    - `fx.deck1` to `fx.deck4` are optional arrays of 3 effect ids, overriding `fx.slots` for that deck. Errors read like `fx.slots`' (`config.toml: fx.deck2: expected exactly 3 effect ids, got 2`).
    - `Config::fx_slots_for(deck: DeckId) -> [EffectKind; 3]`.
    - `configure_engine` sends `ChannelCommand::FxSlot` per deck.
  - **`ControlFrame.fx_slots`** becomes `[[EffectKind; 3]; 4]`, per deck.
  - **Deck panel dropdowns:**
    - Each FX slot name is an egui `ComboBox` of every `EffectKind`. Choosing one sends `UiCommand::FxSlot { deck, slot, kind }`.
    - The session sends `ChannelCommand::FxSlot` and saves the deck's three kinds as `fx.deckN` through Task 9's writer. Until Task 9 lands, it only applies them: leave a `// Task 9 saves this` call site the next task fills.
    - The pad-mode name becomes a `ComboBox` of exactly: "pads (controller)" = `pads.mode_hardware`, "BEAT JUMP", "LOOP ROLL", "SLICER", "PAD SCRATCH". Choosing one sends `UiCommand::Action(Action::PadMode(side, mode), Input::Press)`. The hardware entry keeps showing the last hardware button's name as plan 9 does.

- [ ] **Step 1: Write the failing tests:**
  - **Engine `fx_slot_per_channel`:** after `ChannelCommand::FxSlot(0, Reverb)` on deck 2, only deck 2's slot 0 is reverb. The others keep echo.
  - **Config `fx_decks_parse_and_reject`:** `[fx]\ndeck2 = ["flanger","echo","reverb"]` gives `fx_slots_for(deck 2)` the flanger set and deck 1 the `fx.slots` default. `deck2 = ["echo"]` gives the error above. `deck5 = [...]` is an unknown key.
  - **Session `a_dropdown_fx_choice_reaches_the_engine`:** a `UiCommand::FxSlot` for deck 1 gives the next published frame `fx_slots[0][slot] == kind`.
  - **Kittest `the_pad_dropdown_offers_only_software_modes`:** opening the dropdown lists exactly the five entries above, and no "HOT CUE", "SAMPLER" or "FX FADE".
- [ ] **Step 2:** Run. Expected: FAIL.
- [ ] **Step 3:** Implement. Document `fx.deck1`–`fx.deck4` in `docs/config.md`.
- [ ] **Step 4:** `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `feat: choose pad modes and per-deck effects from the window`.

### Task 9: Settings registry and `config.toml` write-back

**Files:**
- Create: `crates/app/src/session/settings.rs`
- Modify:
  - `crates/app/src/config.rs`: expose a `parse_text(text, file) -> anyhow::Result<Config>` if one is not already public;
  - `crates/app/src/session/{commands.rs,frame.rs}`, `crates/ui/src/model.rs`;
  - `Cargo.toml` and `crates/app/Cargo.toml`: `toml_edit`.
- Test: unit tests in `settings.rs`

**Interfaces:**
- Produces:
  - **`ui::SettingRow`** (`Debug, Clone, PartialEq`):
    - `key: String`, such as `"stems.knobs"`;
    - `category: &'static str`, one of "Audio", "Controller", "Mixer", "Effects", "Sampler", "Pads", "Stems" or "Interface";
    - `value: String`, the current value as TOML text;
    - `default: String`;
    - `help: &'static str`, one line from `docs/config.md`;
    - `live: bool`;
    - `pending: bool`, true for a saved change that applies at next launch;
    - `error: Option<String>`.
  - **`ControlFrame.settings: Arc<Vec<SettingRow>>`**, published when it changes.
  - **`app::session::settings::Settings`:**
    - **`load(path: PathBuf, config: &Config) -> Self`.**
    - **`rows(&self) -> Vec<SettingRow>`:** one row per key in `docs/config.md`'s table, in its order.
      - Current keys: `mixer.crossfader`, `fx.slots`, `fx.deck1` to `fx.deck4`, `sampler.volume`, `sampler.pads`, `beat_jump.pads`, `beat_jump.shift_pads`, `loop_roll.pads`, `backspin.short`, `backspin.long`, `stems.enabled`, `stems.eq_mode`, `stems.knobs`, `stems.kicks_hz`, `stems.cache_gb`, `audio.device`, `audio.master`, `audio.headphones`, `controller.mapping`, `ui.waveform_beats`, `ui.waveform_min_beats`, `ui.waveform_max_beats`.
      - A test asserts the registry and the docs table list the same keys, so new keys can't be forgotten.
    - **`set(&mut self, key: &str, value: &str) -> Result<Applied, String>`:**
      - It edits the `config.toml` text with `toml_edit`: it sets or inserts the key in its table, keeping comments, order and formatting.
      - It re-parses the whole text with `config::parse_text`. On error, the file is untouched and the row's `error` is set to the parse error.
      - On success, it writes the file atomically on a short-lived thread and returns `Applied { config: Config, live: bool }`.
      - It accepts `value` as TOML value text (`"vdj"`, `300`, `[3, 4]`, `true`).
    - **`reset_all(&mut self) -> Result<Applied, String>`:** removes every key the registry knows from `config.toml`, keeping the user's comments and any unknown text.
  - **Live apply (session `commands.rs`):** after a live change, apply the difference between the old and new `Config`:
    - `mixer.crossfader` sends `MasterCommand::CrossfaderEnabled`;
    - `stems.knobs` sends `MasterCommand::StemKnobs`;
    - `stems.eq_mode` sends `MasterCommand::EqMode(startup_eq_mode())`;
    - `fx.*` sends `ChannelCommand::FxSlot` per deck;
    - `sampler.volume` sends `SamplerCommand::Volume`;
    - `beat_jump.*`, `loop_roll.pads` and `backspin.*` set the `ControlState` fields;
    - `ui.*` updates the zoom range and starting beats.
    A next-launch key sets `pending`. Task 8's FX dropdown now saves `fx.deckN` through `set`.
  - **`UiCommand::SetSetting` and `ResetSettings`** call these.
  - **Key notation is not a row.** The spec lists it among the live settings, but it lives in the library database (`library` command), not in `config.toml`, so it stays out of this registry (my call; a later plan can add library settings to the window).

- [ ] **Step 1: Write the failing tests:**
  - **`edits_keep_comments`:** given a file with `# my setup` comments, a `[stems]` table and an unrelated `[mixer]` key, `set("stems.knobs", "\"raw\"")` changes only that value. Every other line is byte-identical. The comment lines survive.
  - **`a_new_key_is_inserted_in_its_table`:** `set("ui.waveform_beats", "24")` on a file without `[ui]` adds a `[ui]` table with that key.
  - **`a_bad_value_is_refused`:** `set("stems.kicks_hz", "10")` returns `Err` containing `stems.kicks_hz: expected a number from 50 to 1000`, and the file bytes are unchanged.
  - **`live_and_next_launch`:** `stems.knobs` gives `live == true`. `audio.device` gives `live == false`, and its row becomes `pending`.
  - **`reset_all_removes_known_keys`:** after reset, the parsed config equals `Config::default()`, and a `# keep me` comment is still there.
  - **`registry_matches_the_docs`:** the set of registry keys equals the keys in `docs/config.md`'s table, read with `include_str!`.
- [ ] **Step 2:** Run. Expected: FAIL.
- [ ] **Step 3:** Implement. Writes go to a temp path in tests only. The session uses the config file in use (`--config` or the default).
- [ ] **Step 4:** `cargo test --workspace && cargo deny check`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(session): edit config.toml from the window, keeping its comments`.

### Task 10: The settings window

**Files:**
- Create: `crates/ui/src/settings/{mod.rs,options.rs,interface.rs}`
- Modify: `crates/ui/src/{lib.rs,topbar.rs,model.rs}`, `crates/app/src/session/frame.rs`
- Test: unit tests in `options.rs`, `crates/ui/tests/settings.rs`

**Interfaces:**
- Consumes: `ControlFrame.settings` (Task 9), `ViewState` (Task 1), `UiCommand` (Task 1).
- Produces:
  - **The gear button** in the top bar (painted, not a font glyph) opens an `egui::Window` titled "Settings", about 70 % of the window, closable. Its sidebar has:
    - **Audio:** the `audio.*` rows, plus a device dropdown filled from `ControlFrame.devices: Vec<String>`. The session fills that list once at start with `audio_io::list_output_devices()`, on its own thread.
    - **Interface:** the four layouts as tiles. Each tile is a small painted sketch of its sections from `layout::sections`, with its name. Clicking one sends `View(SetLayout(..))`.
    - **Controllers:** the `controller.mapping` row and a dropdown of `controller::builtin::builtin_names()`.
    - **Options:**
      - a search box;
      - one row per setting: key, value editor and help text; "applies at next launch" when `pending`; the `error` in the error colour;
      - "Show modified", a toggle filtering to `value != default`;
      - "Reset all", with a confirm dialog, which sends `ResetSettings`.
    - **Later:** a stub listing what will come (recording, broadcast, automix), with no controls.
  - **`ui::settings::options::matches(row: &SettingRow, query: &str, modified_only: bool) -> bool`:** case-insensitive substring match on key, category or help. With `modified_only`, also `value != default`.
  - **Value editors:** a checkbox for booleans; a `ComboBox` for enumerated keys (`stems.knobs`: vdj or raw, `stems.eq_mode`: eq or stems, `fx.*` entries: effect ids); a text field otherwise, sent on Enter or focus loss. Each edit sends `UiCommand::SetSetting { key, value }`.

- [ ] **Step 1: Write the failing tests:**
  - **`search_and_modified_filter`:** the query "knob" matches `stems.knobs`. "STEMS" matches every stems row. `modified_only` with value equal to default does not match.
  - **Kittest `settings_window_opens_on_options`:** click the gear, then "Options", and the rows `stems.knobs` and `audio.device` are listed. Typing "kicks" in search leaves only `stems.kicks_hz`.
  - **Kittest `editing_a_setting_sends_it`:** choosing "raw" in `stems.knobs`' dropdown sends `SetSetting { key: "stems.knobs", value: "\"raw\"" }`.
  - **Kittest `an_error_shows_on_its_row`:** a row with `error: Some("config.toml: stems.kicks_hz: expected a number from 50 to 1000")` shows that text.
  - **Kittest `interface_tiles_switch_layouts`:** clicking the "Performance" tile sends `View(SetLayout(Performance))`.
- [ ] **Step 2:** Run. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(ui): add the settings window`.

### Task 11: Docs and the owner's hands-on

**Files:**
- Modify: `docs/ui.md`, `docs/config.md`, `README.md`, `docs/architecture.md`, `controllers/ddj-sb3/README.md`

- [ ] **Step 1:** Update `docs/ui.md`:
  - the LAYOUT menu and every entry;
  - the four layouts;
  - 4-Decks;
  - waveform styles and position;
  - jog display, beat counter and time cycling;
  - dragging and folding panels, and the sideview;
  - the dropdowns and why the pad dropdown offers only the software modes;
  - the settings window: live and next-launch settings, comments kept;
  - `ui-state.toml` and what it holds.

  Update `docs/architecture.md` for the window → session command path, and `docs/config.md` for `fx.deck1`–`fx.deck4` and the note that the window can edit the file.
- [ ] **Step 2:** `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo deny check`. Expected: PASS.
- [ ] **Step 3: Commit** `docs: document the layouts and the settings window`.
- [ ] **Step 4: Owner hands-on** (`cargo run --release`, SB3 connected):
  1. Each layout from the LAYOUT menu and from a mapped `ui.layout` button (map it in a user mapping with `extends`).
  2. The light scheme, each waveform style in both positions, overview off.
  3. The info jog, the beat counter, and clicking the time display.
  4. 4-Decks with deck 3 on the left side: the right panels follow.
  5. Drag every divider; fold and unfold the info and sideview panels; restart, and everything is as left.
  6. Choose a pad mode and an effect from the dropdowns; the SB3 then acts on them.
  7. In Settings, change `stems.knobs` (live) and `audio.device` (next launch). `config.toml` keeps its comments. A bad value shows its error and changes nothing.
- [ ] **Step 5:** Record the results in `controllers/ddj-sb3/README.md` ("Hands-on check (plan 10)"), apply requested tweaks, and commit.
