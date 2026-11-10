# Library, analysis and beatgrids

The library is a SQLite database of the tracks in your music folders: tags, cue points,
beatgrids, BPM, key and waveforms, and playlists. Audio files are only read, never
modified, and tags are read, never written.

## Quick start

```sh
open-jockey library add-folder ~/Music/DJ      # registers the folder, then scans it
open-jockey library analyze                    # BPM, beatgrid, key and waveform
open-jockey library list --sort bpm --limit 20
open-jockey play --library --controller ddj-sb3 --device "DDJ-SB3" --master 3,4
```

Every `library` subcommand takes `--library-db <PATH>` to use another database file, and
so does `play --library`.

## Where the database and backups live

The default database is `library.sqlite3` in the per-user data directory:

| OS | Data directory |
|---|---|
| macOS | `~/Library/Application Support/open-jockey` |
| Windows | `%APPDATA%\open-jockey\data` |
| Linux | `$XDG_DATA_HOME/open-jockey`, or `~/.local/share/open-jockey` |

The app used to be called `dj-software`. On its first start under the new name, it moves
the old `dj-software` data and configuration folders to the `open-jockey` ones, unless
those already exist.

Every change to cues, grids, playlists and settings is written to the database at once
(there is no "save on quit"). When the library is opened, the existing database is first
copied to `backups/library-<UTC yyyymmdd-hhmmss>.sqlite3` next to it, and only the 10
newest backups are kept. The copy is skipped when the newest backup is less than 10
minutes old (so a series of short commands does not push the real backups out), unless a
schema migration is pending: then a backup is always taken first. A migration runs in a
transaction, so a failed one leaves the original file untouched.

The database is checked with SQLite's `PRAGMA quick_check` when a backup is taken (the
check reads the whole file, so it does not run on the opens in between). If the check
fails, the library is not opened at all: the app stops with an error naming the database
file and the `backups` folder, and the damaged file is not copied into the backups (so
it cannot push a good backup out). Restore a backup as described below. A file that is
not a database at all is refused on any open.

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
  no longer exists is relinked (same track, cues kept), under these rules:
  - the track's old folder must exist: with an unplugged drive, tracks are not relinked
    from it;
  - since the fingerprint covers only the first 3 seconds, the durations must also agree,
    within 1 second or 1 % (whichever is larger), when both are known;
  - if several missing tracks still fit (say, a clean and a dirty edit with the same
    intro, moved together), the one with the same file name is chosen;
  - if that still leaves more than one, or none of them has the same name, the file is
    added as a new track and the old tracks stay `missing` with their cues, rather than
    risk giving a file another song's cues.
- **Deleted files** are flagged `missing`, never deleted, so their cues survive. If the
  file comes back, the track is found again.
- **Changed files:** when a file's size or modification time changes, its tags and
  fingerprint are read again and the track is queued for analysis again (its analysis
  is kept until then; a locked grid is never replaced). This also covers a file that was
  scanned while still being copied: once the copy completes, its size changes and it is
  analyzed again whole. If the audio itself changed (a different fingerprint), the old
  waveform, key, BPM and grid are deleted, since they no longer describe the file; a
  locked grid is kept. Cues are kept.
- Files that cannot be read or decoded are listed as skipped, with the reason, and left
  out. A registered folder that is not found (an unplugged drive) is skipped with a note.

The scan report looks like `added 12, updated 0, audio changed 0, relinked 1, missing 0,
ignored 3 non-audio file(s)`.

### Watching

With `play --library`, the registered folders are watched. A path must stay quiet for 2
seconds before it is looked at. A slow copy can still pause longer than that and be read
half written: it is added as a track, and its analysis, which then covers only part of
the audio, is stored as out of date (with a warning). When the copy completes, the
watcher sees the file change and the track is analyzed again. Removals flag tracks
missing and a rename relinks the track. If the operating system's watcher fails (for
example, too many watches on Linux), the app prints "library watching degraded" and
rescans the affected folders every 10 minutes instead. New tracks found this way are
analyzed in the background.

## Analysis

`library analyze` analyzes the tracks that are not analyzed yet (or whose analysis is
out of date). `--track ID...` analyzes only those tracks. `--force` analyzes again even
tracks that are up to date (a locked grid is kept): `library analyze --force` redoes
every track. `--all` changes nothing on its own (up-to-date tracks are still skipped);
it is kept as an alias of the default. Analysis runs on a pool of worker threads (one
fewer than the CPU cores, at least one) and is also redone for tracks analyzed by an
older version of the algorithms, and for tracks whose audio ended early (a truncated or
still-copying file). In `play --library`, analysis uses a single worker while any deck
is playing, and all of them again once every deck is paused or stopped.

What it produces:

- **Waveform:** three bands, low (below 250 Hz), mid (250 to 2500 Hz) and high (above
  2500 Hz), over a 50 ms window, plus the peak. It is stored at 1024 frames per bucket.
- **BPM and beatgrid:** one constant grid: a BPM and the position of the first beat.
- **Key:** the musical key, shown in one of three notations.

### BPM range

Detected tempos are folded into a range, 90 to 180 BPM by default: a tempo outside it is
doubled or halved until it fits, so 85 BPM reads as 170 and 185 BPM as 92.5.

```sh
open-jockey library set bpm-range 90 180
```

The maximum must be at least twice the minimum, within 30 to 480. Ranges whose minimum is
below about 70 BPM print a warning, because half-time tracks (dubstep) then read at half
tempo. A new range applies to new analyses; run `library analyze --force` to redo the
others.

### Key notation

```sh
open-jockey library set key-notation camelot     # 8A (the default shown in the lists)
open-jockey library set key-notation standard    # Am
open-jockey library set key-notation open-key    # 1m
```

The Camelot and Open Key names are those of the published wheels. The setting only
changes how keys are shown; the stored key is the same.

### Known limits

- **There is no confidence value.** The beatgrid detector gives nothing for silence and
  for tracks under 10 seconds, and the key detector nothing for silence. Otherwise they
  answer whatever the audio is: a key and a grid are also given for percussion-only
  tracks, noise or speech. Treat them as a starting point.
- **Tempo errors:** the most common are halved and doubled tempos (fix with `halve` or
  `double`). Other wrong tempos are possible; set the BPM by hand or tap it.
- **One constant grid per track.** Tracks that change tempo, and long DJ mixes (the list
  shows a note for tracks over 30 minutes), do not fit one grid. Beat positions drift
  away from the audio over time.
- **Key** detection is by the track's overall pitch content. It can be wrong for
  tracks that are atonal, change key, or have little bass or harmony.
- The first beat is the detector's best guess. Check it by ear and use `downbeat`.

### Tested on a real library

First check, November 2026, on a 4,536-track library (mostly hardcore, uptempo, dubstep
and drum and bass) on an external drive, macOS, release build:

- The scan took about 5 minutes. One file was skipped, and that file was empty (0 bytes).
- Analysis took about an hour, around 0.8 s per track. It only runs again for new or
  changed tracks.
- BPM and key were checked by ear against VirtualDJ. They matched overall; this was
  not a per-track count.
- Browsing, loading (deck 3 included), restoring the cue point, grid edits, and
  relinking a moved file (cue and locked grid kept) all worked.

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

Shift, set downbeat, halve and double need a BPM first (analyze the track, or set one
with `library grid <ID> bpm` or by tapping). A tap sequence of fewer than 4 taps changes
nothing.

```sh
open-jockey library grid 12 show          # BPM, first beat, lock state
open-jockey library grid 12 shift -5.5
open-jockey library grid 12 bpm 140
open-jockey library grid 12 halve
open-jockey library grid 12 lock          # or: unlock
```

The grid actions (`deck.grid_shift_back`, `deck.grid_shift_forward`,
`deck.grid_set_downbeat`, `deck.grid_halve`, `deck.grid_double`, `deck.grid_tap`,
`deck.grid_lock_toggle`) exist for mappings, but the DDJ-SB3 mapping does not bind them
yet: use the console.

## Playlists

```sh
open-jockey library playlist create "Peak time"
open-jockey library playlist add "Peak time" 12 40 7 [--at 2]
open-jockey library playlist list ["Peak time"]
open-jockey library playlist move "Peak time" 3 1
open-jockey library playlist remove "Peak time" 2
open-jockey library playlist rename "Peak time" "Warm up"
open-jockey library playlist delete "Peak time"
```

Positions start at 1. Deleting a playlist keeps its tracks in the library. Playlists
appear in the browser tree in library mode.

## Importing from VirtualDJ

A one-time import of your VirtualDJ hotcues, saved loops and playlists. VirtualDJ's files
are only read, never changed.

```sh
open-jockey library import-vdj                    # dry run: prints what would come over
open-jockey library import-vdj --apply merge      # writes it (keep, replace or merge)
```

In the window: Settings, Library, "Import from VirtualDJ…" (see [ui.md](ui.md)).

- **Where it looks:** on macOS, `~/Library/Application Support/VirtualDJ` and
  `~/Documents/VirtualDJ`; on Windows, `Documents\VirtualDJ`; on Linux, nowhere unless you
  pass `--vdj-dir <DIR>` (repeatable; it replaces those defaults). It also looks for
  `VirtualDJ/database.xml` and `VirtualDJ/MyLists` on the drive each library folder is on (on
  Linux that is the filesystem root, so `/VirtualDJ`). If it finds no database and no playlist, it says where it
  looked and does nothing.
- **What comes over:**
  - hotcues, with their colour and name, and saved loops, with their length in beats
    converted to the track's own time;
  - playlists, in the order VirtualDJ has them (nested folders become `Folder / Name`).
- **What does not:**
  - the main cue point, which VirtualDJ does not keep;
  - remix, automix and beatgrid markers (the summary counts them as "other markers");
  - VirtualDJ's grids. A loop in a song VirtualDJ has no grid for comes over as a plain
    hotcue, since its length is unknown.
- **Which tracks:** a VirtualDJ song is matched to a library track by its path, else by the
  audio fingerprint of the file, else by being the only library track with the same file name
  and size. Songs and playlist entries with no match are listed as "not in the library" and
  are not added; scan the folder first. A hotcue keeps its slot number (1 to 8); numbers above
  8 take the free slots in order of position, and what finds no slot is counted as dropped.
  A track listed in two databases takes the entry of the drive's own database, else the one
  with hotcues or loops; the other entry's hotcues and loops are counted as dropped.
- **When a track already has hotcues here** and VirtualDJ has some too, you choose: **keep**
  yours and import nothing for the track, **replace** them with VirtualDJ's, or **merge**
  (yours stay, VirtualDJ's fill the empty slots). Tracks without a conflict just take what
  VirtualDJ has. On the command line `--apply` takes the choice and uses it for every
  track; in the window it asks track by track, and "for all remaining" uses your answer for
  the tracks not asked yet.
- **Playlists:** a playlist is created under its VirtualDJ name; if a different playlist
  already has that name, it becomes `Name (2)`. One with the same name and tracks is
  "already here" and left alone. A playlist file that cannot be read is skipped and listed.
- **The backup:** before it writes, the import takes a library backup (as in
  [Where the database and backups live](#where-the-database-and-backups-live)) and prints its
  path. The whole import is one transaction: all of it or none of it.
- **The dry run:** without `--apply`, nothing is written and no backup is taken. It prints
  the databases read, the tracks matched, those with hotcues or loops and those that conflict
  with cues here, the hotcues and saved loops, how many were dropped over 8 slots, the other
  markers not imported, the playlists to create, already here and skipped, and the first 20 songs not
  in the library. The window shows the same counts before it asks anything.
- **Running it again** changes nothing: slots that would not change are left out, and the
  playlists are "already here".
- **The window needs the decks empty.** A loaded track keeps its old hotcues on the deck and
  would write them back, so the window refuses to start or to write while a deck holds a
  track: unload the decks first. The command line does not check; run it with `play` closed.

## Exporting to a USB stick

Copies playlists, in order, with their hotcues and saved loops, to a USB stick for Serato,
Rekordbox and VirtualDJ. Your music files are never changed. What goes on the stick and what
each app gets is in [interop.md](interop.md); the formats were checked by hand in Serato DJ
Lite, Rekordbox and VirtualDJ on macOS. Still unchecked: a CDJ, Windows or another computer, and Serato DJ Pro's saved loops.

```sh
open-jockey library export --to /Volumes/STICK --playlist "Warm up" --playlist "Peak time"
open-jockey library export --to /Volumes/STICK --playlist "Warm up" --write
```

In the window: Settings, USB export, "Export to USB stick…" (see [ui.md](ui.md)).

- **`--to`** is the stick's mount point (`/Volumes/<name>` on macOS, `/media/<user>/<name>`
  on Linux) or drive (`E:\` on Windows). A `--to` on the drive that holds a library folder
  is refused, as the window lists no such drive; so is a stick that is too small or whose
  free space the system will not tell: nothing is written.
- **`--playlist`** names a playlist exactly; repeat it for more. The playlists keep the order
  given. A name that matches no playlist, or several, is an error that lists them.
- **The dry run:** without `--write`, nothing is written. It prints the stick, the playlists
  and tracks, the tracks to copy with their size and those already on the stick, the free
  space, the tracks copied without Serato cues (not MP3 or WAV), the tracks skipped (missing or
  unreadable), and the copies to remove (no longer exported). The window shows the same lines
  before it writes.
- **`--write`** copies the tracks, writes each app's files and prints what it did:
  `Exported: <n> copied, <n> kept, <n> re-tagged, <n> removed.`, then anything worth knowing
  (a saved loop VirtualDJ cannot take, say).
- **Exporting again** copies only new or changed tracks and removes the playlists no longer
  exported, so the stick holds exactly the playlists of the last export.
- **The decks can stay loaded:** the export only reads the library. In the window it runs in
  the background.

## Stems

With `[stems] enabled = true` ([config.md](config.md)), `play --library` separates each loaded
track into stems and keeps them in a cache, so a reload is instant. The model is installed
once with `open-jockey stems setup`.

- **Where:** the `stems` folder next to the library database (`<data dir>/stems` for the
  default library). The model is in `<data dir>/models`.
- **Identity:** the cache is keyed by the track id and its audio fingerprint. A relinked file
  keeps its stems; changed audio is separated again. Format: Ogg Vorbis, about 20 MB for a
  5-minute track.
- **Budget:** `stems.cache_gb` in `config.toml`, default 5 GB. After each new separation the
  cache is trimmed to the budget. The stems of a track loaded in a deck are never removed. If
  only those are left over budget, a notice says so once and the cache shrinks when they go.
- **Eviction order:** first tracks that are in no playlist, not prepared and not loaded in the
  last 30 days, the least recently loaded first; then everything else, the least recently
  loaded first. Prepared means a cue point above 0, a hotcue, or stems made with
  `library stems`. Evicted stems are separated again at the next load.
- **Limit:** tracks over 20 minutes (DJ mixes) get no stems.
- **A bad cache never blocks a track:** missing, corrupt or half-written stems mean the knobs
  stay EQ until fresh stems are ready.

Prepare stems ahead of time:

```sh
open-jockey library stems --playlist "Friday"
open-jockey library stems --track 3 4
```

`--playlist` takes a playlist name; `--track` takes one or more track ids. Each track is
separated one at a time and marked prepared (kept longest). The command prints one line per
track (`stems ready`, `already cached, marked prepared`, `skipped, <reason>` or
`failed: <reason>`) and a total with the cache size. It needs the model and stops with the
`stems setup` hint without it. Ctrl-C stops after the current chunk. `--config <PATH>` reads
the budget from another config file. A whole track takes about 20 s on an Apple-silicon Mac
(measured). On a CPU it is an estimate, not measured yet: roughly 1–3 minutes per 5-minute
track, depending on the CPU.

## Listing tracks

`library list` prints id, artist, title, BPM, key, length, a bullet for tracks with
cues, and `missing` for missing files. Filters: `--playlist NAME`, `--folder DIR`,
`--search TEXT` (title or artist, case-insensitive); `--sort title|artist|bpm|key|added`
(default artist) and `--limit N` (default 100).

## Library mode: `play --library`

```sh
open-jockey play --library --controller ddj-sb3 --device "DDJ-SB3" --master 3,4
```

`--library` needs `--controller`. The track argument is optional. In this mode:

- The browser is a tree, as in VirtualDJ: the computer's drives (on macOS, Macintosh HD
  then each volume in `/Volumes`) and the folders opened below them, read from the disk,
  then "Playlists" with one row per playlist. The list beside it shows the highlighted
  folder's own audio files (not its subfolders'), sorted by file name, or the
  highlighted playlist's tracks. A file the library has shows its title, BPM and key. Highlighting a
  folder adds its own audio files that the library lacks, in the background (not those
  of its subfolders, and the folder is not registered or watched): they show their file
  name until the next refresh, then get their tags, and BPM and key follow as the
  analysis runs. LOAD on a file not added yet says nothing is selected; it works once the
  file shows its tags. A folder that can't be read opens empty, marked "cannot read". Hidden (`.`) folders and
  files are not shown.
- The BROWSE knob scrolls the focused pane, pressing it switches between the tree and the
  list, SHIFT + BROWSE press opens or closes the highlighted drive or folder, and
  LOAD (left or right) loads the selected track onto the deck that side currently shows,
  decks 3 and 4 included. See [the DDJ-SB3 page](../controllers/ddj-sb3/README.md).
- A browser line is printed: `▸ <path>  <n>/<count>  Artist – Title  <BPM>  <key>  •`
  (`<path>` is the highlighted tree row after its parents, such as
  `Macintosh HD ▸ Users ▸ me ▸ Music` or `Playlists ▸ Peak time`; the bullet means the track has a cue point above 0 or at least one hotcue, `missing` flags a missing file), followed by
  analysis progress while the background analysis runs.
- A hotcue can also have a colour and a name; they come from the VirtualDJ import, and
  nothing in the app sets them yet.
- Tracks load in the background. A cue point or hotcue you set is remembered with the
  track and restored when you load it again, and so is a hotcue you delete (it stays
  deleted). An empty hotcue pad pressed while a normal loop is active (not a held roll or slice)
  saves that loop in the slot instead of a point; pressing the pad later jumps to
  the loop's start and turns the loop on. With the same track on two decks, a hotcue set or
  deleted on one shows on the other too. If the track's hotcues cannot be read when it loads, a warning says
  they won't be remembered for this load, and hotcue changes are not saved until it is
  loaded again. The track's beatgrid is applied on load too, so SYNC and the loop buttons
  work as soon as the track is analyzed. A load onto a playing deck prints
  "warning: deck N was playing".
- Tracks not analyzed yet are analyzed in the background; failures and warnings are
  printed. A track the watcher sees change is analyzed again. While any deck plays,
  analysis runs on one worker thread only.
- A cue point or hotcue that could not be saved is retried once a second later (if it is
  still the latest for that track); a warning is printed if the retry fails too.
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
