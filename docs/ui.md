# The window

`open-jockey` with no command opens a window that mirrors a Pioneer DDJ-SB3 in the layout of
VirtualDJ's Pro view: scrolling waveforms, deck panels, the mixer and a folder-tree browser.
The window only displays; everything is done on the controller.

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

- **Top bar:** the LAYOUT menu, status and events, the master meter, CPU load, the clock, and
  the controller and audio state. The LAYOUT menu asks for a layout, a colour scheme (dark or
  light), waveform and jogwheel displays, the beat counter, 4 decks and the info and side
  panels; the colour scheme already changes the window, the other entries take effect in
  later plan 10 tasks.
- **Waveform lanes:** the decks both sides show, plus any other deck that is playing. The LAYOUT
  menu picks the style: Frequency (colour follows frequency), Deck colour, Bands (highs,
  mids and bass stacked) or Overlaid (the two side decks share one lane at 55 % opacity),
  and puts the lanes at the Top or in the Middle (between the deck panels and the browser).
  Overview switches the deck panels' overview waveform on or off. Beat ticks are drawn, with a red tick on the first beat of each bar.
  Cue and hotcue markers and the loop band show on the lane. The playhead is fixed at the
  centre.
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
  times (click the brighter one to cycle it through remaining, total and elapsed;
  the dimmer one beside it is the elapsed time, or the remaining time while the
  brighter one shows elapsed), the bar.beat
  counter (LAYOUT menu), the FX slots, the jog (the LAYOUT menu's info jog draws the BPM,
  pitch and times inside it), the pitch fader with its pickup marker, VINYL,
  SLIP and KEYLOCK, the pad mode name and the 8 pads, the loop size, and CUE, PLAY and SYNC.
  SYNC is never lit: sync is one-shot here, and there is no MASTER badge.
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
- **Panels:** drag the dividers under the waveform lanes and the deck panels, and between
  the tree, the list, the info panel and the sideview; the sizes are saved with the rest of the
  view. The LAYOUT menu's Panels submenu folds the info panel and the sideview away, and a small
  "info" or "sideview" tab at the browser's bottom right edge brings each back. The sideview
  shows the sampler's 8 slots (lit when loaded, brighter while playing); it only displays. The
  window will not shrink below 1024x700. Until a deck divider is dragged, the deck panels take
  their layout's height; after that the dragged height applies to every layout.

## Browsing from the DDJ-SB3

| Control | Does |
|---|---|
| BROWSE (turn) | Moves in the tree or the list |
| BROWSE (press) | Switches between the tree and the list |
| SHIFT + BROWSE (press) | Opens or closes the folder |
| LOAD | Loads the selected track on that side's deck |

## Pad mode name

The deck panel shows `pads` until a pad mode button is pressed after launch: the DDJ-SB3 does
not report its mode, so the window cannot know it before that.

## Fonts

Titles in scripts the built-in font lacks (Japanese, Cyrillic and so on) use the system's fonts
as a fallback.

## Known limits

- One audio device: master and headphones must be channels of the same device. Two devices
  come in a later plan.
- No clickable controls: the window only displays.
- Plan 10 brings the LAYOUT menu, other waveform styles, a 4-deck view, resizable panels and a
  settings window.
