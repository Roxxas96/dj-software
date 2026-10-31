# Pioneer DDJ-SB3

Files in this directory:

- `mapping.toml`: the default mapping, built into the app (`--controller ddj-sb3`).
- `capture.toml`: what every control of the owner's SB3 sent, recorded with
  `dj-software midi capture`. It is the only source of the MIDI values in the mapping.
- `checklist.toml`: the steps of that capture.

Every binding in the mapping is replayed from the capture by
`crates/controller/tests/sb3_replay.rs`.

## Using it

```sh
dj-software play a.mp3 --device "DDJ-SB3" --controller ddj-sb3 --cue-track b.mp3 --headphones 3,4
dj-software midi monitor --controller ddj-sb3
```

`--headphones 3,4` are the SB3's own headphone channels, so `--device "DDJ-SB3"` plays
through the SB3's audio interface.

When the controller first connects, the four channel faders start closed (the mapping
sets them to 0), so no deck is heard until you move its fader. The first move of each
knob or fader is taken where it is.

To change the mapping without rebuilding, copy `mapping.toml` to
`<config dir>/controllers/ddj-sb3/mapping.toml` (on macOS,
`~/Library/Application Support/dj-software/`) and edit it, or pass a file path to
`--controller`.

## Layering: software

The SB3 sends the same bytes for decks 1 and 3 (and for 2 and 4), on the mixer as well
as on the decks, and its DECK buttons have no light. The app keeps track of which deck
each side shows: each DECK press switches the left side between decks 1 and 3, or the
right side between decks 2 and 4. Decks 1 and 2 are shown at start. After a switch the
knobs and faders use soft takeover: they act once they reach the new deck's value.

## Browsing and loading

With `play --library`, the BROWSE knob scrolls the library browser (clockwise moves
down), and pressing it switches between the tree (playlists, folders, all tracks) and the
track list. LOAD loads the selected track onto the deck its side currently shows: the left
LOAD onto deck 1 or 3, the right LOAD onto deck 2 or 4, following the DECK buttons. A cue
point you set is remembered and comes back when the track is loaded again. See
[docs/library.md](../../docs/library.md).

## Supported controls

| SB3 control | action |
|---|---|
| PLAY/PAUSE | play/pause |
| CUE | cue (hold to preview when paused at the cue point) |
| SHIFT + CUE | return to the start of the track |
| TEMPO slider | tempo; toward you is faster (checked on the owner's SB3) |
| SHIFT + KEY LOCK | next tempo range (6, 10, 16, 100 %) |
| SHIFT | shift |
| DECK | switch the side's deck |
| TRIM, HI, MID, LOW, FILTER, channel fader | channel controls |
| headphone CUE | headphone cue (PFL) on/off |
| crossfader | crossfader |
| headphone MASTER button | headphones hear the master instead of the cue |
| BROWSE turn, BROWSE press | scroll the library, switch tree and list (`--library`) |
| LOAD left, LOAD right | load the selected track onto the side's deck (`--library`) |

The MASTER and HEADPHONES level knobs send MIDI, but the SB3 applies them in its own
audio interface, so they are left unbound and the software levels are set to full when
the controller attaches.

## LEDs

Only the headphone MASTER LED is driven. An LED scan of all 16 channels (note-on, with
and without audio streaming to the SB3) found no message that lights PLAY or CUE, and
the headphone CUE LED did not respond during the capture. SHIFT and DECK have no
lights. The PLAY and CUE LEDs probably need a "software connected" handshake first;
that is still to be investigated.

## Troubleshooting

The SB3 is both the audio interface and the MIDI controller, on one USB cable. A short
USB dropout can make the audio stream report an error and leave the MIDI link silent
even though the SB3 is still listed. When `play` sees an audio stream error saying the
device went away or changed, it prints the error and reconnects the controller by itself
("controller reconnected after an audio/USB hiccup"), at most once every 5 seconds. A
mere audio glitch (the computer could not keep up) is only printed as a warning. If the
controller still does not respond, press Enter in the terminal running `play` to
reconnect it by hand (this needs `play` to run in a terminal, not with its input
redirected). Unplugging and plugging the SB3 back in
also works. After any reconnect, values set meanwhile are kept, and knobs and faders
must be picked up again (soft takeover).

## Not mapped yet

SYNC, KEY LOCK, VINYL, the loop buttons, the jog wheels, the pad mode buttons and
pads, the FX section and fader start: they need actions from later plans. The MIDI
messages these unmapped controls send are listed in a comment at the end of
`mapping.toml`.

The beatgrid actions (`deck.grid_*`) exist but no SB3 control is bound to them yet; use
the `grid` console commands.
