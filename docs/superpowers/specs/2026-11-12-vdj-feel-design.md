# VirtualDJ feel: a compact window you can play with the mouse

Agreed with the owner on 2026-11-12. Calls marked **(my call)** were not discussed with the
owner.

## Intent

The owner compared Open Jockey with VirtualDJ side by side and said: "for now the software
doesn't give me the will to switch." The engine, controller, library and interop are there.
What is missing is the feel of the window:

- **The mouse does nothing.** The owner tried to click tracks in the library, drag them onto
  a deck, browse with the mouse, click pads, turn knobs, drag the waveforms, play and pause,
  set a cue, name and colour a hotcue, and change the master, headphone and channel levels.
  In their words: "basically every control that is displayed". Today the window only sets
  things up (layout, dropdowns, settings). Everything else needs the DDJ-SB3, so without it
  the owner could not even load a track.
- **The waveforms read as one colour (red).** The owner wants one colour per frequency band,
  drawn on top of each other.
- **The panes feel off.** Every section is an outlined box with gaps. Big areas sit empty: the
  lower half of each deck panel, the tall mixer columns, and the browser. The browser's tree is
  centred. VirtualDJ's sections touch, each is as tall as its contents, and the whole window
  feels like one compact piece.

The on-screen pads are fine as they are. They only need to respond to the mouse.

Success: the owner can prepare and play a set in Open Jockey without the controller, using
VirtualDJ habits, and wants to switch.

This comes **before** plan 11b (friends' sticks) and the release.

## Goals

1. **Every control on screen works with the mouse**, the way VirtualDJ's do:
   - **Transport:** PLAY/PAUSE, CUE and SYNC.
   - **Pads:**
     - in hotcue mode, click to jump to or set a hotcue, and delete one;
     - in the other pad modes, click to do what the hardware pad does;
     - the pad mode dropdown, as today.
   - **Loop:** IN, OUT, size, and the loop buttons.
   - **Knobs:** GAIN, EQ (HIGH/MID/LOW or the stem knobs), FILTER and the FX level. Drag up or
     down to turn. Double-click resets to the default (centre for EQ and filter, 0 dB for
     gain). The mouse wheel turns a knob too **(my call, VirtualDJ does the same)**.
   - **Faders and sliders:** channel faders, the crossfader, the master level and the pitch
     slider. Drag to move; double-click resets (pitch to 0 %).
   - **Toggles:** VINYL, SLIP, KEYLOCK, PFL (headphone cue), and the FX on/off.
   - **Jog:** drag around the jog to nudge (playing) or scrub (paused), like turning it.
   - **Waveforms:**
     - the scrolling waveform: drag it sideways to scrub (paused) or to nudge or scratch
       (playing), the way VirtualDJ's does;
     - the track overview (the full-track strip): click or drag to jump to that point.
   - **Master and headphones:** the master level, plus a headphone level and a headphone
     mix (cue ↔ master), as VirtualDJ's master section has. On the DDJ-SB3 the headphone
     level is also a hardware knob. The on-screen one is the software level.
2. **The library works with the mouse:**
   - **Browsing:**
     - click a folder or playlist in the tree to show it;
     - open or close a tree node with its arrow;
     - scroll the list with the wheel;
     - click a track to select it.
   - **Sorting:** click a column header to sort by it, and click again to reverse.
   - **Loading a track:**
     - drag it onto a deck, its waveform, or its deck panel;
     - or double-click it **(my call)**: it loads on the first deck that is not playing,
       left first, and never on a playing deck;
     - or right-click it and choose "Load on deck 1/2" **(my call)**.
   - **Searching:** a search box filters the current list by title or artist as you type.
   - **Playlists:** drag a track (or several) onto a playlist in the tree to add it.
     Right-click a playlist to create, rename or delete one, and right-click a track in a
     playlist to remove it from that playlist. Drag a track within a playlist to reorder it.
     The owner hit this gap during the 11c hands-on.
3. **Hotcues can be named and coloured with the mouse.** Right-click a hotcue pad, or its
   flag on the waveform, to get:
   - a name field;
   - a colour palette (the colours Open Jockey already uses for hotcues);
   - "Delete".

   Changes are saved like any hotcue: at once, in the library, under plan 3's cue-safety
   rules.
4. **Waveforms in one colour per frequency band, drawn on top of each other.** Bass is
   drawn at the back, then mids, then highs in front, each in its own colour. The colours are
   a choice in the settings, with three palettes:
   - **Red / green / blue** (the default): bass red, mids green, highs blue;
   - **Blue / orange / white:** like Rekordbox's 3-band view;
   - **Orange / yellow / cyan.**

   This applies to the scrolling waveforms, the deck's track overview and the sideview's.
   The existing styles (deck colour, separate bands, both decks in one lane) stay as they
   are.
5. **A compact window that reads like VirtualDJ's Pro layout:**
   - **Sections touch.** A slightly different shade or a thin dark gap separates them; there
     are no outlined boxes. Small vertical labels on a section's edge (FX, PADS, LOOP) replace
     titles above it.
   - **Each section is as tall as its contents.** There are no empty areas. The deck panels,
     the mixer and the browser share the window with nothing left blank.
   - **The deck header is one dense strip:**
     - the cover;
     - the title and artist;
     - BPM, key and pitch;
     - elapsed and remaining time;
     - a coloured overview of the whole track under it, showing the played part, the hotcues
       and the loop.
   - **The jog shows BPM, pitch and time inside its ring.** The ring itself shows the playing
     position.
   - **The mixer is compact.** The two channel faders sit in the middle with a level meter
     between them. The EQ and filter knobs sit outside the faders, on each deck's side. GAIN
     goes on top, and PFL headphone buttons go under the faders. The crossfader sits below
     all of these. The master and headphone levels sit at the top of the mixer, or in the top
     bar **(my call; the builder picks whichever fits)**.
   - **The browser reads like a file manager.** The tree on the left is left-aligned, with
     icons for drives, folders and playlists, and opens and closes with arrows. The track list
     has column headers (Title, Artist, Remix, Length, BPM, Key), alternating row shades, and
     a highlight on the selected and loaded tracks. The info panel (sideview) sits on the
     right. The search box goes above the list.
   - **The other layouts get the same rules:** Starter, Essentials and Performance get the
     compact sections and mouse control too. Their content stays as plan 10 defined it.
6. **No regression for the controller.** Everything the DDJ-SB3 does today keeps working.
   A control moved with the mouse and then with the controller follows the existing
   soft-takeover rules: the ghost marker shows where the hardware is until it catches up.

## Non-goals

- A pixel copy of VirtualDJ's skin, logos or exact artwork. The arrangement and the feel
  are the target, not its branding.
- Video, scratch, the AUDIO/VIDEO/SCRATCH/MASTER tabs, online music, and VirtualDJ's
  Ideas, Stems or iTunes nodes.
- Keyboard shortcuts **(my call: the owner asked for the mouse; shortcuts can come later)**.
- Rating stars, the editable tag fields and play counts in the info panel **(my call)**.
  It shows what the library has: cover, title, artist, BPM, key and length.
- A second audio device for headphones (its own later plan, as before).

## Anti-goals

- **Mouse control must not make the controller worse.** No added latency on MIDI, no lost
  soft takeover, no knob jumping because the mouse moved it.
- **A misclick must not ruin a set.**
  - Dropping or double-clicking onto a playing deck never loads over it. The track goes
    nowhere, and a short message says the deck is playing.
  - Deleting a hotcue or playlist from a menu is a separate menu item, never a single click
    on a pad.
- **The window must stay smooth.** The waveform colours and the compact layout must not drop
  frames on the owner's MacBook. The plan 9 frame rules still hold: no allocation per frame
  in the drawing paths that already avoid it.

## Constraints

- Everything from the earlier designs still holds:
  - the Rust workspace and egui/eframe 0.37;
  - mise, prek, Conventional Commits, `cargo deny`;
  - builds on macOS, Windows and Linux.
- **The window never acts on the engine directly.** A mouse control sends the same
  `UiCommand::Action(action, input)` a controller input would send. A knob or fader sends
  `Input::Absolute`, a button sends `Press` and `Release`, and the jog sends `Relative`. The
  session applies it on the control thread, as today. Where no action exists yet, the
  builder adds one to the `actions` crate. Examples: load the selected track on a deck, set
  the headphone level, seek to a fraction of the track.
- **Library writes go through the library's writer**, in one transaction where the change has
  several parts (reordering a playlist, adding several tracks).
- **Hotcue names and colours use the existing storage:** `HotcuePoint`, `WriteOp::PutHotcue`
  and the memory-sync path.
- **Tests:** egui_kittest for widgets and mouse gestures, as plans 9 and 10 do. Tests never
  touch the owner's real config, data or music.
- The layouts, colour scheme, panel sizes and folding from plan 10 stay. The compact look
  replaces how sections are drawn, not the layout system.

## How VirtualDJ does it (reference for the builder)

From the owner's screenshot of VirtualDJ's Pro layout, decks A and B with the mixer between
them:

- **Top:** two scrolling waveforms stacked, full width, coloured per band, with hotcue and
  loop names on flags above them.
- **Deck (left; right is mirrored):**
  - **Header strip:** cover, title (bold), artist, BPM (large) and key, pitch, total time,
    "MASTER/SYNC", and a coloured overview under the title.
  - **Below:** FX slots (dropdown, a level slider and a settings button each), a "USER" block
    of small buttons, the jog (BPM large in the centre, pitch, elapsed and remaining), and
    the pitch slider with +/- buttons.
  - **Bottom row:** the pad mode dropdown with arrows, 8 pads in two rows of 4, and the loop
    section ("<" size ">", IN, OUT). CUE, PLAY (highlighted while playing) and SYNC sit under
    the jog.
- **Mixer:** stem or EQ knobs on the outside (HIHAT, MEL/VOX, KICK, FILTER), with GAIN, the
  two channel faders and a centre meter in the middle. Headphone buttons sit under the
  faders, and the crossfader is at the bottom.
- **Browser:** a search box and the tree on the left (Local Music, Volumes, My Lists, …,
  left-aligned with icons). In the middle, a list with column headers, a file count and
  alternating rows. On the right, the info panel: cover, BPM, key, title and artist.

## Decisions after the builder check (all my calls)

- **Library operations** go through new `UiCommand`s, not through controller actions:
  - loading a track (`Load { deck, track }`);
  - the playlist edits (create, rename, delete, add tracks, remove, move);
  - search and sort.

  The session applies them through the library's writer, and refuses or fails with a message
  in the frame's events. Search and sort run in the session, as browsing does today. The
  session queries the library and publishes the page the window shows, so a library of
  50,000 tracks stays fast. Loading stays bindable on the controller through the existing
  LOAD action.
- **The playing-deck rule is checked in the session**, which is authoritative. A load onto a
  deck whose transport is playing is refused, and an event says "Deck N is playing". The
  window also skips playing decks when it picks the deck for a double-click, but the
  session's check is the one that counts. When both decks are playing, the double-click
  loads nothing and shows the same message.
- **Mouse and soft takeover:**
  - A mouse move sets the value at once and does not go through pickup.
  - Afterwards, the control behaves as after a deck switch: the hardware must cross the value
    on screen before it takes over, and the ghost marker shows where the hardware is.
  - Every control that has a hardware twin and an absolute value gets a ghost: trim, EQ,
    filter, channel fader, crossfader, pitch, master level and headphone level.
- **Waveform drag** reuses the jog's actions:
  - `JogTouch` when the drag starts and when it ends, which is a held platter (a scratch)
    while playing;
  - `Relative` steps while dragging, scaled so a drag tracks the waveform under the mouse.

  A click on the overview seeks without quantising, on a playing deck too, as VirtualDJ
  does. A seek is not a cue edit, so plan 3's cue-safety rules don't apply.
- **Hotcue edits:**
  - Names and colours already live in `HotcuePoint` and in the memory-sync path from plan
    11a, so there is no schema change.
  - An edit writes at once and shows live, even on a playing deck.
  - The palette is Open Jockey's existing hotcue colours, plus "no colour", which means the
    slot's default.
  - A right-click on an empty pad shows "Set hotcue here", which sets it at the playhead.
- **Spare height** goes to the browser, as in VirtualDJ. The waveforms and decks keep their
  content size, adjustable with plan 10's dividers. Below plan 10's minimum window size the
  layouts behave as they do today.
- **Waveform data:** the analysis already stores `[peak, low, mid, high]` per bucket, so the
  colour work needs no re-analysis and no migration. The three-layer drawing keeps plan 9's
  no-allocation-per-frame rule.
- **Headphones:** the engine already has a headphone level and a cue/master mix
  (`MasterCommand::HeadphoneLevel` and the mix). 12b only adds the actions and controls for
  them.
- **Verifying the look:** plan 12a gets a check by the owner part-way through. After the
  layout task, the window is rendered with a track loaded on both decks, and the owner
  compares that render with VirtualDJ before the remaining tasks go on. Each plan ends with
  the owner's hands-on, as before.

## Left to the builder

- Exact sizes, spacing and shades within the compact rules, and how each layout fits the
  window.
- The knob and fader drag sensitivity. A fine-drag modifier (e.g. Shift) is optional.
- How the mouse's nudge and scratch map onto the existing jog actions.
- Where the master and headphone levels sit (mixer top or top bar).
- The right-click menus' exact entries beyond those listed.
- How the work is split. **My call:** two plans, in this order:
  1. **12a, compact window and waveform colours:** goals 4 and 5, plus the jog and header
     information.
  2. **12b, mouse control:** goals 1, 2, 3 and 6.

  The release becomes plan 13, after 11b. 12a goes first, so the mouse work lands on the
  final layout.
