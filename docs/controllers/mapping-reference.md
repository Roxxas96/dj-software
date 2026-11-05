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

Copy a built-in `mapping.toml` to the user location and edit it to change a mapping without
rebuilding the app. The file is read when the app starts.

## Errors

A mapping that does not load stops the app with a message in the form
`<file>: <binding>: <problem>`, for example `input[3]: unknown action `deck.plya``.
`<binding>` is the kind of binding and its position in the file counting from 0
(`init[0]`, `input[12]`, `output[1]`).

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
[controller]          # exactly one
[[init]]              # any number: values set when the controller attaches
[[input]]             # any number: controls
[[output]]            # any number: LEDs
```

### `[controller]`

| Key | Meaning |
|---|---|
| `name` | Display name. |
| `port` | Text matched against the names of the MIDI input and output ports. Any port whose name contains it is used. |
| `jog_steps_per_turn` | Optional, 1 to 10000. How many relative steps the jog wheel sends for one full turn of the platter. Required whenever a binding uses `deck.jog_top`, `deck.jog_ring` or `deck.jog_search`: a mapping with such a binding and no key fails to load. |

`name` and `port` are required.

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

#### Value kinds

| `value` | Reads the message as | Works with |
|---|---|---|
| `button` | Pressed when note on, or a control change above 0. Released when note off, or a control change of 0. | note, cc |
| `absolute` | A position from 0 to 1 (0-127 is divided by 127; 14-bit values by 16383). | note, cc, cc14, pitchbend |
| `relative_twos` | Steps as two's complement: 1 to 63 forward, 64 to 127 backward (64 is -64). | cc |
| `relative_offset` | Steps offset by 64: 65 is +1, 63 is -1. | cc |

If you leave `value` out, notes are `button` and everything else is `absolute`.

The action decides which kinds it accepts (the "Takes" column below). The relative actions are
`browser.scroll`, `deck.jog_top`, `deck.jog_ring` and `deck.jog_search`; a relative binding to
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
| `deck.jog_top` | deck | relative | The top of the jog wheel. While touched in vinyl mode it scratches: the playhead follows the platter, and a held record is silent. Otherwise it nudges a playing deck (a turn bends the speed, up to 50 %, and the bend fades within about 0.1 s of the hand stopping), or moves the playhead of a paused one (one platter turn is 1.8 s of track). |
| `deck.jog_ring` | deck | relative | The outer ring of the jog wheel. It nudges a playing deck (a speed bend that fades in about 0.1 s) and moves a paused one's playhead, like the top without vinyl mode, and is ignored while the deck is scratching. |
| `deck.jog_search` | deck | relative | Search: moves the playhead 10 s of track per platter turn, leaving a loop it lands outside of. |
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
| `browser.toggle_focus` | none | button | Press to switch the browser between the tree (playlists, folders, all tracks) and the track list. |

Relative steps that arrive together (a fast turn of an encoder) are added up and applied as one
move; a button press or release in between keeps the steps on each side of it apart.

Only "absolute" actions can be used in `[[init]]`. `soft_takeover` has an effect only on absolute bindings, and `invert` flips absolute values (or negates relative steps).
Button actions ignore releases, except `deck.cue`, `deck.hotcue_1` to `deck.hotcue_8`, `deck.jog_touch`, `deck.jog_release` and `modifier.shift`, and `deck.return_to_start` and `deck.hotcue_delete_1` to `deck.hotcue_delete_8`, whose release ends a CUE or pad hold of the same deck (`deck.return_to_start` a CUE preview, `deck.hotcue_delete_N` a preview of pad N). A controller that sends a different note with SHIFT held sends the release there when SHIFT goes down during the hold, so binding the shifted note to these actions keeps that hold from sticking.

Jog wheel actions turn the steps a wheel sends into platter turns using the `jog_steps_per_turn` key of `[controller]`, which jog bindings (`deck.jog_top`, `deck.jog_ring`, `deck.jog_search`) require: a mapping that binds one without it fails to load.

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
