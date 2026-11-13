# Plan 12c: Waveforms That React, a Library Keyboard, and Covers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the gaps from the plan 12b hands-on:
- the waveforms show the held roll or slicer's landing point and every active loop, and they
  follow the EQ, stem and gain knobs;
- the library works from the keyboard, with a context-aware Delete that always asks first and
  only moves files to the Trash;
- tracks get covers looked up online and kept in the library.

**Architecture:**
- **Waveform feedback** is drawing work in `ui::waveform` and the deck overview, from data the
  frame already has: the knob values and `DeckSnapshot`'s `ghost_frames`, `roll_active`,
  `slicer_block_beat` and the loop fields.
- **Keyboard** handling lives in `ui::browser` and sends the existing `LibraryStep`s plus a few
  new ones.
- **Deleting files** is a new library-mode job, using the `trash` crate.
- **Covers** are a new app-owned lookup worker that writes files into a `covers` folder and a
  new `covers` table (schema V4). The frame carries each loaded or highlighted track's stored
  cover path. The window's `Covers` loader reads the embedded picture first, then that file.

**Tech Stack:** Rust; egui 0.37; the existing `ureq` 3.5.1 (native-tls), as `stems` uses it; the
`trash` crate (new, MIT or Apache); `serde_json` for the cover services' JSON if it is not
already a dependency (check `cargo tree`).

**Spec:** `docs/superpowers/specs/2026-11-13-12c-waveform-keyboard-covers-design.md`. Its
"Decisions after the builder check" section is binding.

## Global Constraints

- Everything from the project still holds:
  - mise, prek, Conventional Commits;
  - `cargo clippy --all-targets -- -D warnings`, `cargo deny check`;
  - builds and tests on macOS, Windows and Linux.
- "Never delete outside the library folders, never permanently, and never without a
  confirmation." The check compares canonical paths (symlinks resolved). A failed Trash move
  deletes nothing and says "<drive> has no Trash: nothing was deleted".
- "Never send anything but artist and title to a cover service." Without tags, send the file
  name without its extension. Never send the owner's name, email or a path.
- "A slow or offline cover service must never slow the app": no lookup on the control or UI
  thread.
- "The waveform scaling must not drop frames. It is a per-frame multiply, with no allocation."
  This is plan 9's rule.
- The window only sends `UiCommand`s. Library writes go through the writer.
- Tests never touch the owner's files, the network or the real Trash. The cover service and
  the Trash are behind traits with fakes in tests.

## Review Focus

1. **Deleting a folder that holds the track playing on a deck, or a library root.** The delete
   is refused before the confirmation can be accepted, and nothing moves. Test: Task 4
   `a_folder_holding_a_loaded_track_cannot_be_deleted` and `a_library_root_cannot_be_deleted`.
2. **A symlink inside a library folder pointing outside.** Deleting through it is refused.
   Test: Task 4 `a_link_out_of_the_library_is_refused` (unix).
3. **Typing in the search box, or a dialog's name field, while the browser has the
   keyboard.** Letters go to the field, and Delete or Backspace edits text and never deletes
   tracks. Test: Task 3 `delete_in_the_search_box_edits_text`.
4. **The cover service offline, slow or returning garbage.** Nothing blocks, the track keeps
   its tile, and the miss is recorded once. Test: Task 6
   `an_unreachable_service_records_a_miss_and_never_blocks`.
5. **Knobs at their extremes, and a gain boost.** A band at 0 disappears, and a boost never
   draws outside the lane. Test: Task 1 `boosted_bands_stay_inside_the_lane`.

---

## Decisions this plan makes (my calls)

- **Band scale.**
  - Each band's scale is `engine::mixer::eq_gain(knob)` times `engine::mixer::trim_gain(gain)`,
    with the drawn height clamped to the lane. At knob 0, `eq_gain` is a kill, so the band
    disappears.
  - In stems mode the same three knob positions are used (top, middle and bottom), as the
    spec says.
  - The scale applies to the scrolling lanes and the deck overview, not the sideview.
- **Landing marker.**
  - It is a downward triangle, 8 pt wide, at the lane's top edge in the deck's accent, plus a
    1-pt vertical line through the lane at 50 % alpha.
  - It shows while `ghost_frames` is `Some`.
  - On the overview, the same triangle sits over the strip.
- **Loop shading.** The `loop_band` shading is drawn for:
  - a normal loop (`loop_active` with `loop_in` and `loop_out`);
  - a loop roll (`roll_active`, whose span is the same loop fields);
  - a slicer block (`slicer_block_beat`, one block wide from the grid).
- **Keyboard ownership.**
  - The browser owns the keyboard after a click inside the tree or list, until a click
    elsewhere or a text field takes focus. It is stored as an egui memory flag keyed on the
    browser.
  - The focused pane draws its cursor row with `highlight`, and the other pane with
    `highlight_dim`. That shading already exists; it now follows keyboard ownership too.
- **Select all** sends `LibraryStep::Browse(BrowserEdit::SelectAll)`. The library resolves it
  as "every row of the current list". Delete or drag of it act on all rows through the
  library. The window shows "N selected" from the view.
- **The delete confirmation** is a `egui::Modal`, like the playlist dialogs, with the click
  outside swallowed. Its text:
  - "Remove N tracks from <playlist>? The files stay."
  - "Move <file> to the Trash?" or "Move N files to the Trash?"
  - "Delete the playlist <name>? Its tracks stay in the library."
  - "Move the folder <name> and its N files to the Trash?"

  A refusal replaces the buttons with the reason and OK:
  - "<file> is loaded on deck N: unload it first."
  - "<folder> holds a library folder: remove it from the library first."
  - "<path> is outside the library folders."
- **Cover services.**
  - Deezer's public search first (`https://api.deezer.com/search?q=artist:"…" track:"…"`, no
    key), then MusicBrainz recording search with the Cover Art Archive front image. MusicBrainz
    needs a User-Agent of `OpenJockey/<version> ( https://github.com/Roxxas96/open-jockey )` and
    at most 1 request per second.
  - Images are kept at most 512 px on their longest side and saved as JPEG.
  - The setting is `library.cover_lookup` (bool, default `true`) in the existing settings
    registry, on Settings → Library, with a "Retry missing covers" button.

## File structure

```
crates/ui/src/waveform/{styles.rs, lane.rs}   band scale input; slip marker; loop shading for roll/slicer
crates/ui/src/deck.rs                          overview: band scale, slip marker, loop shading
crates/ui/src/browser.rs                       keyboard handling, select all, focus look, delete key
crates/ui/src/delete_dialog.rs (new)           the confirmation modal
crates/ui/src/command.rs                       LibraryStep::{Delete…}, BrowserEdit::SelectAll
crates/library/src/browser/mod.rs              SelectAll; resolve "all rows" for delete/drag
crates/library/src/schema.rs                   V4: covers table
crates/library/src/covers.rs (new)             covers table access + WriteOp::{SetCover, MissCover, ClearCover}
crates/app/src/library_mode/{delete.rs (new), worker.rs, mod.rs}   delete jobs (Trash trait)
crates/app/src/covers/{mod.rs, service.rs} (new)   lookup worker, CoverService trait, Deezer + MusicBrainz
crates/app/src/session/{frame.rs, settings.rs}  cover paths into the frame; the setting
crates/ui/src/cover.rs                          load a stored cover file after the embedded picture
docs/ui.md, docs/library.md, README.md
```

---

### Task 1: The waveform follows the knobs

**Files:**
- Modify: `crates/ui/src/waveform/styles.rs`, `crates/ui/src/waveform/lane.rs`,
  `crates/ui/src/deck.rs` (`overview_columns`).
- Modify: the frame or `DeckReads`, only if the per-deck knob values aren't already reachable
  where the lanes draw. They are in `ControlState` / the frame's channel values, which the
  mixer uses.
- Test: unit tests in `styles.rs`; `crates/ui/tests/waveform.rs`.

**Interfaces:**
- Produces:
  - `pub struct BandScale { pub low: f32, pub mid: f32, pub high: f32 }` in `ui::waveform`,
    with `BandScale::UNITY` and
    `pub fn band_scale(trim: f32, eq_high: f32, eq_mid: f32, eq_low: f32) -> BandScale`,
    which uses `engine::mixer::{eq_gain, trim_gain}`.
  - `styles::bars` gains a `scale: BandScale` parameter. Each band's level is multiplied by
    its scale, then clamped to 255 before reach is computed.

- [ ] **Step 1: Write the failing tests.**
  - `a_killed_band_disappears`: `band_scale(0.5, 0.5, 0.0, 0.5)` makes the mid bar absent, or
    of zero reach.
  - `gain_scales_every_band`: trim at the knob value where `trim_gain` is 0.5 halves every
    reach.
  - `boosted_bands_stay_inside_the_lane`: full boost on gain and on all EQs keeps every bar
    within 0..=1.
  - `the_overview_follows_the_knobs` (kittest): with deck 1's LOW at 0, the overview strip
    paints no low-colour mesh.
  - `the_lane_follows_its_decks_knobs` (kittest): the same for a lane, and deck 2's lane is
    unaffected.
- [ ] **Step 2: Run them and see them fail. Step 3: Implement**, with no allocation per frame:
  compute the scale once per deck per frame. **Step 4: Run** `cargo test -p ui`, re-render,
  and look.
- [ ] **Step 5: Commit** `feat(ui): the waveform follows the EQ, stem and gain knobs`

---

### Task 2: Where a held roll or slicer lands, and every loop shaded

**Files:**
- Modify: `crates/ui/src/waveform/lane.rs` and `crates/ui/src/deck.rs` (the overview).
- Test: `crates/ui/tests/waveform.rs` and `crates/ui/tests/deck.rs`.

**Interfaces:**
- Consumes: `DeckSnapshot::{ghost_frames, roll_active, slicer_block_beat, loop_in, loop_out,
  loop_active, grid}`.

- [ ] **Step 1: Write the failing tests.**
  - `a_held_roll_shows_where_it_lands`: with `ghost_frames = Some(x)` and `roll_active`, the
    lane paints an accent triangle whose tip x is the lane position of `x` (±1 pt), plus the
    vertical line.
  - `no_marker_without_slip`: there is no triangle when `ghost_frames` is `None`.
  - `a_loop_roll_is_shaded`: the `loop_band` rect spans `loop_in..loop_out` while
    `roll_active`.
  - `a_slicer_block_is_shaded`: one block's span is shaded from `slicer_block_beat` and the
    grid.
  - `the_overview_shows_the_landing_point`.
- [ ] **Step 2: Run them and see them fail. Step 3: Implement. Step 4: Run them; they pass.**
  Re-render.
- [ ] **Step 5: Commit** `feat(ui): show where a held roll lands, and shade every loop`

---

### Task 3: The library from the keyboard

**Files:**
- Modify: `crates/ui/src/browser.rs` and `crates/ui/src/command.rs`.
- Modify: `crates/library/src/browser/mod.rs`, which gets `SelectAll`.
- Modify: `crates/app/src/library_mode/*`, which routes `SelectAll`.
- Test: `crates/ui/tests/browser.rs` and the library browser tests.

**Interfaces:**
- Produces:
  - `BrowserEdit::SelectAll`. The view gains `all_selected: bool`; it is cleared by any other
    selection change or list change.
  - The keys, active only while the browser owns the keyboard and no text field has focus:

    | Key | Effect |
    |---|---|
    | ↑ ↓ | `SelectRow` / `SelectTree` ±1 |
    | Page Up / Page Down | ± the rows that fit |
    | Home / End | first and last row |
    | ← → (tree) | close or open the highlighted folder, `ToggleTreeAt` |
    | Tab | switch focus pane (the existing `ToggleFocus`) |
    | Enter (tree) | open the entry (`ToggleTreeAt`, or `SelectTree` for a playlist) |
    | Enter (list) | load the highlighted track on the first deck not playing, as a double-click does |
    | Cmd/Ctrl+A | `SelectAll` |
    | Delete, Backspace, Cmd+Backspace | open the delete confirmation (Task 4) for the focused pane |
    | A letter or digit with no modifier | appended to the search box, which takes focus |

- [ ] **Step 1: Write the failing tests.**
  - `arrows_move_the_cursor_in_the_focused_pane`;
  - `tab_switches_panes`;
  - `enter_loads_the_highlighted_track`;
  - `cmd_a_selects_the_whole_list` (`all_selected` in the view, and `list_count` shows as "N
    selected");
  - `typing_goes_to_the_search_box`;
  - `delete_in_the_search_box_edits_text` (no confirmation opens);
  - `keys_do_nothing_until_the_browser_is_clicked`;
  - `the_focused_pane_looks_focused`.
- [ ] **Step 2: Run them and see them fail. Step 3: Implement. Step 4: Run** `cargo test -p ui
  -p library -p open-jockey`.
- [ ] **Step 5: Commit** `feat(ui): drive the library from the keyboard`

---

### Task 4: Delete, depending on where you are, always asking first

**Files:**
- Create: `crates/ui/src/delete_dialog.rs` and `crates/app/src/library_mode/delete.rs`.
- Modify: `crates/ui/src/browser.rs` (the Delete key, and "Delete…" entries in the track
  menu and on tree folders), `crates/ui/src/command.rs`, and
  `crates/app/src/library_mode/{mod.rs, worker.rs}`.
- Modify: `Cargo.toml` (the `trash` crate in `app`).
- Test: `crates/ui/tests/browser.rs` and `crates/app/src/library_mode/delete.rs` tests.

**Interfaces:**
- Produces:
  - `LibraryStep::AskDelete(DeleteTarget)` and `LibraryStep::Delete(DeleteTarget)`, where
    `DeleteTarget` is one of:
    - `Entries { playlist, entries: Vec<(usize, i64)> }`, which reuses plan 12b's removal;
    - `AllEntries { playlist }`;
    - `Files { tracks: Vec<i64> }`;
    - `AllFiles` (the current folder list);
    - `Playlist { id }`;
    - `Folder { path }`.
  - `AskDelete` makes the session reply with a `DeleteCheck` in the frame. `DeleteCheck` holds
    the confirmation text and an `Ok`, or the refusal text. The dialog shows it, and OK sends
    `Delete` with the same target.
  - `pub trait Trash { fn trash(&self, paths: &[PathBuf]) -> Result<(), TrashError>; }` has
    two implementations:
    - `SystemTrash`, which wraps the `trash` crate;
    - `FakeTrash`, for tests, which records paths and can fail on demand.
  - `fn check_delete(target, library_folders, loaded: [Option<PathBuf>; 4]) -> Result<Plan,
    Refusal>`. It is pure, and the session and the worker both run it. It:
    - canonicalises every path;
    - refuses outside the canonical library folders;
    - refuses a library root, or a folder containing one;
    - refuses a loaded file, or a folder holding one;
    - counts the files.
  - After a successful Trash move:
    - `WriteOp::MarkMissing` for the tracks under the trashed paths;
    - then a browser refresh;
    - then the event "Moved N files to the Trash".

- [ ] **Step 1: Write the failing tests.**
  - `the_delete_key_in_a_playlist_removes_entries_after_asking`. Nothing changes until OK, and
    Cancel is focused.
  - `a_file_goes_to_the_trash_and_is_marked_missing`, using `FakeTrash` and a temp library.
  - `a_playlist_is_deleted_after_asking`.
  - `a_folder_goes_to_the_trash_with_its_count`. The text counts non-audio files.
  - `a_folder_holding_a_loaded_track_cannot_be_deleted`.
  - `a_library_root_cannot_be_deleted`.
  - `a_link_out_of_the_library_is_refused` (unix).
  - `a_failed_trash_deletes_nothing`. `FakeTrash` fails, no `MarkMissing` happens, and the
    message is exact.
  - `select_all_then_delete_in_a_folder_counts_every_file`.
- [ ] **Step 2: Run them and see them fail. Step 3: Implement. Step 4: Run** `cargo test -p ui
  -p library -p open-jockey`. Run `cargo deny check` with `trash` added.
- [ ] **Step 5: Commit** `feat(app): delete from the browser, to the Trash, after asking`

---

### Task 5: Covers kept in the library and shown

**Files:**
- Modify: `crates/library/src/schema.rs` (V4).
- Create: `crates/library/src/covers.rs`.
- Modify: `crates/library/src/writer.rs` and `lib.rs`.
- Modify: `crates/app/src/session/frame.rs`, so `DeckInfo` and the browser info row carry
  `cover_file: Option<PathBuf>`.
- Modify: `crates/ui/src/cover.rs`: after no embedded picture, read `cover_file`.
- Test: `crates/library/tests/covers.rs`, plus a `cover.rs` test.

**Interfaces:**
- Produces:
  - V4: `CREATE TABLE covers (track_id INTEGER PRIMARY KEY REFERENCES tracks(id), state TEXT
    NOT NULL CHECK (state IN ('found','missed','removed')), file TEXT, updated_at INTEGER NOT
    NULL);`
  - `WriteOp::SetCover { track, file }`, `WriteOp::MissCover { track }` and
    `WriteOp::ClearCover { track }`. `ClearCover` sets the state to `removed` and deletes the
    file.
  - `Reader::cover(track) -> Result<Option<CoverRow>, LibraryError>` and
    `Reader::tracks_needing_covers(limit) -> Result<Vec<CoverQuery>, LibraryError>`. The second
    returns tracks with no row, or with `missed` only when a retry flag is set.
  - `CoverQuery { track: i64, artist: Option<String>, title: Option<String>, file_stem: String
    }`.
  - The covers folder is `<library db dir>/covers/<track id>.jpg`.

- [ ] **Step 1: Write the failing tests.**
  - `set_and_read_a_cover`;
  - `a_miss_is_remembered`;
  - `removed_is_never_looked_up_again`;
  - `tracks_needing_covers_skips_found_and_missed`;
  - `the_window_shows_a_stored_cover_when_the_file_has_none`;
  - `an_embedded_picture_wins_over_a_stored_one`.
- [ ] **Step 2: Run them and see them fail. Step 3: Implement. Step 4: Run them; they pass.**
- [ ] **Step 5: Commit** `feat(library): keep covers found for tracks`

---

### Task 6: Looking covers up online

**Files:**
- Create: `crates/app/src/covers/{mod.rs, service.rs}`.
- Modify: `crates/app/src/session/settings.rs` (the `library.cover_lookup` setting).
- Modify: `crates/ui/src/settings/library.rs` (the switch and "Retry missing covers").
- Modify: `crates/ui/src/browser.rs` ("Find another cover" and "Remove cover" in the track
  menu).
- Modify: `crates/app/Cargo.toml` (`ureq`, and `image` for resizing).
- Modify: docs.
- Test: `crates/app/src/covers` tests.

**Interfaces:**
- Produces:
  - `pub trait CoverService: Send { fn find(&self, query: &CoverQuery) -> Result<Option<Vec<u8>>,
    LookupError>; }`. It returns image bytes.
  - `DeezerService` and `MusicBrainzService`, plus `Chain(Vec<Box<dyn CoverService>>)`.
  - `FakeService` for tests.
  - `CoverWorker::start(lib, service, setting) -> CoverWorker`. It:
    - runs on its own thread;
    - takes batches of `tracks_needing_covers(20)`;
    - for each one, calls `find` with a 10 s timeout, resizes the image to 512 px and saves it
      as JPEG to the covers folder, then sends `SetCover` (or `MissCover`);
    - sleeps between requests so MusicBrainz stays at or under 1 per second;
    - stops when the setting turns off.
  - The query text is `artist + " " + title` when a title tag exists, else `file_stem`.
  - `LibraryStep::FindCover { track }` (forget the miss or removal and look the track up now)
    and `LibraryStep::RemoveCover { track }`.

- [ ] **Step 1: Write the failing tests:**
  - `a_found_cover_is_saved_resized_and_recorded`, using `FakeService` returning a 2000×2000
    PNG. The file is ≤ 512 px.
  - `only_artist_and_title_are_sent`. `FakeService` records the query, and no path appears.
  - `a_track_without_tags_is_looked_up_by_its_file_name`.
  - `an_unreachable_service_records_a_miss_and_never_blocks`. The fake sleeps, then errors. The
    control loop's command drain still returns at once (time it), and one `MissCover` is
    written.
  - `turning_the_setting_off_stops_lookups`.
  - `find_another_cover_retries_a_miss`.
  - `remove_cover_shows_the_tile_again`.
  - `deezer_and_musicbrainz_parse_their_answers`. Use recorded JSON fixtures in
    `crates/app/tests/fixtures/covers/`, written by hand from the services' documented
    formats. No network in tests.
- [ ] **Step 2: Run them and see them fail. Step 3: Implement. Step 4: Run** `cargo test
  --workspace`, `cargo clippy --all-targets -- -D warnings` and `cargo deny check`.
- [ ] **Step 5: Commit** `feat(app): look covers up online` and `docs: covers, the library
  keyboard and the reacting waveforms`. The docs:
  - say what is sent (artist and title, or the file name) and to which services;
  - say how to turn it off;
  - list the keyboard map and the delete rules.

---

## Hands-on (the owner)

1. **Waveforms:**
   - Hold a loop roll, then the slicer, with slip on. Check the loop is shaded and the marker
     shows where the track lands.
   - Turn LOW, MID and HIGH (and the stem knobs) down, and GAIN up and down. Check the bands
     shrink, vanish and grow.
2. **Keyboard:** click the browser, then use the arrows, Tab, Enter to open or load, Cmd+A,
   and typing to search.
3. **Delete:**
   - Delete from a playlist: it asks, then removes.
   - Delete a test file in a folder: it asks, the file goes to the Trash, and you can restore
     it from there.
   - Try a folder with a loaded track: it is refused.
4. **Covers:** let it run a few minutes. Covers appear for tracks it finds. Try "Find another
   cover" and "Remove cover". Turn the setting off and check lookups stop.
5. Record the results in `controllers/ddj-sb3/README.md`.
