//! In-process JP-Extra AIVMX inference through ONNX Runtime, without an HTTP or
//! Python speech engine. Existing user-selected models remain external assets.
use crate::{
    aivis_japanese::JapaneseFrontend,
    aivis_model::{self, AivisModelInfo, ModelData},
    providers::TtsProvider,
    types::*,
};
use anyhow::{Context, Result, ensure};
use ort::{
    session::{
        Session,
        builder::GraphOptimizationLevel,
        run_options::{OutputSelector, RunOptions},
    },
    value::Tensor,
};
use std::{ops::Range, path::PathBuf};
use tokenizers::Tokenizer;

#[derive(Clone, Debug)]
pub struct AivisNativePaths {
    pub model: PathBuf,
    pub bert_model: PathBuf,
    pub tokenizer: PathBuf,
    pub runtime_library: PathBuf,
}
#[derive(Clone, Debug)]
pub struct AivisNativeMark {
    pub start_sample: u64,
    pub end_sample: u64,
    pub source: Range<usize>,
}
#[derive(Clone, Debug)]
pub struct AivisNativeAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub marks: Vec<AivisNativeMark>,
}
pub struct NativeAivis {
    model: Session,
    bert: Session,
    tokenizer: Tokenizer,
    frontend: JapaneseFrontend,
    data: ModelData,
    alignment_output: Option<String>,
}
impl NativeAivis {
    pub fn load(paths: &AivisNativePaths) -> Result<Self> {
        Self::load_with_cuda(paths, false)
    }
    pub fn load_with_cuda(paths: &AivisNativePaths, use_cuda: bool) -> Result<Self> {
        let data = aivis_model::model_data(&paths.model)?;
        aivis_model::initialize_runtime(&paths.runtime_library)?;
        let model = session(&paths.model, use_cuda)?;
        aivis_model::validate_session(&model)?;
        let bert = session(&paths.bert_model, use_cuda)?;
        ensure!(
            bert.inputs.len() == 2
                && bert.inputs.iter().any(|v| v.name == "input_ids")
                && bert.inputs.iter().any(|v| v.name == "attention_mask"),
            "Unsupported Japanese BERT interface"
        );
        let tokenizer =
            Tokenizer::from_file(&paths.tokenizer).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        // The exported model exposes its actual monotone frame/phone alignment.
        // Unknown exports stay untimed rather than estimating from text length.
        let alignment_output = model
            .outputs
            .iter()
            .find(|output| output.name == "onnx::Squeeze_7111")
            .map(|output| output.name.clone());
        Ok(Self {
            model,
            bert,
            tokenizer,
            frontend: JapaneseFrontend::new()?,
            data,
            alignment_output,
        })
    }
    pub fn info(&self) -> &AivisModelInfo {
        &self.data.info
    }
    pub fn synthesize(
        &mut self,
        text: &str,
        speaker_id: u32,
        style_id: u32,
    ) -> Result<AivisNativeAudio> {
        self.synthesize_cancellable(text, speaker_id, style_id, &Cancellation::uncancelled())
    }
    pub fn synthesize_cancellable(
        &mut self,
        text: &str,
        speaker_id: u32,
        style_id: u32,
        cancel: &Cancellation,
    ) -> Result<AivisNativeAudio> {
        cancel.check()?;
        let speaker = self
            .data
            .info
            .speakers
            .iter()
            .find(|s| s.local_id == speaker_id)
            .context("Unknown Aivis speaker")?;
        ensure!(
            speaker.styles.iter().any(|s| s.local_id == style_id),
            "Unknown Aivis style"
        );
        let style = self
            .data
            .styles
            .get(style_id as usize)
            .context("Style vector is missing")?
            .clone();
        let prepared = self.frontend.prepare(text)?;
        cancel.check()?;
        let mut token_ids = vec![
            self.tokenizer
                .token_to_id("[CLS]")
                .context("BERT tokenizer has no CLS token")? as i64,
        ];
        for c in prepared.text.chars() {
            let encoded = self
                .tokenizer
                .encode(c.to_string(), false)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            ensure!(
                encoded.len() == 1,
                "BERT tokenizer does not encode a character as one token"
            );
            token_ids.push(encoded.get_ids()[0] as i64);
        }
        token_ids.push(
            self.tokenizer
                .token_to_id("[SEP]")
                .context("BERT tokenizer has no SEP token")? as i64,
        );
        ensure!(
            token_ids.len() <= 512 && prepared.phones.len() <= 1500,
            "Aivis chunk exceeds the model context; split this text before synthesis"
        );
        let n = token_ids.len();
        let features = self.bert.run(ort::inputs![
         "input_ids"=>Tensor::from_array(([1usize,n],token_ids))?,
         "attention_mask"=>Tensor::from_array(([1usize,n],vec![1i64;n]))?,
        ])?;
        let (shape, features) = features[0].try_extract_tensor::<f32>()?;
        ensure!(
            (shape.as_ref() == [n as i64, 1024] || shape.as_ref() == [1, n as i64, 1024])
                && features.len() == n * 1024,
            "Unexpected Japanese BERT feature shape"
        );
        ensure!(
            features.iter().all(|v| v.is_finite()),
            "BERT produced nonfinite features"
        );
        cancel.check()?;
        let mut ids = Vec::new();
        let mut tones = Vec::new();
        let mut language = Vec::<i64>::new();
        let mut chars = Vec::new();
        let mut sources = Vec::new();
        if self.data.add_blank {
            ids.push(0);
            tones.push(0);
            language.push(0);
            chars.push(0);
            sources.push(None);
        }
        for phone in &prepared.phones {
            ids.push(phone.id);
            tones.push(phone.tone + 6);
            language.push(1);
            chars.push(phone.bert_char);
            sources.push(phone.source.clone());
            if self.data.add_blank {
                ids.push(0);
                tones.push(0);
                language.push(0);
                chars.push(phone.bert_char);
                sources.push(phone.source.clone());
            }
        }
        let length = ids.len();
        let mut expanded = vec![0f32; 1024 * length];
        for (position, &c) in chars.iter().enumerate() {
            ensure!(c < n, "BERT/source expansion mismatch");
            for dimension in 0..1024 {
                expanded[dimension * length + position] = features[c * 1024 + dimension];
            }
        }
        // Release borrowed outputs before running the independent audio graph.
        let mut selector = OutputSelector::no_default().with("output");
        if let Some(duration) = &self.alignment_output {
            selector = selector.with(duration);
        }
        let options = RunOptions::new()?.with_outputs(selector);
        let output = self.model.run_with_options(
            ort::inputs![
             "x_tst"=>Tensor::from_array(([1usize,length],ids))?,
             "x_tst_lengths"=>Tensor::from_array(([1usize],vec![length as i64]))?,
             "sid"=>Tensor::from_array(([1usize],vec![i64::from(speaker_id)]))?,
             "tones"=>Tensor::from_array(([1usize,length],tones))?,
             "language"=>Tensor::from_array(([1usize,length],language))?,
             "bert"=>Tensor::from_array(([1usize,1024,length],expanded))?,
             "style_vec"=>Tensor::from_array(([1usize,256],style))?,
             "length_scale"=>scalar(1.0)?,"sdp_ratio"=>scalar(0.0)?,
             "noise_scale"=>scalar(0.667)?,"noise_scale_w"=>scalar(0.8)?,
            ],
            &options,
        )?;
        let (_, samples) = output["output"].try_extract_tensor::<f32>()?;
        ensure!(
            !samples.is_empty()
                && samples.len() <= self.data.sample_rate as usize * 600
                && samples.iter().all(|v| v.is_finite()),
            "Aivis produced invalid PCM"
        );
        let samples = samples.to_vec();
        let marks = self
            .alignment_output
            .as_ref()
            .and_then(|name| output.get(name))
            .and_then(|value| value.try_extract_tensor::<f32>().ok())
            .map(|(shape, alignment)| {
                real_alignment_marks(
                    shape.as_ref(),
                    alignment,
                    &sources,
                    self.data.hop_length,
                    samples.len(),
                )
            })
            .unwrap_or_default();
        Ok(AivisNativeAudio {
            samples,
            sample_rate: self.data.sample_rate,
            marks,
        })
    }
}
fn scalar(value: f32) -> Result<Tensor<f32>> {
    Ok(Tensor::from_array((Vec::<usize>::new(), vec![value]))?)
}
fn session(path: &std::path::Path, use_cuda: bool) -> Result<Session> {
    let mut builder = Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .with_intra_threads(4)?;
    if use_cuda {
        builder = builder
            .with_memory_pattern(false)?
            .with_parallel_execution(false)?
            .with_execution_providers([
                ort::execution_providers::CUDAExecutionProvider::default()
                    .with_conv_algorithm_search(
                        ort::execution_providers::cuda::CuDNNConvAlgorithmSearch::Heuristic,
                    )
                    .with_tf32(false)
                    .build()
                    .error_on_failure(),
            ])?;
    }
    Ok(builder.commit_from_file(path)?)
}

fn real_alignment_marks(
    shape: &[i64],
    alignment: &[f32],
    sources: &[Option<Range<usize>>],
    hop: u32,
    samples: usize,
) -> Vec<AivisNativeMark> {
    if shape.len() != 4
        || shape[0] != 1
        || shape[1] != 1
        || shape[2] <= 0
        || shape[3] != sources.len() as i64
    {
        return vec![];
    }
    let frames = shape[2] as usize;
    if frames.checked_mul(sources.len()) != Some(alignment.len())
        || frames.checked_mul(hop as usize) != Some(samples)
    {
        return vec![];
    }
    let mut durations = vec![0f32; sources.len()];
    let mut previous = 0usize;
    for row in alignment.chunks_exact(sources.len()) {
        let mut current = None;
        for (i, value) in row.iter().enumerate() {
            if *value == 1.0 {
                if current.replace(i).is_some() {
                    return vec![];
                }
            } else if *value != 0.0 {
                return vec![];
            }
        }
        let Some(current) = current else {
            return vec![];
        };
        if current < previous {
            return vec![];
        }
        previous = current;
        durations[current] += 1.0;
    }
    real_duration_marks(&durations, sources, hop, samples)
}
fn real_duration_marks(
    durations: &[f32],
    sources: &[Option<Range<usize>>],
    hop: u32,
    samples: usize,
) -> Vec<AivisNativeMark> {
    if durations.len() != sources.len()
        || durations
            .iter()
            .any(|d| !d.is_finite() || *d < 0.0 || d.fract() != 0.0)
    {
        return vec![];
    }
    let Some(total) = durations
        .iter()
        .try_fold(0u64, |total, d| total.checked_add(*d as u64))
    else {
        return vec![];
    };
    if total.checked_mul(u64::from(hop)) != Some(samples as u64) {
        return vec![];
    }
    let mut cursor = 0u64;
    let mut marks: Vec<AivisNativeMark> = Vec::new();
    for (duration, source) in durations.iter().zip(sources) {
        let end = cursor + (*duration as u64) * u64::from(hop);
        if end > cursor
            && let Some(source) = source
        {
            if let Some(previous) = marks.last_mut()
                && previous.source == *source
                && previous.end_sample == cursor
            {
                previous.end_sample = end;
            } else {
                marks.push(AivisNativeMark {
                    start_sample: cursor,
                    end_sample: end,
                    source: source.clone(),
                });
            }
        }
        cursor = end;
    }
    marks
}

pub(crate) struct AivisNativeProvider {
    config: ProviderConfig,
    synthesizer: Option<NativeAivis>,
    paths: Option<AivisNativePaths>,
    gpu: bool,
}
impl AivisNativeProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            config,
            synthesizer: None,
            paths: None,
            gpu: false,
        }
    }
}
impl TtsProvider for AivisNativeProvider {
    fn name(&self) -> &str {
        "AivisSpeech · native ONNX"
    }
    fn synthesize(
        &mut self,
        units: &[SpeechUnit],
        cancel: &Cancellation,
        progress: &dyn Fn(String),
    ) -> Result<SynthesizedAudio> {
        let ProviderConfig::AivisNative {
            runtime_dir,
            backend,
            auto_download,
            allow_nvidia_download,
            speaker,
            style,
            ..
        } = &self.config
        else {
            unreachable!()
        };
        cancel.check()?;
        if self.synthesizer.is_none() {
            let mut paths = crate::aivis_assets::prepare(&self.config, cancel, progress)?;
            let auto = backend == "auto";
            ensure!(
                matches!(backend.as_str(), "auto" | "cpu" | "cuda"),
                "Aivisの実行方法はauto/cpu/cudaを指定してください"
            );
            let cuda = backend != "cpu" && crate::kokoro_ort_assets::cuda_available();
            ensure!(
                backend != "cuda" || cuda,
                "CUDAを利用できません。CPUまたは自動を選択してください"
            );
            let (library, cuda) = match crate::kokoro_ort_assets::prepare(
                runtime_dir,
                *auto_download,
                cuda,
                *allow_nvidia_download,
                cancel,
                progress,
            ) {
                Ok(path) => (path, cuda),
                Err(error) if cuda && auto && !cancel.is_cancelled() => {
                    progress(format!(
                        "AivisのGPUを利用できないため同じモデルをCPUで準備します: {error}"
                    ));
                    (
                        crate::kokoro_ort_assets::prepare(
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
            paths.runtime_library = library;
            progress(
                "Aivisの音声モデルと日本語BERTを読み込んでいます（初回は数秒以上かかります）"
                    .into(),
            );
            let (synthesizer, gpu) = match NativeAivis::load_with_cuda(&paths, cuda) {
                Ok(synth) => (synth, cuda),
                Err(error) if cuda && auto && !cancel.is_cancelled() => {
                    progress(format!(
                        "AivisのGPU初期化に失敗したため同じモデルをCPUで準備します: {error}"
                    ));
                    (NativeAivis::load(&paths)?, false)
                }
                Err(error) => return Err(error),
            };
            self.synthesizer = Some(synthesizer);
            self.paths = Some(paths);
            self.gpu = gpu;
        }
        cancel.check()?;
        let text: String = units.iter().map(|unit| unit.text.as_str()).collect();
        let result = self
            .synthesizer
            .as_mut()
            .unwrap()
            .synthesize_cancellable(&text, *speaker, *style, cancel);
        let result = match result {
            Err(error)
                if self.gpu
                    && backend == "auto"
                    && !cancel.is_cancelled()
                    && error
                        .chain()
                        .any(|cause| cause.downcast_ref::<ort::Error>().is_some()) =>
            {
                progress(format!(
                    "AivisのGPU推論に失敗したため同じ音声モデルをCPUで再試行します: {error}"
                ));
                // Drop failed GPU allocations before constructing CPU sessions.
                self.synthesizer = None;
                let synth = NativeAivis::load(self.paths.as_ref().unwrap())?;
                self.synthesizer = Some(synth);
                self.gpu = false;
                self.synthesizer
                    .as_mut()
                    .unwrap()
                    .synthesize_cancellable(&text, *speaker, *style, cancel)?
            }
            result => result?,
        };
        let mut boundaries: Vec<_> = result
            .marks
            .into_iter()
            .filter_map(|mark| {
                let ranges = ranges_for_text(units, mark.source.start, mark.source.end);
                (!ranges.is_empty()).then_some(TimedBoundary {
                    start_sample: mark.start_sample,
                    end_sample: mark.end_sample,
                    ranges,
                    precision: BoundaryPrecision::AlignedGroup,
                })
            })
            .collect();
        if boundaries.is_empty() {
            progress(
                "このAIVMXには対応する実アラインメント出力がないため、文単位でハイライトします"
                    .into(),
            );
            boundaries.push(TimedBoundary {
                start_sample: 0,
                end_sample: result.samples.len() as u64,
                ranges: unit_ranges(units),
                precision: BoundaryPrecision::Chunk,
            });
        }
        // Keep completed PCM available to the generation cache even if the user
        // changed position during inference. The engine guards playback epochs.
        let audio = SynthesizedAudio {
            samples: result.samples,
            sample_rate: result.sample_rate,
            boundaries,
        };
        audio.validate()?;
        Ok(audio)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_request_does_not_load_a_model_or_start_any_asset_preparation() {
        let mut config = ProviderConfig::aivis_native_default();
        if let ProviderConfig::AivisNative {
            model_path,
            auto_download,
            ..
        } = &mut config
        {
            *model_path = "nonexistent-native-fixture.aivmx".into();
            *auto_download = false;
        }
        let mut provider = AivisNativeProvider::new(config);
        let epoch = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1));
        let cancel = Cancellation::new(epoch, 0);
        let result = provider.synthesize(
            &[SpeechUnit {
                id: 9,
                text: "読書。".into(),
            }],
            &cancel,
            &|_| panic!("stale request must not prepare assets"),
        );
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert!(provider.synthesizer.is_none());
    }
    #[test]
    fn source_group_ranges_can_span_units_without_losing_ruby_unit_identity() {
        let units = [
            SpeechUnit {
                id: 10,
                text: "とうきょう".into(),
            },
            SpeechUnit {
                id: 22,
                text: "へ。".into(),
            },
        ];
        let ranges = ranges_for_text(&units, 0, 18);
        assert_eq!(
            ranges,
            [
                UnitRange {
                    id: 10,
                    start: 0,
                    end: 15
                },
                UnitRange {
                    id: 22,
                    start: 0,
                    end: 3
                }
            ]
        );
    }
    #[test]
    fn malicious_duration_overflow_is_rejected_without_panicking() {
        assert!(
            real_duration_marks(&[f32::MAX, f32::MAX], &[Some(0..1), Some(1..2)], 512, 512)
                .is_empty()
        );
    }
    #[test]
    fn actual_one_hot_monotone_alignment_maps_audio_frames_to_source_groups() {
        let sources = [Some(0..3), Some(3..9)];
        let alignment = [1., 0., 1., 0., 0., 1.];
        let marks = real_alignment_marks(&[1, 1, 3, 2], &alignment, &sources, 512, 1536);
        assert_eq!(marks.len(), 2);
        assert_eq!(marks[1].start_sample, 1024);
        assert!(
            real_alignment_marks(
                &[1, 1, 3, 2],
                &[0., 1., 1., 0., 0., 1.],
                &sources,
                512,
                1536
            )
            .is_empty()
        );
        assert!(real_alignment_marks(&[1, 1, 3, 2], &alignment, &sources, 512, 1500).is_empty());
    }
    #[test]
    fn only_real_integer_durations_matching_pcm_generate_marks() {
        let sources = [None, Some(0..6), Some(0..6), Some(6..9)];
        let marks = real_duration_marks(&[1., 2., 3., 4.], &sources, 512, 5120);
        assert_eq!(marks.len(), 2);
        assert_eq!(marks[0].start_sample, 512);
        assert_eq!(marks[0].end_sample, 3072);
        assert_eq!(marks[0].source, 0..6);
        assert!(real_duration_marks(&[1., 2., 3., 4.], &sources, 512, 5000).is_empty());
        assert!(real_duration_marks(&[1., 2., 3.5, 4.], &sources, 512, 5376).is_empty());
    }
}
