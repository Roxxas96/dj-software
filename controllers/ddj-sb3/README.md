# Pioneer DDJ-SB3

Files in this directory:

- `mapping.toml`: the default mapping, built into the app (`--controller ddj-sb3`).
- `capture.toml`: what every control of the owner's SB3 sent, recorded with
  `open-jockey midi capture`. It is the only source of the MIDI values in the mapping.
- `checklist.toml`: the steps of that capture.

- `firmware-modes.txt`, `firmware-modes-2.txt`: MIDI monitor recordings of the SB3's own
  pad routines (FX FADE, PAD SCRATCH, TRANS), used as test fixtures.

Every binding in the mapping is replayed from the capture by
`crates/controller/tests/sb3_replay.rs`, and the recorded routines are replayed in time
by `crates/controller/tests/sb3_firmware_replay.rs`.

## Using it

```sh
open-jockey play a.mp3 --device "DDJ-SB3" --controller ddj-sb3 --cue-track b.mp3 --headphones 3,4
open-jockey midi monitor --controller ddj-sb3
```

`--headphones 3,4` are the SB3's own headphone channels, so `--device "DDJ-SB3"` plays
through the SB3's audio interface.

When the controller first connects, the four channel faders start closed (the mapping
sets them to 0), so no deck is heard until you move its fader. The first move of each
knob or fader is taken where it is.

To change the mapping without rebuilding, copy `mapping.toml` to
`<config dir>/controllers/ddj-sb3/mapping.toml` (on macOS,
`~/Library/Application Support/open-jockey/`) and edit it, or pass a file path to
`--controller`.

## Layering: software

The SB3 sends the same bytes for decks 1 and 3 (and for 2 and 4), on the mixer as well
as on the decks, and its DECK buttons have no light. The app keeps track of which deck
each side shows: each DECK press switches the left side between decks 1 and 3, or the
right side between decks 2 and 4. Decks 1 and 2 are shown at start. After a switch the
knobs and faders use soft takeover: they act once they reach the new deck's value.

## Browsing and loading

With `play --library`, the BROWSE knob scrolls the library browser (clockwise moves
down), and pressing it switches between the tree (the drives' folders, then the playlists) and
the track list; SHIFT + BROWSE press opens or closes the highlighted folder. LOAD loads the selected track onto the deck its side currently shows: the left
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
| KEY LOCK | keylock on or off (the deck keeps its pitch when the tempo is not 0 %) |
| FX 1, 2, 3 | switch the effect in that slot on or off on the side's deck (which effect: `[fx] slots` in `docs/config.md`) |
| FX LEVEL | how much of the deck's effects is heard (soft takeover) |
| SHIFT + KEY LOCK | next tempo range (6, 10, 16, 100 %) |
| SHIFT | shift |
| DECK | switch the side's deck |
| TRIM, HI, MID, LOW, FILTER, channel fader | channel controls |
| headphone CUE | headphone cue (PFL) on/off |
| crossfader | crossfader |
| headphone MASTER button | headphones hear the master instead of the cue |
| BROWSE turn, BROWSE press | scroll the library, switch tree and list (`--library`) |
| SHIFT + BROWSE press | open or close the highlighted folder (`--library`) |
| LOAD left, LOAD right | load the selected track onto the side's deck (`--library`) |
| SYNC | match tempo and beats to the playing deck on the other side |
| VINYL | vinyl mode on/off (on at start): the touched jog top scratches |
| jog top touch | holds the record in vinyl mode |
| jog top | scratch (touched, vinyl on), otherwise nudge a playing deck or move a paused one |
| jog outer ring | nudge a playing deck, move a paused one |
| SHIFT + jog top | fast search |
| AUTO LOOP | loop of the current length from the beat before the playhead; again to exit |
| 1/2X, 2X | halve, double the loop length |
| SHIFT + 1/2X | loop in |
| SHIFT + 2X | loop out |
| SHIFT + AUTO LOOP | reloop / exit |
| HOT CUE pads 1-8 | set, jump to or (paused, while held) preview a hotcue |
| SHIFT + HOT CUE pads 1-8 | delete the hotcue |
| SAMPLER pads 1-8 (hardware pad mode) | play sample 1-8 from the start; again restarts it |
| SHIFT + SAMPLER pads 1-8 | stop sample 1-8 |
| SHIFT + HOT CUE, SHIFT + FX FADE, SHIFT + PAD SCRATCH | software pad mode of the side: beat jump, loop roll, slicer |
| HOT CUE, FX FADE, SAMPLER, SHIFT + SAMPLER | back to the hardware pad mode |
| PAD SCRATCH | the pad scratch mode of the side (the routine's anchor, see below) |
| pads in beat jump mode | jump by `beat_jump.pads` beats (default -4, 4, -8, 8, -16, 16, -32, 32) |
| SHIFT + pads in beat jump mode | jump by `beat_jump.shift_pads` beats (default -1, 1, -2, 2, -4, 4, -64, 64) |
| pads in loop roll mode | hold to roll `loop_roll.pads` beats (default 1/16 to 8), slip back on release |
| pads in slicer mode | hold to repeat beat 1-8 of the current 8-beat bar, slip back on release |
| SHIFT + VINYL | slip mode on/off |
| FX FADE, PAD SCRATCH and TRANS pads | the SB3's own routines (see below) |

The MASTER and HEADPHONES level knobs send MIDI, but the SB3 applies them in its own
audio interface, so they are left unbound and the software levels are set to full when
the controller attaches.

## Performance controls

The controls above act on the deck their side shows (decks 1/3 on the left, 2/4 on the
right). The pads send on their own MIDI channels (8 left, 9 right), but they follow the
side like every other control. Hotcues are remembered with the track in the library
(`--library`) and come back on the next load.

The mapping sets `jog_steps_per_turn = 720`, a first guess at how many steps one turn of
a wheel sends. The hands-on check below confirmed it.

The SHIFT loop functions (loop in, loop out, reloop/exit) follow the labels printed on
the SB3 in Pioneer's manual.

### Hands-on check

On 2026-11-05 the owner checked the deck controls with their SB3 on macOS:

- Jog top scratch in VINYL mode feels right at `jog_steps_per_turn = 720`.
- Hotcues work: set, trigger (playing, and the paused preview), SHIFT delete, remembered
  across reloads, and on deck 3.
- Loops work, including the SHIFT loop in, loop out and reloop functions, as labelled.
- SYNC works: tempo, beat alignment, slider pickup, and sync on a paused deck followed by
  PLAY.
- The outer-ring nudge lagged by about 1 s when starting and stopping. It was changed from
  a displacement to a speed bend that fades out within about half a second. To re-check.

## Sampler and software pad modes

The sampler plays up to eight samples listed in `config.toml` (see
[docs/config.md](../../docs/config.md)); both sides' SAMPLER pads play the same samples.

The SB3 does not switch its pads for the SHIFT pad modes (SHIFT + HOT CUE, SHIFT + FX FADE,
SHIFT + PAD SCRATCH): its pads keep sending the notes of the last hardware mode and its
mode lights do not change. So the app remembers each side's pad mode. The mode shows in the
status line (`left: beat jump`, `right: slicer`), because the SB3 has no light for it. The
software modes read the HOT CUE bank and the SAMPLER bank alike, so they work whichever of
the two the pads were in.

Caveat: if a side was in FX FADE or PAD SCRATCH before a software mode was chosen, the SB3
keeps running its own routine on those pads (filter or fader moves, jog touches). Press HOT
CUE or SAMPLER first, then the SHIFT mode.

A deck's line shows `slip` while slip mode is on and `roll` while a roll or slice is held.
A line `sampler: 2 5` lists the playing samples.

Crossfader: `mixer.crossfader = false` in `config.toml` makes the crossfader do nothing.

### Hands-on check (plan 5)

Done by the owner on 2026-11-06 with their SB3 on macOS and a `config.toml` holding a few
of their samples (`play --library --controller ddj-sb3`, master on the laptop speakers).
Everything worked:

- [x] Sampler: SAMPLER pads play from start to end, again restarts, SHIFT + pad stops, on
  both sides (this confirms the left SHIFT + pad notes 0x38-0x3F).
- [x] Beat jump: the pads and SHIFT + pads jump by the default sizes; a size changed in
  `config.toml` applies after a restart.
- [x] Loop roll and slicer (after HOT CUE): the pads roll and repeat, and the release lands
  in time.
- [x] Slip: with slip on, a scratch, loop or hotcue lands where the track would have been.
- [x] Crossfader: with `crossfader = false`, moving it changes nothing.

### Hands-on check (plan 6)

Done by the owner on 2026-11-07 with their SB3 on macOS (`play --library --controller
ddj-sb3`). Everything worked:

- [x] Keylock: KEY LOCK keeps the pitch at ±8 % and ±16 % on the owner's music; hotcues,
  CUE and the jog still feel instant; scratching sounds raw; nudging at 0 % is click-free.
- [x] FX: FX 1/2/3 switch echo, reverb and flanger on the side's deck; LEVEL sets the
  amount on the DJ-mixer dry/wet curve; the echo follows the beat; an effect stays on its
  deck after DECK.
- [x] Config: `[fx] slots` changes the slot effects after a restart.

## Stems

With `[stems] enabled = true` and `eq_mode = "stems"` in `config.toml`, the EQ knobs of a deck
whose stems are ready control stems. By default (`knobs = "vdj"`) LOW is the kicks, MID is the
melody (vocals, instruments and bass) and HI is the hats; `knobs = "raw"` gives HI to the vocals,
MID to the instruments and LOW to the bass. The knob law is in the [README](../../README.md#stems).
Until the stems are ready the knobs are the normal EQ. The SB3 needs no mapping change for that.

The seven stem pads (`deck.stem_toggle_vocals`, `_instruments`, `_bass`, `_rhythm`, `_kicks`,
`_melody` and `_hats`) have no default binding. To use them, extend the built-in mapping and
list only the pads you change: the
[mapping reference](../../docs/controllers/mapping-reference.md#extending-a-mapping) has a file
that puts them on SAMPLER pads 1-7 of both sides and keeps pad 8 playing sample 8. Save it as
`controllers/ddj-sb3/mapping.toml` in the config folder. The SB3's pad LEDs do not show stem
state.

Check the file with `open-jockey midi monitor --controller <path-to-your-mapping.toml>`, which
shows what each pad does.

### Hands-on check (plan 8)

Owner, 2026-11-08: "Test results are very good, everything works." Stems setup, separation on
load, the knobs, the pads, the instant reload and `library stems` all worked. Afterwards the
owner asked for the VirtualDJ knob layout and the right-half boost, which is the addendum
below.

### Hands-on check (plan 8 addendum: knob layouts and pads)

Owner, 2026-11-08: "Everything is perfect." The vdj and raw layouts, the right-half boost, the
seven stem pads and their overlaps all worked. The owner heard a bit too much kick in HI and not
enough in LOW with the split at 250 Hz, so the split became the `stems.kicks_hz` setting, 300 Hz
by default (`docs/config.md`).

### Hands-on check (plan 9: the window)

Owner, 2026-11-08: "very good for a first draft. I like the look of it and everything is very
smooth." The window, the waveforms and the SB3 mirroring work as a prototype. The owner has
tweaks in mind and will bring them to the next plans.

### Hands-on check (plan 10: customising the window)

Owner, 2026-11-09: "Check went well, i so far very happy." The four layouts from the menu and
from `ui.layout`, the light scheme, the waveform styles and positions, the info jog, the beat
counter and the time display, 4-Decks, the panel dividers and folds kept across a restart, the
pad and FX dropdowns, and the settings window all worked.

### Hands-on check (plan 11a: saved loops and the VirtualDJ import)

Owner, 2026-11-09: "Hands on test are all successful." Saving a loop on an empty hotcue pad
and playing it back (kept across a restart), the `library import-vdj` dry run, the import in
the window with Merge, the test track's coloured and named hotcues and its 8-beat loop, hotcue
positions matching VirtualDJ, and a second import changing nothing all worked. The owner
noticed that loading a track takes a few seconds.

### Hands-on check (plan 11c: USB export)

Owner, 2026-11-10 and 11 (macOS; Serato DJ Lite, Rekordbox, VirtualDJ 2026). Playlists showed
in order in all three, and hotcue positions were exact in all three (Serato MP3 with an Info
frame 1152 frames later, the others unshifted). Exporting again copied nothing new and showed
a moved hotcue; an unticked playlist disappeared from all three. Serato showed a repeated
track once (its limit; the export now says so) and has no saved loops in DJ Lite (Pro only,
not checked). Rekordbox imported the playlists through the XML; the steps are now in
`IMPORT TO REKORDBOX.txt` on the stick and on the export screen. VirtualDJ showed playlists and
cues correctly, and offered a one-time "database is corrupted, fix it?" because of the drive
letter; an export now keeps VirtualDJ's letter. To confirm: a re-export no longer asks and no
track is listed twice. Not verified: CDJ (not exported), another computer or Windows, Serato
DJ Pro's saved loops.

## The SB3's own pad routines

In FX FADE, PAD SCRATCH and TRANS (SHIFT + SAMPLER) the SB3 runs routines of its own on
the pads: it moves the fader and the FILTER itself, touches and turns a virtual jog, and
sends a few notes of its own. The app follows those messages, so the routines work as on
VirtualDJ:

- **FX FADE pads 1, 2, 5, 6:** the SB3 sweeps the FILTER and fades the fader.
- **FX FADE pad 3 (pad 7):** the first press starts a 1-beat (4-beat) loop while the fader
  falls; the SB3 shrinks pad 7's loop with its own 1/2X presses. The second press ends the
  loop. A loop the DJ had on is replaced by the routine's. The routine leaves the deck's
  AUTO LOOP size at 1 beat (pad 3) or at 4 beats halved by the SB3's own 1/2X presses
  (pad 7), as in VirtualDJ. If DECK was pressed in between,
  the second press still ends the loop on the deck it started on.
- **FX FADE pad 4 (pad 8):** a backspin of `backspin.short` (`backspin.long`) seconds,
  1 s and 2 s by default (`docs/config.md`).
- **PAD SCRATCH pads:** pressing PAD SCRATCH puts the side in its pad scratch mode. The
  routine's pads (notes 0x20-0x27) store where the deck is as its scratch anchor, then the
  SB3 keeps sending HOT CUE pad 1 about every beat to restart its scratch pattern, and the
  app seeks the deck back to the anchor (a seek: it drops a slip ghost or a roll), while
  the jog scratches whatever VINYL says. So the routine always scratches from where you
  were in the track. It never sets or uses a hotcue. The pad note comes before the first
  HOT CUE 1 note, so that first return stays put. A deck keeps its anchor until a routine
  pad sets another (a track load does not clear it); a return with no anchor yet stores
  the playhead. HOT CUE, FX FADE, SAMPLER or a SHIFT mode button leaves the mode.
- **TRANS pads:** the SB3 cuts the fader in rhythm.

When a routine ends, the deck is where the physical controls are: the SB3 sends the real
fader and FILTER positions at the start and end of every routine, no loop is left on,
nothing spins and the jog is not left touched. The routines act on the deck the side
shows, like every other control. A loop the DJ ends with AUTO LOOP meanwhile is not
restarted by the pad's second press.

Known limit: do not press DECK while a routine runs. The SB3 sends the fader's real
position when the routine ends, and that message goes to the deck the side shows by then.
So if DECK was pressed during, say, FX FADE pad 3, the routine's deck can be left with its
fader where the routine put it (possibly closed), and the newly shown deck's fader takes
the message only once soft takeover picks it up. Move that deck's fader to bring it back.
Only the fade loop itself follows its deck (see pad 3 above). The same goes for the SB3's
own 1/2X presses after DECK mid pad 7: they also go to the newly shown deck.

The SB3 sends its own jog touch and a small backward nudge with some FX FADE pad 4/8
presses, within a millisecond of the pad. The engine treats jog touches and moves within
50 ms of a backspin's start as part of it, so they neither block nor stop the spin; a
touch after that stops it as usual.

`LOOP_FADE_LOOP` (note 0x4D) is left unbound: the SB3 also sends it when TRANS is chosen
and during the backspin. The firmware messages were recorded on the left side only; the
right side is bound by symmetry (channel 2, pads on channel 9), which the capture
confirms for the same messages.

### Hands-on check (plan 7)

Done by the owner on 2026-11-07 with their SB3 on macOS, fader below full and filter off
center. Everything worked:

- [x] FX FADE pads 1-8: filter sweeps, the 1-beat fade loop, backspins and the shrinking
  loop; each ends with the deck at the physical fader and filter positions.
- [x] TRANS: fader cuts at the physical fader level, never louder.
- [x] PAD SCRATCH: scratches from where the track is when the pad is pressed (changed at
  the owner's request from hotcue 1), with VINYL on or off; hotcues are left alone.
- [x] The routines on deck 3 after DECK.

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

SHIFT + SYNC, SHIFT + the FX buttons (effect parameters) and fader start: they need
actions from later plans. The MIDI
messages these unmapped controls send are listed in a comment at the end of
`mapping.toml`.

The beatgrid actions (`deck.grid_*`) exist but no SB3 control is bound to them yet; use
the `grid` console commands.
