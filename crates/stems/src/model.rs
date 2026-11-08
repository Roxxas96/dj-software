//! The real separator: htdemucs (Demucs v4, `955717e8`) through ONNX Runtime, CoreML on
//! macOS and the CPU elsewhere (docs/stems-backend.md, "`ort` session settings").

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

use engine::{StemSet, TrackBuffer};
use ort::environment::Environment;
use ort::session::Session;
use ort::session::builder::{AutoDevicePolicy, GraphOptimizationLevel, SessionBuilder};
use ort::value::TensorRef;

use crate::chunked::{self, KEPT, Network};
use crate::dsp::{self, BINS, FRAMES, SEGMENT, Stft};
use crate::{SeparateError, Separator};

/// The network's sources, in its output order.
const SOURCES: usize = 4;
/// For each kept stem (vocals, bass, rhythm), its source: 0 drums, 1 bass, 2 other,
/// 3 vocals. Drums are rhythm; other is dropped (instruments is the residual).
const SOURCE_OF_KEPT: [usize; KEPT] = [3, 1, 0];

/// How to open the model.
#[derive(Debug, Clone, Default)]
pub struct ModelOptions {
    /// The CoreML compile cache (macOS); `None` is [`crate::coreml_cache_dir`].
    pub coreml_cache: Option<PathBuf>,
    /// ONNX Runtime's intra-op threads on the CPU path; `None` is its default. Fewer
    /// threads leave room for the audio callback while a deck plays.
    pub threads: Option<usize>,
    /// The CPU path even on macOS.
    pub cpu_only: bool,
    /// Where kicks split from rhythm, in Hz; `None` is [`engine::DEFAULT_KICKS_HZ`].
    pub kicks_hz: Option<f32>,
}

/// htdemucs behind [`Separator`]: chunked, overlapped and mapped to vocals, bass and
/// rhythm.
pub struct ModelSeparator {
    network: OrtNetwork,
    start_frame: usize,
    kicks_hz: f32,
}

impl std::fmt::Debug for ModelSeparator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelSeparator").finish_non_exhaustive()
    }
}

fn ort_error(e: impl std::fmt::Display) -> SeparateError {
    SeparateError::Model(e.to_string())
}

/// ONNX Runtime allows one environment per process, ever: it is made once and shared.
fn environment() -> Result<Environment, SeparateError> {
    static ENVIRONMENT: Mutex<Option<Environment>> = Mutex::new(None);
    let mut slot = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(environment) = slot.as_ref() {
        return Ok(environment.clone());
    }
    let environment = ort::init()
        .with_name("open-jockey-stems")
        .build()
        .map_err(ort_error)?;
    *slot = Some(environment.clone());
    Ok(environment)
}

fn cpu_builder(
    builder: SessionBuilder,
    threads: Option<usize>,
) -> Result<SessionBuilder, SeparateError> {
    // With api-22 and later, a plain builder picks a device by itself: on a Mac that routes
    // a "CPU" session to the Neural Engine, at 15 GB and 7 s a chunk.
    let builder = builder
        .with_auto_device(AutoDevicePolicy::PreferCPU)
        .map_err(ort_error)?;
    match threads {
        Some(threads) => builder.with_intra_threads(threads).map_err(ort_error),
        None => Ok(builder),
    }
}

fn open(model: &Path, options: &ModelOptions) -> Result<Session, SeparateError> {
    let environment = environment()?;
    let builder = Session::builder(&environment)
        .map_err(ort_error)?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(ort_error)?;
    #[cfg(target_os = "macos")]
    let mut builder = if options.cpu_only {
        cpu_builder(builder, options.threads)?
    } else {
        use ort::ep;
        let cache = options
            .coreml_cache
            .clone()
            .unwrap_or_else(crate::coreml_cache_dir);
        std::fs::create_dir_all(&cache)
            .map_err(|e| SeparateError::Model(format!("creating {}: {e}", cache.display())))?;
        let coreml = ep::CoreML::default()
            .with_model_format(ep::coreml::ModelFormat::MLProgram)
            .with_compute_units(ep::coreml::ComputeUnits::All)
            .with_static_input_shapes(true)
            .with_model_cache_dir(cache.display());
        builder
            .with_execution_providers([coreml.build().error_on_failure()])
            .map_err(ort_error)?
    };
    #[cfg(not(target_os = "macos"))]
    let mut builder = cpu_builder(builder, options.threads)?;
    builder
        .commit_from_file(model)
        .map_err(|e| SeparateError::Model(format!("loading {}: {e}", model.display())))
}

/// The network's two outputs for one chunk, `[SOURCES][4][BINS][FRAMES]` and
/// `[SOURCES][2][SEGMENT]`.
struct Outputs {
    spec: Vec<f32>,
    time: Vec<f32>,
}

struct OrtNetwork {
    session: Session,
    stft: Stft,
    mag: Vec<f32>,
}

impl OrtNetwork {
    fn new(session: Session) -> Self {
        Self {
            session,
            stft: Stft::new(),
            mag: vec![0.0; 4 * BINS * FRAMES],
        }
    }

    /// Runs the network on a planar `[2][SEGMENT]` `mix`, with its spectrogram.
    fn outputs(&mut self, mix: &[f32]) -> Result<Outputs, SeparateError> {
        dsp::magnitude(&mut self.stft, mix, &mut self.mag);
        let mix = TensorRef::from_array_view(([1usize, 2, SEGMENT], mix)).map_err(ort_error)?;
        let mag = TensorRef::from_array_view(([1usize, 4, BINS, FRAMES], &self.mag[..]))
            .map_err(ort_error)?;
        let outputs = self
            .session
            .run(ort::inputs!["mix" => mix, "mag" => mag])
            .map_err(ort_error)?;
        let (_, spec) = outputs["spec_out"]
            .try_extract_tensor::<f32>()
            .map_err(ort_error)?;
        let (_, time) = outputs["time_out"]
            .try_extract_tensor::<f32>()
            .map_err(ort_error)?;
        if spec.len() != SOURCES * 4 * BINS * FRAMES || time.len() != SOURCES * 2 * SEGMENT {
            return Err(SeparateError::Model("unexpected output shapes".into()));
        }
        Ok(Outputs {
            spec: spec.to_vec(),
            time: time.to_vec(),
        })
    }
}

impl Network for OrtNetwork {
    fn run(&mut self, mix: &[f32], out: &mut [f32]) -> Result<(), SeparateError> {
        let Outputs { spec, time } = self.outputs(mix)?;
        let plane = BINS * FRAMES;
        for (kept, &source) in SOURCE_OF_KEPT.iter().enumerate() {
            for channel in 0..2 {
                // A source is its time branch plus the iSTFT of its spectrogram branch.
                let dst = &mut out[(kept * 2 + channel) * SEGMENT..][..SEGMENT];
                dst.copy_from_slice(&time[(source * 2 + channel) * SEGMENT..][..SEGMENT]);
                let base = (source * 4 + 2 * channel) * plane;
                self.stft.ispec_add(
                    &spec[base..base + plane],
                    &spec[base + plane..base + 2 * plane],
                    dst,
                );
            }
        }
        Ok(())
    }
}

impl ModelSeparator {
    /// Opens the filled model at `model`. On macOS the first open compiles it for CoreML
    /// (about a minute; `stems setup` does it once), later opens read the CoreML cache.
    pub fn new(model: &Path, options: &ModelOptions) -> Result<Self, SeparateError> {
        Ok(Self {
            network: OrtNetwork::new(open(model, options)?),
            start_frame: 0,
            kicks_hz: options.kicks_hz.unwrap_or(engine::DEFAULT_KICKS_HZ),
        })
    }

    /// Runs the network once on a silent chunk: `stems setup` checks with it that the model
    /// runs on this machine.
    pub fn warm_up(&mut self) -> Result<(), SeparateError> {
        self.network.outputs(&vec![0.0; 2 * SEGMENT]).map(drop)
    }

    /// The next separation starts with the chunk around `frame` (the playhead) and works
    /// outward, so the part about to be played is ready first.
    pub fn start_at(&mut self, frame: usize) {
        self.start_frame = frame;
    }
}

impl Separator for ModelSeparator {
    fn separate(
        &mut self,
        track: &TrackBuffer,
        progress: &dyn Fn(f32),
        cancel: &AtomicBool,
    ) -> Result<StemSet, SeparateError> {
        let (set, _) = chunked::separate(
            &mut self.network,
            track,
            self.start_frame,
            self.kicks_hz,
            progress,
            cancel,
        )?;
        Ok(set)
    }
}

/// The test vector's expected per-source sums of squares (PyTorch, docs/stems-backend.md),
/// in source order (drums, bass, other, vocals).
const EXPECTED_SPEC: [f64; SOURCES] = [15_736.628_5, 93.753_09, 15_151.549_6, 257.953_97];
const EXPECTED_TIME: [f64; SOURCES] = [216.815_94, 32.594_88, 7_860.118_6, 1.756_936_8];
/// Each within this relative error.
const TOLERANCE: f64 = 2e-3;

/// Runs the recorded test vector through the model at `model` on the CPU and checks its
/// outputs: the fallback check of a filled model whose bytes differ from the recorded hash.
pub(crate) fn check_test_vector(model: &Path) -> Result<(), String> {
    let options = ModelOptions {
        cpu_only: true,
        ..ModelOptions::default()
    };
    let mut network = OrtNetwork::new(open(model, &options).map_err(|e| e.to_string())?);
    let outputs = network
        .outputs(&dsp::test_vector_mix())
        .map_err(|e| e.to_string())?;
    let per_source = |data: &[f32], expected: &[f64; SOURCES], name: &str| {
        let size = data.len() / SOURCES;
        for (source, chunk) in data.chunks_exact(size).enumerate() {
            let squares: f64 = chunk.iter().map(|&v| f64::from(v).powi(2)).sum();
            let error = (squares / expected[source] - 1.0).abs();
            if error > TOLERANCE {
                return Err(format!(
                    "{name} source {source}: sum of squares {squares}, expected {} (relative error {error:.1e})",
                    expected[source]
                ));
            }
        }
        Ok(())
    };
    per_source(&outputs.spec, &EXPECTED_SPEC, "spec_out")?;
    per_source(&outputs.time, &EXPECTED_TIME, "time_out")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `OPEN_JOCKEY_STEMS_MODEL=<filled model> cargo nextest run -p stems --run-ignored only test_vector`
    #[test]
    #[ignore = "needs the filled model (OPEN_JOCKEY_STEMS_MODEL)"]
    fn the_filled_model_passes_its_test_vector() {
        let model = std::env::var_os("OPEN_JOCKEY_STEMS_MODEL")
            .map(PathBuf::from)
            .unwrap_or_else(crate::model_path);
        check_test_vector(&model).unwrap();
    }
}
