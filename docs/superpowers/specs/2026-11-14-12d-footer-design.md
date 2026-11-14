# Plan 12d: a footer that says what the app is doing

Agreed with the owner on 2026-11-14, during the plan 12c hands-on. Calls marked **(my call)**
were not discussed with the owner.

## Intent

Open Jockey does long jobs in the background: analysing BPM and key, fetching covers,
importing and exporting. Today the window gives almost no sign of them. A short
"analyzing 312/4536" sometimes shows in the top bar's status line, squeezed between the menu
and the clock, and nothing shows for the others. During the 12c hands-on the owner could not
tell whether the cover lookup was running, stuck or done.

The owner wants two things while a job runs: **how much is left and how long it will take**.
They also want general feedback in a footer: what just happened, and facts about the list
they are looking at, such as how many tracks it holds.

## Goals

1. **A footer line.** One thin line along the bottom of the window, under the browser,
   always visible in every layout. Layout A from the mockup, as the owner chose it:
   - **Left: the open list.** Its name and its track count, e.g.
     "Midtempo Arsène · 42 tracks".
     - Also its total length, "2 h 31 min" **(my call)**.
     - After Cmd+A it says "N selected" instead of the count.
     - A folder list counts files, as the browser header does today **(my call)**.
   - **Middle: the latest message.** Events such as "Moved 3 files to the Trash", deck loads,
     refusals and errors.
     - A message shows for a few seconds, then fades **(my call: the 8 s the top bar uses
       today)**.
     - An error stays, in the error colour, until it is cleared or replaced, as the top bar's
       error does today.
   - **Right: one pill per running job.**
     - Each pill shows the job's name, done / total, a small progress bar and the estimated
       time left, e.g. "Analysing 312 / 4,536 ▬▬ ~6 min".
     - A pill appears when its job starts and goes when it ends.
2. **The messages move down.** The top bar's status line (messages, errors and the short
   status text) moves to the footer. The top bar keeps the menu, the settings gear, the audio
   device, the controller, the clock, the CPU and the master meter. The owner chose this.
3. **Which jobs.** Every long job the window can start or that runs in the background
   **(my call: the full list beyond what the owner named, which was analysis, covers,
   import and export)**:
   - analysing tracks (BPM, key, waveform);
   - fetching covers;
   - scanning the library folders;
   - the VirtualDJ import;
   - the USB stick export;
   - stem separation.
4. **Time left.** It is estimated from the job's own pace so far. It shows once the job has
   done a few items or run a few seconds **(my call)**, and as "~6 min" or "~1 h", rounded.
   A paused job shows "Paused" in place of the time.
5. **Pause, Resume, Stop.** The owner asked for "pause stop resume etc...":
   - Each pill has a pause/resume button and a stop button **(my call: small icons in the
     pill)**.
   - **Pause** freezes the job; its count stays. **Resume** carries on from where it was,
     without redoing finished items.
   - **Stop** ends the job; what was not done is left.
     - Analysis and covers pick up what is left the next time they start: at the next
       launch, scan or retry **(my call)**.
     - One-shot jobs (the VirtualDJ import, a stem separation) are cancelled cleanly and do
       not come back on their own **(my call)**.
     - **The USB export stops only between files**, so the stick never holds a half-written
       database. The next export finishes the stick **(my call)**.
   - **A pause does not survive a relaunch.** When Open Jockey starts again, jobs start again
     on their own. The owner chose this.
   - Hovering a pill shows the item it is on, e.g. "Fetching cover: Justice — Genesis"
     **(my call)**.

## Non-goals

- A job history or log window. The footer shows the latest message only.
- Changing what the jobs do or how fast they run. This plan reports them and lets you pause
  them.
- Pausing jobs automatically while a deck plays.
- Keyboard control of the footer.

## Anti-goals

- The footer must never slow the audio or the window. It reads job state from the frame; no
  job waits on the window, and the window never waits on a job.
- A stopped USB export must never leave a broken stick. A stopped import must never leave a
  half-applied playlist or hotcue set.
- A paused background cover lookup must make no network request until it is resumed.
- The footer must not take more than one text line from the browser.

## Constraints

- Everything from the earlier designs still holds:
  - Rust, egui 0.37, mise, prek and `cargo deny`;
  - macOS, Windows and Linux;
  - the window only sends `UiCommand`s and reads the frame;
  - library writes go through the writer;
  - plan 12a's compact look.
- Job state reaches the window through the frame, like the rest of the session state.
- Pause, resume and stop are new `UiCommand`s, routed to the job's owner.
- The render harness (`cargo test -p ui --test render -- --ignored`) gets a footer fixture,
  with two jobs running and a message.

## Decisions after the builder check (all my calls)

- **Where today's status text goes.** The top bar's short status text (the browser path,
  such as "▸ Music 1/3", and the deck line) has these homes:
  - The browser path becomes the footer's left slot (the open list).
  - The analysis progress text becomes the analysis pill.
  - Anything else left in the status text shows in the middle slot whenever no message is
    live, as the top bar does today. The middle slot is never blank while there is
    something to say.
- **Jobs with no total, or that never end.**
  - **Covers:** a new library count query gives the total of tracks needing a cover. The
    total is fixed when a pass starts. The pill shows while a pass has work and goes when
    the pass finds nothing left. The idle polling between passes shows no pill.
  - **Library scan:** an indeterminate pill with no bar and no time left, e.g. "Scanning
    Music…".
  - **VirtualDJ import:** an indeterminate pill naming its stage ("Reading VirtualDJ…",
    "Writing playlists…").
  - **Stem separation:** one pill for the whole queue, e.g. "Separating stems 2 / 5".
- **Stopping an export or an import.**
  - The export gets a cancel check, a new parameter or a callback return. Stop takes effect
    between files. A stopped export skips writing the Serato, Rekordbox and VirtualDJ
    library files, so the stick keeps its previous, consistent databases. The files already
    copied are extra but harmless, and the next export finishes the stick. The stick must
    stay readable by every app after a stop.
  - The import can be stopped only before its write starts. During the write, its stop and
    pause buttons are disabled, with a tooltip saying why.
- **Analysis.**
  - A user pause overrides the playing-deck throttle: playing must never resume a paused
    analysis.
  - Tracks already being analysed finish after a pause; nothing new starts. The pill says
    "Pausing…" until they finish.
  - Stop clears the queue. The next launch, or a rescan, queues what is left.
- **Covers.**
  - Pause is an in-memory flag, separate from the `library.cover_lookup` setting. It is
    checked before every request, like the off switch.
  - Stop ends lookups until the next launch, scan or "Retry missing covers".
  - "Find another cover" or "Find a cover" is an explicit request, so it still runs while the
    background lookup is paused or stopped. The anti-goal covers the background lookup only.

## Left to the builder

- The pill icons, spacing and truncation when the window is narrow. Suggested order of what
  gives way first: the time left, then the list's total length, then the message.
- How each job counts its items: tracks, files or bytes. Exports and stem separation may
  count tracks.
- The estimate's smoothing.
- How the jobs publish their state, as long as the control thread never blocks.
- **The order (my call):** finish and merge plan 12c first. Then build this as plan 12d on a
  new branch.
