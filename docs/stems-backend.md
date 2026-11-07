# Stems: separation backend (decision record)

Date: 2026-11-07. Plan 8, Task 1. Measured on the owner's MacBook: **Apple M1 Pro (10 cores), 32 GB, macOS 27.0.1**.

This records which model, runtime and cache format the `stems` crate uses, what each candidate measured,
and the numbers later tasks rely on (chunking, model I/O, cache layout). The trial programs lived in a
scratch folder and are not part of the repository; the steps to rebuild the model file are at the end.

## Open item: the weights license (needs the owner's ruling)

The spec requires a permissive license for the code **and the weights**. The Demucs **code** is MIT, but the
**weights' license is not stated as MIT** anywhere official:

- [facebookresearch/demucs README](https://github.com/facebookresearch/demucs#license) (archived Meta repo):
  "Demucs is released under the MIT license as found in the LICENSE file". It says nothing about the weights.
- [adefossez/demucs README](https://github.com/adefossez/demucs#license) (the maintained fork, by the author;
  demucs 4.1.0 on PyPI loads weights from here) says: *"The license of the weights is not clear: MusDB
  contains some data that were only usable for research purpose. I sadly cannot help granting a license...
  The 6 source variant of htdemucs was trained only on Meta internal dataset, which likely means CC-NC-BY
  license is applicable"*.
- The Hugging Face weights repository [adefossez/HTDemucs](https://huggingface.co/adefossez/HTDemucs) has no
  license field in its model card.

So `htdemucs` weights cannot be called "permissively licensed" with confidence. The same is true of the other
open models checked: Open-Unmix `umxl` weights are CC BY-NC-SA 4.0
([README](https://github.com/sigsep/open-unmix-pytorch#-pre-trained-models)); `umxhq` is trained on MUSDB18-HQ
(the same research-only data); the Spleeter README licenses only its code
([README](https://github.com/deezer/spleeter#license)). No candidate has weights that are clearly permissive
**and** separate well.

**Technical decision below assumes Demucs `htdemucs`**, as the spec expects. Nothing in the code depends on
it beyond the model file and its I/O shape, so if the owner rules against these weights, another model
exported to ONNX would replace only the model file and the `Separator`'s pre/post-processing.

Also because of this, **the app never bundles the weights**: `open-jockey stems setup` downloads them once, at
the user's explicit request (as the spec already says), and the README should say where they come from and
that their license is unclear.

## Results

Test input: a synthetic 5-minute 44.1 kHz stereo WAV (kick, noise hats, square bass, vibrato tone, chords,
noise). It measures speed, memory and numerical agreement only; separation quality on real music is judged
with the owner later. "Agreement" is the SNR of each output against the reference PyTorch `demucs`
`apply_model` (same model, `shifts=0`, `overlap=0.25`): above ~60 dB means the same output.

| Candidate | Accel. | Separation, 5 min | Peak RSS | Agreement with PyTorch | Notes |
|---|---|---|---|---|---|
| **htdemucs via ONNX Runtime (`ort`), CoreML EP, compute units ALL** | GPU + Neural Engine + CPU | **22.4 s** (+13.0 s first model compile; **2.3 s** with the CoreML model cache) | 3.3 GB | 90-126 dB | **chosen** |
| same, CoreML CPUAndGPU | GPU | 24.7 s (+8.6 s compile) | 3.3 GB | 90-126 dB | |
| same, CoreML CPUAndNeuralEngine | Neural Engine | 39.6 s (+9.2 s compile) | 4.3 GB | 76-105 dB | |
| same, CPU EP only (what Windows/Linux get) | none | 68.9 s (load 0.2 s) | 5.8 GB | 61-87 dB | |
| demucs.cpp (C++/Eigen + Accelerate BLAS), 8 threads | none | 178 s | 18.6 GB | 13-20 dB (−4 dB vocals) | its threaded driver splits the track its own way; not chased, too slow anyway |
| candle (pure Rust, Metal) | — | not measured | — | — | no Demucs port exists for candle; porting HTDemucs (~1500 lines of PyTorch, transformer + two U-Nets) is a project of its own |
| reference: PyTorch 2.15 `demucs` 4.1.0, MPS | GPU | 25.1 s | 1.7 GB | — | Python; not shippable, reference only |
| reference: PyTorch, CPU | none | 81.4 s | 2.6 GB | — | |

Per 7.8 s chunk (network only): CoreML ALL 0.395 s, CPUAndGPU 0.439 s, CPUAndNeuralEngine 0.726 s, CPU 1.29 s. A 5-minute
track is 52 chunks. The STFT/iSTFT in Rust (`realfft`, one thread) costs 1.8 s per track in total.

Model output order (from the model's own metadata, `sources`): **0 drums, 1 bass, 2 other, 3 vocals**.
Mapped per the spec: drums → rhythm, bass → bass, other (+ residual) → instruments, vocals → vocals.

| Candidate | License, code | License, weights | Builds on macOS / Windows / Linux |
|---|---|---|---|
| Demucs + ONNX Runtime + `ort` | Demucs MIT ([LICENSE](https://github.com/adefossez/demucs/blob/main/LICENSE)); ONNX Runtime MIT ([LICENSE](https://github.com/microsoft/onnxruntime/blob/main/LICENSE)); `ort`/`ort-sys` MIT OR Apache-2.0 ([LICENSE-MIT](https://github.com/pykeio/ort/blob/main/LICENSE-MIT)) | **unclear** (see above) | yes: prebuilt ONNX Runtime 1.31.0 for `aarch64-apple-darwin` (+CoreML), `x86_64`/`aarch64-pc-windows-msvc` (+DirectML), `x86_64`/`aarch64-unknown-linux-gnu`. **No Intel-Mac (`x86_64-apple-darwin`) binary**: there stems would need a self-built ONNX Runtime (`ORT_LIB_PATH`), or stay unavailable |
| demucs.cpp | MIT ([LICENSE](https://github.com/sevagh/demucs.cpp/blob/main/LICENSE)); Eigen MPL-2.0 | same Demucs weights, **unclear** | yes in principle (C++17, header-only Eigen; built here with plain `clang++`, so `cc` would do) |
| candle | MIT/Apache-2.0 | — | — |

## Decision

- **Model:** Demucs v4 `htdemucs` (single model, 4 sources; signature `955717e8`), subject to the open item
  above. Not `htdemucs_ft` (a bag of 4 models, 4× slower) and not `htdemucs_6s`.
- **Runtime:** ONNX Runtime through **`ort` 2.0.0-rc.15**, with the network exported to ONNX **without the
  STFT/iSTFT**, which run in Rust. CoreML execution provider (MLProgram format, compute units ALL, model cache
  directory) on macOS; CPU elsewhere (DirectML on Windows is available but untested and not required).
- **Speed:** 22.4 s per 5-minute track plus a one-time ~13 s model compile (2.3 s once cached), inside the
  10-30 s goal. CPU-only platforms take about 70 s on this Mac's CPU.
- **Model file:** `htdemucs-core-v1.onnx`, **129,679,471 bytes**, SHA-256
  **`b03863005b79053959f8e0dd4f05749667b33fb9bcbedf4d8d9b47ec85d5d974`**. Reproducible: two independent
  exports gave the same hash (recipe below).
  - It is our own conversion, so there is no upstream URL for it. Proposed fixed address, **to be uploaded by
    the owner** as a release asset: `https://github.com/Roxxas96/open-jockey/releases/download/stems-model-v1/htdemucs-core-v1.onnx`.
    `stems setup` downloads that URL and checks the SHA-256 above.
  - Source weights it was built from: `https://huggingface.co/adefossez/HTDemucs/resolve/main/955717e8.safetensors`
    (84,025,440 bytes, SHA-256 `d9fa14133cfcc034a6758923bb3a8ca9f8dfd0b582134643bbf83f72c17576dd`). The
    same weights as Meta's original `https://dl.fbaipublicfiles.com/demucs/hybrid_transformer/955717e8-8726e21a.th`
    (SHA-256 `8726e21a993978c7ba086d3872e7608d7d5bfca646ca4aca459ffda844faa8b4`; a pickle, not used).
- **Cache format:** **Ogg Vorbis, VBR targeting 192 kbit/s, one stereo file per stored stem** (vocals, bass,
  rhythm), encoded with `vorbis_rs` and decoded with the workspace's existing `symphonia` (`vorbis` + `ogg`
  features, already enabled in `crates/decode`). Measured, 5-minute track:

  | Format | 3 stems | Encode (one thread) | Decode | Error energy of the 3 stored stems vs the mix |
  |---|---|---|---|---|
  | **Vorbis VBR 192k** | **20.7 MB** | 7.4 s | 0.7 s | vocals −59.5 dB, bass −36.0 dB, rhythm −23.3 dB |
  | Vorbis quality 0.5 | 16.7 MB | 7.4 s | 0.7 s | −54.6 / −33.8 / −17.8 dB |
  | FLAC 16-bit (`flacenc`) | 62.0 MB | 0.5 s | not read: `symphonia` 0.6.1 fails to probe `flacenc` 0.5.1 output ("unexpected end of file"); libsndfile reads it | ≈ −86 dB |
  | FLAC 12-bit (`flacenc`) | 31.2 MB | 0.5 s | same probe failure | — |

  Instruments is derived at load as `mix − (vocals + bass + rhythm)`, so the four always sum to the mix
  exactly. **Caveat for the owner's listening check:** the codec error of the three stored stems lands in
  instruments. With every knob in the middle it cancels exactly, but when a stem is turned off its codec error
  stays audible inside instruments (e.g. with rhythm off you hear `mix − rhythm'`, which keeps
  `rhythm − rhythm'`). On the synthetic test that residue is −23 dB below the mix for the noise-like hats,
  which is a worst case (a perceptual codec does not keep a noise waveform); on real music it should be far
  lower, but it must be checked by ear with the owner. If it is audible, the fallback is FLAC 16-bit (about
  60 MB per 5 minutes, so the 5 GB budget holds ~80 tracks instead of ~240), which needs the probe issue above
  fixed or a different FLAC encoder.
  The three stems can be encoded in parallel (estimated ~2.5 s wall time, not measured).

## Crates to add

| Crate | Version | License | Where | Why |
|---|---|---|---|---|
| `ort` | `=2.0.0-rc.15` (pin: release candidate) | MIT OR Apache-2.0 | `stems` | ONNX Runtime binding; default features (download-binaries, tls-native, api-31) |
| `ort`, macOS only | same, `features = ["coreml"]` | | `[target.'cfg(target_os = "macos")'.dependencies]` | CoreML EP; selects the `+coreml` prebuilt |
| `ort`, Windows only | same, `features = ["directml"]` | | `[target.'cfg(windows)'.dependencies]` | the only Windows prebuilts are `+directml`/`+openvino`; without a feature the build fails to pick one |
| `realfft` | `3.5.1` | MIT OR Apache-2.0 | `stems` | real FFT on top of the workspace's `rustfft` 6.4.1 |
| `vorbis_rs` | `0.5.6` | BSD-3-Clause (bundles aoTuV libvorbis and libogg, BSD-3-Clause, built with `cc`) | `stems` or `library` cache | Vorbis encoder |
| `symphonia` | `0.6` (workspace), `vorbis` + `ogg` | MPL-2.0 | already present | Vorbis decoder |

`ort`'s build script downloads ONNX Runtime from `cdn.pyke.io` at **build** time (hash-checked against the
table in `ort-sys`), links it statically, and caches it under `~/Library/Caches/ort.pyke.io` (per-OS cache
dir). That is build-time network, not run-time; offline builds can set `ORT_LIB_PATH` to a local ONNX Runtime.

### `cargo deny check licenses`

Run with the project's `deny.toml` on a scratch crate holding exactly the rows above (all targets):
**fails** on one crate, then **passes** with a scoped exception:

```
error[rejected]: failed to satisfy license requirements
   ├ CDLA-Permissive-2.0 - Community Data License Agreement Permissive 2.0
   ├ webpki-root-certs v1.0.11
     └── ureq v3.5.1
         └── (build) ort-sys v2.0.0-rc.15
```

`webpki-root-certs` is Mozilla's CA root list as data, used only by `ort-sys`'s build script to fetch the
ONNX Runtime binaries over HTTPS; nothing of it ends up in the app. Every TLS option of `ort` pulls it (or
`webpki-roots`, same license). Add to `deny.toml` under `[licenses]`:

```toml
exceptions = [
  { allow = ["CDLA-Permissive-2.0"], crate = "webpki-root-certs" },
]
```

With it: `advisories ok, bans ok, licenses ok, sources ok` (warnings only: duplicate `syn`, `windows-sys`).

## Model I/O, chunking and overlap

The ONNX graph is the HTDemucs network between the spectrogram and the output, with static shapes:

| Name | Shape (f32) | Meaning |
|---|---|---|
| input `mix` | `[1, 2, 343980]` | one chunk of the normalized stereo signal |
| input `mag` | `[1, 4, 2048, 336]` | its spectrogram, complex-as-channels: L re, L im, R re, R im |
| output `spec_out` | `[1, 4, 4, 2048, 336]` | per source, the separated spectrogram (same channel layout) |
| output `time_out` | `[1, 4, 2, 343980]` | per source, the time-branch output |

A source's chunk output is `time_out + iSTFT(spec_out)`.

- **Chunk:** 343,980 samples (7.8 s, the model's training segment; the graph only accepts this length).
- **Overlap:** 0.25, so the stride is 257,985 samples (5.85 s); chunk *k* starts at `k × 257985`.
- **Cross-fade:** triangular weight over the chunk, `w[i] = (i+1)/171990` rising for the first 171,990
  samples, then falling from 1 to 1/171990; overlapping outputs are summed weighted and divided by the sum of
  weights. The last (short) chunk is cut from a full-length window centered on it (zeros past the track's
  end) and its output trimmed back by the same offset (demucs' `TensorChunk.padded` + `center_trim`).
- **Normalization:** over the whole track, `x = (x − mean) / std` with mean and (unbiased) std of the mono
  mix; outputs are mapped back with `y × std + mean`. The network normalizes each chunk internally too.
- **STFT** (demucs `_spec`, per channel): reflect-pad by 1536 on the left and `1536 + 336·1024 − 343980` on the
  right, then a centered STFT (reflect-pad 2048 both sides), n_fft 4096, hop 1024, periodic Hann window, scaled
  by `1/√4096`; keep bins 0..2048 (drop Nyquist) and frames 2..338.
- **iSTFT** (demucs `_ispec`): add a zero Nyquist bin and 2 zero frames each side, inverse real FFT scaled by
  `1/√4096`, Hann window, overlap-add with hop 1024, divide by the summed squared window, drop 2048 (center)
  + 1536 (pad) samples, keep 343,980.
- **Around the playhead first:** chunks are independent, so the separator runs chunk indices in the order
  `p, p+1, p−1, p+2, p−2, ...` with `p = playhead / 257985`. A region is final once both chunks overlapping it
  are done.
- Per-chunk working memory is ~70 MB of tensors; the process peak (3.3 GB with CoreML) is mostly ONNX
  Runtime and CoreML, not the audio.

### `ort` gotchas found in the trial

- With `api-22`+ (the default `api-31`), `SessionBuilder::new` sets the **automatic device policy
  `MaxEfficiency`**, which on this Mac silently routes a "CPU" session to CoreML NeuralNetwork on the Neural Engine:
  6.9 s per chunk, 15 GB RSS, 20 s load. The CPU path must call
  `.with_auto_device(AutoDevicePolicy::PreferCPU)`; the CoreML path registers the EP explicitly.
- CoreML: `ModelFormat::MLProgram`, `ComputeUnits::All`, `with_static_input_shapes(true)`, and
  `with_model_cache_dir(<data dir>/stems/coreml-cache)` (438 MB; turns the 13 s compile into 2.3 s). The
  CoreML EP leaves 25 of 1,414 nodes on the CPU (26 partitions): the time-branch 1-D convolutions exceed
  CoreML's 16,384-sample limit. The output still matches PyTorch.
- The exported graph must be simplified (`onnxslim`, constant folding): un-simplified, the CoreML compile
  takes 62 s and a chunk 0.6 s.

## How `Separator` wraps it

```rust
pub struct OnnxSeparator {
    session: ort::session::Session, // CoreML EP on macOS, CPU (PreferCPU) elsewhere
    dsp: Stft,                      // realfft plans, Hann window, scratch buffers
}

impl Separator for OnnxSeparator {
    /// `mix` is the decoded track (stereo f32, 44.1 kHz; other rates are resampled first).
    fn separate(&mut self, mix: &Track, start_frame: usize, cancel: &AtomicBool,
                progress: &mut dyn FnMut(f32)) -> Result<StemSet, StemsError> {
        let (mean, std) = mono_mean_std(mix);
        let chunks = chunk_order(mix.frames(), start_frame);   // outward from the playhead
        let mut acc = [[vec![0f32; n]; 2]; 4];                 // drums, bass, other, vocals
        let mut wsum = vec![0f32; n];
        for (done, k) in chunks.iter().enumerate() {
            if cancel.load(Relaxed) { return Err(StemsError::Cancelled); }
            let chunk = window(mix, k * STRIDE, SEG, mean, std); // zero-padded, centered if short
            let mag = self.dsp.spec(&chunk);                      // [4, 2048, 336]
            let out = self.session.run(ort::inputs!["mix" => chunk, "mag" => mag])?;
            let sources = add_istft(out["time_out"], out["spec_out"], &self.dsp);
            overlap_add(&mut acc, &mut wsum, &sources, k);     // triangular weights
            progress((done + 1) as f32 / chunks.len() as f32);
        }
        let [drums, bass, other, vocals] = finish(acc, wsum, mean, std);
        // vocals, bass, rhythm stored; instruments = mix − (vocals + bass + rhythm)
        Ok(StemSet::from_three(mix, vocals, bass, drums))
    }
}
```

The session is built once per process (the CoreML compile is the slow part) and runs on the low-priority
separation thread; `ort` sessions are `Send`. Tests use the synthetic linear-split separator behind the same
trait, so CI never needs the model or ONNX Runtime's accelerators.

## Cache layout

One directory per entry, keyed by library track id + audio fingerprint:
`<data dir>/stems/<track id>-<fingerprint>/` holding `vocals.ogg`, `bass.ogg`, `rhythm.ogg` and a small
`stems.toml` (format version, model SHA-256, frames, sample rate). It is written to a temporary directory and
renamed into place, so a half-written entry is never seen; a missing or unreadable file means "not cached".
Size and time per 5-minute track: 20.7 MB, encode 7.4 s on one thread (about a third of that if the three stems are encoded in parallel; not measured), decode 0.7 s.

## Rebuilding the model file

In a scratch virtualenv (Python 3.14.9): `pip install torch==2.15.0 demucs==4.1.0 onnx==1.23.2
onnxscript==0.7.2 onnxslim==0.1.99 safetensors==0.8.0 numpy==2.5.4`, then run each script with `python -I`.

1. Download `955717e8.safetensors` from the Hugging Face URL above and check its SHA-256. (Safetensors, not
   the `.th` pickle; loaded with `demucs.hf.load_safetensors_model`, whose metadata names the class
   `demucs.htdemucs.HTDemucs`.)
2. Export (`torch.onnx.export`, `dynamo=False`, opset 17, after
   `torch.backends.mha.set_fastpath_enabled(False)`; the fast path's `_native_multi_head_attention` has no ONNX
   export): a wrapper module that copies `HTDemucs.forward` from the normalization of `mag`/`mix` down to
   `x * std + mean` / `xt * stdt + meant`, including the `channel_upsampler`/`downsampler` around the
   cross-transformer, and returns `(x.view(1,4,4,2048,336), xt.view(1,4,2,343980))`. Its full output
   (with demucs' own `_mask` + `_ispec`) matches `model(mix)` exactly (max difference 0.0).
3. `onnxslim htdemucs.onnx htdemucs_slim.onnx`.
4. Store every float32 initializer of at least 64 values that is exactly representable in fp16 as an fp16
   initializer followed by a `Cast` to float (the original weights were saved in fp16, so this is lossless;
   constants folded in step 3 stay fp32). Result: 129,679,471 bytes, the SHA-256 above, same outputs.
