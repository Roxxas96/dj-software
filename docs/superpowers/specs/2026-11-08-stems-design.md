# Stems: design

Date: 2026-11-08
Status: approved by the owner (2026-11-08)
Extends: [the main design](2026-10-30-dj-software-design.md), which listed "stems separation" as a non-goal for v1. The owner has now asked for it, so this document replaces that non-goal.

## Intent

**Why (owner):** in VirtualDJ the owner uses stems on every transition, more than the normal EQ. The EQ knobs control vocals, instruments and bass instead of frequency bands, and pads switch stems on and off. Open Jockey should do the same, configurable for other users.

**Prior art:** VirtualDJ's Stems 2.0.
- It separates on the user's computer (an M1 Mac or an NVIDIA GPU makes it real-time), and nothing is sent online.
- It falls back to a faster, lower-quality method for tracks that are not ready.
- It suggests preparing tracks or loading them a little early.

Open Jockey works the same way, with an open-source model.

## Goals

1. **Four stems per track:** vocals, instruments, bass and rhythm. They are separated on the user's computer by an open-source model; nothing is ever sent over the network.
2. **Stems mode for the EQ knobs, chosen in `config.toml`** (later also in the UI). In stems mode:
   - HI controls vocals, MID instruments and LOW bass.
   - Rhythm has no knob; it is controlled by the pads and by the other knobs' isolation (goal 3).
3. **Knob law** (owner-confirmed):
   - Each knob position `p` runs from 0 to 1, with 0.5 as the middle.
   - **Own level:** `own = min(1, 2p)`. The left half fades the knob's stem from normal (0.5) to silent (0).
   - **Isolation:** `iso = max(0, 2p − 1)`. The right half fades the stems no knob is isolating. Fully right on one knob is the same as turning the other two knobs fully left, with rhythm also silenced.
   - **Every knob turned right keeps its own stem.** With HI and LOW fully right you hear vocals + bass, with instruments and rhythm off.
   - **Formula:** let `I = max_k iso_k` over the three knobs.
     - A knob stem `s` with `iso_s > 0` gets `own_s` (= 1).
     - A knob stem with `iso_s = 0` gets `own_s × (1 − I)`.
     - Rhythm gets `1 − I`.
     - Each is multiplied by the stem's pad on/off (goal 4).
   - Gains ramp over one block, so turning a knob never clicks.
4. **Stem pads:** four actions that switch a stem on or off on the deck: vocals, instruments, bass, rhythm. They are bound to no SB3 pad mode by default.
   - The owner assigns them in their own mapping, for example in place of the sampler.
   - To make that easy, a user mapping file can **extend** a built-in one and list only its changes (goal 7).
5. **Separation on load:**
   - When a track loads and its stems are not cached, separation starts in the background, working outward from the playhead's region, so the part about to be played is ready first.
   - The track plays normally meanwhile.
   - In stems mode the knobs act as the normal EQ until the deck's stems are ready. The changeover from EQ to stems is crossfaded, so nothing jumps.
   - Target: about 10-30 s for a whole track on an Apple-silicon MacBook, using its GPU or Neural Engine.
6. **Stem cache:**
   - Separated stems are saved, compressed, in the library's data folder, so reloading a track gives its stems at once.
   - The cache has a size budget in `config.toml`, default 5 GB.
   - When the budget is exceeded, the stems least likely to be played are evicted first. The order is:
     1. tracks in no playlist and not prepared (no cue or hotcues) and not played recently;
     2. then, within each group, the least recently played or loaded first.
   - Prepared tracks, tracks in playlists and recently played tracks are kept the longest.
   - A command prepares stems ahead of time: `library stems --playlist "<name>"` or `--track ID...`.
   - Evicted stems are simply separated again on the next load.
7. **Extending mappings:**
   - A mapping file may say `extends = "ddj-sb3"`. It then inherits every binding of that built-in mapping.
   - Its own `[[input]]` bindings are added. A binding of the same MIDI message, the same `pad_mode` and the same `shift` *replaces* the inherited ones for that message.
   - It may also remove inherited bindings with `[[remove]] midi = {…}`.
   - This is how the owner puts stems on the SAMPLER pads without copying the whole SB3 file.

## Non-goals

- **No kick/hi-hat split.** The open models give drums as one stem (VirtualDJ splits kick and hi-hat with its own model).
- **No live switch** between EQ and stems mode while mixing: it is a config setting (later a UI setting).
- **No stem effects** and no per-stem filter.
- **No network:** no cloud separation and no model download at run time beyond what the build ships. How the model file is shipped is left to the builder (constraints below).

## Anti-goals

The main spec's anti-goals stand: no freeze, crash or dropout; a control does only what its mapping says; cues never disappear. Stems add these:

- **Separation must never cause an audio dropout or a freeze.** It runs off the audio thread at lower priority. While a deck is playing it may slow down, as the analyzer does, but playback always wins.
- **Switching a deck from EQ to stems must not change what is heard.** With the knobs at their middle, the summed stems must sound like the track itself (no level jump and no audible phasing at the changeover).
- **A stem cache problem must never block playing a track.** Missing, corrupt, half-written or evicted stems mean the knobs stay EQ until fresh stems are ready.
- **No audio leaves the computer.**

## Constraints

- **License Apache-2.0; no GPL.**
  - The model must be permissively licensed (code and weights). Demucs (MIT, Meta) is the expected choice; its 4-stem "htdemucs" output maps drums → rhythm, bass → bass, other → instruments, vocals → vocals.
  - The inference runtime must be permissive too, for example ONNX Runtime (MIT) through a permissive Rust binding, or a pure-Rust runtime.
  - `cargo deny` enforces both.
- **Platforms:** macOS, Windows and Linux build and run.
  - Hardware acceleration is required on Apple silicon (CoreML or Metal), because the owner plays on a MacBook.
  - Other platforms may use the CPU (slower, but still in the background).
  - CI has no GPU: tests use the CPU path or a tiny test model.
- **Real-time rules unchanged.**
  - The audio thread mixes four stem buffers per deck instead of one track buffer, with no allocation.
  - Stem buffers arrive like tracks, through the engine's command queue and garbage queue, swapped in only when complete (or region by region, if the builder streams them in; see below).
- **Memory:** the four stems of a track held in memory cost about 4× the track's decoded size. A 2-hour mix's stems may not fit; the builder sets a length limit beyond which stems are refused, with a status message.
- **Config errors** are reported as `<file>: <key>: <problem>` (plan 5 rules).

## Decided

**By the owner:**
- Four stems: vocals, instruments, bass and rhythm.
- HI = vocals, MID = instruments, LOW = bass.
- Fully right on a knob = all other stems off.
- Pads switch stems on and off.
- No pad mode gets stems by default; the user assigns them in configuration.
- Stems mode is set in `config.toml` (later in the UI).
- Separation runs locally, with no API.
- Stems are cached so a reload is instant.
- The cache has a budget in config, default 5 GB, keeping the tracks most likely to be played.
- Until stems are ready, the knobs act as the normal EQ.

**My calls:**
- The exact knob formula (goal 3).
- Separation starts around the playhead.
- The eviction order and the `library stems` prepare command.
- Mapping `extends` and `[[remove]]`.
- Demucs as the model.
- The EQ-to-stems crossfade.

## Settled after the builder check

**By the owner:**
- The knob law above, where every knob turned right keeps its own stem.
- **The model is never redistributed by Open Jockey** (owner, 2026-11-08). The repository ships only a weight-free model graph. `open-jockey stems setup` downloads the Demucs author's official weights from their source, fills the graph locally and verifies it. The docs say the weights' license is unclear (the author trained them partly on research-only data) and that each user fetches them from the author.
- **A one-time model download:** `open-jockey stems setup` downloads the model from a fixed address and verifies its checksum. After that, everything runs offline and no audio ever leaves the computer. The app never downloads anything by itself; without the model, stems are simply unavailable, with a message saying how to set them up.

**My calls:**
- **Signal path:**
  - Stems mode *replaces* the EQ stage for a deck whose stems are ready. Trim, filter and FX stay as they are, before and after it.
  - Until the stems are ready, or with stems mode off, the existing EQ runs.
  - The changeover is a one-block crossfade between the EQ path and the stem path.
- **Exact sum:** whatever the model leaves unexplained (the mix minus the four separated stems) is added to *instruments*. The four stems therefore sum to the original exactly, and with every knob in the middle stems mode sounds like the track itself (anti-goal).
- **Model output:** Demucs' drums go to rhythm, bass to bass, other plus the residual to instruments, and vocals to vocals.
- **Config:**
  - `[stems] enabled = false` by default. Separation and the cache run only when enabled, so users who don't want stems pay nothing.
  - `eq_mode = "eq" | "stems"`.
  - `cache_gb = 5`.
  - The owner sets `enabled = true` and `eq_mode = "stems"`.
  - The stem pads work whenever stems are enabled, whatever the knob mode.
- **Pads:**
  - Actions: `deck.stem_toggle_vocals`, `deck.stem_toggle_instruments`, `deck.stem_toggle_bass` and `deck.stem_toggle_rhythm`.
  - The on/off flags are per deck and reset to all on when a track loads.
  - A press before the stems are ready is remembered and applies once they are.
  - No LED feedback (the SB3's pad LEDs don't respond).
- **Cache identity:** the cache is keyed by the library track id plus its audio fingerprint, so a relinked file keeps its stems and changed audio redoes them. Tracks loaded outside the library (`play` without `--library`) get no stems.
- **Eviction inputs:**
  - A new `last_loaded_at` column on tracks, set at each load (library migration).
  - "Recently played" means loaded in the last 30 days. "Prepared" means a cue point above 0 or any hotcue.
  - Stems prepared with `library stems` count as prepared.
  - The budget is enforced after each new separation, but stems held by a loaded deck are never evicted.
- **Memory:**
  - In memory, stems are stored as 16-bit samples, three stems plus the mix (the fourth stem is the mix minus the other three), so about 1.75× the track's decoded size.
  - Stems are refused for tracks longer than 20 minutes (DJ mixes), and the deck's status line says so.
  - The status line also shows `stems: 40 %` while separating and `stems` when they are active.
- **Speed:**
  - The 10-30 s target is a goal, not a gate. Because stems are cached and can be prepared ahead, a slower result still ships.
  - The plan starts with a spike that picks the model variant, runtime and Apple-silicon acceleration, and measures them on the owner's MacBook.
  - No lower-quality fallback model.
- **Deck features:** the stem mix replaces the single-buffer read in one place, so keylock, scratching, backspin, slip, loops, rolls and beat jump work on the summed stems unchanged. The sampler is not stemmed.
- **Mapping `extends`:**
  - Inherited bindings for other messages carry over unchanged, and `[[init]]` and `[[output]]` are inherited.
  - User mappings live where the mapping reference says (the config folder's `controllers/<name>/mapping.toml`).
  - An unknown parent or a cycle is a load error naming the file.
- **Tests:** CI uses a tiny synthetic model (a fixed linear split) behind the same interface. Real-model quality is checked by hand with the owner.

## Left to the builder

- The model variant and its format:
  - which Demucs variant and quantization;
  - converted to ONNX or CoreML, or run through a Rust runtime.
- The runtime and its acceleration on each OS.
- The cache format (compressed, for example FLAC or Opus; the quality must keep the summed stems close to the original).
- Chunking, overlap and the "around the playhead first" order.
- Whether partial stems stream into the engine region by region, or the deck switches when the whole track is done.
- The plan split (my guess: the engine stem mixing, the knob law and the pads; then separation and the cache; then mapping `extends`). This work comes before the UI plan, as the owner asked.
