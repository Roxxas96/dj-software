# The window

`open-jockey` with no command opens a window that mirrors a Pioneer DDJ-SB3 in the layout of
VirtualDJ's Pro view: scrolling waveforms, deck panels, the mixer and a folder-tree browser.
You perform on the controller (transport, jogs, faders, pads); the window sets things up: its
layout, colours and displays, a pad mode or an effect, and the settings.

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

## What each area shows

- **Top bar:** the LAYOUT menu (see [The LAYOUT menu](#the-layout-menu)), the layout's name,
  the settings gear, status and events, the master meter, CPU load, the clock, and the
  controller and audio state.
- **Waveform lanes:** the decks both sides show, plus any other deck that is playing; in the
  4-Decks view, all four. The LAYOUT menu picks the style: Frequency (colour follows
  frequency), Deck colour, Bands (low, mid and high drawn as separate bands) or Overlaid
  (the first two lanes' decks share one lane at 55 % opacity), and puts the lanes at the Top
  or in the Middle (between the deck panels and the browser). Beat ticks are drawn, with a
  red tick on the first beat of each bar. Cue and hotcue markers and the loop band show on
  the lane. The playhead is fixed at the centre.
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
- **Deck panels:** cover, title, artist and remix, the overview waveform, BPM, key, the
  bar.beat counter, two times, the FX slots (each a dropdown that puts another effect in that
  deck's slot), the jog, the pitch fader with its pickup marker, VINYL, SLIP and KEYLOCK, the
  pad mode dropdown and the 8 pads, the loop size, and CUE, PLAY and SYNC. SYNC is never lit:
  sync is one-shot here, and there is no MASTER badge. Which of these a panel shows depends
  on the layout. Details:
  - **Times:** the brighter time is the one you cycle, by clicking it: remaining (the
    default), total, then elapsed. The dimmer one beside it is the elapsed time, or the
    remaining time while the brighter one shows elapsed. Each deck keeps its own choice,
    which is saved.
  - **Beat counter:** `bar.beat` under the key, counted from 1 from the grid's first beat,
    4 beats to a bar. Before the first beat the bars count backwards from 0. It shows only
    when the LAYOUT menu's Beat Counter is on and the track has a grid.
  - **Jog:** with the Info jogwheel display the wheel shows the BPM, the pitch and both the
    elapsed and the remaining time inside it, and its marker moves to the rim.
- **Mixer:** GAIN, three EQ knobs, FILTER, the channel fader, PFL, the level meters (the live
  level, with a line marking the highest peak of the last second), the crossfader and the
  master level. The EQ labels follow the mode: HIGH, MID, LOW, or once
  stems are active HIHAT, MEL/VOX, KICK (`vdj` knobs) or VOCALS, INSTRU, BASS (`raw` knobs).
  Ghost markers show where a physical control is while soft takeover has not caught up with
  the value on screen (after a DECK switch, for instance).
- **Browser:** the drives and real folders, then playlists, as a tree. A drive plugged in or
  out shows at the next refresh, which follows library changes. Columns are Title,
  Artist, Remix, Length, BPM and Key. "•" marks a track with saved cues, and "missing" a file
  that is gone. An info panel shows the selected track. The own files of a folder that was
  never scanned are added and analysed in the background, so BPM and key fill in as they
  finish.
- **Panels:** the dividers, the info panel and the sideview are described in
  [Dividers, folding and the sideview](#dividers-folding-and-the-sideview).

## The LAYOUT menu

The LAYOUT button at the left of the top bar opens VirtualDJ's menu; the name of the current
layout is shown beside it. An entry asks the session to change the view, and the menu then
shows the state the session published, so a choice shows one frame later. Every choice is
saved in `ui-state.toml` (see below).

| Entry | Choices |
|---|---|
| Starter, Essentials, Pro, Performance | The layout (below) |
| Colour Scheme | Dark or Light |
| Waveform Display | Frequency, Deck colour, Bands or Overlaid; Top or Middle for the position of the lanes; Overview, which shows or hides the deck panels' overview waveform |
| Jogwheel Display | Plain or Info |
| Beat Counter | On shows `bar.beat` in the deck panels |
| 4-Decks | The 4-Decks view |
| Panels | Info and Sideview, which show or fold the browser's info panel and the sideview |

The `ui.layout` action ([mapping reference](controllers/mapping-reference.md)), unmapped by
default, moves to the next layout from a controller button: Starter, Essentials, Pro,
Performance, then back to Starter. The Settings window's Interface page picks a layout too.

## The four layouts

Each layout decides which controls the deck panels and the mixer draw, and how the height is
shared between the deck panels and the browser (the lanes always take 22 %). Pro is the
default.

| Layout | Deck panels | Mixer | Decks' share of the height |
|---|---|---|---|
| Starter | No FX slots and no pads | No level meters | 42 % |
| Essentials | Everything but the FX level bar | Everything | 40 % |
| Pro | Everything | Everything | 38 % |
| Performance | No jog; the pads, FX and the rest stay | Everything | 26 % |

The deck panels never get less height than every one of their areas needs (about 300 points
with the jog or the pads, a little less without the overview): in a small window the decks
take more than their share, so the pads, the pad mode, CUE, PLAY, SYNC and the loop always
show. Performance's 26 % is below that up to a window about 1160 points high. The mixer's
knobs shrink to the height and width their strip gets.

Until the deck divider itself is dragged, the deck panels take their layout's height;
dragging another divider leaves that alone. Once the deck divider is dragged, its height
applies to every layout.

## 4-Decks

The 4-Decks entry shows all four decks at once, whatever the layout: two compact panels on
each side, in the order deck 1 above deck 3 on the left and deck 2 above deck 4 on the right,
and the mixer with four strips ordered 3, 1, 2, 4. The compact panels keep the overview,
the loop controls, the EQ and the meters; they have no FX, pads, jog or tempo slider (the
transport, the states and the loop share one row) and take 40 % of the height, or what two
stacked panels need when that is more. While 4-Decks is on, the layout cannot be chosen (the
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
holds the layout, colour scheme, waveform style and position, jogwheel display, the beat
counter, 4-Decks, overview, info panel and sideview switches, each deck's time mode, the
panel sizes (`[view.sizes]`: lanes, decks, tree, info, sideview) and the waveform zoom
(`waveform_beats`). It is saved about 2 seconds after the last change, and when the window
closes. A value that cannot be read keeps its default, the others still load; delete the file
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
- The window sets things up (the LAYOUT menu, the dropdowns, the settings window); performing
  (transport, jog, faders, pads) and the library's actions (browsing and loading) are on the
  controller only.
