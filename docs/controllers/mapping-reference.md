# Mapping file reference

A mapping file tells the app what each control on a MIDI controller does and which
messages light its LEDs. It is a TOML file. The built-in mappings live in
[`controllers/`](../../controllers/); the DDJ-SB3 one is a good example to read next to this page.

New to this? [Adding a controller](adding-a-controller.md) walks through the whole process.

## Where mappings are loaded from

`open-jockey play --controller <name-or-path>` and `open-jockey midi monitor --controller <name-or-path>` look for a mapping like this:

1. If the argument is a path to a file, that file is loaded.
2. Otherwise it is a mapping name. A user file at `controllers/<name>/mapping.toml` inside the
   app's config directory wins:
   - macOS: `~/Library/Application Support/open-jockey/controllers/<name>/mapping.toml`
   - Windows: `%APPDATA%\open-jockey\config\controllers\<name>\mapping.toml`
   - Linux: `$XDG_CONFIG_HOME/open-jockey/controllers/<name>/mapping.toml`
     (`~/.config/open-jockey/controllers/<name>/mapping.toml` when `XDG_CONFIG_HOME` is not set)
3. If there is no user file, the built-in mapping with that name is used.

To change a few controls of a built-in mapping without rebuilding the app, put a short file
at the user location that [extends](#extending-a-mapping) it. To change everything, copy the
built-in `mapping.toml` there and edit it. The file is read when the app starts.

## Extending a mapping

A file with a top-level `extends` starts from another mapping and changes only what it says:

- `extends = "ddj-sb3"` names a built-in mapping. Any other value is a path to a mapping
  file; a relative path is read from the folder of the file that says `extends`. The parent
  may extend another mapping in turn.
- Everything of the parent is inherited: `[controller]`, `[[init]]`, `[[input]]` and
  `[[output]]`.
- `[controller]` is optional. Each key it sets (`name`, `port`, `jog_steps_per_turn`)
  overrides the parent's; the others are kept.
- `[[init]]` and `[[output]]` entries are added after the parent's.
- An `[[input]]` with the same MIDI message, the same `pad_mode` and the same `shift` as
  inherited bindings *replaces* them (a missing `pad_mode` or `shift` matches only a binding
  without one). Bindings of other messages, and of the same message in another pad mode or
  SHIFT layer, are kept unchanged. A `notes` range is matched note by note. `side`, `deck`
  and `active_deck` play no part in the match: only the MIDI message, `pad_mode` and `shift`
  do. The file's own inputs come after the inherited ones.
- `[[remove]]` deletes inherited bindings. It takes `midi` (as in `[[input]]`, `notes`
  ranges included) and optionally `pad_mode` and `shift`; a missing `pad_mode` or `shift`
  matches any. It only removes inherited bindings, never the file's own.

These are load errors that name the file: a parent that is neither a built-in mapping nor a
file, a chain of `extends` that comes back to a file already in it, a `[[remove]]` that
matches no inherited binding (`remove[0]: matches no inherited binding`), and a `[[remove]]`
in a file without `extends`.

For example, this file puts the seven [stem toggles](#actions) on SAMPLER pads 1-7 of the
DDJ-SB3, on both sides: the raw vocals, instruments, bass and rhythm on pads 1-4, then kicks,
melody and hats (the knob parts) on pads 5-7. Pad 8 is inherited and keeps playing sample 8.
Save it as `controllers/ddj-sb3/mapping.toml` in the config directory (see above), so it
replaces the built-in `ddj-sb3` it extends:

```toml
extends = "ddj-sb3"

# SAMPLER pads 1-7 (notes 0x30-0x36; channel 8 is the left side, 9 the right)
[[input]]
midi = { channel = 8, note = 0x30 }
action = "deck.stem_toggle_vocals"
side = "left"
pad_mode = "hardware"

[[input]]
midi = { channel = 8, note = 0x31 }
action = "deck.stem_toggle_instruments"
side = "left"
pad_mode = "hardware"

[[input]]
midi = { channel = 8, note = 0x32 }
action = "deck.stem_toggle_bass"
side = "left"
pad_mode = "hardware"

[[input]]
midi = { channel = 8, note = 0x33 }
action = "deck.stem_toggle_rhythm"
side = "left"
pad_mode = "hardware"

[[input]]
midi = { channel = 8, note = 0x34 }
action = "deck.stem_toggle_kicks"
side = "left"
pad_mode = "hardware"

[[input]]
midi = { channel = 8, note = 0x35 }
action = "deck.stem_toggle_melody"
side = "left"
pad_mode = "hardware"

[[input]]
midi = { channel = 8, note = 0x36 }
action = "deck.stem_toggle_hats"
side = "left"
pad_mode = "hardware"

[[input]]
midi = { channel = 9, note = 0x30 }
action = "deck.stem_toggle_vocals"
side = "right"
pad_mode = "hardware"

[[input]]
midi = { channel = 9, note = 0x31 }
action = "deck.stem_toggle_instruments"
side = "right"
pad_mode = "hardware"

[[input]]
midi = { channel = 9, note = 0x32 }
action = "deck.stem_toggle_bass"
side = "right"
pad_mode = "hardware"

[[input]]
midi = { channel = 9, note = 0x33 }
action = "deck.stem_toggle_rhythm"
side = "right"
pad_mode = "hardware"

[[input]]
midi = { channel = 9, note = 0x34 }
action = "deck.stem_toggle_kicks"
side = "right"
pad_mode = "hardware"

[[input]]
midi = { channel = 9, note = 0x35 }
action = "deck.stem_toggle_melody"
side = "right"
pad_mode = "hardware"

[[input]]
midi = { channel = 9, note = 0x36 }
action = "deck.stem_toggle_hats"
side = "right"
pad_mode = "hardware"
```

Each binding replaces the inherited `sampler.play_N` of its pad in the hardware pad mode; the
software pad modes keep their bindings on these notes, and SHIFT + SAMPLER pads 1-7 still stop
samples 1-7. To leave a pad doing nothing instead, use `[[remove]]`:

```toml
[[remove]]
midi = { channel = 8, notes = [0x34, 0x37] }
pad_mode = "hardware"
```

## Errors

A mapping that does not load stops the app with a message in the form
`<file>: <binding>: <problem>`, for example `input[3]: unknown action `deck.plya``.
`<binding>` is the kind of binding and its position in the file counting from 0
(`init[0]`, `input[12]`, `output[1]`, `remove[0]`).

Everything is checked when the file loads:

- unknown keys, anywhere in the file;
- unknown action ids and unknown state ids;
- MIDI channels outside 1-16, and note or controller numbers below 0 or above 127;
- decks outside 1-4;
- an action that needs a `deck` or `side` and has none (or has one it does not take);
- a value kind the action does not accept, or that does not fit the MIDI message type.

MIDI channels in mapping files are 1-based, as printed in the app's output. Numbers can be
written in decimal or hex (`note = 0x0B`).

## File layout

```toml
extends = "ddj-sb3"   # optional: start from another mapping (see above)
[controller]          # exactly one (optional with extends)
[[init]]              # any number: values set when the controller attaches
[[input]]             # any number: controls
[[output]]            # any number: LEDs
[[remove]]            # any number, with extends only: inherited bindings to delete
```

### `[controller]`

| Key | Meaning |
|---|---|
| `name` | Display name. |
| `port` | Text matched against the names of the MIDI input and output ports. Any port whose name contains it is used. |
| `jog_steps_per_turn` | Optional, 1 to 10000. How many relative steps the jog wheel sends for one full turn of the platter. Required whenever a binding uses `deck.jog_top`, `deck.jog_ring`, `deck.jog_search` or `deck.jog_scratch`: a mapping with such a binding and no key fails to load. |

`name` and `port` are required, except in a file that `extends` another.

### `[[init]]`

Sets a value when the controller first attaches. Use it for knobs the controller applies
itself, so the software level does not also quiet the sound, and for values that should not
follow wherever a control happens to sit at startup (see [soft takeover](#soft-takeover)).
It is not applied again after a reconnect, so values you set meanwhile are kept.

| Key | Meaning |
|---|---|
| `action` | An action that takes a position (see the table below). |
| `deck` | 1-4. Required for deck actions, not allowed for master actions. |
| `value` | A number from 0 to 1. Required. |

### `[[input]]`

| Key | Meaning |
|---|---|
| `midi` | Which message the control sends (see below). Required. |
| `action` | An action id. Required. |
| `deck` | 1-4: always this deck. |
| `side` | `"left"` or `"right"`: whatever deck that side shows now (see [Layers](#layers-and-deck-switching)). Use `deck` or `side`, never both. |
| `shift` | `true` or `false`: only fire while SHIFT of the binding's side is held, or not held. For master and browser actions, any side's SHIFT counts. |
| `active_deck` | 1-4: only fire while the binding's side shows this deck. Must be a deck on the binding's side. |
| `pad_mode` | `"hardware"`, `"beat_jump"`, `"loop_roll"`, `"slicer"` or `"pad_scratch"`: only fire while the binding's side has its performance pads in this mode. Needs a side: a `side`, or a `deck` (that deck's side). A master action takes `side` only together with `pad_mode`, where it just scopes the condition and the action stays global (for example `sampler.play_3` with `side = "left"`); `pad_mode` on a master action without `side` is a load error (`pad_mode needs a side`), and so is `side` on a master action without `pad_mode`. A press keeps its release even if the pad mode changes while it is held. |
| `value` | How to read the message: `"button"`, `"absolute"`, `"relative_twos"` or `"relative_offset"`. |
| `invert` | `true` flips an absolute value (`1 - v`) or negates relative steps. Default `false`. |
| `soft_takeover` | `false` turns [soft takeover](#soft-takeover) off for an absolute binding. Default `true`. |

Which of `deck` and `side` an action takes is in its row of the action table. Master and
browser actions take neither. `side.deck_toggle` and `modifier.shift` need `side`. Deck actions take `deck` or
`side`.

All bindings that match a message and whose conditions hold fire, in file order. A message
nothing is bound to is ignored.

#### `midi`

Exactly one of these, plus `channel` (1-16):

| Form | Message |
|---|---|
| `{ channel = 1, note = 0x0B }` | Note on and note off for that note. |
| `{ channel = 1, cc = 0x0D }` | Control change with that controller number. |
| `{ channel = 7, cc14 = [0x08, 0x28] }` | A 14-bit control change: the first number carries the high 7 bits, the second the low 7 bits. Gives finer steps than a plain `cc`. |
| `{ channel = 1, pitchbend = true }` | Pitch bend (14-bit). |
| `{ channel = 8, notes = [0x00, 0x07] }` | A range of notes, first to last inclusive (`[[input]]` only). See below. |

##### Note ranges

A `notes` range stands for one `[[input]]` per note, in order, and is expanded when the
file loads. Every key of the binding is copied unchanged to each one, except that `{n}`
in the `action` is replaced by the position in the range, counting from 1:

```toml
[[input]]
midi = { channel = 8, notes = [0x00, 0x07] }
action = "deck.loop_roll_{n}"     # deck.loop_roll_1 on note 0x00 ... deck.loop_roll_8 on 0x07
side = "left"
pad_mode = "loop_roll"
```

The expanded bindings take the range's place in the file, in note order. `notes` can't be
combined with `note`, `cc`, `cc14` or `pitchbend`, and `{n}` is only allowed in a binding
with `notes`. A reversed range, a note outside 0-127 and an expanded action id that does
not exist are load errors; an error in an expanded binding is reported as
`input[3] (note 0x05)`, with the position of the `[[input]]` in the file.

#### Value kinds

| `value` | Reads the message as | Works with |
|---|---|---|
| `button` | Pressed when note on, or a control change above 0. Released when note off, or a control change of 0. | note, cc |
| `absolute` | A position from 0 to 1 (0-127 is divided by 127; 14-bit values by 16383). | note, cc, cc14, pitchbend |
| `relative_twos` | Steps as two's complement: 1 to 63 forward, 64 to 127 backward (64 is -64). | cc |
| `relative_offset` | Steps offset by 64: 65 is +1, 63 is -1. | cc |

If you leave `value` out, notes are `button` and everything else is `absolute`.

The action decides which kinds it accepts (the "Takes" column below). The relative actions are
`browser.scroll`, `deck.jog_top`, `deck.jog_ring`, `deck.jog_search` and `deck.jog_scratch`; a relative binding to
any other action fails to load.

#### Soft takeover

Faders and knobs only report their own position. When the position on the hardware does not
match the value in the app, for example after you switch decks, the control would make the
value jump. Soft takeover prevents that: the control is ignored until it reaches the current
value, then takes over.

- A move reaches the value when it comes within 0.02 (2 % of the range) of it, or crosses it.
- When the controller first attaches, nothing is known about where its controls are, so
  every control starts disarmed: its first move is taken as is, wherever it sits. To keep a
  value from jumping at startup, set it with `[[init]]` (the DDJ-SB3 mapping closes every
  channel fader, so no deck is heard until its fader is touched).
- A control is armed (it must be picked up again) when the controller reattaches after
  being unplugged or reconnected, when a deck becomes the one a side shows, when a SHIFT
  layer or deck layer moves the same physical control onto a different value, and when
  something else changes the value after the control last set it.
- Moves that no binding acts on still count. A knob bound only with `shift = false`, turned
  while SHIFT is held, changes nothing, but the app remembers where it went: once SHIFT is
  released, the knob is armed and must be picked up again (by coming within 0.02 of the
  value or crossing it) before it changes the value.
- Once the control has taken over, every move passes through until it is armed again.
- Buttons and relative controls are never affected.

Turn it off per binding with `soft_takeover = false` (for example for a control that always
jumps, like a pad).

### `[[output]]`

Drives an LED from app state.

| Key | Meaning |
|---|---|
| `state` | A state id (see the table below). Required. |
| `deck` / `side` | The target, as in the state table. A `side` follows the deck that side shows. |
| `midi` | `{ channel = 1, note = 0x0B }` or `{ channel = 1, cc = 0x0B }`. Outputs take notes and control changes only. |
| `on` | The value sent to light the LED. Default 127. |
| `off` | The value sent to switch it off. Default 0. |

LED values are sent only when they change. A `note` output is sent as a note on with velocity
`on` or `off`. It is never a note off, because controllers such as the DDJ-SB3 expect a note
on with velocity 0. A `cc` output is a control change with value `on` or `off`. Blinking LEDs
share one clock with a 500 ms period (lit for the first 250 ms). When the controller comes
back after being unplugged, every LED is sent again.

## Actions

Ids are what you write in `action = "..."`. "Target" is what the binding needs:
`deck` (a deck, or a side to follow the deck it shows), `side`, or nothing (master and
browser actions).

| Id | Target | Takes | What it does |
|---|---|---|---|
| `deck.play_pause` | deck | button | Press to toggle play and pause. |
| `deck.cue` | deck | button | Press while playing: jump back to the cue point and pause. Press while paused away from the cue point: set the cue point here. Hold while paused at the cue point: preview from there (release returns and stops). |
| `deck.tempo` | deck | absolute | The tempo slider: 0.5 is the original tempo. Which end is faster depends on the controller, so use `invert` to match the slider's printed direction. |
| `deck.tempo_range_next` | deck | button | Press to go to the next tempo range: 6 %, 10 %, 16 %, 100 %, then back to 6 %. |
| `deck.return_to_start` | deck | button | Press to jump to the start of the track. Release ends a CUE preview. |
| `deck.grid_shift_back` | deck | button | Press to move the beatgrid 5 ms earlier, or 1 ms with SHIFT held on the binding's side. |
| `deck.grid_shift_forward` | deck | button | Press to move the beatgrid 5 ms later, or 1 ms with SHIFT held on the binding's side. |
| `deck.grid_set_downbeat` | deck | button | Press to move the beatgrid so a beat falls exactly at the playhead. |
| `deck.grid_halve` | deck | button | Press to halve the BPM, keeping the beat nearest the playhead. |
| `deck.grid_double` | deck | button | Press to double the BPM, keeping the beat nearest the playhead. |
| `deck.grid_tap` | deck | button | Tap tempo: from the fourth tap on, the BPM follows the mean interval of the taps. A pause of more than 2 s starts over. |
| `deck.grid_lock_toggle` | deck | button | Press to lock or unlock the beatgrid. A locked grid is kept when the track is analyzed again. |
| `deck.load_selected` | deck | button | Press to load the track selected in the library browser onto the deck. |
| `deck.hotcue_1` | deck | button | Hotcue pad 1. Press on an empty pad: set it at the playhead. Press on a set pad: jump to it, which switches an active loop off when the pad lies outside that loop; while paused, hold to preview from there, and release to stop the preview and return to the pad. |
| `deck.hotcue_2` | deck | button | Hotcue pad 2. Press on an empty pad: set it at the playhead. Press on a set pad: jump to it, which switches an active loop off when the pad lies outside that loop; while paused, hold to preview from there, and release to stop the preview and return to the pad. |
| `deck.hotcue_3` | deck | button | Hotcue pad 3. Press on an empty pad: set it at the playhead. Press on a set pad: jump to it, which switches an active loop off when the pad lies outside that loop; while paused, hold to preview from there, and release to stop the preview and return to the pad. |
| `deck.hotcue_4` | deck | button | Hotcue pad 4. Press on an empty pad: set it at the playhead. Press on a set pad: jump to it, which switches an active loop off when the pad lies outside that loop; while paused, hold to preview from there, and release to stop the preview and return to the pad. |
| `deck.hotcue_5` | deck | button | Hotcue pad 5. Press on an empty pad: set it at the playhead. Press on a set pad: jump to it, which switches an active loop off when the pad lies outside that loop; while paused, hold to preview from there, and release to stop the preview and return to the pad. |
| `deck.hotcue_6` | deck | button | Hotcue pad 6. Press on an empty pad: set it at the playhead. Press on a set pad: jump to it, which switches an active loop off when the pad lies outside that loop; while paused, hold to preview from there, and release to stop the preview and return to the pad. |
| `deck.hotcue_7` | deck | button | Hotcue pad 7. Press on an empty pad: set it at the playhead. Press on a set pad: jump to it, which switches an active loop off when the pad lies outside that loop; while paused, hold to preview from there, and release to stop the preview and return to the pad. |
| `deck.hotcue_8` | deck | button | Hotcue pad 8. Press on an empty pad: set it at the playhead. Press on a set pad: jump to it, which switches an active loop off when the pad lies outside that loop; while paused, hold to preview from there, and release to stop the preview and return to the pad. |
| `deck.hotcue_delete_1` | deck | button | Press to clear hotcue pad 1. Release ends a preview of pad 1. |
| `deck.hotcue_delete_2` | deck | button | Press to clear hotcue pad 2. Release ends a preview of pad 2. |
| `deck.hotcue_delete_3` | deck | button | Press to clear hotcue pad 3. Release ends a preview of pad 3. |
| `deck.hotcue_delete_4` | deck | button | Press to clear hotcue pad 4. Release ends a preview of pad 4. |
| `deck.hotcue_delete_5` | deck | button | Press to clear hotcue pad 5. Release ends a preview of pad 5. |
| `deck.hotcue_delete_6` | deck | button | Press to clear hotcue pad 6. Release ends a preview of pad 6. |
| `deck.hotcue_delete_7` | deck | button | Press to clear hotcue pad 7. Release ends a preview of pad 7. |
| `deck.hotcue_delete_8` | deck | button | Press to clear hotcue pad 8. Release ends a preview of pad 8. |
| `deck.loop_in` | deck | button | Press to start a manual loop at the playhead. This clears the previous loop-out and switches an active loop off. |
| `deck.loop_out` | deck | button | Press to end the loop at the playhead and switch it on, wrapping the playhead from the end back to the start. Does nothing without a loop-in before the playhead. |
| `deck.auto_loop` | deck | button | Press to switch on a loop of the current length (4 beats at first) starting at the beat before the playhead, which needs a beatgrid. With a loop active (a manual one too), it switches the loop off instead, with or without a grid. |
| `deck.loop_halve` | deck | button | Press to halve the loop length (down to 1/32 beat). An active loop shrinks to half its current length, keeping its start, and stops at 1/32 beat (with a beatgrid) or once the loop length can't halve further. |
| `deck.loop_double` | deck | button | Press to double the loop length (up to 64 beats). An active loop grows to twice its current length, keeping its start, and stops at 64 beats (with a beatgrid) or once the loop length can't double further. |
| `deck.reloop_exit` | deck | button | Press while a loop is on to leave it. Press while it is off to jump to its start and switch it on again. |
| `deck.sync` | deck | button | Beat sync: matches the deck's tempo to a playing deck (the active deck of the other side first, otherwise the lowest-numbered one) and aligns the beats, by half or double time when that fits the deck's tempo better. It widens the tempo range when needed, and the tempo slider must be picked up again afterwards (soft takeover). On a paused deck the alignment happens when it starts playing. Needs a beatgrid on both decks; otherwise it does nothing. |
| `deck.vinyl_toggle` | deck | button | Press to turn vinyl mode on or off for the deck. In vinyl mode, touching the jog wheel top scratches. |
| `deck.jog_touch` | deck | button | Press and release track whether a hand is on the top of the jog wheel. |
| `deck.jog_release` | deck | button | Press and release both mark the top of the jog wheel as let go, so a touch never starts from it. For the touch a controller sends with SHIFT held: a SHIFT search keeps playing instead of holding the record, and a touch begun without SHIFT still ends. |
| `deck.jog_top` | deck | relative | The top of the jog wheel. While touched in vinyl mode it scratches: the playhead follows the platter, and a held record is silent. Otherwise it nudges a playing deck (a turn bends the speed, up to 50 %, and the bend fades out within about half a second of the hand stopping), or moves the playhead of a paused one (one platter turn is 1.8 s of track). |
| `deck.jog_ring` | deck | relative | The outer ring of the jog wheel. It nudges a playing deck (a speed bend that fades out within about half a second) and moves a paused one's playhead, like the top without vinyl mode, and is ignored while the deck is scratching. |
| `deck.jog_search` | deck | relative | Search: moves the playhead 10 s of track per platter turn, leaving a loop it lands outside of. |
| `deck.jog_scratch` | deck | relative | Platter scratch: like `deck.jog_top`, but a touched deck is scratched even with vinyl mode off, and a running backspin is cancelled only by a touched platter move after its first 50 ms (an untouched move nudges). |
| `deck.backspin_short` | deck | button | Backspin of the playing deck, `backspin.short` seconds from `config.toml` (default 1 s). |
| `deck.backspin_long` | deck | button | Backspin of the playing deck, `backspin.long` seconds (default 2 s). |
| `deck.scratch_anchor_set` | deck | button | Stores the deck's current playhead as its scratch anchor (one per deck, kept until set again; not cleared when a track loads). |
| `deck.scratch_anchor_return` | deck | button | Seeks the deck to its scratch anchor (a plain seek: it drops slip ghosts, rolls and a backspin, and keeps the play state). With no anchor yet, it stores the playhead as the anchor and does not move. |
| `deck.loop_fade_1` | deck | button | With no normal loop active, loops 1 beat from the last beat and sets the auto-loop size to 1 beat (the next `deck.auto_loop` uses it). Does nothing while a loop is on. |
| `deck.loop_fade_4` | deck | button | With no normal loop active, loops 4 beats from the last beat and sets the auto-loop size to 4 beats (the next `deck.auto_loop` uses it). Does nothing while a loop is on. |
| `deck.loop_exit` | deck | button | Leaves a normal loop. Does nothing if none is on, and never reloops. With a `side` binding, the side's first press after a `deck.loop_fade_N` press goes to the deck that fade loop started on, even after a deck switch. |
| `channel.trim` | deck | absolute | Channel gain (TRIM). |
| `channel.eq_high` | deck | absolute | High EQ. |
| `channel.eq_mid` | deck | absolute | Mid EQ. |
| `channel.eq_low` | deck | absolute | Low EQ. |
| `channel.filter` | deck | absolute | Channel filter. |
| `channel.fader` | deck | absolute | Channel fader. |
| `channel.pfl_toggle` | deck | button | Press to send the channel to the headphone cue (PFL) or take it off. |
| `master.crossfader` | none | absolute | Crossfader. |
| `master.level` | none | absolute | Master level. |
| `master.headphone_level` | none | absolute | Headphone level. |
| `master.headphone_mix` | none | absolute | Headphone cue/master mix: 0 is only the cue, 1 is only the master. |
| `master.headphone_master_toggle` | none | button | Press to flip the headphone mix between all cue and all master. |
| `side.deck_toggle` | side | button | Press to make the side show its other deck (left: 1 and 3, right: 2 and 4). |
| `modifier.shift` | side | button | Press and release track whether SHIFT is held on that side. |
| `browser.scroll` | none | relative | Move the cursor of the focused browser pane by the steps sent (positive is down). It stops at the first and last rows. |
| `browser.toggle_focus` | none | button | Press to switch the browser between the tree (the drives and their folders, then the playlists) and the list of the highlighted entry. |
| `browser.toggle_folder` | none | button | Press to open or close the highlighted folder in the browser tree. |
| `pads.mode_hardware` | side | button | Press to set the side's pad mode to hardware (the controller's own pad modes); no sound changes by itself. |
| `pads.mode_hot_cue`, `pads.mode_fx_fade`, `pads.mode_sampler`, `pads.mode_trans` | side | button | Press to set the side's pad mode to hardware and remember which hardware mode button it was (the window shows its name); no sound changes by itself. |
| `pads.mode_beat_jump` | side | button | Press to set the side's pad mode to beat jump; no sound changes by itself. |
| `pads.mode_loop_roll` | side | button | Press to set the side's pad mode to loop roll; no sound changes by itself. |
| `pads.mode_slicer` | side | button | Press to set the side's pad mode to slicer; no sound changes by itself. |
| `pads.mode_pad_scratch` | side | button | Press to set the side's pad mode to pad scratch (for a controller's scratch routine: bind `deck.scratch_anchor_set` and `deck.scratch_anchor_return` with `pad_mode = "pad_scratch"`); no sound changes by itself. |
| `deck.beat_jump_1` | deck | button | Press to jump the playhead by the size of beat jump pad 1 (default -4, 4, -8, 8, -16, 16, -32, 32 beats for pads 1 to 8). Needs a beatgrid; the jump stops at the track's start and end. |
| `deck.beat_jump_2` | deck | button | Press to jump the playhead by the size of beat jump pad 2 (default -4, 4, -8, 8, -16, 16, -32, 32 beats for pads 1 to 8). Needs a beatgrid; the jump stops at the track's start and end. |
| `deck.beat_jump_3` | deck | button | Press to jump the playhead by the size of beat jump pad 3 (default -4, 4, -8, 8, -16, 16, -32, 32 beats for pads 1 to 8). Needs a beatgrid; the jump stops at the track's start and end. |
| `deck.beat_jump_4` | deck | button | Press to jump the playhead by the size of beat jump pad 4 (default -4, 4, -8, 8, -16, 16, -32, 32 beats for pads 1 to 8). Needs a beatgrid; the jump stops at the track's start and end. |
| `deck.beat_jump_5` | deck | button | Press to jump the playhead by the size of beat jump pad 5 (default -4, 4, -8, 8, -16, 16, -32, 32 beats for pads 1 to 8). Needs a beatgrid; the jump stops at the track's start and end. |
| `deck.beat_jump_6` | deck | button | Press to jump the playhead by the size of beat jump pad 6 (default -4, 4, -8, 8, -16, 16, -32, 32 beats for pads 1 to 8). Needs a beatgrid; the jump stops at the track's start and end. |
| `deck.beat_jump_7` | deck | button | Press to jump the playhead by the size of beat jump pad 7 (default -4, 4, -8, 8, -16, 16, -32, 32 beats for pads 1 to 8). Needs a beatgrid; the jump stops at the track's start and end. |
| `deck.beat_jump_8` | deck | button | Press to jump the playhead by the size of beat jump pad 8 (default -4, 4, -8, 8, -16, 16, -32, 32 beats for pads 1 to 8). Needs a beatgrid; the jump stops at the track's start and end. |
| `deck.beat_jump_shift_1` | deck | button | Press to jump the playhead by the size of shifted beat jump pad 1 (default -1, 1, -2, 2, -4, 4, -64, 64 beats for pads 1 to 8). Needs a beatgrid. |
| `deck.beat_jump_shift_2` | deck | button | Press to jump the playhead by the size of shifted beat jump pad 2 (default -1, 1, -2, 2, -4, 4, -64, 64 beats for pads 1 to 8). Needs a beatgrid. |
| `deck.beat_jump_shift_3` | deck | button | Press to jump the playhead by the size of shifted beat jump pad 3 (default -1, 1, -2, 2, -4, 4, -64, 64 beats for pads 1 to 8). Needs a beatgrid. |
| `deck.beat_jump_shift_4` | deck | button | Press to jump the playhead by the size of shifted beat jump pad 4 (default -1, 1, -2, 2, -4, 4, -64, 64 beats for pads 1 to 8). Needs a beatgrid. |
| `deck.beat_jump_shift_5` | deck | button | Press to jump the playhead by the size of shifted beat jump pad 5 (default -1, 1, -2, 2, -4, 4, -64, 64 beats for pads 1 to 8). Needs a beatgrid. |
| `deck.beat_jump_shift_6` | deck | button | Press to jump the playhead by the size of shifted beat jump pad 6 (default -1, 1, -2, 2, -4, 4, -64, 64 beats for pads 1 to 8). Needs a beatgrid. |
| `deck.beat_jump_shift_7` | deck | button | Press to jump the playhead by the size of shifted beat jump pad 7 (default -1, 1, -2, 2, -4, 4, -64, 64 beats for pads 1 to 8). Needs a beatgrid. |
| `deck.beat_jump_shift_8` | deck | button | Press to jump the playhead by the size of shifted beat jump pad 8 (default -1, 1, -2, 2, -4, 4, -64, 64 beats for pads 1 to 8). Needs a beatgrid. |
| `deck.loop_roll_1` | deck | button | Hold to roll a loop of the size of loop roll pad 1 (default 1/16, 1/8, 1/4, 1/2, 1, 2, 4, 8 beats for pads 1 to 8), its start snapped down to a multiple of that size on the beatgrid; release to end the roll, after which the track continues from where it would have been. Needs a beatgrid. |
| `deck.loop_roll_2` | deck | button | Hold to roll a loop of the size of loop roll pad 2 (default 1/16, 1/8, 1/4, 1/2, 1, 2, 4, 8 beats for pads 1 to 8), its start snapped down to a multiple of that size on the beatgrid; release to end the roll, after which the track continues from where it would have been. Needs a beatgrid. |
| `deck.loop_roll_3` | deck | button | Hold to roll a loop of the size of loop roll pad 3 (default 1/16, 1/8, 1/4, 1/2, 1, 2, 4, 8 beats for pads 1 to 8), its start snapped down to a multiple of that size on the beatgrid; release to end the roll, after which the track continues from where it would have been. Needs a beatgrid. |
| `deck.loop_roll_4` | deck | button | Hold to roll a loop of the size of loop roll pad 4 (default 1/16, 1/8, 1/4, 1/2, 1, 2, 4, 8 beats for pads 1 to 8), its start snapped down to a multiple of that size on the beatgrid; release to end the roll, after which the track continues from where it would have been. Needs a beatgrid. |
| `deck.loop_roll_5` | deck | button | Hold to roll a loop of the size of loop roll pad 5 (default 1/16, 1/8, 1/4, 1/2, 1, 2, 4, 8 beats for pads 1 to 8), its start snapped down to a multiple of that size on the beatgrid; release to end the roll, after which the track continues from where it would have been. Needs a beatgrid. |
| `deck.loop_roll_6` | deck | button | Hold to roll a loop of the size of loop roll pad 6 (default 1/16, 1/8, 1/4, 1/2, 1, 2, 4, 8 beats for pads 1 to 8), its start snapped down to a multiple of that size on the beatgrid; release to end the roll, after which the track continues from where it would have been. Needs a beatgrid. |
| `deck.loop_roll_7` | deck | button | Hold to roll a loop of the size of loop roll pad 7 (default 1/16, 1/8, 1/4, 1/2, 1, 2, 4, 8 beats for pads 1 to 8), its start snapped down to a multiple of that size on the beatgrid; release to end the roll, after which the track continues from where it would have been. Needs a beatgrid. |
| `deck.loop_roll_8` | deck | button | Hold to roll a loop of the size of loop roll pad 8 (default 1/16, 1/8, 1/4, 1/2, 1, 2, 4, 8 beats for pads 1 to 8), its start snapped down to a multiple of that size on the beatgrid; release to end the roll, after which the track continues from where it would have been. Needs a beatgrid. |
| `deck.slice_1` | deck | button | Hold to play slice 1 of the 8-beat block around where the track would be; release to return to the track. Needs a beatgrid. |
| `deck.slice_2` | deck | button | Hold to play slice 2 of the 8-beat block around where the track would be; release to return to the track. Needs a beatgrid. |
| `deck.slice_3` | deck | button | Hold to play slice 3 of the 8-beat block around where the track would be; release to return to the track. Needs a beatgrid. |
| `deck.slice_4` | deck | button | Hold to play slice 4 of the 8-beat block around where the track would be; release to return to the track. Needs a beatgrid. |
| `deck.slice_5` | deck | button | Hold to play slice 5 of the 8-beat block around where the track would be; release to return to the track. Needs a beatgrid. |
| `deck.slice_6` | deck | button | Hold to play slice 6 of the 8-beat block around where the track would be; release to return to the track. Needs a beatgrid. |
| `deck.slice_7` | deck | button | Hold to play slice 7 of the 8-beat block around where the track would be; release to return to the track. Needs a beatgrid. |
| `deck.slice_8` | deck | button | Hold to play slice 8 of the 8-beat block around where the track would be; release to return to the track. Needs a beatgrid. |
| `deck.slip_toggle` | deck | button | Press to turn slip mode on or off. In slip mode, scratching, loops, hotcue holds, rolls and slices run on a ghost playhead, and the track rejoins where it would have been. |
| `deck.keylock_toggle` | deck | button | Press to turn keylock on or off. With keylock on, a deck away from its original tempo keeps its pitch. |
| `deck.fx_toggle_1` | deck | button | Press to turn effect slot 1 of the deck's channel on or off. The slot's effect is set with `[fx] slots` in `config.toml`. |
| `deck.fx_toggle_2` | deck | button | Press to turn effect slot 2 of the deck's channel on or off. |
| `deck.fx_toggle_3` | deck | button | Press to turn effect slot 3 of the deck's channel on or off. |
| `deck.fx_level` | deck | absolute | Sets how much of the deck's effects is heard (the effect amount), 0 to 1; 0.5 until moved. Takes soft takeover like the other knobs. |
| `deck.stem_toggle_vocals` | deck | button | Press to turn the deck's vocals stem off or on (all are on after a load). The rhythm pad covers kicks and hats, the melody pad covers vocals, instruments and bass. Works whatever `[stems] eq_mode` is, once the deck has its stems; a press before then is remembered and applies when they are ready. Needs `[stems] enabled = true`. No LED feedback. |
| `deck.stem_toggle_instruments` | deck | button | Press to turn the deck's instruments stem off or on (all are on after a load). The rhythm pad covers kicks and hats, the melody pad covers vocals, instruments and bass. Works whatever `[stems] eq_mode` is, once the deck has its stems; a press before then is remembered and applies when they are ready. Needs `[stems] enabled = true`. No LED feedback. |
| `deck.stem_toggle_bass` | deck | button | Press to turn the deck's bass stem off or on (all are on after a load). The rhythm pad covers kicks and hats, the melody pad covers vocals, instruments and bass. Works whatever `[stems] eq_mode` is, once the deck has its stems; a press before then is remembered and applies when they are ready. Needs `[stems] enabled = true`. No LED feedback. |
| `deck.stem_toggle_rhythm` | deck | button | Press to turn the deck's rhythm stem off or on (all are on after a load). The rhythm pad covers kicks and hats, the melody pad covers vocals, instruments and bass. Works whatever `[stems] eq_mode` is, once the deck has its stems; a press before then is remembered and applies when they are ready. Needs `[stems] enabled = true`. No LED feedback. |
| `deck.stem_toggle_kicks` | deck | button | Press to turn the deck's kicks (the low part of the rhythm) off or on (all are on after a load). The rhythm pad covers kicks and hats, the melody pad covers vocals, instruments and bass. Works whatever `[stems] eq_mode` is, once the deck has its stems; a press before then is remembered and applies when they are ready. Needs `[stems] enabled = true`. No LED feedback. |
| `deck.stem_toggle_melody` | deck | button | Press to turn the deck's melody (vocals, instruments and bass together) off or on (all are on after a load). The rhythm pad covers kicks and hats, the melody pad covers vocals, instruments and bass. Works whatever `[stems] eq_mode` is, once the deck has its stems; a press before then is remembered and applies when they are ready. Needs `[stems] enabled = true`. No LED feedback. |
| `deck.stem_toggle_hats` | deck | button | Press to turn the deck's hats (the high part of the rhythm) off or on (all are on after a load). The rhythm pad covers kicks and hats, the melody pad covers vocals, instruments and bass. Works whatever `[stems] eq_mode` is, once the deck has its stems; a press before then is remembered and applies when they are ready. Needs `[stems] enabled = true`. No LED feedback. |
| `sampler.play_1` | none | button | Press to play sampler slot 1 from the start, restarting it if it is playing. An empty slot does nothing. |
| `sampler.play_2` | none | button | Press to play sampler slot 2 from the start, restarting it if it is playing. An empty slot does nothing. |
| `sampler.play_3` | none | button | Press to play sampler slot 3 from the start, restarting it if it is playing. An empty slot does nothing. |
| `sampler.play_4` | none | button | Press to play sampler slot 4 from the start, restarting it if it is playing. An empty slot does nothing. |
| `sampler.play_5` | none | button | Press to play sampler slot 5 from the start, restarting it if it is playing. An empty slot does nothing. |
| `sampler.play_6` | none | button | Press to play sampler slot 6 from the start, restarting it if it is playing. An empty slot does nothing. |
| `sampler.play_7` | none | button | Press to play sampler slot 7 from the start, restarting it if it is playing. An empty slot does nothing. |
| `sampler.play_8` | none | button | Press to play sampler slot 8 from the start, restarting it if it is playing. An empty slot does nothing. |
| `sampler.stop_1` | none | button | Press to stop sampler slot 1. |
| `sampler.stop_2` | none | button | Press to stop sampler slot 2. |
| `sampler.stop_3` | none | button | Press to stop sampler slot 3. |
| `sampler.stop_4` | none | button | Press to stop sampler slot 4. |
| `sampler.stop_5` | none | button | Press to stop sampler slot 5. |
| `sampler.stop_6` | none | button | Press to stop sampler slot 6. |
| `sampler.stop_7` | none | button | Press to stop sampler slot 7. |
| `sampler.stop_8` | none | button | Press to stop sampler slot 8. |

Relative steps that arrive together (a fast turn of an encoder) are added up and applied as one
move; a button press or release in between keeps the steps on each side of it apart.

Only "absolute" actions can be used in `[[init]]`. `soft_takeover` has an effect only on absolute bindings, and `invert` flips absolute values (or negates relative steps).
Button actions ignore releases, except `deck.cue`, `deck.hotcue_1` to `deck.hotcue_8`, `deck.loop_roll_1` to `deck.loop_roll_8`, `deck.slice_1` to `deck.slice_8`, `deck.jog_touch`, `deck.jog_release` and `modifier.shift`, and `deck.return_to_start` and `deck.hotcue_delete_1` to `deck.hotcue_delete_8`, whose release ends a CUE or pad hold of the same deck (`deck.return_to_start` a CUE preview, `deck.hotcue_delete_N` a preview of pad N). A controller that sends a different note with SHIFT held sends the release there when SHIFT goes down during the hold, so binding the shifted note to these actions keeps that hold from sticking. A roll or slice pad can be bound to the shifted note as well as the plain one: a release on either ends the held roll or slice of the same kind on the deck that pad press went to.

Jog wheel actions turn the steps a wheel sends into platter turns using the `jog_steps_per_turn` key of `[controller]`, which jog bindings (`deck.jog_top`, `deck.jog_ring`, `deck.jog_search`, `deck.jog_scratch`) require: a mapping that binds one without it fails to load.

The `deck.grid_*` actions edit the loaded track's beatgrid in the library. Every grid edit
except `deck.grid_lock_toggle` also locks the grid, so analyzing the track again never undoes
a manual correction. The BPM stays between 40 and 300. Shifting, setting the downbeat,
halving and doubling all need a BPM, so analyze the track (or tap or set a BPM) first.

The player does not perform grid edits yet: a binding to a `deck.grid_*` action is accepted
but does nothing until the library-mode player handles them. The built-in DDJ-SB3 mapping
does not bind the grid actions.

## State ids for LEDs

| Id | Target | Lit when |
|---|---|---|
| `deck.playing` | deck or side | The deck is loaded and playing. |
| `deck.play_led` | deck or side | Lit while playing, blinking while paused with a track loaded, off with no track. The usual PLAY button behavior. |
| `deck.cue_led` | deck or side | Lit while previewing, or while paused at the cue point. Blinking while paused elsewhere. Off while playing, or with no track. |
| `channel.pfl` | deck or side | The channel is on the headphone cue. |
| `side.alt_deck` | side | The side shows its alternate deck (3 or 4). |
| `side.shift` | side | SHIFT of that side is held. |
| `master.headphone_master` | none | The headphones hear the master rather than the cue (headphone mix at 0.5 or above). |

A `side` target on a deck state follows the deck the side shows, so one LED always shows the
state of the visible deck.

## Pad modes

A controller whose pads cannot be switched from the software (or whose firmware modes cannot
be changed) keeps each side's *pad mode* in the app. Bind the mode buttons to `pads.mode_*`
(they take a `side`), and give the pad bindings a `pad_mode` so only the side's current mode
applies. The DDJ-SB3 does it like this (shortened to the left side; pad notes on channel 8):

```toml
[[input]]
midi = { channel = 1, note = 0x69 }      # SHIFT + HOT CUE
action = "pads.mode_beat_jump"
side = "left"

[[input]]
midi = { channel = 1, note = 0x1B }      # HOT CUE
action = "pads.mode_hardware"
side = "left"

[[input]]
midi = { channel = 8, note = 0x00 }
action = "deck.hotcue_1"
side = "left"
pad_mode = "hardware"

[[input]]
midi = { channel = 8, note = 0x01 }
action = "deck.beat_jump_2"
side = "left"
pad_mode = "beat_jump"

# The sampler is global; side only scopes pad_mode.
[[input]]
midi = { channel = 8, note = 0x30 }
action = "sampler.play_1"
side = "left"
pad_mode = "hardware"
```

A held pad (`deck.loop_roll_N`, `deck.slice_N`) should also be bound on the shifted note of
the same pad, so a release that arrives after SHIFT went down still ends it.

## Layers and deck switching

Many controllers have a four-deck layout with two sets of controls. There are two ways a
controller can do that, and the mapping depends on which one it is:

- **Hardware layering**: the controller sends different messages for deck 1 and deck 3
  (for example on different MIDI channels). Bind each to its own `deck`.
- **Software layering**: the controller sends the same messages for decks 1 and 3, and the
  app decides. Bind with `side = "left"` or `side = "right"` and a `side.deck_toggle` button.
  Each side shows its main deck (1 or 2) at start. Every toggle press swaps it with
  the alternate deck (3 or 4). The DDJ-SB3 works this way.

Use `shift` and `active_deck` when a binding should only fire in one layer.
