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
dj-software play a.mp3 --controller ddj-sb3 --cue-track b.mp3 --headphones 3,4
dj-software midi monitor --controller ddj-sb3
```

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

## Supported controls

| SB3 control | action |
|---|---|
| PLAY/PAUSE | play/pause |
| CUE | cue (hold to preview when paused at the cue point) |
| SHIFT + CUE | return to the start of the track |
| TEMPO slider | tempo; toward you is faster |
| SHIFT + KEY LOCK | next tempo range (6, 10, 16, 100 %) |
| SHIFT | shift |
| DECK | switch the side's deck |
| TRIM, HI, MID, LOW, FILTER, channel fader | channel controls |
| headphone CUE | headphone cue (PFL) on/off |
| crossfader | crossfader |
| headphone MASTER button | headphones hear the master instead of the cue |

The MASTER and HEADPHONES level knobs send MIDI, but the SB3 applies them in its own
audio interface, so they are left unbound and the software levels are set to full when
the controller attaches.

## LEDs

Only the headphone MASTER LED is driven. An LED scan of all 16 channels (note-on, with
and without audio streaming to the SB3) found no message that lights PLAY or CUE, and
the headphone CUE LED did not respond during the capture. SHIFT and DECK have no
lights. The PLAY and CUE LEDs probably need a "software connected" handshake first;
that is still to be investigated.

## Not mapped yet

SYNC, KEY LOCK, VINYL, the loop buttons, the jog wheels, the pad mode buttons and
pads, the FX section, BROWSE, LOAD and fader start: they need actions from later plans.
Their messages are listed at the end of `mapping.toml`.
