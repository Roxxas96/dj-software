# Adding a controller

This is the process for adding a MIDI DJ controller, written for someone who has not worked
with MIDI before. You need the controller, a computer, and the app built from source (see the
[README](../../README.md#building-from-source)). The DDJ-SB3 files in
[`controllers/ddj-sb3/`](../../controllers/ddj-sb3/) are the worked example. There is no in-app
MIDI learn yet; it comes with the UI.

## A few MIDI words

- A controller sends **messages** when you touch a control. The ones that matter here are
  **note on / note off** (buttons and pads), **control change**, or **CC** (knobs and faders),
  and **pitch bend** (some sliders and jog wheels).
- Each message has a **channel** from 1 to 16 and a **number** (the note or CC number). A
  control is identified by message type, channel and number. Channels are written 1-based in
  the app and in mapping files.
- Some controls send a 14-bit value split over two CCs (a **cc14**) for finer steps.
- LEDs work the other way round: the app sends a message and the controller lights up.

## 1. Plug it in and look

```sh
cargo run --release -p dj-software -- midi ports
cargo run --release -p dj-software -- midi monitor --port "<part of the port name>"
```

`midi ports` lists MIDI inputs and outputs. `midi monitor` prints every message until
Ctrl-C (the port defaults to the first one containing "DDJ"). Move a control and watch what it
sends. Note the port name; it goes into the mapping's `[controller] port`.

## 2. Write a checklist

The checklist lists the physical controls so the capture tool can ask for them one at a time.
Start from [`controllers/ddj-sb3/checklist.toml`](../../controllers/ddj-sb3/checklist.toml) and
copy it to `controllers/<name>/checklist.toml`. Its header explains the keys. In short:

- Steps are grouped in sections, and a section can repeat per deck.
- A step can also be captured with SHIFT held.
- A step can ask you to prepare something before it listens.

Take the list of controls from the controller's manual or its manufacturer's MIDI message
list. Take **only the list of controls** from there, not the MIDI values: those come from your
own capture. Do not copy code or mappings from other projects with incompatible licenses
(for example GPL ones); this project is Apache-2.0.

## 3. Capture every control

```sh
cargo run --release -p dj-software -- midi capture \
  --checklist controllers/<name>/checklist.toml \
  --out controllers/<name>/capture.toml
```

Each step is shown and starts listening straight away: do what it says (move the knob, press
the button, turn the jog wheel). Then check the summary. Keys, followed by Enter:

| Key | Does |
|---|---|
| Enter | Finish the step now while it is listening. After the summary, keep what was recorded. |
| `r` | Record the step again. |
| `x` | Mark what was just recorded as noise (see below). |
| `s` | Skip this step. |
| `g <step>` | Go to a step by its id. |
| `u <status> <data1>` | Stop ignoring a source, for example `u B5 02` (hex). |
| `q` | Save and quit. |

The file is saved after every step. If you quit or the command stops, run the same command
again to continue. To capture only some steps again, pass their ids with `--redo`:

```sh
... midi capture --checklist ... --out ... --redo deck1.play --redo deck1.cue
```

`--redo` needs the fixture to exist already and can be repeated. `--port` picks the port if the
default is wrong. After some steps the tool also checks the control's LED by sending the control's own note back to it at a few velocities, and asks if it lit.

### Noise and the `[ignore]` list

Some controllers send messages you did not cause, such as a periodic "alive" message or the
position of a knob when you press a button. These are noise: they would confuse the mapping.
Press `x` after a step that recorded noise, or answer yes when the tool says "These look like
noise". The source (a status byte and a first data byte) is added to the ignore list. To
ignore sources from the start, put them in the checklist:

```toml
[ignore]
sources = [{ status = 0xB5, data1 = 0x02 }]
```

Ignored sources are dropped while capturing and removed from loaded captures. The tool
tells you how to undo an ignore (`u <status> <data1>`). Recordings already stripped are not
restored; use `g <step>` to record them again. The DDJ-SB3 checklist ignores nothing: the
knob positions it sends along with some buttons are handled by the tool.

## 4. Decide the layering

Look at your capture. If deck 3 controls send different messages from deck 1 (typically on a
different channel), the controller has **hardware layering**: bind each deck separately with
`deck = N`. If they are byte-for-byte the same, it has **software layering** and the app has
to remember which deck each side shows: bind with `side = "left"` and `"right"` and add a
`side.deck_toggle` binding for the DECK buttons. The DDJ-SB3 is software layering; its mapping
starts with a comment that explains the evidence. See
[Layers and deck switching](mapping-reference.md#layers-and-deck-switching).

## 5. Find the LED messages

Messages that light LEDs are usually listed in the manufacturer's documentation, and
sometimes the same note numbers as the buttons work. When they are not known, search for
them:

```sh
cargo run --release -p dj-software -- midi led-scan --port "<name>"
```

The tool lights groups of messages and asks whether the LED you are watching lit up, narrowing
the group by binary search until it finds the message. It switches everything off again
afterwards, and appends what it finds to `controllers/ddj-sb3/led-scan.toml` by default
(change that with `--out`). Useful options: `--channels 1,2,5` (1-based), `--kind note|cc`,
`--value 127`, and `--try <STATUS> <DATA1> [<VALUE>]` to check a single message in hex. Some
controllers only light their LEDs after a handshake the app does not send yet. If no
message works, say so in the PR.

## 6. Write the mapping

Create `controllers/<name>/mapping.toml`. The full file format is in the
[mapping reference](mapping-reference.md). Use the captured numbers only. Start with
`[controller]`, then one `[[input]]` per control, then `[[output]]` for the LEDs. Add a comment
at the top with the layering decision and where the numbers came from. Try it live:

```sh
cargo run --release -p dj-software -- midi monitor --controller controllers/<name>/mapping.toml
```

With `--controller`, the monitor shows what each message does under the mapping (the port
comes from the mapping). Errors in the file name the binding that is wrong. To play with it:

```sh
cargo run --release -p dj-software -- play <track> --controller controllers/<name>/mapping.toml \
  --deck3 <track> --deck4 <track> --device "<output device>"
```

(`--cue-track` loads deck 2; `--deck3` and `--deck4` load decks 3 and 4. All load paused.)

To ship it with the app, add it to the `BUILTINS` list in
[`crates/controller/src/builtin.rs`](../../crates/controller/src/builtin.rs) so
`--controller <name>` works.

## 7. Add replay tests

Every binding should be tested against what the real hardware sent. A replay test loads your
`capture.toml`, feeds the recorded messages through the mapping and a real engine, and checks
what changed. Copy the structure of
[`crates/controller/tests/sb3_replay.rs`](../../crates/controller/tests/sb3_replay.rs). A
control that does what its mapping says, also after deck switching, is a hard rule of this
project. Run:

```sh
mise run test
mise run lint
```

## 8. Open a pull request

Open an issue with the "Controller support" issue form first if you want help or want to
tell us you are working on it; it asks for the model, your operating system, whether it
has a sound card, the manufacturer's MIDI list and the capture. Then open a PR against `main`
with:

- `controllers/<name>/` containing `mapping.toml`, `capture.toml`, `checklist.toml` and a short
  `README.md` (supported controls, layering, LED status, what is not mapped yet);
- the replay tests;
- a note about what you tested on the hardware.

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/); see
[CONTRIBUTING.md](../../CONTRIBUTING.md).
