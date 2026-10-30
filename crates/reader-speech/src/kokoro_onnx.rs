//! Fast native Kokoro inference. ONNX Runtime is loaded in-process, and the
//! Japanese phonemizer is native MeCab with the published Kokoro resources.
use crate::{
    kokoro_frontend::{self, Japanese},
    kokoro_ort_assets, kokoro_timing, native,
    providers::TtsProvider,
    types::*,
};
use anyhow::{Result, ensure};
use ort::{
    session::{Session, builder::GraphOptimizationLevel},
    value::Tensor,
};
use std::path::{Path, PathBuf};
const MODEL_URL: &str = "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/468588286ebb2dd77c25b9771e5d165896538cce/onnx/model_quantized.onnx";
const MODEL_SHA: &str = "fbae9257e1e05ffc727e951ef9b9c98418e6d79f1c9b6b13bd59f5c9028a1478";
const MODEL_SIZE: u64 = 92_361_116;
const GPU_URL: &str = "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/468588286ebb2dd77c25b9771e5d165896538cce/onnx/model.onnx";
const GPU_SHA: &str = "8fbea51ea711f2af382e88c833d9e288c6dc82ce5e98421ea61c058ce21a34cb";
const GPU_SIZE: u64 = 325_532_232;
const FRONTEND_URL: &str = "https://huggingface.co/audio-cpp/audio.cpp-gguf/resolve/462b4cec041e06ffb3d8fcf0d8c655051b1cd9f5/Kokoro-82M-GGUF/kokoro-82m-multilingual-q8_0.gguf";
const FRONTEND_SHA: &str = "c6864b5b66e94c83b94c45ad71f6cd1a445f25db19cfc0e59de28c694424ab18";
const FRONTEND_SIZE: u64 = 932_617_696;

pub(crate) struct KokoroOnnxProvider {
    config: ProviderConfig,
    session: Option<Session>,
    frontend: Option<Japanese>,
    voice: Vec<f32>,
    aligner: Option<native::NativeAligner>,
    gpu: bool,
    timed: bool,
}
impl KokoroOnnxProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            config,
            session: None,
            frontend: None,
            voice: Vec::new(),
            aligner: None,
            gpu: false,
            timed: false,
        }
    }
}
impl TtsProvider for KokoroOnnxProvider {
    fn name(&self) -> &str {
        "Kokoro · native ONNX Runtime"
    }
    fn synthesize(
        &mut self,
        units: &[SpeechUnit],
        cancel: &Cancellation,
        progress: &dyn Fn(String),
    ) -> Result<SynthesizedAudio> {
        let ProviderConfig::Kokoro {
            runtime_dir,
            model_path,
            backend,
            auto_download,
            allow_nvidia_download,
            voice,
            aligner_path,
        } = &self.config
        else {
            unreachable!()
        };
        ensure!(
            kokoro_voices().iter().any(|(id, _)| id == voice),
            "Kokoroの日本語の声を選択してください"
        );
        cancel.check()?;
        let profile = std::env::var_os("YOMI_TTS_PROFILE").is_some();
        let started = std::time::Instant::now();
        if self.session.is_none() {
            // Old DirectML profiles remain usable without the removed provider.
            // Only auto/CUDA opt into NVIDIA; CPU and old backend names stay CPU.
            let use_gpu = wants_cuda(backend) && kokoro_ort_assets::cuda_available();
            let resources = model_path.with_file_name("kokoro-82m-multilingual-q8_0.gguf");
            if *auto_download {
                crate::kokoro_notices::retain(runtime_dir)?;
                native::download(
                    FRONTEND_URL,
                    &resources,
                    FRONTEND_SIZE,
                    FRONTEND_SHA,
                    cancel,
                    progress,
                    "Kokoroの日本語発音データ",
                )?;
            }
            let root = model_path.with_file_name("kokoro-japanese-frontend-v1");
            progress("日本語の発音データを準備しています".into());
            kokoro_frontend::extract(&resources, &root)?;
            let mecab =
                native::kokoro_mecab_library(runtime_dir, *auto_download, cancel, progress)?;
            let frontend = Japanese::new(&root, &mecab)?;
            self.voice = frontend.voice(voice)?;
            self.frontend = Some(frontend);
            let (library, use_gpu) = match kokoro_ort_assets::prepare(
                runtime_dir,
                *auto_download,
                use_gpu,
                *allow_nvidia_download,
                cancel,
                progress,
            ) {
                Ok(library) => (library, use_gpu),
                Err(error) if use_gpu => {
                    if *allow_nvidia_download {
                        progress(format!("GPUを利用できないためCPUで準備します: {error}"));
                    } else {
                        progress("CPUの音声を準備しています".into());
                    }
                    (
                        kokoro_ort_assets::prepare(
                            runtime_dir,
                            *auto_download,
                            false,
                            *allow_nvidia_download,
                            cancel,
                            progress,
                        )?,
                        false,
                    )
                }
                Err(error) => return Err(error),
            };
            // ort's dynamic loader can panic for a corrupt/old override DLL.
            // Preserve the synthesis worker and surface a normal provider error.
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ort::init_from(library.to_string_lossy()).with_name("yomi-kokoro").commit()))
                .map_err(|_| anyhow::anyhow!("ONNX Runtimeを初期化できません。ランタイムまたはORT_DYLIB_PATHを確認し、読書ウィンドウを開き直してください"))??;
            // Runtime preparation may have fallen back before a Session exists.
            // Choose the CPU model here as well, rather than running FP32 on CPU.
            let (model, use_gpu) =
                match prepare_model(model_path, use_gpu, *auto_download, cancel, progress) {
                    Ok(model) => (model, use_gpu),
                    Err(error) if use_gpu && !cancel.is_cancelled() => {
                        progress(format!(
                            "GPUモデルを利用できないためCPUで準備します: {error}"
                        ));
                        (
                            prepare_model(model_path, false, *auto_download, cancel, progress)?,
                            false,
                        )
                    }
                    Err(error) => return Err(error),
                };
            progress("Kokoroの高速音声モデルを読み込んでいます".into());
            let timed_model = kokoro_timing::timed_model(&model).ok();
            self.timed = timed_model.is_some();
            self.gpu = use_gpu;
            self.session = Some(
                match build_session(timed_model.as_deref().unwrap_or(&model), use_gpu) {
                    Ok(session) => session,
                    Err(error) if use_gpu => {
                        progress(format!("GPUを利用できないためCPUで準備します: {error}"));
                        let cpu =
                            prepare_model(model_path, false, *auto_download, cancel, progress)?;
                        self.gpu = false;
                        let timed = kokoro_timing::timed_model(&cpu).ok();
                        self.timed = timed.is_some();
                        build_session(timed.as_deref().unwrap_or(&cpu), false)?
                    }
                    Err(error) => return Err(error),
                },
            );
            if !self.timed
                && let Some(path) = aligner_path
            {
                native::prepare_aligner(runtime_dir, path, *auto_download, cancel, progress)?;
                self.aligner = Some(native::NativeAligner::load(runtime_dir, path)?);
            }
        }
        cancel.check()?;
        let text: String = units.iter().map(|unit| unit.text.as_str()).collect();
        let frontend_started = std::time::Instant::now();
        let phones = self.frontend.as_ref().unwrap().tokens(&text)?;
        let frontend_ms = frontend_started.elapsed().as_millis();
        let inference_started = std::time::Instant::now();
        let mut samples = Vec::new();
        let mut boundaries = Vec::new();
        let mut valid_timing = true;
        for chunk in phones.chunks(500) {
            cancel.check()?;
            let style_index = chunk.len().saturating_sub(1).min(509);
            let style = self.voice[style_index * 256..(style_index + 1) * 256].to_vec();
            let mut tokens = Vec::with_capacity(chunk.len() + 2);
            tokens.push(0);
            tokens.extend(chunk.iter().map(|phone| phone.id));
            tokens.push(0);
            let inference = |session: &mut Session| -> Result<(Vec<f32>, Vec<i64>)> {
                let ids = Tensor::from_array((
                    [1usize, tokens.len()],
                    tokens.clone().into_boxed_slice(),
                ))?;
                let style = Tensor::from_array(([1usize, 256], style.clone().into_boxed_slice()))?;
                let speed = Tensor::from_array(([1usize], vec![1.0f32].into_boxed_slice()))?;
                let output =
                    session.run(ort::inputs!["input_ids"=>ids,"style"=>style,"speed"=>speed])?;
                let (_, pcm) = output[0].try_extract_tensor::<f32>()?;
                ensure!(
                    !pcm.is_empty() && pcm.iter().all(|sample| sample.is_finite()),
                    "Kokoro returned invalid PCM"
                );
                let durations = if let Some(value) = output.get(kokoro_timing::DURATION_OUTPUT) {
                    value.try_extract_tensor::<i64>()?.1.to_vec()
                } else {
                    Vec::new()
                };
                Ok((pcm.to_vec(), durations))
            };
            let (pcm, durations) = match inference(self.session.as_mut().unwrap()) {
                Ok(result) => result,
                Err(error) if self.gpu => {
                    progress(format!("GPUを利用できないためCPUで読み上げます: {error}"));
                    let model = prepare_model(model_path, false, *auto_download, cancel, progress)?;
                    let timed = kokoro_timing::timed_model(&model).ok();
                    self.timed = timed.is_some();
                    self.gpu = false;
                    self.session = Some(build_session(timed.as_deref().unwrap_or(&model), false)?);
                    inference(self.session.as_mut().unwrap())?
                }
                Err(error) => return Err(error),
            };
            if self.timed && valid_timing {
                match duration_boundaries(
                    chunk,
                    &durations,
                    units,
                    samples.len() as u64,
                    pcm.len() as u64,
                ) {
                    Ok(marks) => boundaries.extend(marks),
                    Err(error) => {
                        valid_timing = false;
                        boundaries.clear();
                        progress(format!(
                            "発音時刻を取得できないため文単位で表示します: {error}"
                        ));
                    }
                }
            } else {
                valid_timing = false;
                boundaries.clear();
            }
            samples.extend_from_slice(&pcm);
        }
        let inference_ms = inference_started.elapsed().as_millis();
        let mut audio = SynthesizedAudio {
            boundaries: vec![TimedBoundary {
                start_sample: 0,
                end_sample: samples.len() as u64,
                ranges: unit_ranges(units),
                precision: BoundaryPrecision::Chunk,
            }],
            samples,
            sample_rate: 24000,
        };
        let alignment_started = std::time::Instant::now();
        if !boundaries.is_empty() {
            audio.boundaries = boundaries;
        } else if let Some(aligner) = &self.aligner
            && let Ok(boundaries) = aligner.align(&text, units, &audio)
            && !boundaries.is_empty()
        {
            audio.boundaries = boundaries;
        }
        if profile {
            eprintln!(
                "kokoro backend={} frontend_ms={frontend_ms} inference_ms={inference_ms} alignment_ms={} total_ms={} phones={}",
                if self.gpu { "cuda" } else { "cpu" },
                alignment_started.elapsed().as_millis(),
                started.elapsed().as_millis(),
                phones.len()
            );
        }
        audio.validate()?;
        Ok(audio)
    }
}

fn wants_cuda(backend: &str) -> bool {
    matches!(
        backend.trim().to_ascii_lowercase().as_str(),
        "auto" | "cuda"
    )
}

fn model_for_backend(configured: &Path, gpu: bool) -> PathBuf {
    if gpu {
        configured.with_file_name("kokoro-82m-fp32.onnx")
    } else if configured.extension().is_some_and(|ext| ext == "gguf")
        || configured
            .file_name()
            .is_some_and(|name| name == "kokoro-82m-fp32.onnx" || name == "kokoro-82m-fp16.onnx")
    {
        configured.with_file_name("kokoro-82m-quantized.onnx")
    } else {
        configured.to_owned()
    }
}

fn prepare_model(
    configured: &Path,
    gpu: bool,
    auto: bool,
    cancel: &Cancellation,
    progress: &dyn Fn(String),
) -> Result<PathBuf> {
    let model = model_for_backend(configured, gpu);
    let managed = gpu
        || model
            .file_name()
            .is_some_and(|name| name == "kokoro-82m-quantized.onnx");
    if auto && (managed || !model.is_file()) {
        let (url, size, sha) = if gpu {
            (GPU_URL, GPU_SIZE, GPU_SHA)
        } else {
            (MODEL_URL, MODEL_SIZE, MODEL_SHA)
        };
        native::download(
            url,
            &model,
            size,
            sha,
            cancel,
            progress,
            "Kokoroの音声モデル",
        )?;
    }
    ensure!(
        model.is_file(),
        "Kokoroの音声モデルがありません: {}",
        model.display()
    );
    Ok(model)
}

fn build_session(model: &Path, gpu: bool) -> Result<Session> {
    let builder = Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .with_intra_threads(8)?
        .with_inter_threads(1)?;
    #[cfg(windows)]
    let builder = if gpu {
        builder
            .with_memory_pattern(false)?
            .with_parallel_execution(false)?
            .with_execution_providers([ort::execution_providers::CUDAExecutionProvider::default(
            )
            .with_conv_algorithm_search(
                ort::execution_providers::cuda::CuDNNConvAlgorithmSearch::Heuristic,
            )
            .with_conv_max_workspace(true)
            .build()
            .error_on_failure()])?
    } else {
        builder
    };
    #[cfg(not(windows))]
    let _ = gpu;
    Ok(builder.commit_from_file(model)?)
}

fn duration_boundaries(
    phones: &[crate::kokoro_frontend::Phone],
    durations: &[i64],
    units: &[SpeechUnit],
    offset: u64,
    samples: u64,
) -> Result<Vec<TimedBoundary>> {
    ensure!(
        durations.len() == phones.len() + 2 && durations.iter().all(|duration| *duration > 0),
        "Invalid Kokoro duration output"
    );
    let total: u64 = durations
        .iter()
        .try_fold(0u64, |sum, value| sum.checked_add(*value as u64))
        .ok_or_else(|| anyhow::anyhow!("Kokoro duration overflow"))?;
    // The pinned Kokoro export expands one predicted duration frame into
    // exactly 600 PCM samples at 24 kHz. Check that invariant; never stretch
    // timings proportionally to make an incompatible model appear aligned.
    const SAMPLES_PER_FRAME: u64 = 600;
    ensure!(
        total.checked_mul(SAMPLES_PER_FRAME) == Some(samples),
        "Unsupported Kokoro duration/PCM shape"
    );
    let mut frame = durations[0] as u64;
    let mut boundaries = Vec::<TimedBoundary>::new();
    for (phone, duration) in phones.iter().zip(&durations[1..]) {
        let end = frame + *duration as u64;
        if let Some(source) = &phone.source {
            let ranges = ranges_for_text(units, source.start, source.end);
            let start_sample = offset + frame * SAMPLES_PER_FRAME;
            let end_sample = offset + end * SAMPLES_PER_FRAME;
            if !ranges.is_empty() && end_sample > start_sample {
                if let Some(last) = boundaries.last_mut()
                    && last.ranges == ranges
                {
                    last.end_sample = end_sample;
                } else {
                    boundaries.push(TimedBoundary {
                        start_sample,
                        end_sample,
                        ranges,
                        precision: BoundaryPrecision::Word,
                    });
                }
            }
        }
        frame = end;
    }
    Ok(boundaries)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn obsolete_profiles_and_cpu_fallback_select_quantized_native_inference() {
        assert!(wants_cuda("auto"));
        assert!(wants_cuda("CUDA"));
        for backend in ["cpu", "directml", "vulkan", "metal", "unknown"] {
            assert!(!wants_cuda(backend));
        }
        for configured in [
            "model/kokoro-82m-multilingual-q8_0.gguf",
            "model/kokoro-82m-fp32.onnx",
            "model/kokoro-82m-fp16.onnx",
            "model/kokoro-82m-quantized.onnx",
        ] {
            assert_eq!(
                model_for_backend(Path::new(configured), false),
                Path::new("model/kokoro-82m-quantized.onnx")
            );
            assert_eq!(
                model_for_backend(Path::new(configured), true),
                Path::new("model/kokoro-82m-fp32.onnx")
            );
        }
        assert_eq!(
            model_for_backend(Path::new("custom/model.onnx"), false),
            Path::new("custom/model.onnx")
        );
    }
    #[test]
    fn real_duration_frames_map_to_source_units_and_preserve_pause() {
        use crate::kokoro_frontend::Phone;
        let units = vec![
            SpeechUnit {
                id: 10,
                text: "東京".into(),
            },
            SpeechUnit {
                id: 11,
                text: "です".into(),
            },
        ];
        let phones = vec![
            Phone {
                id: 1,
                source: Some(0..6),
            },
            Phone {
                id: 2,
                source: Some(0..6),
            },
            Phone {
                id: 3,
                source: None,
            },
            Phone {
                id: 4,
                source: Some(6..12),
            },
        ];
        let boundaries =
            duration_boundaries(&phones, &[1, 2, 3, 2, 4, 1], &units, 100, 7800).unwrap();
        assert_eq!(boundaries.len(), 2);
        assert_eq!(
            (boundaries[0].start_sample, boundaries[0].end_sample),
            (700, 3700)
        );
        assert_eq!(
            (boundaries[1].start_sample, boundaries[1].end_sample),
            (4900, 7300)
        );
        assert_eq!(boundaries[1].ranges[0].id, 11);
        assert_eq!(boundaries[0].precision, BoundaryPrecision::Word);
        assert!(duration_boundaries(&phones, &[1, 0], &units, 0, 1300).is_err());
        assert!(duration_boundaries(&phones, &[1, 2, 3, 2, 4, 1], &units, 0, 7799).is_err());
    }
}
