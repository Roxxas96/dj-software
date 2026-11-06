# Sound and pad modes: design

Date: 2026-11-06
Status: draft, awaiting the owner's approval
Extends: [the main design](2026-10-30-dj-software-design.md) (sections "Engine features (v1)" and "DDJ-SB3 specifics"). Everything there still holds; this document settles what it left open for the sound features and the SB3's pad modes.

## Intent

Open Jockey's decks, mixer, library, hotcues, loops and sync already work on the owner's DDJ-SB3 (plans 1 to 4). What is left for the owner to play a full set without VirtualDJ is the "sound" layer:

- the sampler,
- beat jump,
- the SB3's other pad modes,
- the FX section,
- keylock.

**How the owner plays (from the conversation):**
- In a set they use the FILTER knobs, loops, EQ, the tempo range, hotcues, beat jump and the sampler. The filter is the only effect they use for transitions, and it already works.
- They use the sampler for quick transition sounds. These are sound files they collected themselves and put on the pads through VirtualDJ's library.
- A sample pad plays its sound from start to end. SHIFT + pad stops it.
- Beat jump runs from the pads, with jump sizes they can configure.

**Owner's decision:** keylock, the FX section and the firmware pad modes (FX FADE, PAD SCRATCH, TRANS) are built too, even though the owner rarely uses them, "in case I need them".

## Goals

1. **Sampler:** sounds the owner chooses are put on the SAMPLER pads through a config file. A pad plays its sound once, start to end, and SHIFT + pad stops it.
2. **Beat jump** on the SHIFT + HOT CUE pad mode, with sizes from the config file. Defaults (the owner's):
   - pads 1/2 jump −4 and +4 beats, pads 3/4 jump −8 and +8, pads 5/6 jump −16 and +16, pads 7/8 jump −32 and +32;
   - SHIFT + pads give the less common sizes: −1/+1, −2/+2, −4/+4 and −64/+64 beats on pad pairs 1/2, 3/4, 5/6 and 7/8.
3. **The SB3's other app-driven pad modes, as VirtualDJ does them:**
   - **Loop roll** (SHIFT + FX FADE): a momentary loop of 1/16, 1/8, 1/4, 1/2, 1, 2, 4 or 8 beats on pads 1-8. Releasing the pad resumes playback where the track would be if the roll had never happened (slip).
   - **Slicer** (SHIFT + PAD SCRATCH): the 8 pads are 8 consecutive beats of the grid. Pressing a pad repeats that beat while held, and releasing resumes as if it was never pressed (slip).
4. **The SB3's firmware pad modes** (FX FADE, PAD SCRATCH, TRANS): the SB3 itself runs them and sends MIDI that emulates other controls. For example, a filter sweep, fader cuts, jog moves or a backspin. The app responds to that MIDI so these modes sound as they do in VirtualDJ. What the firmware sends is not known yet, so it is captured from the hardware first (see "Capture").
5. **The FX section:**
   - Each side's FX 1/2/3 buttons switch an effect on or off on the deck that side shows.
   - The side's FX LEVEL knob sets how much effect is heard (dry/wet).
   - The default effects are echo, reverb and flanger, synced to the beat where it applies (the echo time and flanger rate follow the deck's BPM). The effect in each slot is set in the config file. A filter effect also exists, per the main spec's list.
6. **Keylock:**
   - KEY LOCK toggles it per deck. While it is on, tempo changes do not change the pitch.
   - It is off by default, as in VirtualDJ.
   - SHIFT + KEY LOCK keeps its plan-2 function (next tempo range).
7. **Slip mode:**
   - SHIFT + VINYL toggles slip per deck, as in VirtualDJ and Mixxx.
   - While slip is on, scratching, loops, hotcue jumps and reverse resume where the track would have been. Loop roll and slicer always slip.
8. **A global config file** `config.toml`, in the app's config folder next to the controller mappings (see the mapping reference for the per-OS paths). It holds:
   - the sample on each sampler pad;
   - the beat jump and loop roll sizes;
   - the effect in each FX slot;
   - the sampler volume;
   - **an option to disable the crossfader** (owner's request). When it is disabled, every channel ignores the crossfader and plays at full level, whatever its assignment and the crossfader's position.

## Non-goals (for this work)

- **No UI.** Everything is configured through `config.toml` and the mapping until the UI plan.
- **No sample editing,** no sample banks beyond one set of 8, and no sampler recording.
- **No new effects beyond the main spec's list** (echo, reverb, flanger, filter), and no effect chains or parameters other than on/off and level.
- **LEDs stay as they are.** The LED handshake is still unknown. The owner accepted no LEDs for now.

## Anti-goals

These are the main spec's, unchanged:

- no freeze, crash or audio dropout;
- a control never does something other than its mapping says, including after deck switching;
- cues never disappear.

This work adds four more:

- **A sample that does not play when its pad is hit,** with no message saying why (missing file, unreadable file). This counts as a failure.
- **A firmware pad mode that leaves the deck changed after the pad is released.** If the deck is left filtered, faded, looping or stopped when the SB3's own routine has ended, that is a failure. The owner would hear it on the master mid-set.
- **Keylock that audibly smears or drops out** at normal DJ tempo ranges (±16 %).
- **A config file mistake that stops the app at a gig without saying what is wrong.** The app checks the file at startup and names the key and the problem, in the same way mapping errors are reported.

## Decided

**By the owner:**
- Sampler sounds come from their own files, set in `config.toml`.
- A sample plays start to end; SHIFT + pad stops it.
- Beat jump runs on pads, with the sizes above, configurable.
- Keylock, the FX section and the firmware pad modes are all in scope.
- A config option disables the crossfader.

**My calls** (the owner approved the summary):
- **Sampler pads:** one set of 8 sounds, shared by both sides (VirtualDJ's default bank).
  - Each pad is one sample, given as a file path or a library track id.
  - Pressing a pad again while it plays restarts it.
  - Samples play through the master at their own speed, not synced to any deck. The headphones hear them as part of the master.
  - The sampler volume comes from the config file, default 1.0.
  - Samples are decoded and held in memory when the app starts. A sample that fails to load is reported at startup, and its pad does nothing.
- **Effects in VirtualDJ's slot order:** the defaults are echo, reverb and flanger. Each slot applies to the deck its side shows.
- **The FX LEVEL knob** is one dry/wet amount for the side's active effects.
- **The firmware modes and the loop roll and slicer sizes follow VirtualDJ's SB3 behavior.** VirtualDJ's SB3 manual describes these modes (the pads page of its DDJ-SB3 hardware manual). Where VirtualDJ is silent, they follow Mixxx's SB3 manual (for example, the loop roll pad sizes).

## Capture first

Before the pad-mode work, the owner records what the SB3 sends, using the capture tool. The capture adds new checklist steps to the existing fixture, using the tool's resume behavior. Left deck 1 and right deck 2 are enough, because deck 3/4 layering is done in software. The steps:

1. **Pads in each SHIFT pad mode:** SHIFT + HOT CUE, SHIFT + FX FADE, SHIFT + PAD SCRATCH and SHIFT + SAMPLER, then tap the 8 pads.
2. **SHIFT + pads** in SAMPLER mode and in the beat jump mode.
3. **The firmware modes while a track plays:**
   - FX FADE pads 1-8;
   - PAD SCRATCH pads 1-8, plus SHIFT + pads 1-5;
   - TRANS pads 1-8.

   Each pad is held and released. Every message the SB3 sends during and after the pad is recorded raw, with timestamps. The firmware generates the messages itself; earlier captures treated them as noise.

   A track must be playing in Open Jockey on the SB3's sound card. The SB3 may only run these routines while its audio interface is in use, and decks 3/4 need it too.
4. **KEY LOCK** and **SHIFT + VINYL** are already in the fixture (plan 2). They are only checked.

The capture tells the builder whether the firmware modes reuse the normal controls' messages or send messages of their own. The normal controls are the filter CC, the channel fader CC, the jog CCs and the loop notes. The firmware-mode bindings are written from the capture, never from guesses.

## Constraints

- **Real-time rules unchanged.** The audio thread never allocates, locks or blocks:
  - samples are decoded off the audio thread;
  - effect buffers (echo delay lines, reverb state) are allocated before use;
  - the keylock time-stretcher is built off the audio thread with its maximum buffer sizes;
  - the `no_alloc` test covers every new render path.
- **License: Apache-2.0, no GPL.**
  - Keylock needs a permissive time-stretch library. Signalsmith Stretch is MIT, per the main spec.
  - Effects are written in-house or taken from permissive crates.
  - `cargo deny` enforces both.
- **Cross-platform.** Builds and tests run on macOS, Windows and Linux. CI has no audio or MIDI devices.
- **Mapping and config errors** are reported at startup with the file, the key and the problem.

## Settled after the builder check (my calls)

- **The capture is plan 5's first step.** Engine work (config, sampler, beat jump, roll, slicer, slip) can start without it. The bindings for the SHIFT pad modes and SHIFT + pads are written only once the capture exists: their note numbers are not known yet.
- **Firmware modes:**
  - The plan that implements them decides from the capture how the app treats their messages. Messages that copy a normal control (filter, fader, jog) may get their own bindings or skip soft takeover. When the routine ends, the deck must be back where the physical controls put it (the anti-goal).
  - If the SB3 sends nothing usable for a mode, the app implements that mode in software, as VirtualDJ's own pads would. The owner wants them available.
- **Slicer:**
  - The 8 pads are the 8-beat block of the grid that contains the playhead. Blocks are counted from the grid's first beat.
  - The block moves on as playback crosses into the next one, unless a pad is held.
- **Beat grid required:** beat jump, loop roll and slicer do nothing on a deck without a beatgrid.
- **Slip:**
  - Slip covers scratching, loops, hotcue jumps, loop roll and slicer. The app has no reverse control, so reverse is out.
  - The slip position advances at the deck's tempo, including any nudge.
  - Loop roll and slicer on a deck that is already looping act on the playback position, not the loop.
  - On a paused deck they do nothing.
- **Samples:**
  - They are decoded and resampled to the output rate when the app starts.
  - A sample longer than 60 s is refused at startup with a message naming it.
  - Samples join the master bus, so the headphones hear them through the headphone cue/master mix like the rest of the master.
- **FX routing:**
  - Effects sit per deck, after the EQ and filter and before the channel fader, so the headphone cue hears them.
  - An effect switched on stays with the deck it was switched on for. The FX buttons always act on the deck their side shows, and switching layers never moves an effect to another deck.
  - Several effects can be on at once on one deck. The side's LEVEL knob sets the dry/wet for the effects on the deck that side shows.
- **Crossfader disable** affects only the decks' master mix. It is read at startup.
- **Config errors:**
  - A missing `config.toml` means defaults.
  - A value that is invalid (unknown key, unknown effect, bad size) stops startup with the file, key and problem, as a bad mapping does.
  - A sample file that cannot be read is reported and its pad stays silent, but the app starts.

## Left to the builder

- The time-stretch library and how keylock is wired into the deck renderer, provided:
  - quality at ±16 % is clean on the owner's music;
  - latency is low enough that the jog, cue and hotcues still feel instant.
- The effect algorithms and their parameters.
- How slip state is tracked in the engine.
- The `config.toml` schema details, documented in a config reference page next to the mapping reference.
- **The plan split.**
  - My suggestion is two plans:
    - **Plan 5:** config file, sampler, beat jump, loop roll, slicer, slip, crossfader disable and the shifted-mode bindings. This covers what the owner uses.
    - **Plan 6:** keylock, the FX section and the firmware pad modes. This is what they asked for "in case".
  - The UI, interop and release plans move to 7, 8 and 9.
