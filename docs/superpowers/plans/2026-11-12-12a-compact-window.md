# Plan 12a: Compact Window and Waveform Colours Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Open Jockey's window read like VirtualDJ's Pro layout. Sections touch,
each is as tall as its contents, the mixer is compact, and the browser reads like a file
manager. Waveforms draw one colour per frequency band, on top of each other, from three
palettes.

**Architecture:** The work stays in the `ui` crate's drawing code. The engine, actions and
session are untouched, except that the session stores one new view setting (the waveform
palette) the way plan 10 stores the others. A full-window render harness (egui_kittest with
wgpu) draws the window with two loaded decks to PNG files. The owner compares those
renders with VirtualDJ partway through.

**Tech Stack:** Rust, egui/eframe 0.37, egui_kittest 0.37 (adding its `wgpu` and
`snapshot` features as a dev-dependency).

**Spec:** `docs/superpowers/specs/2026-11-12-vdj-feel-design.md`. Goals 4 and 5 belong
here, plus the jog and header information. Goals 1, 2, 3 and 6 (mouse control) are plan
12b.

## Global Constraints

- Everything from the project still holds:
  - mise, prek, Conventional Commits (commitizen);
  - `cargo clippy --all-targets -- -D warnings`, `cargo fmt`, `cargo deny check`;
  - builds and tests on macOS, Windows and Linux (CI runs all three).
- "A pixel copy of VirtualDJ's skin, logos or exact artwork" is a non-goal: copy the
  arrangement and the feel, not its branding.
- "The waveform colours and the compact layout must not drop frames on the owner's MacBook.
  The plan 9 frame rules still hold: no allocation per frame in the drawing paths that
  already avoid it." (`styles::bars` stays allocation-free.)
- "The layouts, colour scheme, panel sizes and folding from plan 10 stay. The compact look
  replaces how sections are drawn, not the layout system." Starter, Essentials, Pro and
  Performance, the 4-deck view, the light scheme, the dividers and folding all keep working.
- "No regression for the controller": nothing the DDJ-SB3 does changes. The ghost markers
  still draw.
- The window still never acts on the engine directly and sends only `UiCommand`s.
- Tests never touch the owner's real config, data, music or `/Volumes`. Renders are written
  under `target/`.
- The palettes, exact values (`0xRRGGBB`):

  | Palette (id) | Low | Mid | High |
  |---|---|---|---|
  | `rgb` (default) | `0xE8322A` | `0x38C24A` | `0x3D8BFF` |
  | `blue_orange_white` | `0x1E64FF` | `0xFFA21E` | the palette's `wave_high` (white when dark, dark grey when light) |
  | `orange_yellow_cyan` | `0xFF4D1A` | `0xFFD23D` | `0x5EE6FF` |

## Review Focus

1. **A deck with no track, or a track with no waveform yet** (analysis pending). The compact
   panels and the three-layer waveform draw without gaps or panics, and the deck keeps its
   content height. Test: Task 3 `empty_decks_keep_their_height`.
2. **The window at its minimum size (1024×700) in every layout, and in the 4-deck view.**
   Nothing overlaps or is clipped, and the browser still shows at least 3 rows. Test: Task 6
   `every_layout_fits_the_smallest_window`.
3. **The light colour scheme.** Section shades and band colours stay readable, and the
   white highs of `blue_orange_white` use the dark scheme's `wave_high` only on dark. Test:
   Task 1 `blue_orange_white_highs_follow_the_scheme`.
4. **A user who dragged a divider before this plan.** Their saved deck share still applies.
   Only the unset default changes to "fit the contents". Test: Task 3
   `a_saved_deck_share_still_applies`.
5. **Long titles, artists and playlist names, and non-ASCII text.** They are cut with "…"
   and stay left-aligned in the tree, the list and the deck header. Test: Task 5
   `long_names_are_cut_and_left_aligned`.

---

## Decisions this plan makes (my calls, within the spec's "left to the builder")

- **The waveform palette** is a view setting saved in `ui-state.toml` like the waveform
  style. It is chosen in LAYOUT → Waveform Display (under the styles) and in Settings →
  Interface. Every per-band drawing uses it: the Frequency and Bands styles, the deck
  overview and the sideview. Deck colour and Overlaid keep the decks' accents.
- **"Frequency" becomes the three layers.** Each band is a bar mirrored around the lane's
  centre, reaching `level/255` of the half height. They are drawn low, then mid, then high,
  each opaque, so the louder high-frequency content shows in front. The peak no longer
  scales brightness. The old single blended colour (`colour::colour`) goes, and its two
  unit tests go with it.
- **Sections are separated by a 2-point gap** showing `palette.background` between
  `palette.panel` areas. There are no strokes around panels, mixer strips, the tree, the list
  or inactive buttons (`visuals.widgets.{noninteractive,inactive}.bg_stroke = Stroke::NONE`).
  Hovered and active buttons keep egui's feedback.
- **The active deck** is marked by a 4-point accent bar on the panel's outer edge, like
  VirtualDJ's A/B tab. This replaces the `ACTIVE_BORDER` outline.
- **Edge labels:** a 14-point strip on a section's outer edge holds its name written
  vertically: FX, PADS, LOOP. It is 9-point `text_dim`, and replaces any title above the
  section.
- **The deck header** keeps its contents, and the track overview moves inside it, under the
  title and artist, to the right of the cover. The header stays `COVER_SIDE` (64) tall, so the
  separate overview row is gone.
- **The jog's info display becomes the default** (`JogDisplay::Info`). A user who saved
  "plain" keeps it.
- **The deck row's default height is its contents' height** (`deck::row_height`), so the
  browser gets every point left over. This replaces the layouts' fixed `decks_share` default.
- **The search box** above the list belongs with search, in plan 12b. 12a only keeps the
  list's top row (path and file count) as it is.

## File structure

```
crates/ui/src/view.rs            WaveformPalette, ViewState.waveform_palette, ViewChange::SetWaveformPalette, JogDisplay default
crates/ui/src/waveform/colour.rs band colours per palette (replaces the blended colour)
crates/ui/src/waveform/styles.rs Frequency = three layers; Bands uses the palette
crates/ui/src/deck.rs            overview columns via the palette; compact panel; header with overview; edge labels; accent tab
crates/ui/src/sideview.rs        palette colours
crates/ui/src/menu.rs            Waveform Display → palette entries
crates/ui/src/settings/interface.rs  palette choice row
crates/ui/src/theme.rs           no strokes, section gap, row_alt, edge_label()
crates/ui/src/layout.rs          default deck share = content height
crates/ui/src/mixer.rs           compact VirtualDJ arrangement
crates/ui/src/browser.rs         left-aligned cells, column headers, alternating rows, loaded markers
crates/ui/src/lib.rs             root_with_reads() (root calls it)
crates/ui/tests/render.rs        full-window renders to target/ui-render/*.png (ignored test)
crates/ui/tests/support/mod.rs   shared fixtures: loaded decks with synthetic waveforms
crates/app/src/session/*         persist waveform_palette (as waveform_style is)
docs/ui.md, README.md            the new look and the palette setting
```

---

### Task 1: Waveform palettes and three-layer waveforms

**Files:**
- Modify: `crates/ui/src/view.rs`, `crates/ui/src/waveform/colour.rs`,
  `crates/ui/src/waveform/styles.rs`, `crates/ui/src/deck.rs` (`overview_columns`),
  `crates/ui/src/sideview.rs`, `crates/ui/src/menu.rs`,
  `crates/ui/src/settings/interface.rs`.
- Modify: where the session loads and saves `ui-state.toml` and applies `ViewChange`s. Find
  it by `SetWaveformStyle` in `crates/app/src/session/`.
- Test: unit tests in `colour.rs`, `styles.rs` and `view.rs`; `crates/ui/tests/menu.rs` and
  `settings.rs`; the session's view-state test next to the waveform-style one.

**Interfaces:**
- Produces:
  - `pub enum WaveformPalette { #[default] Rgb, BlueOrangeWhite, OrangeYellowCyan }` in
    `view.rs`, with `ids!` `"rgb"`, `"blue_orange_white"`, `"orange_yellow_cyan"`.
  - `ViewState.waveform_palette: WaveformPalette`.
  - `ViewChange::SetWaveformPalette(WaveformPalette)`.
  - `pub fn band_colours(palette: WaveformPalette, scheme: &Palette) -> [Color32; 3]` in
    `colour.rs`, returning `[low, mid, high]`.
  - `styles::column` and `styles::bars` gain a `bands: [Color32; 3]` parameter; the callers
    compute it once per frame.
  - The saved key is `waveform_palette` in `ui-state.toml`, next to `waveform_style`.

- [ ] **Step 1: Write the failing tests.**
  - `colour.rs`:
    - `rgb_is_the_default_and_matches_the_spec`: `band_colours(WaveformPalette::default(),
      &Palette::DARK)` is `[0xE8322A, 0x38C24A, 0x3D8BFF]` as RGB.
    - `blue_orange_white_highs_follow_the_scheme`: the high is `Palette::DARK.wave_high` on
      dark and `Palette::LIGHT.wave_high` on light; low and mid are `0x1E64FF` and
      `0xFFA21E`.
    - `orange_yellow_cyan_values`.
  - `styles.rs`:
    - `frequency_draws_three_layers_back_to_front`: bucket `[255, 200, 100, 50]` with rgb
      bands gives 3 bars in the order low, mid, high. Each bar is centred on 0.5, with
      reaches 200/255, 100/255 and 50/255 of 0.5, in the band colours.
    - `frequency_skips_silent_bands`: `[255, 0, 80, 0]` gives 1 bar (mid).
    - `bands_use_the_palette`: the Bands style's bars use `band_colours`.
    - Replace `frequency_matches_plan_9` with these.
  - `view.rs`: `waveform_palette_ids_round_trip` for all three ids; an unknown id gives
    `None`.
  - `menu.rs` kittest, `waveform_display_lists_the_palettes`: the Waveform Display submenu
    shows "Red / green / blue", "Blue / orange / white" and "Orange / yellow / cyan". A click
    sends `ViewChange::SetWaveformPalette(..)`.
  - `settings.rs` kittest: Settings → Interface has a "Waveform colours" row with the same
    three names, and choosing one sends the change.
  - Session test, `waveform_palette_is_saved_and_restored` (mirror the waveform-style test):
    `ui-state.toml` gets `waveform_palette = "orange_yellow_cyan"` and loads back.

- [ ] **Step 2: Run them and see them fail**
  (`cargo test -p ui` and the app's session test by name).
- [ ] **Step 3: Implement.** The deck overview and the sideview colour their columns from
  `band_colours` with the same three-layer rule. Remove `colour::colour` and its constants
  once nothing uses them.
- [ ] **Step 4: Run `cargo test -p ui -p open-jockey`; it passes.**
- [ ] **Step 5: Commit** `feat(ui): one colour per frequency band, from three palettes`

---

### Task 2: A full-window render for the owner's checks

**Files:**
- Modify: `crates/ui/src/lib.rs`, plus `deck.rs`, `mixer.rs` and `waveform/lane.rs` where they
  read the monitor.
- Modify: `crates/ui/Cargo.toml`, where dev-dependency `egui_kittest` gains
  `features = ["wgpu", "snapshot"]`.
- Create: `crates/ui/tests/support/mod.rs`, `crates/ui/tests/render.rs`.
- Modify: `crates/ui/tests/fit.rs`, so it uses `support`'s fixture instead of its own `info()`
  and `loaded()`.

**Interfaces:**
- Produces:
  - `pub fn root_with_reads(ui: &mut egui::Ui, frame: &ControlFrame, reads: &deck::DeckReads,
    screen: &mut Screen)`. It draws the whole window from `reads`, the same `DeckReads` the
    panels already take. `root` becomes `root_with_reads(ui, frame,
    &DeckReads::read(monitor), screen)`.
  - Any part that still needs the monitor gets what it reads from `DeckReads`. If the lanes
    need more (their playheads), add those fields to `DeckReads` and fill them in
    `DeckReads::read`.
  - `support::loaded(layout: Layout, four_decks: bool) -> (ControlFrame, DeckReads)`: two (or
    four) loaded, playing decks. They have:
    - covers absent;
    - the current `fit.rs` titles and remixes;
    - a key and a grid at 126 BPM;
    - hotcues in slots 1–3, one of them a saved loop;
    - a synthetic `overview` and `detail` waveform (see Step 3);
    - a browser view with 30 rows of tracks, the tree showing drives and playlists, and deck
      1's track among the rows.
  - `support::render(frame, reads, size: Vec2) -> image::RgbaImage`, using
    `Harness::builder().with_size(size).wgpu().build_ui(...)` and `harness.render()`.

- [ ] **Step 1: Write `render.rs`.** It contains one `#[ignore]` test, `render_every_layout`.
  For Starter, Essentials, Pro and Performance at 1440×900, and Pro with four decks, it
  writes `target/ui-render/<layout>.png` (`pro-4decks.png` for the four-deck one), and at
  1024×700 `target/ui-render/pro-small.png`. It asserts each image is the window's size, not
  blank, and has more than 50 distinct colours.
- [ ] **Step 2: Run** `cargo test -p ui --test render -- --ignored`. It fails until the
  harness exists.
- [ ] **Step 3: Implement `root_with_reads` and `support`.** The synthetic waveform is
  deterministic, with no randomness. Every 16th bucket is a kick (low 230). Hats come every
  4th bucket (high 160). Mids are steady around 110. There is a quieter breakdown from 55 %
  to 65 % of the track, with all bands × 0.3. The peak is the bands' maximum.
- [ ] **Step 4: Run the render test** with `-- --ignored`; it passes and writes the ONGs.
  Run `cargo test -p ui` as well; the fit tests pass with the shared fixture. CI does not
  run ignored tests, so it needs no GPU.
- [ ] **Step 5: Check** `cargo deny check` is still clean with the new dev-features (add a
  license exception only for a permissive license, with a comment). **Commit** `test(ui):
  render the whole window to PNG for visual checks`.

---

### Task 3: The compact look, and decks as tall as their contents

**Files:**
- Modify: `crates/ui/src/theme.rs`, `crates/ui/src/deck.rs`, `crates/ui/src/layout.rs`,
  `crates/ui/src/lib.rs` (the row panels' gaps), `crates/ui/src/view.rs` (`JogDisplay`
  default).
- Test: `crates/ui/tests/deck.rs`, `crates/ui/tests/panels.rs`, `crates/ui/tests/fit.rs`,
  and unit tests in `theme.rs` and `layout.rs`.

**Interfaces:**
- Consumes: `support::loaded` (Task 2).
- Produces:
  - `theme::SECTION_GAP: f32 = 2.0`;
  - `Palette.row_alt: Color32`, a shade between `background` and `panel` for every other
    list row;
  - `pub fn edge_label(ui: &Ui, strip: Rect, text: &str)`, which draws `text` vertically,
    centred in `strip`;
  - `theme::EDGE_LABEL_WIDTH: f32 = 14.0`.
  - `layout::drawn_sizes` gives the deck row `deck::row_height(sections) / window.y` while
    the saved share is the unset default (`PanelSizes::default().decks`). A set share
    applies as before.

- [ ] **Step 1: Write the failing tests.**
  - `theme.rs`: `no_strokes_on_panels_and_idle_widgets`. After `theme::apply`, the
    `noninteractive` and `inactive` widgets' `bg_stroke` is `Stroke::NONE`.
  - `layout.rs`:
    - `default_deck_row_fits_its_contents`: for every layout at 1440×900, the deck row's
      drawn height equals `deck::row_height(&sections)` within 1 point.
    - `a_saved_deck_share_still_applies`: a set `decks` share of 0.5 draws at half the
      window.
  - `panels.rs`: `spare_height_goes_to_the_browser`. In Pro at 1440×900, the browser's
    height equals the window minus the top bar, the lanes and the deck row, within 2 points.
  - `deck.rs`:
    - `overview_sits_in_the_header`: the overview's rect lies inside the header's rect, to
      the right of the cover.
    - `active_deck_has_an_accent_tab_not_a_border`: the active deck's panel has a 4-point
      accent rect on its outer edge, and the panel has no stroke.
    - `edge_labels_name_the_sections`: the labels "FX", "PADS" and "LOOP" exist in a loaded
      Pro deck. Query them by text via kittest.
    - `jog_shows_info_by_default`: with `ViewState::default()`, the jog draws the BPM text.
    - `empty_decks_keep_their_height`: the row height is the same with no tracks loaded.
  - `fit.rs` (existing): it must still pass for every layout, at both window sizes.

- [ ] **Step 2: Run them and see them fail. Step 3: Implement.** Panels are filled with
  `palette.panel` on the background, with `SECTION_GAP` between neighbours. Drop the outlined
  `Frame`s and the active border. Lay the deck panel out as the spec's reference: the header
  strip with the overview inside, then FX, jog and pitch with the states, then the pads with
  the transport and the loop. Use the edge labels on FX, PADS and LOOP. Keep every control
  that is there today.
- [ ] **Step 4: Run `cargo test -p ui`; it passes.** Run the render (`-- --ignored`) and
  look at `pro.png` yourself. Compare it with the spec's "How VirtualDJ does it" and fix
  obvious gaps.
- [ ] **Step 5: Commit** `feat(ui): compact deck panels that fit their contents`

---

### Task 4: The compact mixer

**Files:**
- Modify: `crates/ui/src/mixer.rs` (`strip_layout`, `strip` and the bottom row), and
  `crates/ui/src/layout.rs` (`mixer_order` if needed).
- Test: `crates/ui/tests/mixer.rs` and `crates/ui/tests/fit.rs`.

**Interfaces:**
- Consumes: the theme items from Task 3.
- Produces: a new `MixerLayout`, `pub fn mixer_layout(rect: Rect, eq: bool, meters: bool) ->
  MixerLayout`, which replaces `StripLayout` and `strip_layout`. Each side's part is
  `[SideLayout; 2]` (left, right) with these fields:
  - `knobs: Vec<Pos2>`: the centres, top to bottom, in the order gain, then the three EQ
    knobs, then the filter;
  - `fader: Rect`;
  - `meter: Option<Rect>`;
  - `pfl: Rect`;
  - `header: Rect`.

  Shared parts:
  - `centre_meter: Option<Rect>`;
  - `crossfader: Rect`;
  - `master: Rect`.

- [ ] **Step 1: Write the failing tests** (in `mixer.rs`, against `mixer_layout` for a 360×300
  rect, and a kittest of the drawn mixer):
  - `faders_sit_in_the_middle_band`: both faders' rects lie within the middle 40 % of the
    mixer's width, left fader left of centre and right fader right of it.
  - `knobs_sit_outside_their_fader`: every left knob's x is less than the left fader's
    `min.x`, and every right knob's x is greater than the right fader's `max.x`.
  - `gain_is_on_top`: the first knob of each side is the highest.
  - `pfl_under_the_faders`: each `pfl` lies under its fader (its `min.y` ≥ the fader's
    `max.y`).
  - `crossfader_at_the_bottom_and_master_on_top`: the crossfader is the lowest element and
    centred; `master` is in the top row.
  - `centre_meter_between_the_faders`, when meters are shown.
  - `ghosts_still_draw` (kittest): with a trim ghost set, a hollow marker still draws on the
    left gain knob. Reuse the existing ghost test's method.
  - The existing `fit.rs` tests still pass, so the mixer fits the deck row.
- [ ] **Step 2: Run them and see them fail. Step 3: Implement**, with no outlines around the
  strips; the mixer is one `panel` area. **Step 4: Run** `cargo test -p ui`, plus the render
  and a look at `pro.png`.
- [ ] **Step 5: Commit** `feat(ui): a compact mixer with the faders in the middle`

---

### Task 5: The browser reads like a file manager

**Files:**
- Modify: `crates/ui/src/browser.rs`.
- Test: `crates/ui/tests/browser.rs`.

**Interfaces:**
- Consumes: `Palette.row_alt` (Task 3).
- Produces:
  - `cell` draws its text left-aligned at `x`, not centred: it uses a left-to-right child
    `Ui` or a left-aligned `Label` in a sized `UiBuilder`;
  - `pub const COLUMNS: [&str; 6] = ["Title", "Artist", "Remix", "Length", "BPM", "Key"];`.

- [ ] **Step 1: Write the failing tests.**
  - `tree_labels_are_left_aligned`: each tree label's `rect.min.x` equals the row's indent
    x (`4 + depth·INDENT + 28`) within 1 point, whatever the pane's width.
  - `list_has_column_headers`: a header row shows the six `COLUMNS` labels. Each one is
    left-aligned with its column's cells, within 1 point.
  - `rows_alternate_shades`: in a list of 6 rows, rows 1, 3 and 5 (0-based) are filled with
    `row_alt`. Read the painted shapes via the harness, or test the row-fill helper as a
    unit.
  - `loaded_tracks_are_marked`: the row whose track id is deck 1's has a 3-point accent
    marker in deck 1's colour at its left edge. The marker is deck 2's colour for deck 2's
    track.
  - `long_names_are_cut_and_left_aligned`: a 200-character title and a playlist name
    with "é" and "🎵" end in "…", inside their cells, left-aligned.
  - The existing browser tests still pass.
- [ ] **Step 2: Run them and see them fail. Step 3: Implement.** Keep the path and file
  count row above the column headers. The panes have no outlines, and the tree, list, info
  and sideview are separated by `SECTION_GAP` on the background. **Step 4: Run** `cargo test
  -p ui`, then the render.
- [ ] **Step 5: Commit** `feat(ui): a left-aligned browser with column headers`

---

### Checkpoint: the owner compares the renders with VirtualDJ

Not a coding task. The controller stops here and asks the owner. This is one of the stops
the subagent-driven process allows: a decision only the owner can make.

1. Run `cargo test -p ui --test render -- --ignored`.
2. Give the owner the paths of `target/ui-render/pro.png`, `pro-small.png` and
   `pro-4decks.png`, and ask them to open them next to VirtualDJ's Pro layout.
3. Write the owner's remarks into the ledger as a list. Task 6 starts with them.

---

### Task 6: The other layouts, the owner's remarks, and docs

**Files:**
- Modify: whatever the checkpoint's remarks touch.
- Modify: `crates/ui/src/deck.rs` and `layout.rs` for the compact rules in Starter,
  Essentials and Performance and the 4-deck view.
- Modify: `docs/ui.md` (the look, edge labels, the palette setting, the jog default),
  `README.md` (the feature list line on waveforms).
- Test: `crates/ui/tests/fit.rs`, `four_decks.rs`, `panels.rs`.

**Interfaces:**
- Consumes: everything above.

- [ ] **Step 1: Write the failing tests** for each remark that can be tested. Then:
  - `every_layout_fits_the_smallest_window`: every layout, with two and with four decks, at
    1024×700. The deck row fits its contents. The browser shows at least 3 list rows. No two
    sections' rects overlap by more than `SECTION_GAP`.
  - `light_scheme_keeps_section_shades`: under `ColourScheme::Light`, `panel` and
    `background` differ by at least 8 in luminance, and `row_alt` lies between them.
- [ ] **Step 2: Run them and see them fail. Step 3: Implement** the remarks and the other
  layouts. **Step 4: Run** `cargo test --workspace` and `cargo clippy --all-targets -- -D
  warnings`, re-render every layout, and look at each PNG.
- [ ] **Step 5: Commit** `feat(ui): the compact look in every layout` and `docs: describe
  the compact window and waveform palettes`.

---

## Hands-on (the owner, after the final review)

1. Run the app (Pro layout). Load two tracks with the controller, or with the console's
   `load` in `play --library`. Compare with VirtualDJ on these points:
   - **Sections:** they touch and nothing is empty.
   - **Mixer:** the faders are in the middle.
   - **Browser:** it is left-aligned, with column headers and alternating rows.
   - **Deck:** the overview sits in the header, and the jog shows BPM and time.
2. Switch the waveform palette in LAYOUT → Waveform Display, and in Settings → Interface.
   Check the bass, mids and highs read apart in each palette. Restart, and the choice is
   kept.
3. Try Starter, Essentials and Performance, the 4-deck view, and the light scheme. Drag the
   dividers.
4. Play a set with the SB3 for a few minutes. The waveforms stay smooth and the controller
   works as before.
5. Record the result in `controllers/ddj-sb3/README.md`.
