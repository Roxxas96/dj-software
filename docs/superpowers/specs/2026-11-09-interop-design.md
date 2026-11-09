# Interop design: USB sticks and the VirtualDJ import (plan 11)

Approved with the owner on 2026-11-09. This narrows and replaces the "Interop (import/export)"
section of [the original design](2026-10-30-dj-software-design.md) for v1. Calls marked
**(my call)** were not discussed with the owner.

## Intent

The owner prepares sets in Open Jockey: playlists in a set order, hotcues and, if possible,
saved loops. Two things must work at a friend's place or at the owner's:

- **The owner plays on a friend's laptop.** The friend runs Serato or Rekordbox (sometimes
  VirtualDJ). The owner brings a USB stick and the friend's software shows the owner's
  playlists, in order, with the owner's hotcues and loops.
- **A friend plays on the owner's Mac.** The friend plugs in their stick and Open Jockey shows
  their playlists with their hotcues and loops.

The owner also wants a **one-time import of their existing VirtualDJ prep** on the Mac, so
nothing has to be redone.

Success: a stick made by Open Jockey opens in Serato, Rekordbox and VirtualDJ with the right
tracks, order, hotcues and loops; a friend's Serato, Rekordbox or VirtualDJ stick plays in
Open Jockey with their prep; the owner's VirtualDJ hotcues, loops and playlists are in Open
Jockey.

## Goals

1. **Export to a USB stick.** The owner picks one or more playlists in the window and chooses
   "Export to USB stick", then the stick. One export writes everything Serato, Rekordbox and
   VirtualDJ need, at once, with no choice of app (owner).
2. **Guest sticks.** A plugged-in stick made by Serato, Rekordbox (its CDJ device library) or
   VirtualDJ shows in the browser with its playlists, in order, and its tracks load with
   their hotcues, saved loops and main cue. The stick appears while plugged in and disappears
   when it is removed; nothing of it is added to the owner's library (owner).
3. **VirtualDJ import.** A one-time import of the owner's VirtualDJ database and playlists on
   the Mac: hotcues (with colours and names), saved loops, the main cue, and playlists in
   order. When a track already has cues in Open Jockey, the import asks per track: **keep**,
   **replace** or **merge** (owner, as in the original design). It never silently overwrites.
4. **Saved loops** become a thing Open Jockey stores and plays, since every app above has
   them and the owner uses prep loops.

## Non-goals (this plan)

- **CDJ export.** Writing Rekordbox's device library (`PIONEER/` with `export.pdb` and the
  ANLZ files) so a stick plays straight on CDJs. Postponed by the owner: there is no CDJ to
  verify it on. The newer encrypted device library (`exportLibrary.db`, CDJ-3000X, Opus
  Quad) is also out.
- Writing into the owner's own VirtualDJ, Serato or Rekordbox installation at home. Export
  only goes to a stick.
- Keeping a guest's tracks, cues or playlists after the stick is removed.
- Beatgrid export beyond what each format needs for cues and loops to line up (see
  Beatgrids).
- Two-device audio, stems on guest tracks, playlist folders/sub-crates beyond one level
  (**my call**: a flat list of exported playlists; nested folders from a guest stick are
  shown flattened with a "Folder / Playlist" name).

## Anti-goals

- **Never write to a guest's stick.** Reading only, not even a cache or an index file.
- **Never modify the owner's own audio files.** Serato keeps cues inside the audio files, so
  export writes Serato tags only into the **copies** on the stick, never the originals.
- **Never corrupt or partially write the owner's VirtualDJ data.** The import only reads it.
- A stick that looks exported but opens wrong (wrong order, cues shifted, tracks missing) is a
  failure even if the files are there. Unverified formats are not shipped as working.

## What goes on an exported stick

**(my call; the builder may adjust names but not the behaviour)**

- **The music:** each exported track is copied once into a folder on the stick (e.g.
  `Open Jockey/Music/<Artist>/<file>`), with names made safe for FAT32 and exFAT. Exporting
  again skips tracks already there unchanged (same size and fingerprint) and copies only new
  or changed ones, so updating a set before a gig is fast.
- **Before copying:** check free space and show what will be written; a stick that is too
  small is refused before anything is written.
- **Serato:** crates in `_Serato_/Subcrates/<playlist>.crate` (the order kept) and the
  `_Serato_/database V2` entries the crates need; hotcues, saved loops, colours and names in
  the Serato tags (`Serato Markers2` and the related frames/atoms) of the **copies** on the
  stick.
- **Rekordbox:** a `rekordbox.xml` (the documented collection format, `POSITION_MARK` for
  hotcues, memory cues and loops, `TEMPO` for the grid) at a fixed place on the stick, with
  playlists in order, pointing at the copies. The friend imports it from Rekordbox's
  preferences (the XML is the documented way; on CDJs this does nothing, see Non-goals).
- **VirtualDJ:** the drive-level VirtualDJ data VirtualDJ reads from an external drive
  (`VirtualDJ/database.xml` with the POIs: hotcues, loops, cue) and playlists for the
  exported sets, in order, so VirtualDJ on another computer sees them when the stick is
  plugged in.
- A short `Open Jockey/README.txt` saying how to open the stick in each app (Rekordbox needs
  the XML import step).

Interrupted exports (stick pulled, app closed) must leave the stick usable: each library file
(crate, XML, database) is written to a temporary name and renamed into place; a half-copied
track is removed or redone on the next export.

## Guest sticks

- Detection: on plug-in, the stick is checked for `_Serato_/`, `PIONEER/rekordbox/export.pdb`
  (with its `PIONEER/USBANLZ` analysis files), a `rekordbox.xml`, and `VirtualDJ/`. All that
  are present are read; if several apps' data is on the same stick, each shows as its own
  branch under the stick (**my call**).
- The stick shows in the browser tree as its own node named after the volume, with its
  playlists in order. Removing the stick removes the node; a deck playing a track from it
  keeps playing what is already decoded and reports the stick as gone (as for a missing
  file today).
- Tracks load like library tracks. Their hotcues, saved loops, main cue and grid come from the
  stick's data. They are analysed (waveform, and BPM/key if the stick has none) when loaded,
  as today; any cache for that must not make them appear in the owner's library and must not
  be written to the stick (**my call**: the builder chooses where; the temporary data may stay
  in the app's cache folder).
- **(my call)** A guest track gets the same kind of track record as a library track, but in a
  throwaway library of its own (for example an in-memory database with the library's schema)
  that lives while the stick is plugged in. The decks, analysis, waveforms and hotcue pads
  then load guest tracks through the same paths as library tracks, and nothing reaches the
  owner's library file.
- **(my call)** Only volumes carrying one of the apps' markers count as guest sticks, and any
  volume holding one of the owner's library folders is never one (the owner's music drive
  may carry VirtualDJ's per-drive data).
- A plain `rekordbox.xml` on a stick is read too (the same reader as for export).
- Changes the owner or friend makes to a guest track's hotcues during the session are kept
  only in memory for that session (**my call**): never written to the stick, never to the
  library.
- Rekordbox device library reading may use the `rekordcrate` crate (Rust, reads `export.pdb`
  and ANLZ) or equivalent, subject to the project's dependency rules (`cargo deny`). If it
  fails those rules or lacks cues and loops (ANLZ `PCOB`/`PCO2`), a parser of our own is fine.
- Fixtures for it come from the free Rekordbox on the owner's Mac, which can export a
  playlist to a USB stick in its device-library format.

## VirtualDJ import

- Reads VirtualDJ's local database (`database.xml` in the VirtualDJ home folder on macOS)
  and the per-drive databases VirtualDJ keeps on external drives (the owner's music is on an
  external volume), plus its playlists (`.vdjfolder` and `.m3u`), read only.
- Tracks are matched to Open Jockey's library by path, then by fingerprint for moved files.
  Tracks not in the library are listed in the summary, not added (**my call**; the owner can
  scan the folder first).
- Brings over: hotcues with colours and names, saved loops, the main cue, and playlists in
  order (created as Open Jockey playlists; a name already used gets a suffix, **my call**).
- Before writing anything: a summary (tracks matched, cues and loops found, playlists,
  unmatched tracks, items dropped over 8 slots) and, for each track that already has cues,
  **keep / replace / merge**, with "apply to all remaining" (**my call**: hundreds of
  tracks would make one question each unusable). Dropped items never abort the import.
  Merge (**my call**): Open Jockey's own slots win; VirtualDJ's fill the empty slots.
- The import runs in one library transaction after a database backup, so it can be undone by
  restoring the backup. Plan 3's cue-safety rules apply.
- Entry point: in the window (Settings or a library menu), plus a CLI command
  (`open-jockey library import-vdj`) for the summary and dry run (**my call**).

## Saved loops

- Storage: a hotcue slot gains an optional loop length (in frames) through a schema
  migration; a slot with a length is a saved loop, with the hotcue's colour and name. This
  is engine, controller and window work too (pad behaviour, waveform), not only data.
- **(my call)** In Open Jockey a saved loop lives in a **hotcue slot**, as in VirtualDJ and
  Rekordbox's loop hotcues: the slot holds a start and, for a loop, a length. Pressing that
  pad jumps to the start and engages the loop; the pad and the waveform show it as a loop.
  Saving one: an **empty** hotcue pad pressed while a loop is active stores the loop in that
  slot (**my call**); an occupied pad behaves as today. Saving follows plan 3's cue-safety
  rules (written at once, in a transaction).
  This keeps the SB3's 8 hotcue pads as the one place prep lives.
- Formats with separate loop slots (Serato's saved loops, Rekordbox memory loops) map into
  free hotcue slots on import, and Open Jockey's loop slots export as each format's loop kind.
  Rekordbox memory cues: the first becomes the main cue; the rest are not imported (**my
  call**).
- More than 8 hotcues plus loops in a format: the first 8 by position are kept and the
  summary says how many were dropped (**my call**).

## Positions, grids and colours

- Cue and loop positions convert between formats with care for sample offsets versus
  milliseconds and for MP3/AAC encoder delay (Open Jockey trims gapless padding; each app
  counts differently, Serato's MP3 offset in particular). Each format module has round-trip
  tests with **real files made by each app** (see Verification).
- **Beatgrids:** a guest track uses its app's grid when the stick has one, so the friend's
  loops and beat jumps land where they prepared them. Exports write each format's grid (BPM
  and first beat) from Open Jockey's analysis so loops line up there. The VirtualDJ import
  does not replace Open Jockey's grids (**my call**: the owner found the BPMs match).
- Colours: each format's palette maps to the nearest Open Jockey colour and back; names are
  kept where the format has them.

## Stick details to verify against the apps

- VirtualDJ has no "export to stick" of its own, so where it expects drive-level playlists
  and how it writes paths on a drive must be checked against the owner's VirtualDJ before
  the stick format is fixed (the builder investigates; nothing is assumed working until
  VirtualDJ opens a stick).
- Serato tags must be written for **MP3 and WAV**, the owner's file types (owner). Other
  types are still copied, without Serato cues, and the export summary says so (**my call**);
  more types can come later.

## Verification

- The owner has no CDJ, so CDJ playback is not verified and not claimed.
- The owner installs the free **Serato DJ Lite** and **Rekordbox** on the Mac to (a) make
  sample sticks and files with cues and loops, used as test fixtures (with the owner's
  permission, short or synthetic tracks only, no copyrighted music in the repo), and (b) open
  Open Jockey's exported stick and check order, hotcues, loops and colours. VirtualDJ is
  already installed.
- Hands-on: export a playlist to a stick and open it in all three apps; plug a stick made by
  each app into Open Jockey; run the VirtualDJ import on the owner's real database (read
  only), checking keep/replace/merge.

## Constraints

- Everything from the original design and the plan series still holds: Rust workspace,
  mise/prek/Conventional Commits, `cargo deny`, macOS/Windows/Linux builds.
- The spec's crate layout: a new `interop` crate with one module per format (`virtualdj`,
  `rekordbox`, `serato`) behind a shared trait; the builder may adjust the shape.
- Tests never touch the owner's real VirtualDJ data, music volumes or a real stick; fixtures
  live in the repo or a temp dir.
- The owner drives play with the controller; export and import are prep tasks done with the
  mouse in the window.

## Left to the builder

- The trait's exact shape, the stick folder names, file-name sanitising, how guest data is
  cached, how plug-in is detected per OS, and the window's dialogs for export, the
  summary and keep/replace/merge.
- The details inside each of the three plans below.

## Plan split (owner)

Built as three plans, in this order, each merged before the next is written:

1. **11a: saved loops and the VirtualDJ import.**
2. **11b: guest sticks** (Serato, Rekordbox device library and XML, VirtualDJ) on the owner's
   Mac.
3. **11c: export to a USB stick** for Serato, Rekordbox (XML) and VirtualDJ.

Release becomes plan 12 after these.
