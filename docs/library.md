# Library, analysis and beatgrids

The library is a SQLite database of the tracks in your music folders: tags, cue points,
beatgrids, BPM, key and waveforms, and playlists. Audio files are only read, never
modified, and tags are read, never written.

## Quick start

```sh
dj-software library add-folder ~/Music/DJ      # registers the folder, then scans it
dj-software library analyze                    # BPM, beatgrid, key and waveform
dj-software library list --sort bpm --limit 20
dj-software play --library --controller ddj-sb3 --device "DDJ-SB3" --master 3,4
```

Every `library` subcommand takes `--library-db <PATH>` to use another database file, and
so does `play --library`.

## Where the database and backups live

The default database is `library.sqlite3` in the per-user data directory:

| OS | Data directory |
|---|---|
| macOS | `~/Library/Application Support/dj-software` |
| Windows | `%APPDATA%\dj-software\data` |
| Linux | `$XDG_DATA_HOME/dj-software`, or `~/.local/share/dj-software` |

Every change to cues, grids, playlists and settings is written to the database at once
(there is no "save on quit"). When the library is opened, the existing database is first
copied to `backups/library-<UTC yyyymmdd-hhmmss>.sqlite3` next to it, and only the 10
newest backups are kept. The copy is skipped when the newest backup is less than 10
minutes old (so a series of short commands does not push the real backups out), unless a
schema migration is pending: then a backup is always taken first. A migration runs in a
transaction, so a failed one leaves the original file untouched.

### Restoring a backup

1. Quit the app, and any `library` command that is still running.
2. Copy the backup you want over `library.sqlite3` in the data directory.
3. Delete `library.sqlite3-wal` and `library.sqlite3-shm` next to it, if they exist. They
   belong to the old database and must not be applied to the restored one.

## What a scan does

`library add-folder <DIR>` registers a folder and scans it. `library scan` scans every
registered folder. `library remove-folder <DIR>` unregisters a folder; its tracks and
cues are kept. While `play --library` runs, the folders are also watched (see below).

- Files with the extensions mp3, wav, aif, aiff, flac, m4a, aac and ogg (any case) are
  tracks. Hidden files and folders (names starting with a dot) are skipped. Other files
  are not tracks and are only counted as "ignored".
- A track is identified by a fingerprint of the first 3 seconds of its decoded audio,
  not by its path or file bytes. Changing tags, renaming or moving a file does not change
  it, so the track keeps its id, cues, grid and analysis.
- **Moved or renamed files:** a new path whose fingerprint matches a track whose old file
  no longer exists is relinked (same track). It only counts when the track's old folder
  exists: with an unplugged drive, tracks are not relinked from it.
- **Deleted files** are flagged `missing`, never deleted, so their cues survive. If the
  file comes back, the track is found again.
- **Changed files:** when a file's size or modification time changes, its tags and
  fingerprint are read again. If the audio itself changed, the old analysis (BPM, grid,
  key, waveform) is deleted, since it no longer describes the file. Cues are kept.
- Files that cannot be read or decoded are listed as skipped, with the reason, and left
  out. A registered folder that is not found (an unplugged drive) is skipped with a note.

The scan report looks like `added 12, updated 0, audio changed 0, relinked 1, missing 0,
ignored 3 non-audio file(s)`.

### Watching

With `play --library`, the registered folders are watched. A path must stay quiet for
2 seconds before it is looked at, so a file that is still being copied is not read half
written. Removals flag tracks missing and a rename relinks the track. If the operating
system's watcher fails (for example, too many watches on Linux), the app prints
"library watching degraded" and rescans the affected folders every 10 minutes instead.
New tracks found this way are analyzed in the background.

## Analysis

`library analyze` analyzes the tracks that are not analyzed yet. `--all` is every track
(up-to-date ones are skipped), `--track ID...` only those tracks, and `--force` analyzes
again even if the track is up to date (a locked grid is kept). Analysis runs on a pool of
worker threads (one fewer than the CPU cores, at least one) and is also redone for tracks
analyzed by an older version of the algorithms.

What it produces:

- **Waveform:** three bands, low (below 250 Hz), mid (250 to 2500 Hz) and high (above
  2500 Hz), over a 50 ms window, plus the peak. It is stored at 1024 frames per bucket.
- **BPM and beatgrid:** one constant grid: a BPM and the position of the first beat.
- **Key:** the musical key, shown in one of three notations.

### BPM range

Detected tempos are folded into a range, 90 to 180 BPM by default: a tempo outside it is
doubled or halved until it fits, so 85 BPM reads as 170 and 185 BPM as 92.5.

```sh
dj-software library set bpm-range 90 180
```

The maximum must be at least twice the minimum, within 30 to 480. Ranges whose minimum is
below about 70 BPM print a warning, because half-time tracks (dubstep) then read at half
tempo. A new range applies to new analyses; run `library analyze --all --force` to redo
the others.

### Key notation

```sh
dj-software library set key-notation camelot     # 8A (the default shown in the lists)
dj-software library set key-notation standard    # Am
dj-software library set key-notation open-key    # 1m
```

The Camelot and Open Key names are those of the published wheels. The setting only
changes how keys are shown; the stored key is the same.

### Known limits

- **There is no confidence value.** The detectors always answer: a key and a grid are
  also given for percussion-only tracks, noise or speech. Treat them as a starting point.
- **Tempo errors:** besides halved and doubled tempos (fix with `halve` or `double`),
  tracks with triplet hi-hats can come out at a 3:4 ratio of the real tempo. Set the
  BPM by hand or tap it.
- **One constant grid per track.** Tracks that change tempo, and long DJ mixes (the list
  shows a note for tracks over 30 minutes), do not fit one grid. Beat positions drift
  away from the audio over time.
- **Key** detection is by the track's overall pitch content. It can be wrong for
  tracks that are atonal, change key, or have little bass or harmony.
- The first beat is the detector's best guess. Check it by ear and use `downbeat`.

Results of comparing the detectors with other software on real music are added here.

## Beatgrid editing

You can correct a track's grid from the console while `play --library` runs, and from
the CLI. **Every edit except lock/unlock locks the grid**, so a later `library analyze
--force` (or a new analysis version) keeps your correction. Unlock it to let analysis
replace it.

Edits and their rules:

- **Shift:** moves all beats earlier or later: 5 ms, or 1 ms with SHIFT held (on a
  controller). On the CLI, `library grid <ID> shift <MS>` takes any number of
  milliseconds (negative is earlier).
- **Set downbeat:** moves the grid so a beat falls exactly at the playhead, keeping the
  BPM. Stop on a kick drum and use it.
- **Halve and double:** halve or double the BPM, keeping the beat nearest the playhead
  (or the first beat if the track has no playhead).
- **Set BPM:** 40 to 300, keeping the first beat (`library grid <ID> bpm <BPM>`).
- **Tap:** tap the tempo; from 4 taps on, the BPM is set from the average interval of the
  latest 8 taps. A gap of more than 2 seconds between taps starts a new sequence.
- **Lock/unlock:** toggles the lock without changing the grid.

The track needs a BPM first (analyze it, or set one with `library grid <ID> bpm`).

```sh
dj-software library grid 12 show          # BPM, first beat, lock state
dj-software library grid 12 shift -5.5
dj-software library grid 12 bpm 140
dj-software library grid 12 halve
dj-software library grid 12 lock          # or: unlock
```

The grid actions (`deck.grid_shift_back`, `deck.grid_shift_forward`,
`deck.grid_set_downbeat`, `deck.grid_halve`, `deck.grid_double`, `deck.grid_tap`,
`deck.grid_lock_toggle`) exist for mappings, but the DDJ-SB3 mapping does not bind them
yet: use the console.

## Playlists

```sh
dj-software library playlist create "Peak time"
dj-software library playlist add "Peak time" 12 40 7 [--at 2]
dj-software library playlist list ["Peak time"]
dj-software library playlist move "Peak time" 3 1
dj-software library playlist remove "Peak time" 2
dj-software library playlist rename "Peak time" "Warm up"
dj-software library playlist delete "Peak time"
```

Positions start at 1. Deleting a playlist keeps its tracks in the library. Playlists
appear in the browser tree in library mode.

## Listing tracks

`library list` prints id, artist, title, BPM, key, length, a bullet for tracks with
cues, and `missing` for missing files. Filters: `--playlist NAME`, `--folder DIR`,
`--search TEXT` (title or artist, case-insensitive); `--sort title|artist|bpm|key|added`
(default artist) and `--limit N` (default 100).

## Library mode: `play --library`

```sh
dj-software play --library --controller ddj-sb3 --device "DDJ-SB3" --master 3,4
```

`--library` needs `--controller`. The track argument is optional. In this mode:

- The BROWSE knob scrolls, pressing it switches between the tree and the track list, and
  LOAD (left or right) loads the selected track onto the deck that side currently shows,
  decks 3 and 4 included. See [the DDJ-SB3 page](../controllers/ddj-sb3/README.md).
- A browser line is printed: `▸ <path>  <n>/<count>  Artist – Title  <BPM>  <key>  •`
  (the bullet means the track has cues, `missing` flags a missing file), followed by
  analysis progress while the background analysis runs.
- Tracks load in the background and a cue point you set is remembered with the track and
  restored when you load it again. A load onto a playing deck prints
  "warning: deck N was playing".
- Tracks not analyzed yet are analyzed in the background; failures and warnings are
  printed.
- Problems are printed as warnings and do not stop playback: a library change that could
  not be saved, a library worker that stopped, degraded folder watching.

### Console commands

Type these in the terminal running `play` (it must be a terminal):

| Command | Does |
|---|---|
| Enter on a blank line | reconnect the controller |
| `grid <deck> back` / `forward` | move the grid 5 ms earlier or later |
| `grid <deck> downbeat` | put a beat at the playhead |
| `grid <deck> halve` / `double` | halve or double the BPM |
| `grid <deck> tap` | tap the tempo |
| `grid <deck> lock` | lock or unlock the grid |
| `load <deck> <track-id>` | load a library track (ids from `library list`) |
| `help` | show the commands |

Decks are 1 to 4. `grid` and `load` need `play --library`.
