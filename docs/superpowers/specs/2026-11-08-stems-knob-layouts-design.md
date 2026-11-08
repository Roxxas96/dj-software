# Stems: knob layouts and extra pads (addendum)

Date: 2026-11-08
Status: approved by the owner (2026-11-08)
Amends: [the stems design](2026-11-08-stems-design.md). Where this document and that one disagree, this one wins.

## Intent

**Why (owner, after the plan 8 hands-on):** everything in plan 8 works, but in VirtualDJ the owner mixes by cutting one track's kicks, or its voice and melody, and letting the other track fill that space. VirtualDJ's stem knobs cover the whole track:
- LOW is the kicks;
- MID is the vocals, the melody and the bassline;
- HI is the hi-hats.

With all three knobs fully left, nothing is heard. Plan 8's knobs (HI = vocals, MID = instruments, LOW = bass, with rhythm on no knob) cannot do that. The owner also changed their mind about the right half of the knob: turning a knob right should **raise that part's level**, not silence the others.

The model's raw stems stay useful. With eight pads, four switch the raw stems and three more switch the new knob parts.

## Goals

1. **Two knob layouts, chosen in `config.toml`:**
   - `[stems] knobs = "vdj"` is the default:
     - LOW = **kicks**;
     - MID = **melody** (vocals + instruments + bass);
     - HI = **hats**.

     All three fully left is silence.
   - `knobs = "raw"`: HI = vocals, MID = instruments, LOW = bass, as in plan 8.
     - Rhythm has no knob, so all three fully left still leaves the drums.
   - Later the UI can set it too. The other `[stems]` keys are unchanged.
2. **The knob law** is the same for both layouts and replaces plan 8's (goal 3 of the stems design, and the "Settled" knob law):
   - **Centre:** the part plays at its normal level.
   - **Left half:** fades it to silent at fully left, with the same curve as the normal EQ's cut (`eq_gain`: squared law, a true kill at 0).
   - **Right half:** raises it, up to **+6 dB** fully right, the same as the normal EQ's boost (`eq_gain`).
   - A knob only ever changes its own part. There is no isolation term.
   - Gains still ramp over one block, so turning a knob never clicks.
3. **Kicks and hats come from the rhythm stem** (Demucs' drums), split by frequency:
   - kicks = the low end of rhythm;
   - hats = the rest of rhythm (snares and other percussion land there too).

   The model has no kick/hi-hat split, so this is a filter split, not a separation. It is an approximation, and the docs say so.
4. **Seven stem pad actions**, still bound to no SB3 pad by default:
   - **Raw stems:** `deck.stem_toggle_vocals`, `deck.stem_toggle_instruments`, `deck.stem_toggle_bass`, `deck.stem_toggle_rhythm` (unchanged).
   - **New:** `deck.stem_toggle_kicks`, `deck.stem_toggle_melody`, `deck.stem_toggle_hats`.
   - The pads work the same with either knob layout.
   - **When two pads cover the same sound, it plays only if both are on.** The gains multiply. For example:
     - rhythm off silences kicks and hats too;
     - kicks off removes only the low end of rhythm;
     - melody off silences vocals, instruments and bass.
   - Flags reset to all on at each load, and a press before stems are ready is remembered, as in plan 8.
5. **Mapping example:**
   - The docs example in `docs/controllers/mapping-reference.md` puts the raw four on SAMPLER pads 1–4 and kicks, melody and hats on pads 5–7, on both sides.
   - Pad 8 keeps playing sample 8 (inherited).

## Non-goals

- No model-based kick/hi-hat separation.
- No eighth stem action (owner: "nothing").
- No change to separation, the cache format, `stems setup`, the status line or mapping `extends`.

## Anti-goals

The stems design's anti-goals stand. In particular:
- **Centred knobs = exactly the track**, in both layouts, with every pad on.
  - The kicks/hats split must therefore sum back to rhythm exactly: no audible phasing, and no level change against the other stems at the changeover.
  - Plan 8's tests for this (`centered_knobs_changeover_is_inaudible`, `changeover_has_no_click`) must pass in both layouts.
- **No click and no dropout:**
  - when a knob moves;
  - at the EQ↔stems changeover;
  - when a pad toggles.
- **No audio-thread allocation.** Every new path is covered by `no_alloc.rs`.
- **Scratching, keylock, backspin, slip, loops, rolls and beat jump** still work on the stem mix. The source reader reads any position in any direction. The split must give the same result there, not a filter state that breaks on a jump or in reverse.

## Constraints

- **Boost headroom.** A part raised by +6 dB can push the deck above 0 dBFS, as the normal EQ boost already can. The mixer handles it the way it handles an EQ boost today; no new limiter.
- **Memory:**
  - If the split is precomputed (see below), the in-memory `StemSet` grows by one 16-bit stereo stem.
  - Update the memory estimate in `docs/stems-backend.md`. The 20-minute limit stays, unless the estimate no longer fits; if so, say so.
- **Keylock:** the stems-to-EQ changeover alignment (ruling R8) still holds.
- **Config errors** use `<file>: <key>: <problem>`. An unknown `knobs` value is an error.
- **Docs and tests change with the behaviour:**
  - the README stems section, `docs/config.md`, the mapping reference and the SB3 README;
  - the plan 8 knob-law tests (`knob_law_matches_the_spec` becomes a table for both layouts).

## Decided

**By the owner:**
- The VDJ layout: LOW = kicks, MID = vocals + melody + bassline, HI = hats; all three left = silence.
- Turning a knob right raises its part, up to +6 dB like the normal EQ, instead of silencing the others.
- The pads keep the four raw stems, plus kicks, melody and hats; nothing on an eighth action.
- The knob layout is configurable in `config.toml`, with the raw layout still available.

**My calls (the owner may still change them):**
- `vdj` is the default layout.
- Both layouts use the same law.
- The left-half curve is the normal EQ's squared law, not plan 8's linear fade, so the knobs feel like the EQ.
- Overlapping pads multiply.
- The docs example uses pads 1–7 and keeps sample 8 on pad 8.
- The work lands as plan 8 tasks 9 onwards, on the same branch and PR #10.

## Left to the builder

- **How the kick/hat split is made, within the anti-goals.** The suggested way is to precompute the split off the audio thread when the stem set is built:
  - Kicks: a zero-phase low pass of rhythm (forward–backward), stored as a fourth i16 stem.
  - Hats: rhythm minus kicks, computed per frame like instruments.

  This sums exactly, has no filter state on the audio thread and works under scratch and jumps. The precomputed kicks need not be cached; recomputing them is cheap.
- **The split frequency.** Start from the normal EQ's 250 Hz low crossover, and adjust if a listening check on the owner's tracks says otherwise. Record the choice.
- **The config key's name and values,** if `knobs = "vdj" | "raw"` reads badly next to `eq_mode`.
- **The plan split.** Suggested:
  - the engine (law, layouts, split, three new gains);
  - the actions and config;
  - the docs;
  - the owner's hands-on.
