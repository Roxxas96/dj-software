# Plan 12c: waveforms that react, a keyboard for the library, and covers

Agreed with the owner on 2026-11-13, after the plan 12b hands-on. Calls marked
**(my call)** were not discussed with the owner.

## Intent

Open Jockey now looks and plays like VirtualDJ with the mouse. The owner's hands-on asked for
the last things that keep them from removing VirtualDJ:

- The waveform should show what the performance does, as in VirtualDJ:
  - where a held roll or slicer will land;
  - the loop being played;
  - each band shrinking as its knob turns down.
- The library should be driven from the keyboard, the way the owner drives VirtualDJ's: arrows,
  Tab, Enter, select all, and Delete with a confirmation.
- Tracks should show covers. The owner's files carry no picture inside them. VirtualDJ shows
  covers it fetched itself, and the owner wants Open Jockey to do the same without VirtualDJ:
  "my goal is to remove it from my computer".

## Goals

1. **Where a held roll or slicer lands.**
   - While a loop roll or the slicer is held with slip on, the deck's waveforms (the scrolling
     lane and the deck's overview) show the loop being played, plus a small marker at the top
     of the lane where the track will carry on when the pad is released.
   - The engine already publishes this position (`DeckSnapshot::ghost_frames`, `roll_active`,
     `slicer_block_beat`), so this is drawing work.
   - The marker is a small triangle or tick at the lane's top edge in the deck's accent, with a
     thin vertical line through the lane **(my call)**. It moves with the hidden playhead and
     disappears on release.
2. **The loop being played is always visible.**
   - An active loop's span is shaded on the lanes and the overview whether it is a normal loop,
     a loop roll or a slicer block. Today the band shows normal loops.
3. **The waveform follows the mixer.**
   - Each band's height is scaled by its knob's current gain:
     - HIGH (HIHAT in stems mode) scales the highs;
     - MID (MEL/VOX) scales the mids;
     - LOW (KICK) scales the bass.
   - A knob fully down makes its band disappear.
   - GAIN scales all three bands.
   - With the `raw` stem knobs (VOCALS, INSTRU, BASS) the knobs map by position: the top knob to
     the highs, the middle to the mids, the bottom to the bass. This is an approximation: there
     is no per-stem waveform **(my call)**.
   - The channel fader and the filter do not change the waveform, as in VirtualDJ **(my call)**.
   - This applies to the scrolling lanes and the deck overview, in every palette and style.
     The Overlaid style scales each deck by its own knobs.
4. **The library from the keyboard.** These work whenever the browser has keyboard focus,
   meaning after a click in the tree or the list, and not while a text field (search, a dialog)
   is being typed in:
   - **↑ / ↓** move in the focused pane. Page Up / Page Down move a page, and Home / End go to the
     first and last row **(my call)**.
   - **← / →** in the tree close and open the highlighted folder **(my call)**.
   - **Tab** switches between the tree and the track list.
   - **Enter**:
     - on a folder or playlist in the tree, opens it;
     - on a track, loads it like a double-click does, on the first deck that isn't playing.
   - **Cmd+A** (Ctrl+A on Windows and Linux) selects every track in the current list. That is
     the whole list, not only the loaded page **(my call)**.
   - **Delete / Backspace** (Cmd+Backspace on macOS also) deletes, depending on where focus
     is. It always opens a confirmation first, with Cancel focused:

     | Focus | What goes |
     |---|---|
     | Track list of a playlist | The selected tracks are removed from the playlist. The files stay. |
     | Track list of a folder | The selected song files are deleted from disk. |
     | Tree, on a playlist | The playlist is deleted. Its tracks stay in the library. |
     | Tree, on a folder | The folder is deleted from disk, with everything in it. |
   - **Typing** a letter or digit with the list focused puts it in the search box and focuses
     the box **(my call)**. Escape leaves the box as today.
   - The selection (Cmd/Shift-click from 12b) and the keyboard share one cursor and one
     selection.
5. **Deleting from disk is safe.**
   - Files and folders go to the system's Trash or Recycle Bin, never straight to deletion,
     so a mistake can be undone **(my call)**.
   - Open Jockey only deletes inside the library's own folders. It refuses anything outside
     them, and it refuses a library root folder itself.
   - A track loaded on a deck can't be deleted until it is unloaded. The confirmation says so
     and disables Delete.
   - The confirmation names what will go: one file or folder by name, or "N tracks". For a
     folder it says how many files it holds.
   - After the delete, the library marks the tracks gone, as a rescan would. Their hotcues stay
     in the database, as for missing files today. The browser refreshes.
6. **Covers looked up online and kept in the library.**
   - For a library track with no embedded picture, Open Jockey looks a cover up online by
     artist and title. It saves the image in the library database (or a covers folder next to
     it) **(my call: the builder picks)** and shows it everywhere a cover shows today: deck
     headers, the info panel and the sideview.
   - An embedded picture always wins over a looked-up one.
   - Lookups run in the background with a rate limit. They never block loading or browsing, and
     they happen once per track. A miss is remembered, so it isn't asked again every launch. A
     setting can retry misses.
   - A track whose cover was looked up gets a "Find another cover" and a "Remove cover" entry in
     its right-click menu **(my call)**.
   - **Service:** a free service with no account or key. The builder checks terms and coverage
     for electronic music:
     - Deezer's public search, then MusicBrainz with the Cover Art Archive as a fallback **(my
       call)**;
     - or iTunes Search.
   - **Privacy:** only the artist and title text are sent, never paths, the owner's name, email
     or anything else. A setting turns lookups off **(my call: on by default, since the owner
     asked for it)**. The docs say what is sent and where.

## Non-goals

- Per-stem waveforms (separate analysis of each stem). Goal 3 uses the bands.
- Reading VirtualDJ's data for covers. The owner wants no dependence on it.
- Editing tags, or writing covers into the audio files. Covers live in the library only.
- Keyboard control of the decks (play, cue, pads) in this plan. The library only.

## Anti-goals

- Never delete outside the library folders, never permanently, and never without a
  confirmation.
- Never send anything but artist and title to a cover service. Never send the owner's email or
  name.
- A slow or offline cover service must never slow the app. No lookup runs on the control or UI
  thread.
- The waveform scaling must not drop frames. It is a per-frame multiply, with no allocation.

## Constraints

- Everything from the earlier designs still holds:
  - Rust, egui 0.37, mise, prek, `cargo deny`;
  - macOS, Windows and Linux;
  - the window only sends `UiCommand`s;
  - library writes go through the writer.
- A new dependency (an HTTP client, a trash crate) must pass `cargo deny` and must not pull a
  GPL licence. The project already uses the OS trust store for HTTPS (`stems setup`), so reuse
  that client.
- Tests never touch the owner's files, the network or the real Trash. Use a fake cover service
  and a fake trash in tests.

## Decisions after the builder check (all my calls)

- **Deleting a folder.**
  - The folder is refused when any file under it is loaded on a deck.
  - It is also refused when it contains a library root folder: only folders inside a root can
    go.
  - The confirmation counts every file the folder holds, not just music.
  - The library tracks under it are marked missing, by the writer's existing missing-file
    update, exactly as a rescan would after the files vanish. Their hotcues stay.
- **Paths and the Trash.**
  - The "inside a library folder" check compares canonical paths, with symlinks resolved, so a
    link can't point a delete outside.
  - If the Trash move fails (some USB/exFAT volumes have no Trash), nothing is deleted and a
    message says the drive has no Trash. It never falls back to a permanent delete.
- **Keyboard focus.**
  - The browser's existing tree/list focus (`Focus` in the library browser) becomes visible: the
    focused pane's highlight is brighter, as the controller's BROWSE already shows.
  - The browser owns the keyboard when the pointer last clicked inside it and no text field has
    focus. Any click elsewhere hands the keyboard back.
  - There is no other global key handler today. Typing to search takes only letters and digits
    with no modifier.
- **Select all.**
  - Cmd+A sets an "all rows of this list" selection the library resolves, not an id set in the
    window, so a 50k list costs nothing.
  - Delete and drag act on it through the library.
  - The confirmation shows the library's count.
- **Library calls.**
  - Marking files missing reuses the existing writer op.
  - Deleting a playlist and removing entries reuse plan 12b's playlist steps, with the delete
    confirmation in front.
  - Deleting files and folders is a new library-mode job that runs on the worker: Trash, then
    the writer update, then a browser refresh.
- **Cover storage.**
  - Image files go in a `covers` folder next to the library database, named by track id.
  - A new table records each track's cover state: found (with the file), missed (with a date),
    or removed by the user.
  - It is added by a schema migration, through the library's existing migrations.
- **Cover flow.**
  - The app owns a cover-lookup worker, like the analysis jobs. It asks the library which
    tracks need covers, fetches them, saves the file and records it through the writer.
  - The frame tells the window a track's stored cover path. The window's `Covers` loader reads
    an embedded picture first, then that file, so nothing in the UI talks to the library.
  - The on/off switch and "retry misses" are settings on the existing Settings → Library page,
    stored like the other settings.
- **What the lookup sends.**
  - It sends the artist and title tags.
  - Without tags it sends the file name without its extension; DJ files are often named
    "Artist - Title".
  - Nothing else is sent.
  - A lookup happens once per track id. A miss is retried only from the setting, or from "Find
    another cover".
- **Waveform scaling.**
  - The knob values the mixer already shows reach the lanes from the same frame.
  - The stem-mode knob names (`eq_labels`) already tell which mode applies.
  - No engine change is needed.

## Left to the builder

- Marker shapes and sizes within the goals.
- Where looked-up covers are stored, and the cover service order, within the privacy rules.
- The exact confirmation wording beyond naming what goes.
- How the work is split. **My call:** one plan with four areas. The order is waveform
  feedback, then the library keyboard and delete, then covers. Covers come last because they
  carry the new network dependency.
