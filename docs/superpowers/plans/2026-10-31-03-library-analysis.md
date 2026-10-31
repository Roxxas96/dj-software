# Plan 3: Library, Analysis and Beatgrids

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A local music library the owner can browse and load from the SB3. It holds folders, playlists, tags, cue points that are never lost, and background analysis (waveform, BPM with a constant beatgrid, key), plus manual beatgrid correction.

**Architecture:** A new `library` crate owns a SQLite database (WAL mode, schema versioning with migrations, startup backups) behind a single writer thread. Readers use their own connections. A new `analysis` crate holds pure DSP functions (waveform peaks, onset envelope, tempo and beat phase, chroma and key) run by a worker pool in `library`. Non-engine actions (browse, load, grid edits) flow out of `actions::apply` as `AppRequest`s that the app executes against the library and engine. The CLI player gains a library mode: the SB3's BROWSE knob and LOAD buttons pick tracks, and a small line-based console handles grid edits until the UI exists.

**Tech Stack:** Rust 1.99.1, rusqlite 0.41.1 (feature `bundled`), lofty 0.25.5 (tags), notify 8.2.1 (folder watching), rustfft 6.4.1, blake3 1.8.7, walkdir 2.5.0, tempfile 3.27.0 (dev), plus existing crates.

**Spec:** `docs/superpowers/specs/2026-10-30-dj-software-design.md` (sections "Library", "Engine features: Beatgrid editing", "Threads and real-time rules").

**Out of scope:** hotcues, loops, sync, keylock, FX and sampler (plan 4). This plan creates the hotcue table so plan 4 only adds behavior. The UI browser and waveform display come in plan 5. Import/export with other apps is plan 6.

## Global Constraints

- License **Apache-2.0**. No GPL/LGPL/AGPL dependencies (cargo deny). Write BPM and key detection ourselves; never port GPL detectors (aubio, libKeyFinder, Mixxx's analyzers).
- Builds and tests on macOS, Windows and Linux. CI has no audio or MIDI devices.
- **Audio files are never modified.** Tags are read, never written.
- **Cues never disappear:**
  - Every cue/grid change is written immediately in a transaction (write-through, no "save on quit").
  - The DB is backed up on startup, keeping the last **10**.
  - Migrations run in a transaction after the backup. A failed migration leaves the original file untouched.
  - Missing files are flagged, never deleted with their cues.
- The audio thread is untouched except for the one new engine command in Task 11, which must stay allocation-free (no_alloc test).
- Library DB default location: `directories::ProjectDirs::from("", "", "dj-software").data_dir()/library.sqlite3`. Every CLI command takes `--library-db <PATH>` to override it (used by tests).
- Designed for at least **50,000 tracks**: indexed queries and paged lists, with no "load everything" in hot paths.
- Conventional Commits, prek on every commit, branch `feat/plan-3-library`, landing via PR.

## Review Focus

1. **A file is moved or renamed outside the app.** On the next scan, its cues and analysis follow it, because identity is the audio fingerprint, not the path. Test: Task 2, `moved_file_keeps_its_cues`.
2. **A file's tags are edited in another app.** Same track: cues are kept and the tags are refreshed. Test: Task 2, `retagged_file_keeps_identity`.
3. **A crash in the middle of a cue write** leaves the DB consistent, either the old value or the new one. Test: Task 1, `writes_are_atomic_across_crash` (simulated by dropping the writer mid-batch).
4. **A huge or unreadable folder:** permission errors, symlink loops, non-audio files and zero-byte files are skipped with a count. The scan never aborts. Test: Task 2, `scan_skips_unreadable_and_non_audio`.
5. **Half-time/double-time BPM confusion on the owner's genres** (dubstep around 140, DnB around 174). Detection reports a tempo inside the configured range (default **90–180**), and halve/double fixes the rest. Test: Task 7, `dnb_174_not_reported_as_87`.

---

## File Structure

```
crates/library/src/{lib.rs,db.rs,schema.rs,backup.rs,writer.rs,scan.rs,fingerprint.rs,tags.rs,watch.rs,playlists.rs,cues.rs,grid.rs,browser.rs,jobs.rs}
crates/library/tests/{db.rs,scan.rs,watch.rs,playlists.rs,cues.rs,grid.rs,browser.rs,jobs.rs}
crates/analysis/src/{lib.rs,waveform.rs,onset.rs,tempo.rs,key.rs,notation.rs}
crates/analysis/tests/{waveform.rs,tempo.rs,key.rs}  + tests/support/synth.rs (test signal generators)
crates/decode/src/lib.rs              (truncated flag, prefix decode for fingerprints)
crates/engine/src/{command.rs,deck.rs} (DeckCommand::SetCue)
crates/actions/src/{action.rs,apply.rs,request.rs}
crates/controller/src/runtime.rs      (sum relative steps per run)
crates/app/src/{main.rs,library_cmd.rs,play.rs,console.rs}
controllers/ddj-sb3/mapping.toml      (browse + load bindings)
docs/library.md
```

---

### Task 1: Library database: schema, migrations, backups, writer thread

**Files:** Create `crates/library/{Cargo.toml,src/lib.rs,src/db.rs,src/schema.rs,src/backup.rs,src/writer.rs}` and `crates/library/tests/db.rs`. Add `rusqlite` (features `bundled`), `blake3`, `lofty`, `notify`, `walkdir` and `tempfile` to `[workspace.dependencies]`.

**Interfaces (produces):**
- `pub struct Library`, with `pub fn open(path: &Path) -> Result<Library, LibraryError>` doing these steps in order:
  1. Create the parent dirs.
  2. **Back up** the existing file to `<dir>/backups/library-<UTC yyyymmdd-hhmmss>.sqlite3` with the SQLite online backup API, keeping the newest 10.
  3. Open in WAL mode with `foreign_keys = ON`.
  4. Run migrations to the latest `PRAGMA user_version` inside one transaction.
  5. Start the writer thread.

  A migration error returns `LibraryError::Migration` with the DB left at its previous version. `Library::reader(&self) -> Reader` opens a read-only connection for the calling thread. `Library::writer(&self) -> Writer` is a cloneable handle.
- `Writer`: methods send a closure-free `WriteOp` enum over a bounded channel (capacity 1024) to the single writer thread, which applies each op in its own transaction. `fn submit(&self, op: WriteOp) -> Result<(), LibraryError>` returns once the op is queued. `fn submit_wait(&self, op: WriteOp) -> Result<WriteOutcome, LibraryError>` returns after commit. The CLI uses wait; the play loop uses submit.
- **Schema v1** (`schema.rs`, plain SQL strings):
  - `tracks(id INTEGER PK, path TEXT UNIQUE NOT NULL, fingerprint BLOB NOT NULL, size INTEGER, mtime INTEGER, missing INTEGER NOT NULL DEFAULT 0, title TEXT, artist TEXT, album TEXT, genre TEXT, duration_frames INTEGER, sample_rate INTEGER, added_at INTEGER NOT NULL)`, with an index on `fingerprint` and indexes on `artist`/`title` (`COLLATE NOCASE`).
  - `folders(id INTEGER PK, path TEXT UNIQUE NOT NULL)`.
  - `analysis(track_id PK REFERENCES tracks ON DELETE CASCADE, analyzer_version INTEGER NOT NULL, bpm REAL, first_beat_frames REAL, grid_locked INTEGER NOT NULL DEFAULT 0, key INTEGER, waveform BLOB, analyzed_at INTEGER)`.
  - `cues(track_id PK REFERENCES tracks, cue_frames REAL NOT NULL, updated_at INTEGER)`.
  - `hotcues(track_id REFERENCES tracks, slot INTEGER CHECK(slot BETWEEN 1 AND 8), frames REAL NOT NULL, color INTEGER, name TEXT, PRIMARY KEY(track_id, slot))`.
  - `playlists(id INTEGER PK, name TEXT NOT NULL, position INTEGER NOT NULL)`.
  - `playlist_tracks(playlist_id REFERENCES playlists ON DELETE CASCADE, track_id REFERENCES tracks, position INTEGER NOT NULL, PRIMARY KEY(playlist_id, position))`.
  - `settings(key TEXT PK, value TEXT)`.

  Tracks are never deleted by scans. Cues and hotcues deliberately have **no** cascade on track delete; deleting a track is a separate explicit user action, not in this plan.
- Migrations are an ordered `&[&str]` list. Version N runs `MIGRATIONS[N-1]`.

- [ ] **Step 1: Write failing tests** (`tests/db.rs`, using tempdirs):
  - `open_creates_schema_v1` (asserts user_version == 1 and that the tables exist).
  - `open_backs_up_existing_db_and_keeps_ten` (open 12 times → 10 backups, newest kept).
  - `failed_migration_leaves_previous_version`: inject a broken migration via a test-only `open_with_migrations(path, &[...])`; user_version is unchanged and the data is intact.
  - `writer_applies_ops_in_order`.
  - `writes_are_atomic_across_crash`: submit a multi-row op, kill the writer thread mid-op via a test hook that panics inside the transaction, then reopen; either none or all rows exist.
  - `concurrent_reader_sees_committed_data`.
- [ ] **Step 2: Run** `cargo nextest run -p library`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: all pass, `cargo deny check` clean (rusqlite/libsqlite3-sys are MIT; SQLite is public domain).
- [ ] **Step 5: Commit:** `feat(library): add SQLite library with migrations, backups and a writer thread`

---

### Task 2: Scanning folders: tags, audio fingerprints, relinking

**Files:** Create `crates/library/src/{scan.rs,fingerprint.rs,tags.rs}` and `crates/library/tests/scan.rs`. Modify `crates/decode/src/lib.rs`, adding `pub fn decode_prefix(path: &Path, max_frames: usize) -> Result<TrackBuffer, DecodeError>`, which stops after `max_frames` and reuses the decode loop.

**Interfaces:**
- `fingerprint::of_file(path) -> Result<[u8; 32], FingerprintError>`: blake3 of the first **3 seconds** of decoded audio (the native rate, as the sample rate, frames count and samples quantized to i16 LE). Because it hashes decoded audio, editing tags or moving the file doesn't change it.
- `tags::read(path) -> TrackTags { title, artist, album, genre, duration_frames, sample_rate }` uses lofty. It falls back to the file stem as the title. Unreadable tags → defaults, never an error.
- `pub fn scan_folder(lib: &Library, root: &Path, progress: &mut dyn FnMut(ScanProgress)) -> Result<ScanReport, LibraryError>`, using walkdir: it follows symlinks with loop detection, and only takes the extensions `mp3, wav, aif, aiff, flac, m4a, aac, ogg`. Per file:
  - unchanged path + size + mtime → skip;
  - known path, changed size or mtime → re-read tags and the fingerprint (same id);
  - new path whose fingerprint matches a `missing` or no-longer-existing track → **relink** (update path, clear missing, keep id);
  - otherwise insert.

  After the walk, tracks under `root` whose file no longer exists are set `missing = 1`. `ScanReport { added, updated, relinked, missing, skipped: Vec<(PathBuf, String)> }`. `add_folder(path)` and `remove_folder(path)` only change the `folders` table. Removing a folder never deletes tracks.
- Test fixtures: reuse `crates/decode/tests/fixtures/*` copied into tempdirs.

- [ ] **Step 1: Write failing tests:**
  - `scan_adds_supported_files_with_tags`.
  - `rescan_unchanged_is_a_noop` (`ScanReport` all zero).
  - `moved_file_keeps_its_cues`: insert a cue for the track, move the file to a subfolder, rescan → same track id, path updated, cue intact.
  - `retagged_file_keeps_identity`: rewrite the WAV's LIST/INFO chunk with a test helper, so the size and mtime change but the audio doesn't → same id, new title.
  - `deleted_file_is_flagged_missing_not_removed`.
  - `scan_skips_unreadable_and_non_audio`: a zero-byte .mp3, a .txt, a corrupt .mp3, a symlink loop (unix only, `#[cfg(unix)]`), and a dir without read permission (unix only) → all in `skipped`, the scan completes, valid files added.
  - `fingerprint_ignores_tags_but_not_audio`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo nextest run -p library -p decode`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(library): scan folders with audio fingerprints and relinking`

---

### Task 3: Watching folders for changes

**Files:** Create `crates/library/src/watch.rs` and `crates/library/tests/watch.rs`.

**Interfaces:**
- `pub fn watch(lib: &Library, roots: &[PathBuf]) -> Result<Watcher, LibraryError>`, using notify's recommended watcher. Events are **debounced 2 s** per path, then fed to the same per-file logic as `scan_folder` (a single-path variant `scan_path`).
- Removals mark the track missing.
- Watcher errors (e.g. too many watches on Linux) are reported through `Watcher::errors()`, and the app falls back to periodic rescans every 10 minutes.
- Dropping the `Watcher` stops it.

- [ ] **Step 1: Write failing tests:**
  - `new_file_appears_after_debounce`.
  - `removed_file_becomes_missing`.
  - `burst_of_events_scans_once`.

  Each uses a polling loop with a 10 s cap, not fixed sleeps, and is marked `#[cfg_attr(target_os = "windows", ignore = "notify timing on CI")]` only if CI shows flakiness.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: pass.
- [ ] **Step 5: Commit:** `feat(library): watch library folders for changes`

---

### Task 4: Playlists and set-prep queries

**Files:** Create `crates/library/src/playlists.rs` and `crates/library/tests/playlists.rs`.

**Interfaces (writer ops plus reader queries):**
- Ops:
  - `CreatePlaylist { name }` → id.
  - `RenamePlaylist`, `DeletePlaylist` (deletes membership, never tracks).
  - `MovePlaylist { id, to_position }`.
  - `AddToPlaylist { playlist, track, at: Option<usize> }`. Duplicates are allowed, since a DJ may play a track twice.
  - `RemoveFromPlaylist { playlist, position }`.
  - `MoveInPlaylist { playlist, from, to }`.

  Positions stay dense, 0..n, inside the op's transaction.
- Queries:
  - `playlists() -> Vec<PlaylistSummary { id, name, track_count, unprepared_count }>`, where unprepared = tracks with no row in `cues` **and** no hotcues.
  - `playlist_tracks(id, offset, limit) -> Vec<TrackRow>`.
  - `tracks(filter: TrackFilter { folder: Option<PathBuf>, text: Option<String>, missing: Option<bool> }, sort: TrackSort, offset, limit) -> Vec<TrackRow>`.

  `TrackRow { id, title, artist, bpm: Option<f64>, key: Option<u8> /* raw 0..=23, formatted with analysis::Key from Task 8 */, duration_frames, sample_rate, has_cues: bool, missing: bool }`. Sorting by title, artist, bpm, key or added uses the indexes. Text search is a case-insensitive `LIKE` on title/artist.

- [ ] **Step 1: Write failing tests:**
  - `create_rename_delete_playlist`.
  - `add_move_remove_keep_positions_dense`.
  - `deleting_playlist_keeps_tracks`.
  - `unprepared_count_counts_tracks_without_cues`.
  - `paged_queries_return_stable_order`.
  - `text_search_is_case_insensitive`.
  - `queries_stay_fast_with_50k_tracks`: insert 50,000 synthetic rows; a sorted, paged query by artist completes in under 50 ms in release builds. Mark it `#[ignore]` in debug builds; CI runs it with `--release`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: pass.
- [ ] **Step 5: Commit:** `feat(library): add playlists and set-prep queries`

---

### Task 5: Decoder follow-ups (carried from plan 1)

**Files:** Modify `crates/decode/src/lib.rs`, `scripts/gen-fixtures.sh`, `crates/decode/tests/decode.rs`. Add the fixtures `sine440_s16be.aiff` and `sine440_s24.wav` (mono, 1 s, each under 140 KB).

**Interfaces:** `decode_file` now returns `Decoded { track: TrackBuffer, truncated: bool }` (`truncated` = a read or decode error ended the track after some audio). Update all callers (the app's `play`, the library). The library stores nothing about it yet. The app prints `warning: <path> may be incomplete (decoding stopped early)`.

- [ ] **Step 1: Write failing tests:**
  - `big_endian_16_bit_aiff_decodes`.
  - `24_bit_wav_decodes` (44,100 frames, 440 ± 5 Hz).
  - `truncated_file_reports_truncated` (`truncated.mp3` → `truncated == true`; `sine440.mp3` → false).
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `mise run test`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(decode): report truncated decodes and cover 16/24-bit PCM`

---

### Task 6: Waveform analysis

**Files:** Create `crates/analysis/{Cargo.toml,src/lib.rs,src/waveform.rs}`, `crates/analysis/tests/waveform.rs` and `crates/analysis/tests/support/synth.rs`. Add `rustfft` to workspace deps.

**Interfaces:**
- `pub struct Waveform { pub frames_per_bucket: u32, pub buckets: Vec<[u8; 4]> }`, where each bucket is `[peak, low, mid, high]` scaled 0..=255:
  - `peak` = max |sample| over L/R.
  - `low`/`mid`/`high` = RMS of the signal split at **250 Hz and 2500 Hz**, using the same crossover points as the mixer's isolator (with its own small LR4 filters; don't depend on the engine), normalized by the track's loudest band value.
- `pub fn waveform(track: &TrackBuffer, frames_per_bucket: u32) -> Waveform`, with default `frames_per_bucket = 256`.
- `Waveform::to_bytes() / from_bytes()` (versioned, 1-byte header) for the DB blob.
- `pub fn downsample(&self, factor: u32) -> Waveform` for overview levels.
- `synth.rs` (test support):
  - `sine(freq, secs, sr)`
  - `clicks(bpm, secs, sr, first_beat_secs)`, kick-like: 60 Hz sine burst with a 50 ms decay plus a short noise transient.
  - `chord(notes_hz, secs, sr)`
  - `concat`, `stereo`

- [ ] **Step 1: Write failing tests:**
  - `bucket_count_matches_length`.
  - `silence_is_zero`.
  - `bass_sine_lands_in_low_band` (60 Hz: low ≫ mid, high).
  - `treble_sine_lands_in_high_band` (8 kHz).
  - `peak_tracks_amplitude`.
  - `bytes_round_trip`.
  - `downsample_keeps_max_peak`.
- [ ] **Step 2: Run** `cargo nextest run -p analysis`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: pass.
- [ ] **Step 5: Commit:** `feat(analysis): compute three-band waveform peaks`

---

### Task 7: BPM and beatgrid detection

**Files:** Create `crates/analysis/src/{onset.rs,tempo.rs}` and `crates/analysis/tests/tempo.rs`.

**Interfaces:**
- `pub struct BpmRange { pub min: f64, pub max: f64 }`. Default **90–180** (my call for the owner's dubstep/DnB library; configurable in Task 9).
- `pub struct Beatgrid { pub bpm: f64, pub first_beat_frames: f64 }`: constant tempo; beat `n` lands at `first_beat_frames + n * 60 * sr / bpm`.
- `pub fn detect_beatgrid(track: &TrackBuffer, range: BpmRange) -> Option<Beatgrid>`. `None` for silence or tracks shorter than 10 s. Approach (fixed for the implementer):
  1. **Onset envelope:** mono-mix and resample (linear) to 11,025 Hz. STFT with a 1024-sample Hann window and hop 128 (about 86 envelope frames per second). Spectral flux = sum of positive log-magnitude increases. Subtract a 0.5 s moving mean and half-wave rectify.
  2. **Tempo:** autocorrelation of the envelope over lags covering 60–240 BPM, weighted by a log-Gaussian centered at 120 BPM (σ = 1 octave), plus harmonic reinforcement (add the 2× and 0.5× lag values at weight 0.5). Fold every candidate into `range` by doubling/halving, then pick the strongest. Refine with parabolic interpolation, and then by maximizing the summed envelope over a comb at that period ±0.5% in 0.01 BPM steps.
  3. **Phase:** choose the offset in [0, period) that maximizes the comb sum. Convert to frames at the track's native rate, then move back to the earliest beat ≥ 0.
  4. **Round:** if within ±0.03 of an integer BPM, snap to the integer (most electronic tracks have integer tempos).
- [ ] **Step 1: Write failing tests** (synthetic signals, 30 s unless stated):
  - `detects_128_bpm_and_phase` (first beat at 0.37 s → bpm 128.00 ± 0.02, first_beat within ±10 ms).
  - `detects_174_bpm`.
  - `dnb_174_not_reported_as_87` (clicks at 174 plus snare-like hits on beats 2 and 4 → 174).
  - `dubstep_140_half_time_reports_140` (kick on 1, snare on 3 of a 140 BPM bar, hats on every eighth note → 140, not 70).
  - `non_integer_bpm_not_snapped` (123.5 → 123.50 ± 0.03).
  - `silence_returns_none`.
  - `short_track_returns_none`.
  - `range_folds_out_of_range_candidates` (clicks at 75 with range 90–180 → 150).
  - `noise_does_not_panic`.

  Plus `#[ignore] analyze_real_files_from_env`: reads `DJ_ANALYSIS_DIR`, prints `path bpm first_beat key` for manual comparison in the hands-on check.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.** Pure functions, no global state. Allocation is fine (worker threads).
- [ ] **Step 4: Run.** Expected: pass in debug and release (`cargo nextest run -p analysis --release`).
- [ ] **Step 5: Commit:** `feat(analysis): detect BPM and a constant beatgrid`

---

### Task 8: Key detection and notation

**Files:** Create `crates/analysis/src/{key.rs,notation.rs}` and `crates/analysis/tests/key.rs`.

**Interfaces:**
- `pub struct Key(u8)`, 0..=23: 0–11 = C, C♯ … B major; 12–23 = C … B minor. `Key::camelot() -> &'static str` ("8B" = C major, "8A" = A minor, …), `Key::standard() -> String` ("C", "Am", "F♯m"…), `Key::open_key() -> &'static str` ("1d" = C major, "1m" = A minor). `KeyNotation { Camelot, Standard, OpenKey }`, default **Camelot**, the VirtualDJ default (my call).
- `pub fn detect_key(track: &TrackBuffer) -> Option<Key>`:
  1. Mono, resample to 11,025 Hz, STFT with a 4096-sample window and hop 2048.
  2. Map bins 55–1760 Hz to 12 pitch classes with log-magnitude weighting.
  3. Average the chroma over the track, skipping frames below −60 dBFS.
  4. Correlate with **Temperley's major/minor profiles** rotated over 12 roots and pick the best.
  5. `None` for silence.
- [ ] **Step 1: Write failing tests:**
  - `notation_tables` (every key's three notations match the published Camelot/Open Key wheels).
  - `c_major_progression_is_8b` (I–IV–V–I triads with sine partials, 4 s each).
  - `a_minor_progression_is_8a`.
  - `f_sharp_minor_is_11a`.
  - `silence_returns_none`.
  - `transposition_moves_key` (shift the C-major test up a fifth → G = 9B).
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: pass.
- [ ] **Step 5: Commit:** `feat(analysis): detect musical key with Camelot, Open Key and standard notation`

---

### Task 9: Analysis jobs: worker pool, caching, locked grids

**Files:** Create `crates/library/src/jobs.rs` and `crates/library/tests/jobs.rs`. Modify `crates/library/Cargo.toml` (depends on `analysis`, `decode`).

**Interfaces:**
- `pub const ANALYZER_VERSION: u32 = 1` (bump it when an analysis algorithm changes).
- `pub struct Analyzer`, with `Analyzer::start(lib: &Library, threads: usize) -> Analyzer`. The default thread count is `available_parallelism − 1` (minimum 1). Workers run at lower priority where the OS allows (`thread::Builder` with no extra crate; best-effort, skip if unsupported).
  - `enqueue(track_ids, force: bool)`: tracks whose `analysis.analyzer_version == ANALYZER_VERSION` are skipped unless `force`.
  - `progress() -> AnalysisProgress { queued, done, failed }`. `failed` carries `(track_id, error)` items.
- Each job:
  1. Decode the track.
  2. Compute the waveform, the beatgrid (using the `bpm_range` from `settings`, default 90–180) and the key.
  3. Write them in one op.
  - **If `grid_locked`, keep the stored bpm and first beat** and only update the waveform and key.
  - A missing file → `failed`, without touching existing analysis.
- `Library::analysis(track_id) -> Option<AnalysisRow { bpm, first_beat_frames, grid_locked, key, waveform }>`.
- Settings: op `SetSetting { key, value }` and query `setting(key) -> Option<String>`. Keys used so far: `bpm_range` ("90-180") and `key_notation` ("camelot").
- [ ] **Step 1: Write failing tests:**
  - `enqueue_analyzes_and_stores` (a 30 s synthetic 128 BPM WAV written to a tempdir → stored bpm 128).
  - `up_to_date_tracks_are_skipped`.
  - `force_reanalyzes`.
  - `locked_grid_survives_reanalysis`.
  - `missing_file_fails_without_touching_analysis`.
  - `bpm_range_setting_is_used`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: pass.
- [ ] **Step 5: Commit:** `feat(library): run analysis jobs in the background with caching`

---

### Task 10: Beatgrid editing

**Files:** Create `crates/library/src/grid.rs` and `crates/library/tests/grid.rs`. Modify `crates/actions/src/{action.rs,apply.rs}` and create `crates/actions/src/request.rs`.

**Interfaces:**
- `pub enum GridEdit { Shift { frames: f64 }, SetDownbeat { at_frames: f64 }, Halve, Double, SetBpm(f64), Tap { at_micros: u64 }, ToggleLock }`. `Library` op `EditGrid { track, edit, playhead_frames: Option<f64> }` → the new `AnalysisRow`.
- Rules:
  - `Shift` moves `first_beat_frames` and wraps it into [0, period).
  - `SetDownbeat` puts a beat exactly at `at_frames` (first_beat = at mod period).
  - `Halve`/`Double` change the bpm and keep the beat at the playhead if given, otherwise the first beat.
  - `SetBpm` keeps the first beat. It accepts 40–300 and rejects anything else with an error.
  - `Tap`: the library keeps the last taps per track in memory. Taps more than 2 s apart restart the sequence. From 4 taps on, the bpm = 60 / mean interval, rounded to 0.01.
  - **Every edit except ToggleLock also sets `grid_locked = 1`**, so a manual correction is never overwritten by re-analysis. That matches the spec's "lock" intent; ToggleLock flips it explicitly.
  - Edits on a track with no analysis create a row with defaults (bpm required for Shift/SetDownbeat → error "analyze or set a BPM first").
- **Actions** (new `ActionKind`s, deck target, Press):
  - `deck.grid_shift_back`, `deck.grid_shift_forward`: ±**5 ms** at the track's rate, ±1 ms with SHIFT on the binding's side.
  - `deck.grid_set_downbeat`, `deck.grid_halve`, `deck.grid_double`, `deck.grid_tap`, `deck.grid_lock_toggle`.

  `apply` returns the new `ApplyResult::Request(AppRequest)`, with `pub enum AppRequest { Grid { deck: DeckId, edit: GridEditRequest } }` defined in `actions::request` (Task 12 adds the `Browser` and `LoadSelected` variants) (`GridEditRequest` mirrors `GridEdit` without frames; the app fills in the playhead and track rate). The controller runtime passes requests out of `handle_batch` (`BatchStats.requests: Vec<AppRequest>`).
- [ ] **Step 1: Write failing tests:**
  - `grid.rs`: one test per rule above (exact values), plus `edits_lock_the_grid`, `toggle_lock_flips` and `shift_without_bpm_errors`.
  - actions: `grid_actions_return_requests`, `ids_round_trip_for_every_action` (updated), `docs_list_every_action_id` (update `docs/controllers/mapping-reference.md`).
  - controller `runtime.rs`: `requests_are_returned_in_order`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `mise run test`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(library): add beatgrid editing and grid actions`

---

### Task 11: Remembered cue points

**Files:** Create `crates/library/src/cues.rs` and `crates/library/tests/cues.rs`. Modify `crates/engine/src/{command.rs,deck.rs}`, `crates/engine/tests/{cue.rs,no_alloc.rs}`.

**Interfaces:**
- Engine: `DeckCommand::SetCue(f64)` sets the cue point (clamped to `[0, frames]`; non-finite ignored) without moving the playhead or changing play state. It's covered by the no_alloc test.
- Library:
  - op `SetCue { track, frames }` (upsert).
  - query `cue(track) -> Option<f64>`.
- The app (Task 13) applies the saved cue right after loading a track, and writes the cue through whenever `DeckSnapshot.cue_frames` changes for a loaded track id. It compares on each control-loop tick and submits through the non-blocking `Writer::submit`.
- [ ] **Step 1: Write failing tests:**
  - engine:
    - `set_cue_sets_cue_without_moving` (paused at 1000, SetCue(500) → cue 500, position 1000, paused).
    - `set_cue_clamps_and_ignores_nan`.
  - library:
    - `cue_upsert_and_read`.
    - `cue_survives_reopen`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: pass.
- [ ] **Step 5: Commit:** `feat(library): remember each track's cue point`

---

### Task 12: Browser model and controller browse/load

**Files:** Create `crates/library/src/browser.rs` and `crates/library/tests/browser.rs`. Modify `crates/actions/src/{action.rs,apply.rs,request.rs}`, `crates/controller/src/runtime.rs`, `controllers/ddj-sb3/mapping.toml` and `crates/controller/tests/sb3_replay.rs`.

**Interfaces:**
- `pub struct Browser`, which holds a `Reader`:
  - Tree: "Playlists" (each playlist), "Folders" (each library folder), "All tracks".
  - Focus: `Tree` or `List`.
  - `apply(&mut self, req: BrowserRequest) -> BrowserView`, where `BrowserRequest { Scroll(i32), ToggleFocus }`.
    - `Scroll` moves the cursor in the focused pane, clamped (no wrap), and a tree move resets the list cursor to 0.
    - `ToggleFocus` switches panes.
  - `selected_track() -> Option<TrackRow>`.
  - `BrowserView { path: String /* "Playlists ▸ Peak time" */, index, count, track: Option<TrackRow>, focus }`.

  Lists are loaded in pages of 200 around the cursor.
- `AppRequest` gains `Browser(BrowserRequest)` and `LoadSelected { deck: DeckId }`.
- **Actions:**
  - `browser.scroll`: global, accepts **Relative**. This is the first relative action, so update `ActionKind::accepts` and the docs.
  - `browser.toggle_focus`: global, Press.
  - `deck.load_selected`: deck target, Press. It returns `AppRequest::LoadSelected { deck }`.

  Mapping validation now allows `relative_*` values for `browser.scroll`.
- **Runtime:** consecutive Relative inputs for the same target within a run are **summed** into one action (carried from plan 2's review). Test `relative_steps_are_summed_per_run`.
- **SB3 mapping (capture values):**
  - BROWSE rotate `B6 40` → `browser.scroll`, `value = "relative_twos"` (capture: 1 = clockwise = down the list, 127 = counter-clockwise).
  - BROWSE press `96 41` → `browser.toggle_focus`.
  - LOAD left `96 46` → `deck.load_selected` `side = "left"`; LOAD right `96 47` → `side = "right"`.

  Remove these from the "not mapped yet" block and extend the replay tests (`browse_scrolls_and_load_requests_side_deck`, which loads to deck 3 after a DECK switch).
- [ ] **Step 1: Write failing tests:**
  - `browser.rs`: `tree_lists_playlists_folders_all`, `scroll_clamps`, `tree_move_resets_list`, `toggle_focus`, `selected_track_follows_cursor`, `pages_large_lists` (5,000 tracks, scrolling to 4,999 works).
  - runtime: `relative_steps_are_summed_per_run`.
  - replay: as above.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `mise run test`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(library): browse the library and load tracks from the controller`

---

### Task 13: CLI: library commands, library-mode player, console

**Files:** Create `crates/app/src/{library_cmd.rs,console.rs}`. Modify `crates/app/src/{main.rs,play.rs}`.

**Interfaces:**
- **`dj-software library`** with `[--library-db PATH]` on every subcommand:
  - `add-folder <DIR>` (adds, then scans with a progress line).
  - `remove-folder <DIR>`.
  - `scan` (all folders, prints the `ScanReport` with up to 20 skipped files and a count).
  - `list [--playlist NAME | --folder DIR] [--search TEXT] [--sort title|artist|bpm|key|added] [--limit N]` (table: id, artist, title, BPM, key in the configured notation, length, a `•` for cues, `missing`).
  - `analyze [--all | --track ID…] [--force]` (progress bar line, then failures).
  - `set bpm-range <MIN> <MAX>` and `set key-notation camelot|standard|open-key`.
  - `playlist create|rename|delete|add|remove|move|list`.
  - `grid <TRACK> show|shift <MS>|bpm <BPM>|halve|double|lock|unlock`.
- **`play --library [--controller …]`** (library mode; tracks optional):
  - Opens the library and starts the `Analyzer` for not-yet-analyzed tracks in the background.
  - The browser line updates in place, on its own status line above the deck lines: `▸ Playlists ▸ Peak time  12/48  Artist – Title  128.00 BPM  8A  •`.
  - `AppRequest::LoadSelected { deck }`:
    1. Decode on a loader thread, never on the control loop.
    2. Send `Load`.
    3. Apply the saved cue with `SetCue`.
    4. Print `Deck 3 ◂ Artist – Title`.

    Loading onto a playing deck is allowed (spec silent; VDJ allows it with a setting; my call: allow, and print a warning line).
  - `AppRequest::Grid`: fill in the deck's playhead and the track's sample rate, submit `EditGrid`, print the new grid.
  - Cue write-through per Task 11.
  - Status lines show each deck's BPM × tempo ratio once analyzed.
- **Console** (`console.rs`): stdin lines on the existing helper thread (only when stdin is a terminal). Blank line = reconnect the controller (existing). Commands:
  - `grid <deck> back|forward|downbeat|halve|double|tap|lock` → the same `AppRequest` path.
  - `load <deck> <track-id>`.
  - `help`.
- [ ] **Step 1: Write failing tests** (clap parsing for every subcommand, plus console line parsing):
  - `parses_library_list_with_filters`.
  - `parses_grid_shift_ms`.
  - `console_parses_grid_and_load`.
  - `console_rejects_unknown`.
  - An end-to-end `library_cli_round_trip` that runs `add-folder`, `analyze` and `list` against a tempdir library with a synthetic 128 BPM WAV and checks that the output contains `128.00`. Run through `assert_cmd`-free `std::process::Command` on the built binary (`env!("CARGO_BIN_EXE_dj-software")`).
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `mise run test && prek run --all-files`. Expected: pass.
- [ ] **Step 5: Commit:** `feat(app): add library commands and a library-mode player`

---

### Task 14: Docs and hands-on check

**Files:** Create `docs/library.md` (where the DB and backups live per OS, how to restore a backup, what scanning does with moved/renamed/deleted files, analysis and its limits, beatgrid editing, console commands). Modify `README.md`, `docs/architecture.md` (library and analysis crates, writer thread, analyzer pool) and `controllers/ddj-sb3/README.md` (browse/load).

- [ ] **Step 1: Write the docs** (every command and flag must exist; add a docs test like `docs_list_every_action_id` if new ids were added).
- [ ] **Step 2: Run** `mise run test && prek run --all-files`.
- [ ] **Step 3: Commit:** `docs: document the library, analysis and beatgrid editing`
- [ ] **Step 4: Hands-on check with the owner** (controller-run). With their real music folder:
  1. `library add-folder "<their folder>"` (timing and skipped count noted).
  2. `library analyze --all`.
  3. Compare BPM and key with VirtualDJ on 10 tracks the owner picks (mixed genres, including dubstep and DnB).
     - **Target:** BPM within ±0.5 on at least 9 of 10 (after at most one halve/double), and key matching VDJ's on at least 7 of 10.
     - Record misses in `docs/library.md` under "known limits".
  4. `play --library --controller ddj-sb3 --device "DDJ-SB3" --master 3,4`:
     - browse with the knob, press to switch panes, LOAD left/right including onto deck 3;
     - set a cue, reload the track and check the cue came back;
     - fix one grid with the console (`grid 1 downbeat` at a kick, `grid 1 halve`).
  5. Move one analyzed file to another subfolder, run `library scan`, and confirm its cue and grid are still there.

  Record the owner's confirmation and any misses.
