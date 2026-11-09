# Saved loops and the VirtualDJ import Implementation Plan (plan 11a)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Saved loops live in hotcue slots (stored, played, shown), hotcues keep colours and names, and the owner's VirtualDJ hotcues, loops and playlists import once into the library with keep / replace / merge.

**Architecture:** The library's `hotcues` table gains a loop length; the engine's hotcue slots become `Hotcue { frames, loop_frames }` so a loop slot jumps and loops; the app's cue write-through carries the loop. A new `interop` crate reads VirtualDJ's `database.xml`, `.vdjfolder`, `.m3u` and `MyLists/order`, plans an import against the library (matching, conversion, conflicts) and turns it into one `WriteOp::Batch`. The CLI prints the plan (dry run) or applies it with one resolution for all; the window walks the owner through it per track.

**Tech Stack:** Rust workspace (edition and lints as in `Cargo.toml`), rusqlite, `quick-xml` (new, MIT) in `interop`, egui 0.37 / egui_kittest for the window.

**Spec:** [docs/superpowers/specs/2026-11-09-interop-design.md](../specs/2026-11-09-interop-design.md) (sections Intent, Goals 3-4, VirtualDJ import, Saved loops, Positions grids and colours, Verification, Constraints, Plan split). The original design is [2026-10-30-dj-software-design.md](../specs/2026-10-30-dj-software-design.md) (cue safety rules under Library).

## What VirtualDJ writes (captured from the owner's VirtualDJ 2025, 2026-11-09)

The owner's music drive is `/Volumes/Mix Gomez`. VirtualDJ keeps a **per-drive** database at
`<volume>/VirtualDJ/database.xml` and the playlists at `<volume>/VirtualDJ/MyLists/*.vdjfolder`;
the local one is `~/Library/Application Support/VirtualDJ/` (macOS) with `database.xml` and
`MyLists/order` (playlist names, one per line, in the owner's order). A track entry, with the
file name replaced:

```xml
<VirtualDJ_Database Version="2025">
<Song FilePath="D:/Artist - Title.mp3" FileSize="9277127" Flag="268435456">
  <Tags Title="Artist - Title" Bpm="0.821918" Key="Cm" Flag="1" />
  <Infos SongLength="231.784490" LastModified="1794228943" FirstSeen="1782069030" Bitrate="320" Cover="16386" />
  <Scan Version="801" Bpm="0.620703" Phase="0.024240" AltBpm="0.362063" Volume="1.138718" Key="F" AudioSig="..." Flag="32768" />
  <Poi Pos="0.020317" Type="automix" Point="realStart" />
  <Poi Pos="6.765714" Num="1" Color="4294901760" Type="cue" />
  <Poi Name="Break 1" Pos="59.204354" Type="remix" />
  <Poi Name="Test named cue" Pos="86.413311" Num="2" Color="4278190335" Type="cue" />
  <Poi Pos="125.960" Num="3" Color="4278255360" Type="cue" />
  <Poi Pos="128.456145" Num="4" Type="loop" Size="8.0" />
  <Poi Pos="227.660045" Type="automix" Point="realEnd" />
 </Song>
```

and a playlist (`MyLists/OJ test.vdjfolder`):

```xml
<?xml version="1.0" encoding="UTF-8"?>
<VirtualFolder noDuplicates="no">
	<song path="/Volumes/Mix Gomez/Artist - Title.mp3" size="9277127" songlength="231.8" bpm="96.665" key="F" title="Artist - Title" idx="0" />
</VirtualFolder>
```

Read as:
- **`FilePath` in a per-drive database** starts with a drive letter (`D:/`) that stands for that
  drive's root: `D:/a/b.mp3` in `/Volumes/X/VirtualDJ/database.xml` is `/Volumes/X/a/b.mp3`.
  In the local database, and in playlists, paths are absolute (on Windows a real `C:/...`).
- **Hotcue:** `Type="cue"` with `Num` 1.. (slot), `Pos` in seconds, optional `Name`, optional
  `Color` as a decimal ARGB `u32` (`4294901760` = `0xFFFF0000`, red).
- **Saved loop:** `Type="loop"` with `Num` (it takes a hotcue slot, as Open Jockey's design
  does), `Pos` seconds, `Size` in **beats** of VirtualDJ's grid. `Scan Bpm` is seconds per beat
  (`60 / 0.620703 = 96.665` BPM), so the loop's length in seconds is `Size * Scan.Bpm`.
- The **main cue is not stored** by VirtualDJ: nothing to import for it.
- `remix`, `automix` and `beatgrid` POIs are not hotcues: they are counted in the summary as
  "other markers, not imported".

## Global Constraints

- Everything from the project still holds: mise, prek, Conventional Commits (commitizen), `cargo clippy -- -D warnings`, `cargo fmt`, `cargo deny check`, builds and tests on macOS, Windows and Linux.
- **Never write the owner's VirtualDJ data.** The import only reads. "Never corrupt or partially write the owner's VirtualDJ data."
- **Imports never silently overwrite cues.** "When a track already has cues in Open Jockey, the import asks per track: keep, replace or merge." Merge: "Open Jockey's own slots win; VirtualDJ's fill the empty slots."
- "The import runs in one library transaction after a database backup."
- "Tracks not in the library are listed in the summary, not added."
- "More than 8 hotcues plus loops in a format: the first 8 by position are kept and the summary says how many were dropped." VirtualDJ's slots 1-8 keep their slot; items with `Num` above 8 fill free slots in position order, then are dropped.
- "The VirtualDJ import does not replace Open Jockey's grids."
- "An empty hotcue pad pressed while a loop is active stores the loop in that slot; an occupied pad behaves as today."
- Tests never touch the owner's real VirtualDJ data, music volumes or config/data folders; fixtures are written to temp dirs from strings in the tests.
- Colours are stored as `0xRRGGBB` in `hotcues.color` (alpha dropped).
- Never modify or stage `controllers/ddj-sb3/capture.toml`.

## Review Focus

1. **Running the import twice** must not duplicate playlists or change anything the second time: a playlist whose name exists with the same tracks in the same order is skipped; only a different one gets a suffix (` (2)`, ` (3)` ...). Test in Task 6.
2. **A `database.xml` that is half-written or not VirtualDJ's** (VirtualDJ saving while we read, a truncated file) must give a clear error naming the file and write nothing. Test in Task 5.
3. **Names and paths with XML entities and non-ASCII** (`&amp;`, `&apos;`, `é`, `ü`) must match the library's paths exactly. Test in Task 5.
4. **A loop saved on a deck whose loop is a roll or slicer loop** must not be stored as a saved loop (only a normal active loop is). Test in Task 2.
5. **A track loaded on a deck during the import** would keep its old hotcues on the deck and could write them back; the window refuses to import while any deck holds a track. Test in Task 8.

---

### Task 1: Library: loop length, colour and name in hotcue slots

**Files:**
- Modify: `crates/library/src/schema.rs` (add `V3`, append to `MIGRATIONS`)
- Modify: `crates/library/src/cues.rs`, `crates/library/src/writer.rs`, `crates/library/src/lib.rs` (exports, `LibraryError::InvalidLoop`), `crates/library/src/backup.rs`
- Test: `crates/library/tests/hotcue_points.rs` (new)

**Interfaces:**
- Produces:
  - `pub struct HotcuePoint { pub frames: f64, pub loop_frames: Option<f64>, pub color: Option<u32>, pub name: Option<String> }` (`Debug, Clone, PartialEq`), exported from `library`.
  - `Reader::hotcue_points(&self, track: i64) -> Result<[Option<HotcuePoint>; 8], LibraryError>` (index = slot - 1). `Reader::hotcues` stays as it is.
  - `WriteOp::SetHotcue { track: i64, slot: u8, frames: f64, loop_frames: Option<f64> }`: as today (keeps colour and name), and stores `loop_frames` (`None` clears it).
  - `WriteOp::PutHotcue { track: i64, slot: u8, point: HotcuePoint }`: replaces the whole slot.
  - `LibraryError::InvalidLoop(f64)` for a loop length that is not finite and > 0.
  - `Reader::track_id_by_path(&self, path: &Path) -> Result<Option<i64>, LibraryError>`; `Reader::track_ids_by_fingerprint(&self, fp: &[u8; 32]) -> Result<Vec<i64>, LibraryError>`; `Reader::track_ids_by_name_and_size(&self, file_name: &str, size: u64) -> Result<Vec<i64>, LibraryError>` (file name compared exactly on the last path component).
  - `Library::backup_now(&self) -> Result<PathBuf, LibraryError>`: a backup regardless of `MIN_INTERVAL`, pruned like the others.

- [ ] **Step 1: Write the failing tests** in `crates/library/tests/hotcue_points.rs`, using the helpers other tests in `crates/library/tests/` use to open a temp library and add a track:
  - `put_hotcue_round_trips_every_field`: `PutHotcue { slot: 4, point: HotcuePoint { frames: 5_664_000.0, loop_frames: Some(219_000.0), color: Some(0xFF0000), name: Some("Drop".into()) } }` then `hotcue_points(track)[3]` equals it, and `hotcues(track)[3] == Some(5_664_000.0)`.
  - `set_hotcue_keeps_colour_and_name_and_sets_the_loop`: after the `PutHotcue` above, `SetHotcue { slot: 4, frames: 100.0, loop_frames: None }` gives `HotcuePoint { frames: 100.0, loop_frames: None, color: Some(0xFF0000), name: Some("Drop".into()) }`.
  - `bad_loops_are_refused`: `loop_frames` of `Some(0.0)`, `Some(-1.0)`, `Some(f64::NAN)` each fail with `LibraryError::InvalidLoop` and change nothing.
  - `a_v2_database_migrates_keeping_hotcues`: a database created at schema 2 (raw `V1`+`V2` SQL with `PRAGMA user_version = 2`, as the existing migration tests do) holding a hotcue opens at version 3 with `loop_frames: None` for it.
  - `tracks_are_found_by_path_fingerprint_and_name_size`: exact path finds the id; a wrong-case path does not; fingerprint and `("b.mp3", size)` find it.
  - `backup_now_writes_a_backup_even_right_after_another`.
- [ ] **Step 2: Run** `cargo test -p library --test hotcue_points` — expect compile failure (no `HotcuePoint`).
- [ ] **Step 3: Implement.** `V3`: `ALTER TABLE hotcues ADD COLUMN loop_frames REAL;`. Update every existing `WriteOp::SetHotcue` construction in the workspace to pass `loop_frames` (the app passes the engine's value in Task 3; until then `None`). `backup_now` reuses `backup.rs`'s copy and prune with the interval check skipped.
- [ ] **Step 4: Run** `cargo test -p library` — all pass.
- [ ] **Step 5: Commit** `feat(library): store saved loops, colours and names in hotcue slots`.

### Task 2: Engine: a hotcue slot can hold a loop

**Files:**
- Modify: `crates/engine/src/memory.rs`, `crates/engine/src/deck/hotcue.rs`, `crates/engine/src/deck/mod.rs`, `crates/engine/src/shared.rs`, `crates/engine/src/command.rs`, `crates/engine/src/lib.rs` (export)
- Modify every user of `hotcues` that no longer compiles (`crates/app`, `crates/ui`, `crates/actions`), changing only types (`.map(|h| h.frames)` where a position is needed); behaviour changes there belong to Tasks 3 and 4.
- Test: `crates/engine/src/deck/` tests module for hotcues (where the existing hotcue tests live)

**Interfaces:**
- Produces:
  - `pub struct Hotcue { pub frames: f64, pub loop_frames: Option<f64> }` (`Debug, Clone, Copy, PartialEq`) in `engine`.
  - `TrackMemory.hotcues: [Option<Hotcue>; HOTCUES]`, `DeckSnapshot.hotcues: [Option<Hotcue>; HOTCUES]`.
  - `DeckCommand::SetHotcue(HotcueSlot, Option<Hotcue>)` (was `Option<f64>`); a non-finite or non-positive `loop_frames` makes it a plain hotcue; frames clamp to the track and a loop end past the track end is cut to it.

- [ ] **Step 1: Write the failing tests:**
  - `an_empty_pad_during_a_loop_saves_the_loop`: grid at 120 BPM, `AutoLoopBeats(4.0)` at playhead, `HotcuePress(slot 2)` on an empty slot → `snapshot.hotcues[1] == Some(Hotcue { frames: loop_in, loop_frames: Some(loop_out - loop_in) })`, and the loop is still active with the playhead not moved.
  - `an_empty_pad_without_a_loop_saves_a_hotcue` (today's behaviour, `loop_frames: None`).
  - `a_roll_is_not_saved_as_a_loop`: during a held `LoopRoll` (and a slicer slice), an empty pad stores `loop_frames: None` at the playhead, as today. (Review Focus 4.)
  - `a_loop_slot_jumps_and_loops`: `SetHotcue(slot 3, Some(Hotcue { frames: 44_100.0, loop_frames: Some(88_200.0) }))`, playing elsewhere, `HotcuePress(slot 3)` → position 44_100, `loop_in == Some(44_100.0)`, `loop_out == Some(132_300.0)`, `loop_active`; rendering 3 s of frames keeps the position inside the loop.
  - `a_loop_slot_while_paused_previews_inside_the_loop`: paused, press and hold → plays from the start looping; release → back to the start, paused (as a hotcue preview).
  - `an_occupied_pad_during_a_loop_jumps_as_today`.
  - `memory_clamps_loop_slots`: `TrackMemory` with a loop slot whose end passes the track end is cut to the end; a NaN `loop_frames` becomes a plain hotcue.
- [ ] **Step 2: Run** `cargo test -p engine` — fails to compile.
- [ ] **Step 3: Implement.** Only a normal loop counts (`loop_active` and no roll/slice running). The loop-slot press reuses the jump/slip/preview path of `hotcue_press`, then sets `loop_in`, `loop_out`, `loop_active`. `loop_beats` is left as it is.
- [ ] **Step 4: Run** `cargo test --workspace` — all pass (other crates only had type changes).
- [ ] **Step 5: Commit** `feat(engine): let a hotcue slot hold a saved loop`.

### Task 3: App: saved loops reach the library and come back on load

**Files:**
- Modify: `crates/app/src/library_mode/memory_sync.rs`, `crates/app/src/library_mode/mod.rs`, `crates/app/src/library_mode/worker.rs`
- Test: the tests modules of those files

**Interfaces:**
- Consumes: Task 1 `Reader::hotcue_points`, `WriteOp::SetHotcue { loop_frames }`; Task 2 `Hotcue`.
- Produces: `MemoryAction::SaveHotcue(HotcueSlot, Hotcue)` (was `f64`).

- [ ] **Step 1: Write the failing tests** next to `hotcues_reach_the_library_and_every_deck_holding_the_track`:
  - `a_saved_loop_reaches_the_library_and_the_other_decks`: same shape as that test, with the deck saving a loop slot → `hotcue_points(7)[slot].loop_frames == Some(len)` and the other deck holding track 7 receives `SetHotcue(slot, Some(Hotcue{..loop..}))`.
  - `a_loop_slot_loads_from_the_library`: a `PutHotcue` loop slot is in `TrackMemory.hotcues` when the track loads.
  - `turning_a_loop_slot_into_a_hotcue_clears_the_loop` (delete then a plain save → `loop_frames: None`).
- [ ] **Step 2: Run** `cargo test -p open-jockey library_mode` — fails.
- [ ] **Step 3: Implement.** The worker reads `hotcue_points` (not `hotcues`) into `TrackMemory`; the memory diff compares whole `Hotcue`s.
- [ ] **Step 4: Run** `cargo test --workspace` — pass.
- [ ] **Step 5: Commit** `feat(app): save and restore saved loops with the hotcues`.

### Task 4: Window: loop slots, colours and names on the pads and waveforms

**Files:**
- Modify: `crates/ui/src/model.rs` (`DeckInfo`), `crates/ui/src/deck.rs` (pads), `crates/ui/src/waveform/lane.rs` (flags), `crates/app/src/session/frame.rs` or `detail.rs` (where `DeckInfo` is filled)
- Test: `crates/ui/src/deck.rs` and `crates/ui/src/waveform/lane.rs` test modules; `crates/app/src/session` test for the fill

**Interfaces:**
- Consumes: Task 1 `Reader::hotcue_points`; Task 2 `DeckSnapshot.hotcues`.
- Produces: `DeckInfo.hotcue_marks: [Option<HotcueMark>; 8]` with `pub struct HotcueMark { pub color: Option<egui::Color32>, pub name: Option<String> }`, read from the library when the deck's track loads and again after an import (Task 8 asks for a refresh). Positions and loops always come from the snapshot.

- [ ] **Step 1: Write the failing tests** (egui_kittest, like the existing pad and lane tests):
  - `a_loop_slot_shows_as_a_loop_on_its_pad`: snapshot slot 4 is a loop → the pad's label is `4 loop` and the waveform draws a band from start to end in the hotcue's colour.
  - `a_named_hotcue_shows_its_name`: `HotcueMark { name: Some("Drop") }` → the window pad's text contains `Drop` (cut to fit), and the waveform flag shows it beside the number.
  - `hotcue_colours_tint_pads_and_flags`: colour red → the pad's fill and the flag use it; no colour → today's hotcue colour (`theme::palette(ctx).hotcue`).
  - `marks_are_read_when_a_track_loads` (session side).
- [ ] **Step 2: Run** `cargo test -p ui` — fail.
- [ ] **Step 3: Implement.** Colour `0xRRGGBB` → `Color32::from_rgb`. In the light scheme a colour keeps enough contrast with the background (darken when its luminance is above 0.8, as the light palette does for markers).
- [ ] **Step 4: Run** `cargo test --workspace` — pass.
- [ ] **Step 5: Commit** `feat(ui): show saved loops, hotcue colours and names`.

### Task 5: `interop` crate: read VirtualDJ's data

**Files:**
- Create: `crates/interop/Cargo.toml` (deps: `quick-xml` latest 0.x, `thiserror` as the workspace uses; add `quick-xml` to `[workspace.dependencies]`), `crates/interop/src/lib.rs`, `crates/interop/src/virtualdj/mod.rs`, `crates/interop/src/virtualdj/database.rs`, `crates/interop/src/virtualdj/playlists.rs`
- Modify: root `Cargo.toml` (member), `deny.toml` only if `cargo deny` asks
- Test: `crates/interop/tests/virtualdj_read.rs`

**Interfaces:**
- Produces:
  - `pub struct VdjSong { pub path: PathBuf, pub size: Option<u64>, pub seconds_per_beat: Option<f64>, pub pois: Vec<VdjPoi>, pub other_markers: usize }`
  - `pub enum VdjPoi { Hotcue { num: u32, seconds: f64, name: Option<String>, color: Option<u32> }, Loop { num: u32, seconds: f64, beats: f64, name: Option<String>, color: Option<u32> } }` (`color` already `0xRRGGBB`)
  - `pub struct VdjPlaylist { pub name: String, pub paths: Vec<PathBuf> }` (name `Folder / Name` for nested folders)
  - `pub struct VdjSources { pub databases: Vec<PathBuf>, pub list_dirs: Vec<PathBuf>, pub order: Option<PathBuf> }`
  - `pub fn find_sources(home_dirs: &[PathBuf], volumes: &[PathBuf]) -> VdjSources`: databases at `<dir>/database.xml` for each existing home dir (callers pass `~/Library/Application Support/VirtualDJ` and `~/Documents/VirtualDJ` on macOS, `Documents/VirtualDJ` on Windows, `~/.vdj`-style dirs are not searched) and `<volume>/VirtualDJ/database.xml` for each volume; `MyLists` dirs beside each; `order` from the first home dir that has `MyLists/order`.
  - `pub fn read_database(path: &Path, drive_root: Option<&Path>) -> Result<Vec<VdjSong>, InteropError>`: `drive_root` set for a per-drive database maps `X:/rest` to `drive_root/rest`.
  - `pub fn read_playlists(list_dirs: &[PathBuf], order: Option<&Path>) -> Result<Vec<VdjPlaylist>, InteropError>`: every `*.vdjfolder` (recursive) and `*.m3u` under them; sorted by `order`'s lines, the rest by name after.
  - `pub enum InteropError { Io { path: PathBuf, source: std::io::Error }, Format { path: PathBuf, why: String } }` with messages naming the file.

- [ ] **Step 1: Write the failing tests** in `crates/interop/tests/virtualdj_read.rs`, writing the captured XML above (and variants) to a `tempfile` dir:
  - `reads_hotcues_loops_names_and_colours`: the captured song read with `drive_root = /tmp/X` → path `/tmp/X/Artist - Title.mp3`, size `9277127`, `seconds_per_beat == Some(0.620703)`, pois exactly `[Hotcue{1, 6.765714, None, Some(0xFF0000)}, Hotcue{2, 86.413311, Some("Test named cue"), Some(0x0000FF)}, Hotcue{3, 125.96, None, Some(0x00FF00)}, Loop{4, 128.456145, 8.0, None, None}]`, `other_markers == 3` (the entry above: two automix, one remix).
  - `a_local_database_keeps_absolute_paths` (no `drive_root`).
  - `entities_and_accents_decode` (`FilePath="D:/Beyoncé &amp; Jay-Z - Drunk &apos;n&apos; Love.mp3"`). (Review Focus 3.)
  - `a_truncated_database_is_a_format_error_naming_the_file` (the fixture cut mid-`<Song>`), and `a_non_vdj_xml_is_a_format_error`. (Review Focus 2.)
  - `playlists_keep_their_order_and_follow_the_order_file`: two `.vdjfolder`s and an `order` file → playlists in `order`'s order, songs in file order, a nested `Sub/Inner.vdjfolder` named `Sub / Inner`; an `.m3u` with `#EXTINF` lines and relative paths resolved against its folder.
  - `find_sources_looks_in_home_dirs_and_volumes`.
- [ ] **Step 2: Run** `cargo test -p interop` — fails.
- [ ] **Step 3: Implement** with `quick_xml::Reader` (streaming; the owner's database is ~5 MB). Unknown attributes and elements are ignored.
- [ ] **Step 4: Run** `cargo test -p interop && cargo deny check` — pass.
- [ ] **Step 5: Commit** `feat(interop): read VirtualDJ databases and playlists`.

### Task 6: `interop`: plan and apply a VirtualDJ import

**Files:**
- Create: `crates/interop/src/import.rs` (format-neutral: what an import brings, conflicts, the batch), `crates/interop/src/virtualdj/import.rs` (VirtualDJ to that)
- Modify: `crates/interop/Cargo.toml` (dep `library`)
- Test: `crates/interop/tests/import.rs`

**Interfaces:**
- Consumes: Task 1 `HotcuePoint`, `WriteOp::PutHotcue`, lookups, `Library::backup_now`; Task 5 readers.
- Produces:
  - `pub struct ImportPlan { pub tracks: Vec<TrackImport>, pub playlists: Vec<PlaylistImport>, pub unmatched: Vec<PathBuf>, pub dropped: usize, pub other_markers: usize }`
  - `pub struct TrackImport { pub track: i64, pub path: PathBuf, pub incoming: [Option<HotcuePoint>; 8], pub existing: [Option<HotcuePoint>; 8] }` with `fn conflicts(&self) -> bool` (any existing slot set and incoming non-empty)
  - `pub struct PlaylistImport { pub name: String, pub tracks: Vec<i64>, pub action: PlaylistAction }`, `pub enum PlaylistAction { Create, SkipSame }` (the name is already suffixed when a different playlist has it)
  - `#[derive(Clone, Copy)] pub enum Resolution { Keep, Replace, Merge }`
  - `pub fn plan_virtualdj(reader: &library::Reader, songs: &[VdjSong], playlists: &[VdjPlaylist]) -> Result<ImportPlan, InteropError>`
  - `impl ImportPlan { pub fn batch(&self, resolve: impl Fn(&TrackImport) -> Resolution) -> library::WriteOp }` (one `WriteOp::Batch`; a track without conflict always takes its incoming slots)
  - `pub fn apply(lib: &library::Library, plan: &ImportPlan, resolve: impl Fn(&TrackImport) -> Resolution) -> Result<ImportReport, InteropError>`: `backup_now`, then `submit_wait(batch)`; `pub struct ImportReport { pub backup: PathBuf, pub tracks_changed: usize, pub playlists_created: usize }`

Conversion: frames = `seconds * sample_rate` with the library track's `sample_rate` (tracks without one are listed in `unmatched` with a reason); loop frames = `beats * seconds_per_beat * sample_rate`, a loop on a song with no `seconds_per_beat` imports as a hotcue. Matching order: exact path; else fingerprint of the file at the VirtualDJ path when it exists (`library::fingerprint::of_file`); else exactly one track with the same file name and size; else unmatched. Slots: `Num` 1-8 keep their slot; `Num` > 8 fill free slots by position, the rest count in `dropped`. When two POIs share a `Num`, the earlier position wins and the other counts in `dropped`.

- [ ] **Step 1: Write the failing tests** (temp library with tracks at 44.1 kHz, songs built in code):
  - `slots_positions_and_loops_convert`: hotcue at `6.765714` s → `298_368.0` frames (± 0.5); loop `8.0` beats at `0.620703` s/beat → `loop_frames ≈ 218_984.0`; colour and name carried.
  - `num_over_8_fills_free_slots_then_drops`: Nums 1-8 used plus `Num` 10 and 12 → `dropped == 2`; with slot 5 free, `Num` 10 lands in 5 and `dropped == 1`.
  - `matching_by_path_then_fingerprint_then_name_and_size`, and an ambiguous name+size (two tracks) goes to `unmatched`.
  - `keep_replace_merge`: existing slot 1 and 3, incoming 1 and 2 → Keep: 1,3 unchanged, 2 not added; Replace: exactly incoming (slot 3 deleted); Merge: 1 and 3 kept, 2 added.
  - `importing_twice_changes_nothing_the_second_time`: apply (Replace), plan again → every playlist `SkipSame`, every track's `incoming == existing`, and the batch changes no row. (Review Focus 1.)
  - `a_different_playlist_with_the_same_name_gets_a_suffix` → `"Set (2)"`, then `"Set (3)"`.
  - `apply_backs_up_first_and_writes_all_or_nothing`: a batch that fails (a track deleted between plan and apply via `WriteOp::Raw`) leaves hotcues and playlists as they were and the backup file exists.
  - `grids_are_never_touched`: analysis rows identical before and after.
- [ ] **Step 2: Run** `cargo test -p interop --test import` — fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` — pass.
- [ ] **Step 5: Commit** `feat(interop): plan and apply a VirtualDJ import with keep, replace or merge`.

### Task 7: CLI: `open-jockey library import-vdj`

**Files:**
- Modify: `crates/app/src/library_cmd.rs`, `crates/app/Cargo.toml` (dep `interop`)
- Test: `crates/app/src/library_cmd.rs` tests module (or its existing tests file)

**Interfaces:**
- Consumes: Task 5 `find_sources`, readers; Task 6 `plan_virtualdj`, `apply`, `Resolution`.
- Produces: `LibraryCommand::ImportVdj { apply: Option<ResolutionArg>, vdj_dir: Vec<PathBuf> }` with `--apply keep|replace|merge` (without it: dry run, prints the summary and writes nothing) and `--vdj-dir DIR` (repeatable; replaces the default home dirs, used by tests). Volumes searched: the volumes of the library's folders (the mount point holding each folder: on macOS `/Volumes/<name>`, else the folder's filesystem root).

Summary printed (exact lines, counts filled in):
```
VirtualDJ import (dry run: nothing written; add --apply keep|replace|merge)
  databases: <n> (<paths, one per line, indented>)
  tracks matched: <n>, with hotcues or loops: <n>, conflicting with cues here: <n>
  hotcues: <n>, saved loops: <n>, dropped over 8 slots: <n>, other markers not imported: <n>
  playlists: <n> to create, <n> already here
  not in the library: <n>
    <path>   (first 20, then "... and <k> more")
```
With `--apply`, the first line is `VirtualDJ import (<resolution> for tracks with cues here)` and the summary ends with `backup: <path>` and `written: <n> tracks, <n> playlists`.

- [ ] **Step 1: Write the failing tests:** a temp config/data dir and a temp VirtualDJ dir with the captured XML; `dry_run_prints_the_summary_and_writes_nothing` (library unchanged); `apply_merge_writes_and_reports_the_backup`; `no_virtualdj_data_says_so` (`no VirtualDJ data found in: <dirs>`, exit status success).
- [ ] **Step 2: Run** `cargo test -p open-jockey import_vdj` — fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` — pass.
- [ ] **Step 5: Commit** `feat(app): add library import-vdj`.

### Task 8: Window: import from VirtualDJ

**Files:**
- Modify: `crates/ui/src/settings/mod.rs` (new `Page::Library` before `Later`), create `crates/ui/src/settings/library.rs`; `crates/ui/src/command.rs` (`UiCommand::Import(ImportStep)`); `crates/ui/src/model.rs` (`ControlFrame.import: ImportView`); `crates/app/src/session/` (new `import.rs`: runs plan/apply on a thread, publishes `ImportView`)
- Test: `crates/ui/tests/settings.rs` (or the settings test module) and `crates/app/src/session/import.rs` tests

**Interfaces:**
- Consumes: Task 6 `plan_virtualdj`, `apply`, `ImportPlan`, `Resolution`; Task 4's mark refresh.
- Produces:
  - `pub enum ImportStep { Scan, Resolve { track: i64, resolution: Resolution, rest_too: bool }, Apply, Cancel }` (`Resolution` re-declared in `ui` as a plain enum to keep `ui` free of `interop`, converted in the session)
  - `pub enum ImportView { Idle, Scanning, Summary(ImportSummary), Ask { track: TrackLabel, existing: usize, incoming: usize, left: usize }, Applying, Done(String), Failed(String), DecksBusy }` with `ImportSummary` holding the counts of Task 7's summary.

Flow: Settings → Library page → "Import from VirtualDJ…" → `Scan` (refused with `DecksBusy`, "Unload the decks to import: a loaded track would keep its old hotcues.", when any deck holds a track) → summary with Import / Cancel → for each conflicting track: "Keep mine / Use VirtualDJ's / Merge" and a "for all remaining" check box → `Applying` → `Done("Imported <n> tracks and <n> playlists. Backup: <path>")`. Afterwards the browser's playlists refresh and Task 4's marks are reread.

- [ ] **Step 1: Write the failing tests:** `import_refuses_while_a_deck_is_loaded` (Review Focus 5); `the_summary_shows_the_counts`; `choosing_for_all_remaining_asks_no_more`; `cancel_writes_nothing`; session: `scan_then_apply_imports_from_a_temp_vdj_dir` (session `Shared` with a temp config pointing the VirtualDJ dir at a temp folder through a test-only path).
- [ ] **Step 2: Run** `cargo test -p ui settings && cargo test -p open-jockey session::import` — fail.
- [ ] **Step 3: Implement.** The plan and the apply run off the control thread; the frame only carries `ImportView`.
- [ ] **Step 4: Run** `cargo test --workspace` — pass.
- [ ] **Step 5: Commit** `feat(ui): import from VirtualDJ in the settings window`.

### Task 9: Docs and the owner's hands-on check

**Files:**
- Modify: `docs/library.md` (saved loops, colours and names, the import), `docs/ui.md` (Library page), `docs/architecture.md` (`interop` crate), `README.md` (feature list), `controllers/ddj-sb3/README.md` (hands-on record)

- [ ] **Step 1: Write the docs.** `docs/library.md` gets an "Importing from VirtualDJ" section: where it looks, what comes over (hotcues with colours and names, saved loops, playlists in order), what does not (the main cue, which VirtualDJ does not keep; remix/automix markers; grids), keep/replace/merge, the backup, and the dry run.
- [ ] **Step 2: Run** `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo deny check` — pass.
- [ ] **Step 3: Commit** `docs: document saved loops and the VirtualDJ import`.
- [ ] **Step 4: Owner hands-on** (controller and window):
  1. Save a loop: start a 4-beat loop, press an empty hotcue pad; exit the loop; press the pad again → it loops from there. Restart → still there.
  2. `open-jockey library import-vdj` (dry run) → the summary counts look right; nothing changed.
  3. In the window: Settings → Library → Import from VirtualDJ, Merge for all → the `OJ test` playlist and the owner's sets appear in order; the test track shows hotcues 1-3 with their colours and the name "Test named cue", and slot 4 is an 8-beat loop.
  4. **Positions:** play the test track's hotcue 1 in Open Jockey and in VirtualDJ — it lands on the same beat (checks the MP3 offset; if it is off, report the difference in ms).
  5. Run the import again → "already here" for every playlist, nothing changed.
- [ ] **Step 5: Record** the result in `controllers/ddj-sb3/README.md` under "Hands-on check (plan 11a: saved loops and the VirtualDJ import)" and commit `docs(ddj-sb3): record the plan 11a hands-on check`.
