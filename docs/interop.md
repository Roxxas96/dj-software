# Exporting to a USB stick

Open Jockey copies playlists onto a USB stick so Serato, Rekordbox and VirtualDJ on another
computer show them in order, with their hotcues and saved loops. One export writes what all
three apps need; there is no choice of app. Your music files are never changed: the Serato
tags go into the copies on the stick only.

**Checked by hand.** The owner opened an exported stick in Serato DJ Lite, Rekordbox and
VirtualDJ 2026 on macOS (2026-11-10 and 11):

- The playlists show in order in all three, and hotcue positions are exact in all three. The
  timing constants below are confirmed: a Serato MP3 with an Info frame is 1152 frames
  later, the others are unshifted.
- Exporting again copies nothing new and shows a hotcue that was moved; a playlist that is
  unticked disappears from all three.
- Serato shows a track that is repeated in a playlist once: that is Serato's own limit (it
  keeps a track once per crate), and the export says so in a note. Saved loops are written,
  but Serato DJ Lite has no saved loops; they are for Serato DJ Pro, which has not been
  checked.
- Rekordbox imports the playlists through the XML, with the steps in
  `IMPORT TO REKORDBOX.txt` at the top of the stick (they are also shown when an export ends).
- VirtualDJ shows the playlists and cues correctly. It offered a one-time "database is
  corrupted, fix it?" because of the drive letter; an export now keeps the letter VirtualDJ
  uses. Still to confirm by the owner: that exporting again no longer asks, and that no track
  is listed twice.

Not verified yet: a CDJ (not exported), another computer or Windows, and Serato DJ Pro's saved
loops.

To export, see [library.md](library.md#exporting-to-a-usb-stick) for the command line, and
[ui.md](ui.md#the-settings-window) for Settings, USB export in the window.

## What goes on the stick

```text
<stick>/
  Open Jockey/
    Music/<artist>/<file>       the copies, one per track
    rekordbox.xml               for Rekordbox's XML import
    README.txt                  how to open the stick in each app
    export.toml                 the manifest: what Open Jockey wrote
  _Serato_/
    Subcrates/<playlist>.crate  one crate per playlist
    database V2                 our tracks, merged in
  VirtualDJ/
    database.xml                our tracks, merged in
    MyLists/<playlist>.vdjfolder
```

- **The copies:** each track is copied once, under `Open Jockey/Music/<artist>/<file name>`
  (`Unknown Artist` with no artist tag). Names are made safe for FAT32 and exFAT: characters
  Windows forbids become `_`, device names such as `CON` get a `_`, and long names are cut.
  Two files that would get the same name (ignoring case) become `name (2).mp3` and so on.
- **The manifest** (`export.toml`) lists the playlists, crates, VirtualDJ lists and copies
  Open Jockey wrote. On the stick, Open Jockey changes or deletes only what the manifest
  lists, and inside Serato's `database V2` and VirtualDJ's `database.xml` only the entries
  that point into `Open Jockey/`. What other apps put on the stick stays as it is.
- **Exporting again** copies only new or changed tracks (by fingerprint), rewrites
  the Serato tags of a copy whose hotcues changed, and deletes the copies, crates and lists of
  playlists no longer exported. The window ticks the playlists the stick already holds.
- **Before anything is written** the export checks the free space (with a 32 MiB margin); a
  stick that is too small, or whose free space the system will not tell, is refused. A stick that is, holds or is inside one of the library's
  folders is refused too, as is one whose manifest, `database V2` or `database.xml` cannot be
  read.
- **An interrupted export** (stick pulled, app closed) leaves the stick usable: every file is
  written under a temporary `.ojpart` name and renamed into place, and the next export
  removes what is left over. Before the first copy the manifest already lists the crates,
  lists and copies the export is about to write, so the next export knows them as its own.

## What each app gets

- **Serato DJ:** a crate per playlist, in order, under the stick's name. The hotcues (with
  colour and name), the saved loops and the beatgrid are in the `Serato Markers2` and
  `Serato BeatGrid` tags of the copies. Serato keeps cues inside audio files, so only MP3 and
  WAV copies carry them; other types are copied without Serato cues, and the summary counts
  them.
- **Rekordbox:** `Open Jockey/rekordbox.xml`, the documented collection format. Each track has
  its grid (`TEMPO`), its main cue as a memory cue, and its hotcues and saved loops
  (`POSITION_MARK`, with colour and name), and each playlist is in order. Rekordbox needs one
  step: Preferences, Advanced, Database, rekordbox xml, choose the file, then import the
  playlists from the "rekordbox xml" view. On a computer where the stick has another name or
  drive letter, Rekordbox shows the tracks as missing: use Relocate on `Open Jockey/Music`.
  CDJs do not read this file; a CDJ export is not planned yet.
- **VirtualDJ:** the drive's own `VirtualDJ/database.xml`, which VirtualDJ reads from an
  external drive, with the hotcues (colour and name) and saved loops, and a list per playlist
  under `VirtualDJ/MyLists`. VirtualDJ keeps a loop's length in beats, so a saved loop on a
  track with no BPM is left out, and the export says how many. The drive letter in the
  database is the one VirtualDJ assigns to the volume, so VirtualDJ may offer a one-time fix
  ("the drive's database is corrupted") on a new computer; say yes, it only updates the letter.
  An export reuses the letter already on the stick (`D:` when there is none yet).

`README.txt` on the stick repeats these steps for whoever plugs it in.

## Timing

Open Jockey stores positions in frames, counted from the first sample after the gapless trim
(an MP3's Info frame and encoder delay are skipped). Each app counts from its own start, so
`export/timing.rs` in the interop crate converts a position for each app:

| App | Shift | Constant |
| --- | --- | --- |
| VirtualDJ | none (the 11a import lined up with no offset) | none |
| Rekordbox | none | none |
| Serato, MP3 with an Info frame | one MP3 frame (1152 samples, about 26 ms) later | `SERATO_INFO_FRAME_FRAMES` |
| Serato, WAV or MP3 without one | none | none |

These values were confirmed in the hands-on with a click track: a 60 s file with a
4 ms, 2 kHz click every half second (120 BPM), as MP3 and as WAV, with hotcue 1 on the click at
8.0 s and a 4-beat saved loop at 16.0 s. Exported and opened in each app at full zoom, hotcue 1
must sit on the click's start. A shift seen in an app changes that app's constant in
`timing.rs`, and the check is run again.

Two things the hands-on checked or still has to check:

- **A track twice in one playlist:** Serato shows it once (its limit); Rekordbox and
  VirtualDJ keep both entries.
- **VirtualDJ lists on another computer (not checked yet):** a `.vdjfolder` names each track by its absolute
  path, so the stick is plugged into a computer where it mounts under another name (or drive
  letter), and VirtualDJ must still find the tracks of each list.

## Known limits

- Not verified: another computer or Windows, and Serato DJ Pro's saved loops (see the top of
  this page).
- One level of playlists: folders are not exported.
- No CDJ export (`PIONEER/` with `export.pdb`): there is no CDJ to verify it on.
- Guest sticks (playing a friend's Serato, Rekordbox or VirtualDJ stick in Open Jockey) come in
  a later plan.
