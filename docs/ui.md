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
| `ui.waveform_beats` | `16` | Beats across a waveform lane, 4 to 64 |

The library opens at the default database. A missing DDJ-SB3 or audio device does not stop the
window: it tries again every 2 seconds, and the top bar says what is missing. A bad
configuration file or mapping does not close it either: the window shows the error, naming
the file and the problem, until it is closed. Closing the window saves cues and hotcues,
like Ctrl-C in `play`.

## What each area shows

- **Top bar:** status and events, the master meter, CPU load, the clock, and the controller
  and audio state.
- **Waveform lanes:** the decks both sides show, plus any other deck that is playing. Colour
  follows frequency. Beat ticks are drawn, with a red tick on the first beat of each bar.
  Cue and hotcue markers and the loop band show on the lane. The playhead is fixed at the
  centre; the zoom is `ui.waveform_beats`.
- **Deck panels:** cover, title, artist and remix, the overview waveform, BPM, key, elapsed
  and remaining time, the FX slots, the jog, the pitch fader with its pickup marker, VINYL,
  SLIP and KEYLOCK, the pad mode name and the 8 pads, the loop size, and CUE, PLAY and SYNC.
  SYNC is never lit: sync is one-shot here, and there is no MASTER badge.
- **Mixer:** GAIN, three EQ knobs, FILTER, the channel fader, PFL, the level meters, the
  crossfader and the master level. The EQ labels follow the mode: HIGH, MID, LOW, or once
  stems are active HIHAT, MEL/VOX, KICK (`vdj` knobs) or VOCALS, INSTRU, BASS (`raw` knobs).
  Ghost markers show where a physical control is while soft takeover has not caught up with
  the value on screen (after a DECK switch, for instance).
- **Browser:** the drives and real folders, then playlists, as a tree. A drive plugged in or
  out shows at the next refresh, which follows library changes. Columns are Title,
  Artist, Remix, Length, BPM and Key. "•" marks a track with saved cues, and "missing" a file
  that is gone. An info panel shows the selected track. The own files of a folder that was
  never scanned are added and analysed in the background, so BPM and key fill in as they
  finish.

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
