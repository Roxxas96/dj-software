# The window

`open-jockey` with no command opens a window that mirrors a Pioneer DDJ-SB3 in the layout of
VirtualDJ's Pro view: scrolling waveforms, deck panels, the mixer and a folder-tree browser.
Everything on screen works with the mouse, so you can perform without a controller (see
[Mouse](#mouse)); a DDJ-SB3 drives the same controls, and the window shows what it does.

```bash
cargo run --release -p open-jockey                       # the default configuration file
cargo run --release -p open-jockey -- --config my.toml   # another file
```

The terminal commands (`play`, `library`, `stems` and `midi`) are unchanged.

## What it reads

From the configuration file ([config.md](config.md)):

| Key | Default | Meaning |
|---|---|---|
| `audio.device`, `audio.master`, `audio.headphones` | the default output | Where the sound goes |
| `controller.mapping` | `"ddj-sb3"` | The controller mapping |
| `ui.waveform_beats` | `16` | Beats across a waveform lane at the first launch (the window's zoom then takes over) |
| `ui.waveform_min_beats`, `ui.waveform_max_beats` | `4`, `64` | The zoom range: the fewest and the most beats across a lane, 1 to 512 |

The library opens at the default database. A missing DDJ-SB3 or audio device does not stop the
window: it tries again every 2 seconds, and the top bar says what is missing. A bad
configuration file or mapping does not close it either: the window shows the error, naming
the file and the problem, until it is closed. Closing the window saves cues and hotcues,
like Ctrl-C in `play`.

## The compact look

The window copies VirtualDJ's compact arrangement. Sections have no outlines: they touch, a
thin gap of background between them, and tell themselves apart by their fills. A section
that VirtualDJ names (FX, PADS, LOOP) carries its name turned a quarter anticlockwise at its
edge. The panel of a deck that a controller side drives has its side's accent along its
outer edge (the active deck's tab), and the right deck's panel is the left one mirrored
below the header: the transport, the pitch fader and the jog sit in the same order counted
from the mixer, and the states, FX and pads on the outside.

The deck row is as tall as its contents, so the browser takes all the rest of the height.
The same holds for every layout and for 4-Decks. Dragging the deck divider overrides that
height (see [Dividers](#dividers-folding-and-the-sideview)).

## Mouse

Every control the window draws can be used with the mouse. A mouse control does what the
controller input bound to it does (the window sends the same action), so a controller and the
mouse can be used together: a knob or fader the mouse moved does not jump when the SB3's
knob is turned; the soft takeover waits until the knob reaches the value on screen.

| Control | Gesture |
|---|---|
| Knobs (gain, EQ, filter, HEADPHONE level, MIX) | Drag up or down; hold Shift for a fine move (a quarter of the speed); scroll over it; double-click to reset: gain, EQ and filter to centre, HEADPHONE to 0.8, MIX to 0.0 (cue only) |
| Channel faders | Click or drag; no double-click reset |
| Crossfader, pitch slider | Click or drag; double-click resets them (the crossfader to the centre, the pitch slider to 0 %) |
| Master level | A bar: click or drag; double-click resets it to 0.8, its default |
| Auto-loop size button | Click to set a loop of that many beats |
| Buttons (PLAY, CUE, SYNC, PFL, LOOP IN and OUT, the loop arrows, VINYL, SLIP, KEYLOCK, the pad-mode arrows) | Click; a button that the controller holds (CUE, a loop roll) is held while the mouse button is down |
| Pads | Click to play; Shift-click for the shifted action (beat jump shift); right-click a hotcue pad to name and colour it |
| Hotcue flags on a waveform | Right-click to open the same editor |
| Jog | Drag round it to scratch (clockwise goes forward) |
| Waveform lane | Drag sideways to scratch; scroll or pinch to zoom |
| Overview waveform | Click to seek |
| Time under the BPM | Click to cycle remaining, total and elapsed |
| FX slots | Click a slot's name to toggle that effect on or off; its arrow opens the effect list; the FX level bar takes a click or drag |
| Pad mode | Pick from the dropdown, or step through the modes with the ◀ ▶ arrows beside it (they wrap round) |
| Browser tree | Click to select; the arrow, or a double-click, opens a drive or folder; right-click a playlist for Rename and Delete, and the Playlists node for New playlist |
| Browser list | Click to select; Shift-click for a range, Cmd/Ctrl-click to add or remove a row; double-click loads on the first free deck; right-click for the track menu; scroll to move |
| Search box and column headers | Type to search; Escape clears it; click a header to sort, again to reverse |
| Tracks | Drag onto a deck to load it, onto a playlist in the tree to add it (in list order), or inside an open playlist to reorder it |

The track menu loads on a deck ("Load on deck N"), adds the selected tracks to a playlist
("Add to playlist") and, in a playlist, removes them ("Remove from playlist"). The tracks
stay in the library. Dropping a playlist's own rows back on its tree row does nothing.

### Playlists from the tree

"New playlist…" on the Playlists node asks for a name and, when the Playlists node is open, selects the new playlist.
"Rename…" asks for the new name, starting with the current one selected. "Delete…" asks
"Delete <name>? Its tracks stay in the library." and has Delete and Cancel, with Cancel
focused, so Enter alone never deletes. In the name dialogs Enter means OK and Escape means
Cancel; OK is greyed out for an empty name. A name that is empty, or that another playlist
already has (the same letters, capitals included), is refused with a message in the events.

### Safety rules

- **A load onto a playing deck is refused.** A double-click, the track menu or a drop onto a
  deck that plays says "Deck N is playing" and leaves its track in place. The SB3's LOAD
  button is not covered; see [Known limits](#known-limits).
- **Closing a menu or dialog never triggers what is under it.** The press that closes the
  hotcue editor or a playlist dialog by clicking outside is used up by the close: it plays no
  pad, selects no row and moves no control.
- **A plain click on the jog or on a waveform does nothing.** Only a drag touches the
  platter, so a stray click never stops or nudges a playing track. (Clicking the overview
  seeks, on purpose.)
- **Deleting is never one click.** A hotcue is deleted from its own menu item, and a
  playlist after a question.
- **Playlist edits check the view they were made from.** A drop, a reorder or a removal
  names the tracks the window saw and where. If the playlist changed since (another window,
  the CLI, an import), nothing is written and the events say "The playlist changed; try
  again".

## What each area shows

- **Top bar:** the LAYOUT menu (see [The LAYOUT menu](#the-layout-menu)), the layout's name,
  the settings gear, status and events, the master meter, CPU load, the clock, and the
  controller and audio state.
- **Waveform lanes:** the decks both sides show, plus any other deck that is playing; in
  the 4-Decks view, all four. The LAYOUT menu picks the style: Frequency (colour follows
  frequency), Deck colour, Bands (low, mid and high drawn as separate bands) or Overlaid
  (the first two lanes' decks share one lane at 55 % opacity), and puts the lanes at the Top
  or in the Middle (between the deck panels and the browser). Beat ticks are drawn, with a
  red tick on the first beat of each bar. Cue and hotcue markers and the loop band show on
  the lane; a hotcue shows its colour, and its name cut at 14 characters when it has one.
  The playhead is fixed at the centre. Each lane blends three layers, low, mid and high, one
  colour per band (see [Waveform colours](#waveform-colours)), so where the bands overlap
  the colours mix as they do in VirtualDJ.
- **Zoom:** scroll (or pinch) over the waveforms, or click the `-` and `+` buttons at their
  top right. Each step is 1.25 times, between `ui.waveform_min_beats` and
  `ui.waveform_max_beats` (4 and 64 beats by default). Both lanes always share the zoom, so
  the two tracks' beats stay lined up. The
  `ui.waveform_zoom_in` and `ui.waveform_zoom_out` actions do the same from a controller
  button (unmapped by default). The zoom is saved with the rest of the window's view state
  in `ui-state.toml` next to `config.toml` (its `[view]` table) and restored at the next
  launch; `ui.waveform_beats` is only the zoom before that file exists (delete the file to
  go back to it). A value in the file that cannot be read keeps its default, and the others
  still load.
- **Deck panels:** cover, title, artist and remix, the overview waveform, BPM, a row under
  it with the stems state (`stems`, or `stems 40%` while they are made), the key and the
  bar.beat counter, two times, the FX slots (each a dropdown that puts another effect in that
  deck's slot), the jog, the pitch fader with its pickup marker, VINYL, SLIP and KEYLOCK, the
  pad mode dropdown and the 8 pads (a set hotcue pad reads `<n> <name>`, or `<n> loop` for a
  saved loop with no name), the loop size, and CUE, PLAY and SYNC. SYNC is never lit:
  sync is one-shot here, and there is no MASTER badge. Which of these a panel shows depends
  on the layout. Details:
  - **Times:** the brighter time is the one you cycle, by clicking it: remaining (the
    default), total, then elapsed. The dimmer one beside it is the elapsed time, or the
    remaining time while the brighter one shows elapsed. Each deck keeps its own choice,
    which is saved.
  - **Beat counter:** `bar.beat` beside the key, right of it on the same row, counted
    from 1 from the grid's first beat, 4 beats to a bar. Before the first beat the bars count backwards from 0. It shows only
    when the LAYOUT menu's Beat Counter is on and the track has a grid.
  - **Jog:** the Info jogwheel display is the default: the wheel shows the BPM, the pitch and
    both the elapsed and the remaining time inside it, and its marker moves to the rim.
- **Mixer:** one strip per deck, compact and in VirtualDJ's order: the master level on top,
  each channel's knobs (the three EQ knobs and FILTER) outside, in the columns next to the
  decks, GAIN over the channel fader in the middle, PFL under it, the level meters beside the
  faders (the live level, with a line marking the highest peak of the last second), and the
  crossfader at the bottom. With four decks the strips are narrower and the knobs carry
  one-letter captions instead of names: G for the gain and F for the filter, and for the
  EQ knobs H, M and B (high, mid, bass) in the EQ mode, or the first letters of the stem
  labels once stems are active (H, M, K for the `vdj` knobs; V, I, B for the `raw` knobs,
  with B for BASS as for LOW). The EQ labels follow the mode: HIGH, MID, LOW, or once stems
  are active HIHAT, MEL/VOX, KICK (`vdj` knobs) or VOCALS, INSTRU, BASS (`raw` knobs).
  Ghost markers show where a physical control is while soft takeover has not caught up with
  the value on screen (after a DECK switch, for instance).
- **Browser:** the drives and real folders, then playlists, as a tree. A drive plugged in
  or out shows at the next refresh, which follows library changes. Columns are Title,
  Artist, Remix, Length, BPM and Key, left-aligned under a header row, and the rows
  alternate in shade. A track that is loaded on a deck has a marker in that deck's accent at
  the left of its row. "•" marks a track with saved cues, and "missing" a file that is gone.
  An info panel shows the selected track. The own files of a folder that was never scanned
  are added and analysed in the background, so BPM and key fill in as they finish.
- **Panels:** the dividers, the info panel and the sideview are described in
  [Dividers, folding and the sideview](#dividers-folding-and-the-sideview).

## Waveform colours

Each frequency band has its own colour, and the lanes and the overviews blend the three. The
LAYOUT menu's Waveform Display entry (below the styles) and the Settings window's Interface
page choose the palette, and the choice is saved:

| Palette | Low | Mid | High |
|---|---|---|---|
| Red / green / blue (default) | red | green | blue |
| Blue / orange / white | blue | orange | white |
| Orange / yellow / cyan | orange | yellow | cyan |

The lanes and the deck panels' overviews stay dark in the light colour scheme too, as in
VirtualDJ's light skins, so every palette, the beat ticks, the hotcue flags and the playhead
read the same in both schemes.

## The LAYOUT menu

The LAYOUT button at the left of the top bar opens VirtualDJ's menu; the name of the current
layout is shown beside it. An entry asks the session to change the view, and the menu then
shows the state the session published, so a choice shows one frame later. Every choice is
saved in `ui-state.toml` (see below).

| Entry | Choices |
|---|---|
| Starter, Essentials, Pro, Performance | The layout (below) |
| Colour Scheme | Dark or Light |
| Waveform Display | Frequency, Deck colour, Bands or Overlaid; the waveform palette; Top or Middle for the position of the lanes; Overview, which shows or hides the deck panels' overview waveform |
| Jogwheel Display | Info (the default) or Plain |
| Beat Counter | On shows `bar.beat` in the deck panels |
| 4-Decks | The 4-Decks view |
| Panels | Info and Sideview, which show or fold the browser's info panel and the sideview |

The `ui.layout` action ([mapping reference](controllers/mapping-reference.md)), unmapped by
default, moves to the next layout from a controller button: Starter, Essentials, Pro,
Performance, then back to Starter. The Settings window's Interface page picks a layout too.

## The four layouts

Each layout decides which controls the deck panels and the mixer draw. The lanes always
take 22 % of the height, the deck row takes what its contents need and the browser the rest.
Pro is the default.

| Layout | Deck panels | Mixer |
|---|---|---|
| Starter | No FX slots and no pads | No level meters |
| Essentials | Everything but the FX level bar | Everything |
| Pro | Everything | Everything |
| Performance | No jog; the pads, FX and the rest stay | Everything |

The deck panels never get less height than every one of their areas needs, so the pads, the
pad mode, CUE, PLAY, SYNC and the loop always show, and at the smallest window (1024 by 700)
the browser still lists at least three rows. The mixer's knobs shrink to the height and
width their strip gets. The Interface page's sketches draw each layout with the
proportions it has in a window of the default size (1440 by 900).

Until the deck divider itself is dragged, the deck panels take their contents' height;
dragging another divider leaves that alone. Once the deck divider is dragged, its height
applies to every layout.

## 4-Decks

The 4-Decks entry shows all four decks at once, whatever the layout: two compact panels on
each side, in the order deck 1 above deck 3 on the left and deck 2 above deck 4 on the right,
and the mixer with four strips ordered 3, 1, 2, 4. The compact panels keep the overview,
the loop controls, the EQ and the meters; they have no FX, pads, jog or tempo slider (the
transport, the states and the loop share one row); the deck row is as tall as two stacked
panels need. While 4-Decks is on, the layout cannot be chosen (the
LAYOUT menu's layouts and the Interface page's tiles are greyed out, and say why), nor can
the Jogwheel Display, which Performance greys out too. The
waveform lanes show all four decks. The deck a controller side drives is still chosen on the
controller (DECK buttons).

## Dividers, folding and the sideview

Drag the dividers under the waveform lanes and the deck panels, and between the browser's
tree, list, info panel and the sideview; each share is kept between 5 % and 70 % of the
window and at least 80 points, the lanes and the deck panels together take at most 85 % of
the height, and the tree, the info panel and the sideview at most 80 % of the width. The
window will not shrink below 1024x700. A click on a divider that does not move it changes
nothing. The sizes are saved.

The Panels submenu folds the info panel and the sideview away, and a small "info" or
"sideview" tab at the browser's bottom right edge brings each back. The sideview shows the
sampler's 8 slots in a 2x4 grid (lit when a sample is loaded, brighter while it plays). It
only displays; the sampler is played from the controller's pads.

## Browsing from the DDJ-SB3

| Control | Does |
|---|---|
| BROWSE (turn) | Moves in the tree or the list |
| BROWSE (press) | Switches between the tree and the list |
| SHIFT + BROWSE (press) | Opens or closes the folder |
| LOAD | Loads the selected track on that side's deck |

## Pad mode name

The deck panel shows `pads` until a pad mode button is pressed after launch: the DDJ-SB3 does
not report its mode, so the window cannot know it before that. After a hardware mode button
it shows that button's name (HOT CUE, SAMPLER and so on).

The name is a dropdown of the modes the app switches itself: "pads (controller)" (the pads
go back to the SB3's own mode, `pads.mode_hardware`), BEAT JUMP, LOOP ROLL, SLICER and PAD
SCRATCH. It offers only these software modes because the SB3's hardware modes (HOT CUE, FX
FADE, SAMPLER, TRANS) are set by the controller itself, and only its own buttons can set
them: the app only learns which button was pressed. A choice does what the mapped pad mode
button does (the `pads.mode_*` actions).

## The FX dropdowns

Each of a deck panel's FX slots is a dropdown of the effects (echo, reverb, flanger,
filter). A choice changes that deck's effect at once and saves the deck's three effects as
`fx.deck1` to `fx.deck4` in `config.toml` ([config.md](config.md)); the other decks keep
theirs.

## `ui-state.toml`

The window's view is kept in `ui-state.toml` next to `config.toml`, as a `[view]` table. It
holds the layout, colour scheme, waveform style, palette and position, jogwheel display, the beat
counter, 4-Decks, overview, info panel and sideview switches, each deck's time mode, the
panel sizes (`[view.sizes]`: lanes, decks, tree, info, sideview) and the waveform zoom
(`waveform_beats`), with `version = 2`. It is saved about 2 seconds after the last change,
and when the window closes. A file from before the info jog (without `version = 2`) always
held `jog_display = "plain"`, chosen or not, so it switches to the info jog once; choose
Plain again and it is kept. A value that cannot be read keeps its default, the others still load; delete the file
to start from the defaults (the zoom then comes from `ui.waveform_beats`). The file is the
window's own, written by the app: settings that belong in `config.toml` are in the Settings
window.

## Fonts

Titles in scripts the built-in font lacks (Japanese, Cyrillic and so on) use the system's fonts
as a fallback.

## The settings window

The gear at the left of the top bar opens a Settings window (about 70 % of the screen,
closable) with a sidebar, as in VirtualDJ:

- **Audio:** a dropdown of the output devices (listed once at launch), and the `audio.*` settings.
- **Interface:** the four layouts as tiles, each a sketch of the sections it shows; click one
  to switch (not while 4-Decks is on).
- **Controllers:** a dropdown of the built-in mappings, and `controller.mapping`.
- **Options:** every `config.toml` setting ([config.md](config.md)) with its value editor
  (checkbox, dropdown, or a text field sent on Enter or when it loses focus) and what it
  does. The search box matches the key, group or help; "Show modified" lists the settings
  that differ from their default; "Reset all" asks first and removes every listed key from
  the file. A setting that applies at the next launch says "applies at next launch" once
  saved, and a value the file would not accept shows why on its row and changes nothing (a
  "Reset all" the file cannot take says why in the events). Text typed and not yet sent is
  dropped when the window closes. Before the session has read the settings the page says so
  (or shows the error that stopped it).
- **Library:** "Import from VirtualDJ…", a one-time import of hotcues, saved loops and
  playlists (see [library.md](library.md#importing-from-virtualdj)). It reads VirtualDJ's data
  and shows the counts, then Import asks track by track what to do with a track that has
  hotcues here too (Keep mine, Use VirtualDJ's or Merge, with a "for all remaining" box), takes
  a library backup, writes, and shows the backup's path. It refuses while a deck holds a
  track: unload the decks first.
- **USB export:** "Export to USB stick…" copies playlists, in order, with their hotcues and
  saved loops, to a stick for Serato, Rekordbox and VirtualDJ (see
  [library.md](library.md#exporting-to-a-usb-stick) and [interop.md](interop.md)). It lists
  the plugged-in sticks (with their free space) and the playlists; the playlists already on
  the chosen stick come ticked. Check shows what the export would do, the same lines as
  `library export`, with a stick too small said in red and Export disabled; Back returns to
  the choice. Export writes in the background with a progress bar, then says what was copied
  and kept and how to open the stick in Rekordbox. With no stick plugged in it says so: plug
  one in, then Refresh. The decks can stay loaded.
- **Later:** what will come (recording, broadcast, automix); nothing to set yet.

Some settings apply at once: the crossfader, the effects, the sampler volume, the pad
tables, the backspin times, `stems.knobs` and the `ui.*` zoom settings (and `stems.eq_mode`
when stems were enabled at launch). The others wait for the next launch: the audio device and
channels, the controller mapping, the sampler pads, `stems.enabled`, `stems.kicks_hz` and
`stems.cache_gb`.

Changes are written to the `config.toml` the window read, keeping its comments, order and
layout. If the file is edited by hand while the window runs, that edit is kept and the
window's change is added to the file as it now is. See [config.md](config.md).

## Known limits

- One audio device: master and headphones must be channels of the same device. Two devices
  come in a later plan.
- The USER buttons are not built.
- There are no keyboard shortcuts: everything is the mouse, or the controller.
- The PAD SCRATCH mode's pads are not clickable; the controller's pads do the scratch
  routine, and the on-screen pads of that mode are not its pads.
- The SB3's LOAD button still loads onto a playing deck (the refusal covers the window's
  loads, which are the ones a misclick makes).
