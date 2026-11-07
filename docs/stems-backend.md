# Stems: separation backend (decision record)

Date: 2026-11-07 (revised the same day for ruling R2). Plan 8, Task 1. Measured on the owner's MacBook:
**Apple M1 Pro (8 performance + 2 efficiency cores), 32 GB, macOS 27.0.1**.

This records the model, runtime, distribution, cache format and the numbers later tasks rely on. The trial
programs lived in a scratch folder. Task 4 committed the export script and wrapper (below) to
`crates/stems/model/`.

## Model distribution (settled, ruling R2)

**Open Jockey never redistributes model weights.**

- The Demucs **code** is MIT ([LICENSE](https://github.com/adefossez/demucs/blob/main/LICENSE)). The
  **weights' license is unclear**: the author's README ([adefossez/demucs#license](https://github.com/adefossez/demucs#license))
  says *"The license of the weights is not clear: MusDB contains some data that were only usable for research
  purpose"*. The [Hugging Face model card](https://huggingface.co/adefossez/HTDemucs) has no license field.
  The user docs must say this, and say that the weights are fetched from their author.
- The repository ships a **weight-free ONNX graph** of the htdemucs network (`htdemucs-core-wf.onnx`). It is
  generated from the MIT-licensed code by the export below, and holds no weight and nothing computed from
  a weight.
- `open-jockey stems setup` (explicit and one-time; never at play time) does this:
  1. Downloads the author's weights from the **pinned URL**
     `https://huggingface.co/adefossez/HTDemucs/resolve/main/955717e8.safetensors`
     (84,025,440 bytes, SHA-256 `d9fa14133cfcc034a6758923bb3a8ca9f8dfd0b582134643bbf83f72c17576dd`), and
     refuses any other hash.
  2. Fills the graph in Rust (rule below) and writes the final model to the data folder.
  3. Verifies the result (see Verification). On macOS it also opens one CoreML session once, so that the slow
     first CoreML compile happens during setup rather than at the first track load.
  No Python is needed at setup.
- For reference only: Meta's original pickle of the same weights is
  `https://dl.fbaipublicfiles.com/demucs/hybrid_transformer/955717e8-8726e21a.th`
  (SHA-256 `8726e21a993978c7ba086d3872e7608d7d5bfca646ca4aca459ffda844faa8b4`). It is not used, because
  loading a pickle executes code.

## Results

Test input: a synthetic 5-minute 44.1 kHz stereo WAV (kick, noise hats, square bass, vibrato tone, chords,
noise). It measures speed, memory and numerical agreement only; separation quality on real music is judged
with the owner later. "Agreement" is the SNR of each output against PyTorch `demucs` 4.1.0 `apply_model`
(same weights, `shifts=0`, `overlap=0.25`). Above ~60 dB is the same output.

| Candidate | Accel. | Separation, 5 min | Peak RSS (trial) | Agreement | Notes |
|---|---|---|---|---|---|
| **htdemucs, R2 filled graph, ONNX Runtime via `ort`, CoreML EP (MLProgram, ALL)** | GPU + Neural Engine + CPU | **23.0 s** | 3.3 GB | 90-126 dB | **chosen**. First CoreML compile 62 s; 11 s once in the CoreML cache |
| same graph, CPU EP, 8 intra-op threads | none | ≈ 53 s (1.02 s/chunk × 52) | 5.1 GB | 61-87 dB | what Windows/Linux get |
| same graph, CPU EP, ORT default threads (10) | none | 69.0 s | 6.0 GB | 61-87 dB | |
| htdemucs folded by `onnxslim` after filling (first trial; **not R2-compatible**, see Name map) | CoreML ALL | 22.4 s | 3.3 GB | 90-126 dB | compile 13 s, cached 2.3 s |
| same, CoreML CPUAndGPU / CPUAndNeuralEngine | GPU / Neural Engine | 24.7 s / 39.6 s | 3.3 / 4.3 GB | ≥ 76 dB | |
| demucs.cpp (C++/Eigen + Accelerate BLAS), 8 threads | none | 178 s | 18.6 GB | 13-20 dB | rejected on speed; its threaded driver splits the track its own way |
| candle (pure Rust, Metal) | — | — | — | — | no Demucs port exists for candle; porting HTDemucs is a project of its own |
| reference: PyTorch 2.15, MPS / CPU | GPU / none | 25.1 s / 81.4 s | 1.7 / 2.6 GB | — | Python, reference only |

Per 7.8 s chunk (network only): CoreML ALL 0.40 s, CPU 8 threads 1.02 s, 4 threads 1.49 s, 2 threads 2.54 s.
A 5-minute track is 52 chunks; the STFT/iSTFT in Rust (`realfft`, one thread) adds 1.8 s per track.
20-minute track (206 chunks): 88.6 s on CoreML.

Model output order (from the model's own metadata, `sources`): **0 drums, 1 bass, 2 other, 3 vocals**,
mapped to rhythm, bass, instruments (other + residual) and vocals.

| Candidate | License, code | License, weights | Builds on macOS / Windows / Linux |
|---|---|---|---|
| Demucs + ONNX Runtime + `ort` | Demucs MIT; ONNX Runtime MIT ([LICENSE](https://github.com/microsoft/onnxruntime/blob/main/LICENSE)); `ort`/`ort-sys` MIT OR Apache-2.0 ([LICENSE-MIT](https://github.com/pykeio/ort/blob/main/LICENSE-MIT)) | unclear (fetched from the author by the user; R2) | yes. Prebuilt ONNX Runtime 1.31.0 exists for `aarch64-apple-darwin` (+CoreML), `x86_64`/`aarch64-pc-windows-msvc` (+DirectML) and `x86_64`/`aarch64-unknown-linux-gnu`. **No Intel-Mac binary**: there stems need a self-built ONNX Runtime (`ORT_LIB_PATH`) or stay unavailable |
| demucs.cpp | MIT ([LICENSE](https://github.com/sevagh/demucs.cpp/blob/main/LICENSE)); Eigen MPL-2.0 | same weights | yes in principle (C++17; built here with plain `clang++`) |

## Decision

- **Model:** Demucs v4 `htdemucs`, a single model with 4 sources, signature `955717e8`. Not `htdemucs_ft`
  (a bag of 4 models, 4× slower) and not `htdemucs_6s`.
- **Runtime:** ONNX Runtime through **`ort` 2.0.0-rc.15**. The network is exported **without the
  STFT/iSTFT**, which run in Rust. CoreML on macOS, CPU elsewhere.
- **Speed:** 23 s per 5-minute track on this Mac (goal 10-30 s); about 53 s on its CPU.
- **Cache:** Ogg Vorbis, VBR targeting 192 kbit/s (details under Cache).

## Weight-free graph and name map

### What the trial measured

After the first trial's `torch.onnx.export` (default constant folding) plus `onnxslim`, the graph's
initializers were compared value-by-value with the 533 safetensors tensors:

| Graph | Weight initializers equal to a safetensors tensor (as-is / reshaped / transposed) | Derived from weights | safetensors tensors with no 1:1 initializer |
|---|---|---|---|
| raw `torch.onnx.export` (folding on) | 284 / 108 / 26 | 9 (6.4 MB): attention `in_proj` slices, the frequency embedding expanded | 115 |
| after `onnxslim` (the first trial's model) | 256 / 108 / 5 | 49 (94.8 MB): LayerScale folded into `linear2` and `out_proj` weights and biases, `in_proj` q/kv slices, frequency embedding × 0.2 expanded to `[1,48,512,336]` | 164 |

So that model cannot be rebuilt from the safetensors file by copying. Its hash `b03863005b...` is
**obsolete**.

### The R2 graph: every weight maps 1:1

The export below avoids all derivation:
1. It runs `torch.onnx.export(..., do_constant_folding=False)`, so every parameter stays one initializer named
   after its `state_dict` key, and slices, transposes and scales stay as ops.
2. It turns each of the 533 parameters into a **graph input** named exactly as its safetensors key,
   declared `float16` with the safetensors shape, and followed by a node `Cast(to=FLOAT)` named
   `cast/<key>`.
3. It runs `onnxslim` with the weights still graph inputs. Slimming then folds shape arithmetic and fixed
   tables, and cannot touch a weight.

Result, `htdemucs-core-wf.onnx`:
- **8,984,989 bytes** (2.9 MB gzipped), SHA-256 **`8d0bf481a32ec762c24aa26a35b1cf45e18171b13c2c8c4f6c277cd101d41042`**.
- 535 inputs (`mix`, `mag` and 533 weights), 2,272 nodes, opset 17, IR 8.
- 89 embedded initializers, none of them weights: 74 int64 shape constants, plus 15 float32 fixed tables
  computed from hyperparameters only. The two large ones are the cross-transformer's sinusoidal positional
  embeddings, `[1,2688,512]` and `[1,1344,512]`, about 8.2 MB.
- **Proof that it holds no weight information:** exporting from deliberately altered weights
  (`w × 1.01 + 0.001` on every parameter) gives a **byte-identical** file with the same SHA-256. Exporting
  twice gives the same hash too.
- All 533 weights are stripped. Every safetensors tensor is `F16` (checked in the file header; the
  `__metadata__` names class `demucs.htdemucs.HTDemucs`, sources `["drums","bass","other","vocals"]`).

### The name map

The map is the identity, and it is declared in the graph itself. For each weight:

| Field | Value |
|---|---|
| graph name | a graph input whose name is a safetensors key (every input except `mix` and `mag`) |
| safetensors key | the same string |
| dtype | safetensors `F16` → initializer `FLOAT16` (data type 10), bytes copied unchanged (little-endian IEEE half) |
| layout | as-is: the graph input's shape equals the safetensors shape; no transpose, reshape or arithmetic |
| fp16 → fp32 | done by the graph's own node `cast/<key>`, already present |

**Filler rule** (what `stems setup` does), checked by the trial filler:
- For each graph input that is a safetensors key:
  - check that the dtype is `F16` on both sides and the shapes are equal;
  - append a `TensorProto { name: key, data_type: FLOAT16, dims: shape, raw_data: <the tensor's bytes> }`
    to `graph.initializer`;
  - remove the input.
- Fail unless all 533 tensors are used and only `mix` and `mag` remain as inputs.

No separate map file is needed. If one is wanted for review, the JSON form, one entry
`{"graph","safetensors","dtype":"F16","shape","layout":"as-is","cast_node"}` per weight, is 533 entries and
115 KB.

The trial filler in Rust (`prost` 0.14.4 with code generated from ONNX 1.23.2's `onnx.proto3`, plus
`safetensors` 0.8.0) produced the final model in **0.1 s with 0.29 GB RSS**: **92,951,153 bytes, SHA-256
`7e2f3095902c8c1fc2e86844d9ef3662b3f7e5add140167479807b0f666411df`**, identical on a second run. A Python
`onnx` filler gives different bytes for the same content (protobuf encoders differ), so the hash is tied to
`prost`'s encoding.

Its outputs match the earlier model: CoreML 23.0 s per 5-minute track, CPU 69.0 s, agreement 90-126 dB.
Only the CoreML first compile is slower with the unfolded graph: 62 s instead of 13 s, and 11 s instead of
2.3 s from the CoreML cache. That is why setup warms the cache, and why the app should create the session in
the background at startup when stems are enabled. Runtime per chunk is unchanged (0.40 s).

### Verification

1. Before filling: the safetensors SHA-256 (above).
2. After filling: the final-model SHA-256 `7e2f3095...` above, valid for `prost` 0.14.x with the generated
   `onnx.rs` committed by Task 4.
3. If 2 differs (for example after a `prost` upgrade), run the **test vector**. It is also the test for any
   CI-built filler (with the model fetched in a manual job, not in CI).
   - **Input:** `mix` `[1,2,343980]` from a 64-bit LCG in planar order (channel 0 samples 0..343979, then
     channel 1):
     - `s = s × 6364136223846793005 + 1442695040888963407` (wrapping u64), starting from
       `s = 0x853c49e6748fea9b`, one step per sample;
     - `value = (((s >> 40) as f64 / 16777216.0) × 2 − 1) as f32 × 0.5`;
     - first values `−0.49231964, 0.03094620, −0.34887993, 0.33613867`; sum −102.5226.
   - `mag` is the separator's own STFT of that `mix` (recipe under Chunking); reference sum 1.905753,
     sum of squares 43022.49. Checking these also tests the Rust STFT.
   - **Expected** (PyTorch, the exported wrapper):

     | Output | sum of squares | per source (drums, bass, other, vocals) |
     |---|---|---|
     | `spec_out` | 31239.885 | 15736.6285, 93.75309, 15151.5496, 257.95397 |
     | `time_out` | 8111.2863 | 216.81594, 32.594880, 7860.1186, 1.7569368 |

   - **Tolerance:** each per-source sum of squares within **2 × 10⁻³ relative**.
     - Measured on the Rust-filled model: CPU EP worst 2.5 × 10⁻⁴; CoreML 9.1 × 10⁻⁷.
     - Agreement with PyTorch on this vector: CPU 74.2 dB (spec) and 97.1 dB (time); CoreML 112.9 and
       127.9 dB.
     - A wrong or misplaced tensor moves these by orders of magnitude.
4. Also hashing the sorted initializer contents (name, data type, dims, raw bytes) is cheap. It catches a
   filler bug even when the protobuf bytes differ, but it repeats checks 1 + 2. The trial's value is
   `d78196725b7416dbed1c968719de81875a1b2cd5cef8d8cb364c56e00525dfc8`, computed over all 622
   initializers with numpy byte order.

### The export (committed by Task 4)

Task 4 committed these under `crates/stems/model/` together with `htdemucs-core-wf.onnx`:
- the wrapper `htdemucs_core.py`;
- the export script, with the Python package versions pinned;
- `onnx.proto3` from ONNX v1.23.2 (Apache-2.0, SHA-256
  `73b989cce008f191ba02b70712303d401eb1581768742b2f302f449dc725a5a9`).

Environment: Python 3.14.9 venv, `torch==2.15.0 demucs==4.1.0 onnx==1.23.2 onnxscript==0.7.2
onnxslim==0.1.99 safetensors==0.8.0 numpy==2.5.4`, scripts run with `python -I`.

The wrapper is `HTDemucs.forward` from demucs 4.1.0 `demucs/htdemucs.py`, lines 540-657: everything from
`mag` to `xt * stdt + meant`. It drops the STFT (`_spec`/`_magnitude`), the mask and the iSTFT, and the
final sum. Full-model check: the wrapper's outputs plus demucs' own `_mask` + `_ispec` equal `model(mix)`
(max difference 0.0).

```python
import torch

class Core(torch.nn.Module):
    """inputs: mix [1,2,343980], mag [1,4,2048,336]; outputs: spec_out [1,4,4,2048,336], time_out [1,4,2,343980]"""
    def __init__(s, m): super().__init__(); s.m = m
    def forward(s, mix, mag):
        m = s.m; x = mag; B, C, Fq, T = x.shape
        mean = x.mean(dim=(1, 2, 3), keepdim=True); std = x.std(dim=(1, 2, 3), keepdim=True)
        x = (x - mean) / (1e-5 + std)
        xt = mix; meant = xt.mean(dim=(1, 2), keepdim=True); stdt = xt.std(dim=(1, 2), keepdim=True)
        xt = (xt - meant) / (1e-5 + stdt)
        saved = []; saved_t = []; lengths = []; lengths_t = []
        for idx, encode in enumerate(m.encoder):
            lengths.append(x.shape[-1]); inject = None
            if idx < len(m.tencoder):
                lengths_t.append(xt.shape[-1]); tenc = m.tencoder[idx]; xt = tenc(xt)
                if not tenc.empty: saved_t.append(xt)
                else: inject = xt
            x = encode(x, inject)
            if idx == 0 and m.freq_emb is not None:
                frs = torch.arange(x.shape[-2]); emb = m.freq_emb(frs).t()[None, :, :, None].expand_as(x)
                x = x + m.freq_emb_scale * emb
            saved.append(x)
        if m.bottom_channels:
            b, c, f, t = x.shape; x = x.reshape(b, c, f * t); x = m.channel_upsampler(x)
            x = x.reshape(b, -1, f, t); xt = m.channel_upsampler_t(xt)
        x, xt = m.crosstransformer(x, xt)
        if m.bottom_channels:
            x = x.reshape(b, -1, f * t); x = m.channel_downsampler(x)
            x = x.reshape(b, -1, f, t); xt = m.channel_downsampler_t(xt)
        for idx, decode in enumerate(m.decoder):
            skip = saved.pop(-1); x, pre = decode(x, skip, lengths.pop(-1))
            offset = m.depth - len(m.tdecoder)
            if idx >= offset:
                tdec = m.tdecoder[idx - offset]; lt = lengths_t.pop(-1)
                if tdec.empty: pre = pre[:, :, 0]; xt, _ = tdec(pre, None, lt)
                else: xt, _ = tdec(xt, saved_t.pop(-1), lt)
        S = len(m.sources)
        x = x.view(B, S, -1, Fq, T) * std[:, None] + mean[:, None]
        xt = xt.view(B, S, -1, mix.shape[-1]) * stdt[:, None] + meant[:, None]
        return x, xt
```

Export script, in outline (the trial's `export_wf.py`):
- `torch.backends.mha.set_fastpath_enabled(False)`. The fast path's `_native_multi_head_attention` has no
  ONNX export.
- `torch.manual_seed(0)`.
- Load the weights with `demucs.hf.load_safetensors_model` from the safetensors file.
- `L = 343980`; `mix = torch.randn(1,2,L)`; `mag = m._magnitude(m._spec(mix))`.
- `torch.onnx.export(Core(m), (mix, mag), raw, input_names=["mix","mag"],
  output_names=["spec_out","time_out"], opset_version=17, dynamo=False, do_constant_folding=False)`.
- Move every initializer `m.<key>` with `<key>` in the `state_dict` (all 533) into a `float16` graph input
  `<key>`, plus a node `Cast(to=FLOAT, name="cast/<key>")` writing the old initializer name. Assert that none
  is missing.
- `onnxslim <pre> htdemucs-core-wf.onnx`.

## Crates to add

| Crate | Version | License | Where | Why |
|---|---|---|---|---|
| `ort` | `=2.0.0-rc.15` (pinned: a release candidate) | MIT OR Apache-2.0 | `stems` | ONNX Runtime binding; default features (download-binaries, tls-native, api-31) |
| `ort`, macOS only | same, `features = ["coreml"]` | | `[target.'cfg(target_os = "macos")'.dependencies]` | CoreML EP; selects the `+coreml` prebuilt |
| `ort`, Windows only | same, `features = ["directml"]` | | `[target.'cfg(windows)'.dependencies]` | the only Windows prebuilts are `+directml`/`+openvino`; without the feature no prebuilt matches |
| `realfft` | `3.5.1` | MIT | `stems` | real FFT on the workspace's `rustfft` 6.4.1 |
| `rubato` | `5.0.1` | MIT OR Apache-2.0 | `stems` | band-limited resampling native rate ↔ 44.1 kHz (the analysis crate's averaging resampler is too crude for audio that is played) |
| `safetensors` | `0.8.0` | Apache-2.0 | `stems` (setup) | reads the author's weights |
| `prost` | `0.14.4` | Apache-2.0 | `stems` (setup) | ONNX protobuf read/write |
| `prost-build` + `protox` (build, or one-time generation with the result committed) | `0.14.4` + `0.10.0` | Apache-2.0; MIT OR Apache-2.0 | `stems` | generate `onnx.rs` from `onnx.proto3` without a `protoc` binary |
| `sha2` | `0.11.1` | MIT OR Apache-2.0 | `stems` (setup) | download and model checks |
| `ureq` | `3.5.1`, `default-features = false, features = ["native-tls-no-default"]` | MIT OR Apache-2.0 | `stems` (setup) | the HTTPS download, using the OS trust store (`RootCerts::PlatformVerifier`), so no bundled root list at run time. Generic User-Agent `open-jockey-dev`, no personal data (ruling R3) |
| `vorbis_rs` | `0.5.6` | BSD-3-Clause (bundles aoTuV libvorbis and libogg, BSD-3-Clause, built with `cc`) | cache | Vorbis encoder |
| `symphonia` | `0.6` (workspace), `vorbis` + `ogg` | MPL-2.0 | already present | Vorbis decoder |

`ort`'s build script downloads ONNX Runtime from `cdn.pyke.io` at **build** time, checks its hash against
the table in `ort-sys`, links it statically, and caches it per user. That is build-time network, not run
time. Offline builds can set `ORT_LIB_PATH`.

### `cargo deny check`

Run on a scratch crate holding exactly the rows above (all targets), with the project's `deny.toml` plus the
R1 exception:

```toml
[licenses]
exceptions = [
  { allow = ["CDLA-Permissive-2.0"], crate = "webpki-root-certs" },
]
```

Result: **`advisories ok, bans ok, licenses ok, sources ok`**. The only warnings are duplicates (`syn`,
`hashbrown`, `foldhash`, `windows-sys`) and allowances that no crate uses.
- Without the exception the check fails only on `webpki-root-certs` (Mozilla's CA list as data), which is
  pulled in solely as a build dependency of `ort-sys`.
- The run-time `ureq` configuration above pulls neither it nor `webpki-roots`.

## Chunking, overlap and model I/O

| Name | Shape (f32) | Meaning |
|---|---|---|
| input `mix` | `[1, 2, 343980]` | one chunk of the normalized stereo signal at 44.1 kHz |
| input `mag` | `[1, 4, 2048, 336]` | its spectrogram, complex-as-channels: L re, L im, R re, R im |
| output `spec_out` | `[1, 4, 4, 2048, 336]` | per source, the separated spectrogram (same channel layout) |
| output `time_out` | `[1, 4, 2, 343980]` | per source, the time-branch output |

A source's chunk output is `time_out + iSTFT(spec_out)`.

- **Sample rate:** the model needs 44.1 kHz. Other tracks are resampled to 44.1 kHz for the model, and each
  separated stem is resampled back to the track's native rate and length.
- **Chunk:** 343,980 samples (7.8 s, the training segment; the graph only accepts this length).
- **Overlap:** 0.25. The stride is 257,985 samples (5.85 s); chunk *k* starts at `k × 257985`.
- **Cross-fade:** a triangular weight `w[i]`:
  - rising as `(i+1)/171990` over the first 171,990 samples, then falling from 1 to 1/171990;
  - overlapping outputs are summed weighted and divided by the sum of weights.
  - The last (short) chunk is cut from a full-length window centered on it, with zeros past the end, and its
    output is trimmed back by the same offset (demucs' `TensorChunk.padded` + `center_trim`).
- **Normalization:** `x = (x − mean) / std` over the whole track, using the mean and unbiased std of the
  mono mix. Outputs map back with `y × std + mean`.
- **STFT** (demucs `_spec`, per channel):
  - reflect-pad by 1536 on the left and `1536 + 336·1024 − 343980` on the right;
  - centered STFT (reflect-pad 2048 on both sides), n_fft 4096, hop 1024, periodic Hann window, scaled by
    `1/√4096`;
  - keep bins 0..2048 (drop Nyquist) and frames 2..338.
- **iSTFT** (demucs `_ispec`):
  - add a zero Nyquist bin and 2 zero frames on each side;
  - inverse real FFT scaled by `1/√4096`, Hann window, overlap-add with hop 1024, divide by the summed
    squared window;
  - drop 2048 (center) + 1536 (pad) samples and keep 343,980.
- **Around the playhead first:** chunks are independent, so they run in the order `p, p+1, p−1, p+2, ...`
  with `p = playhead / 257985`. A region is final once both chunks overlapping it are done.

### Memory, and the 20-minute limit

The trial program kept naive f32 accumulators for all four sources. At 20 minutes its peak RSS was
**6.76 GB**. The breakdown:
- ONNX Runtime with CoreML: about 2.2 GB, independent of track length;
- f32 accumulators: 1.69 GB;
- a 1.69 GB copy made when writing them out;
- the input twice: 0.85 GB.

The `Separator` instead **bounds memory with incremental finalization**:
- Each chunk's f32 output (11 MB per source pair) is overlap-added into a small set of pending chunk
  buffers.
- As soon as a region's two overlapping chunks are both done, it is normalized back, converted to the
  StemSet's 16-bit samples, and written into the final buffers. Its f32 data is then freed.
- Pending data stays at a few chunks (under ~100 MB), whatever the order.

Estimated peak for a 20-minute track (52.9 M frames):
- ONNX Runtime ~2.2 GB;
- the decoded mix as f32, 0.42 GB (already held by the deck);
- three i16 stems, 0.63 GB;
- pending chunks, < 0.1 GB;
- **≈ 3.4 GB in total**, of which 2.2 GB is fixed.

On CPU, ONNX Runtime's share is larger (5.1 GB peak for a 5-minute track with the default arena). Task 6
should drop the session when stems are disabled, and may drop it after a while idle.

### `ort` session settings

- **Automatic device policy.** With `api-22`+ (the default `api-31`), `SessionBuilder::new` sets
  `MaxEfficiency`. On this Mac that silently routes a "CPU" session to CoreML NeuralNetwork on the Neural
  Engine: 6.9 s per chunk, 15 GB RSS, 20 s load. So:
  - the CPU path calls `.with_auto_device(AutoDevicePolicy::PreferCPU)`;
  - the CoreML path registers the EP explicitly.
- **CPU path:**
  - `GraphOptimizationLevel::Level3`; the default memory pattern and arena (on).
  - `with_intra_threads(n)`: 8 threads (the performance cores) gave 1.02 s per chunk; ORT's default of 10
    gave 1.33 s; 4 gave 1.49 s.
  - Use the number of performance cores while no deck plays, and fewer (e.g. half) while one plays: the
    thread runs at low priority, but fewer threads leave room for the audio callback.
  - The 69 s figure above is the default-threads run.
- **CoreML path:**
  - `ModelFormat::MLProgram`, `ComputeUnits::All`, `with_static_input_shapes(true)`;
  - `with_model_cache_dir(<data dir>/stems/coreml-cache)`, about 0.44-0.64 GB. That cache lives **outside the
    `cache_gb` stem budget**, and is documented and removable.
  - The CoreML EP leaves 25 nodes on the CPU (26 partitions): the time-branch 1-D convolutions exceed
    CoreML's 16,384-sample limit. The outputs still match PyTorch.
  - **The slow first compile is cached per executable too** (measured in Task 4): with a warm
    `coreml-cache`, the same test binary opened the session in 14 s. A copy of that binary at another path
    took 54 s, and a different binary took 62 s. So `stems setup` warms the cache for the `open-jockey`
    binary that runs it. After an update that moves or replaces the binary, the first session compiles
    again, which is one more reason to open it in the background at startup.

## How `Separator` wraps it

As built in the `stems` crate (Task 4):

- `trait Separator: Send { fn separate(&mut self, track: &TrackBuffer, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<StemSet, SeparateError>; }`
  checks `cancel` before every chunk and reports progress after each.
- `ModelSeparator::new(model, &ModelOptions)` opens the session (CoreML on macOS, CPU with `PreferCPU`
  elsewhere, or with `cpu_only`). `start_at(frame)` sets the playhead that the next separation works
  outward from.
- `chunked.rs` holds the steps:
  - resample to 44.1 kHz (`rubato` FFT resampler, streamed in blocks; identity at 44.1 kHz);
  - normalize by the mono mean and std;
  - run the chunks in playhead order, each `time_out + iSTFT(spec_out)`;
  - overlap-add into pending chunk buffers, finalizing regions to 16-bit as soon as both of their chunks
    are done (at most 3 chunk buffers are held);
  - resample each stem back to the track's rate.
- **Mapping:** source 3 (vocals) → vocals, 1 → bass and 0 (drums) → rhythm; source 2 (other) is dropped.
  The `StemSet` is built with `StemSet::new(vocals, bass, rhythm, frames)` from the 16-bit stems, and
  instruments is the engine's residual.
- Tracks over 20 minutes return `SeparateError::TooLong`.
- `SyntheticSeparator` is a fixed band split behind the same trait, so CI never needs the model.

`ort` allows one ONNX Runtime environment per process, so the crate creates it once and shares it.

## Cache

**Format:** Ogg Vorbis, VBR targeting 192 kbit/s, one stereo file per stored stem (vocals, bass, rhythm). It
is encoded with `vorbis_rs` and decoded with `symphonia` (`gapless(true)`), **at the track's native sample
rate and frame count**, so it lines up sample for sample with the decoded mix the engine plays.

Measured, 5-minute track. "Error" is the energy of (stored stem − decoded stem), relative to the energy of the
whole mix, in dB:

| Format | 3 stems | Encode (one thread) | Decode | Error: vocals / bass / rhythm |
|---|---|---|---|---|
| **Vorbis VBR 192k** | **20.7 MB** | 7.4 s | 0.7 s | −59.5 / −36.0 / −23.3 dB |
| Vorbis quality 0.5 | 16.7 MB | 7.4 s | 0.7 s | −54.6 / −33.8 / −17.8 dB |
| FLAC 16-bit (`flacenc` 0.5.1) | 62.0 MB | 0.5 s | `symphonia` 0.6.1 fails to probe this output ("unexpected end of file"); libsndfile reads it | ≈ −86 dB |

**Alignment, measured:**
- Decoded Vorbis frame count = original (13,230,000) for every stem, at both settings.
- Best cross-correlation lag against the original = **0**: no encoder-delay offset, since `symphonia` applies
  the Ogg pre-skip and end trim.
- Measured at 44.1 kHz; Task 5 should add a 48 kHz round-trip test.

**Instruments** is derived at load as `mix − (vocals + bass + rhythm)`, so the four always sum to the mix
exactly. The codec error of the three stored stems lands in instruments:
- With every knob in the middle it cancels exactly.
- With a stem turned off, its codec error stays audible. For example, with rhythm off you hear
  `mix − rhythm'`, which still contains `rhythm − rhythm'`.
- On the synthetic test that residue is −23 dB below the mix for noise-like hats. That is a worst case (a
  perceptual codec does not keep a noise waveform), but it needs the owner's listening check.
- Fallback if it is audible: FLAC 16-bit, about 60 MB per 5 minutes, which needs the probe issue fixed or
  another encoder.

**Layout:**
- One directory per entry: `<data dir>/stems/<track id>-<fingerprint>/`, holding `vocals.ogg`, `bass.ogg`,
  `rhythm.ogg` and a `stems.toml`.
- `stems.toml` fields:
  - `format = 1`;
  - `sample_rate` (the track's native rate);
  - `frames`;
  - **model identity:** `model = "htdemucs-955717e8"`,
    `weights_sha256 = "d9fa1413...76dd"` (the safetensors file) and
    `graph_sha256 = "8d0bf481...1042"` (the weight-free graph).
  - A change to either hash makes the entry stale. The filled model's own hash is not part of the identity,
    because it depends on the protobuf encoder.
- The entry is written to a temporary directory and renamed into place. A missing, unreadable or stale file
  means "not cached".
- Per 5-minute track: 20.7 MB, encode 7.4 s on one thread (the three stems can run in parallel; not
  measured), decode 0.7 s.
- The CoreML compile cache (above) is not counted in `cache_gb`.
