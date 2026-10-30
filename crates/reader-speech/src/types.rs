use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpeechUnit {
    /// Stable ID owned by the reader. The caller maps it back to source locators.
    pub id: u64,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct SpeechRun {
    pub id: u64,
    pub text: String,
    pub ruby: Option<String>,
}

/// Ruby is spoken once; its parent text is not appended again.
pub fn normalize_runs(runs: &[SpeechRun]) -> Vec<SpeechUnit> {
    runs.iter()
        .filter_map(|run| {
            let text = run
                .ruby
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(&run.text);
            (!text.trim().is_empty()).then(|| SpeechUnit {
                id: run.id,
                text: text.to_owned(),
            })
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum BoundaryPrecision {
    /// Provider-reported word timing (e.g. SAPI). No text-length interpolation.
    Word,
    /// Acoustic aligner supplied a text group; its Japanese segmentation may differ from a word.
    AlignedGroup,
    /// All source units in a synthesized chunk; finer timing is unavailable.
    Chunk,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnitRange {
    pub id: u64,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimedBoundary {
    pub start_sample: u64,
    pub end_sample: u64,
    pub ranges: Vec<UnitRange>,
    pub precision: BoundaryPrecision,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SynthesizedAudio {
    /// Mono, normalized floating point PCM at sample_rate.
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub boundaries: Vec<TimedBoundary>,
}

impl SynthesizedAudio {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (8_000..=192_000).contains(&self.sample_rate),
            "Unsupported PCM sample rate"
        );
        ensure!(!self.samples.is_empty(), "TTS returned empty audio");
        ensure!(
            self.samples.iter().all(|v| v.is_finite()),
            "TTS returned non-finite PCM"
        );
        ensure!(
            self.samples.len() <= self.sample_rate as usize * 60 * 10,
            "TTS chunk exceeds 10 minutes"
        );
        for boundary in &self.boundaries {
            ensure!(
                boundary.start_sample < boundary.end_sample
                    && boundary.end_sample <= self.samples.len() as u64,
                "Provider timing is outside its PCM audio"
            );
            ensure!(
                boundary.ranges.iter().all(|r| r.start <= r.end),
                "Invalid source range"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderConfig {
    /// Actual local Irodori inference via audio.cpp C ABI. Missing assets download automatically.
    Irodori {
        runtime_dir: PathBuf,
        model_path: PathBuf,
        backend: String,
        auto_download: bool,
        #[serde(default)]
        aligner_path: Option<PathBuf>,
        #[serde(default)]
        reference_audio: Option<PathBuf>,
        #[serde(default)]
        voice_instruction: String,
        #[serde(default = "default_voice_seed")]
        seed: u64,
    },
    /// Native local Kokoro; no HTTP service or Python process is used for synthesis.
    Kokoro {
        runtime_dir: PathBuf,
        model_path: PathBuf,
        backend: String,
        auto_download: bool,
        /// Explicit permission for separately licensed NVIDIA runtime downloads.
        /// Existing compatible local libraries can be used without downloading.
        #[serde(default)]
        allow_nvidia_download: bool,
        #[serde(default = "default_kokoro_voice")]
        voice: String,
        #[serde(default)]
        aligner_path: Option<PathBuf>,
    },
    /// Independent Rust JP-Extra AIVMX inference; no Python engine or HTTP service.
    AivisNative {
        runtime_dir: PathBuf,
        model_path: PathBuf,
        bert_dir: PathBuf,
        speaker: u32,
        style: u32,
        backend: String,
        auto_download: bool,
        #[serde(default)]
        allow_nvidia_download: bool,
    },
    /// Windows installed voices via System.Speech; never labelled Irodori.
    System {
        #[serde(default)]
        voice: Option<String>,
    },
    OpenAi {
        base_url: String,
        #[serde(default)]
        api_key: String,
        model: String,
        voice: String,
    },
    Voicevox {
        base_url: String,
        speaker: u32,
    },
    /// AivisSpeech's style ID is signed; its query semantics differ from VOICEVOX.
    AivisSpeech {
        base_url: String,
        speaker: i32,
        #[serde(default)]
        engine_path: Option<PathBuf>,
        #[serde(default = "enabled_by_default")]
        auto_start: bool,
    },
    /// The installed licensed VOICEROID2 DLL is called by a matching-bitness native helper.
    Voiceroid2 {
        install_dir: PathBuf,
        voice: String,
        #[serde(default = "unit_gain")]
        volume: f32,
        #[serde(default = "unit_gain")]
        pitch: f32,
        #[serde(default = "unit_gain")]
        range: f32,
    },
    /// JSON request on stdin: {text,units,output_wav}. Program writes the WAV to output_wav.
    /// Optional JSON response on stdout: {boundaries:[TimedBoundary,...]}.
    Command {
        program: PathBuf,
        #[serde(default)]
        args: Vec<String>,
    },
}

impl Default for ProviderConfig {
    fn default() -> Self {
        let assets = data_dir().join("speech").join("assets");
        Self::Irodori {
            runtime_dir: assets.join("audio.cpp-v0.9.3-portable"),
            model_path: assets.join("irodori-tts-v4.1-small-mf-q8_0.gguf"),
            backend: if cfg!(target_os = "macos") {
                "metal"
            } else if cfg!(windows) {
                "vulkan"
            } else {
                "cpu"
            }
            .into(),
            auto_download: true,
            aligner_path: Some(assets.join("qwen3-forced-aligner-0.6b-q8_0.gguf")),
            reference_audio: None,
            voice_instruction: String::new(),
            seed: default_voice_seed(),
        }
    }
}

impl ProviderConfig {
    pub fn voiceroid2_default() -> Self {
        Self::Voiceroid2 {
            install_dir: PathBuf::from(r"C:\Program Files (x86)\AHS\VOICEROID2"),
            voice: "yukari_emo_44".into(),
            volume: 1.,
            pitch: 1.,
            range: 1.,
        }
    }
    pub fn aivis_default() -> Self {
        Self::AivisSpeech {
            base_url: "http://127.0.0.1:10101".into(),
            speaker: 888753760,
            engine_path: None,
            auto_start: true,
        }
    }
    pub fn aivis_native_default() -> Self {
        let assets = data_dir().join("speech");
        let (model_path, bert_dir) = crate::aivis_assets::default_paths();
        Self::AivisNative {
            runtime_dir: assets.join("assets/audio.cpp-v0.9.3-portable"),
            model_path,
            bert_dir,
            speaker: 0,
            style: 0,
            backend: "auto".into(),
            auto_download: true,
            allow_nvidia_download: false,
        }
    }
    pub fn kokoro_default() -> Self {
        let assets = data_dir().join("speech").join("assets");
        Self::Kokoro {
            runtime_dir: assets.join("audio.cpp-v0.9.3-portable"),
            model_path: assets.join("kokoro-82m-quantized.onnx"),
            backend: if cfg!(windows) { "auto" } else { "cpu" }.into(),
            auto_download: true,
            allow_nvidia_download: false,
            voice: default_kokoro_voice(),
            aligner_path: None,
        }
    }
    pub fn name(&self) -> &str {
        match self {
            Self::Irodori { .. } => "Irodori TTS · ローカル",
            Self::Kokoro { .. } => "Kokoro · ローカル",
            Self::AivisNative { .. } => "AivisSpeech · 組み込み",
            Self::System { .. } => "システム音声",
            Self::OpenAi { .. } => "OpenAI互換TTS",
            Self::Voicevox { .. } => "VOICEVOX",
            Self::AivisSpeech { .. } => "AivisSpeech · ローカル",
            Self::Voiceroid2 { .. } => "VOICEROID2 · DLL",
            Self::Command { .. } => "外部コマンドTTS",
        }
    }
    pub fn capability(&self) -> ProviderCapability {
        match self {
            Self::AivisNative {model_path,bert_dir,..}=>ProviderCapability{name:self.name().into(),available:model_path.is_file()&&bert_dir.join("model_fp16.onnx").is_file()&&bert_dir.join("tokenizer.json").is_file(),precision:BoundaryPrecision::AlignedGroup,detail:"Rust内でJP-Extra AIVMXを実行します。サーバーは不要です。対応モデルの実アラインメントで文字群をハイライトします。未対応出力のモデルは文単位です。初回モデル読込には数秒以上かかります。発音・抑揚は既存Engineと完全一致しません。".into()},
            Self::AivisSpeech{..}=>ProviderCapability{name:self.name().into(),available:true,precision:BoundaryPrecision::AlignedGroup,detail:"実際の生成音声を位置合わせしてハイライトします。".into()},
            Self::Voiceroid2{install_dir,..}=>ProviderCapability{name:self.name().into(),available:cfg!(windows)&&install_dir.join("aitalked.dll").is_file()&&crate::voiceroid2_helper_path().is_some(),precision:BoundaryPrecision::AlignedGroup,detail:"インストール済みの音声を利用します。音声の実位置イベントで文字群をハイライトします。".into()},
            Self::Kokoro {runtime_dir,model_path,..} => ProviderCapability {name:self.name().into(),available:model_path.is_file()&&runtime_dir.is_dir(),precision:BoundaryPrecision::Word,detail:"日本語5種類の固定音声。ネイティブONNX推論の発音単位の実durationをハイライトへ変換します。初回は日本語データ約933MB、CPUモデル約92MB、ランタイムを取得します。NVIDIA GPU用の新規ライブラリ取得は利用条件への同意後に行います。".into()},
            Self::Irodori { runtime_dir, model_path, aligner_path, .. } => ProviderCapability {
                name: self.name().into(), available: model_path.is_file() && runtime_dir.is_dir(),
                precision: if aligner_path.as_ref().is_some_and(|p| p.is_file()) { BoundaryPrecision::AlignedGroup } else { BoundaryPrecision::Chunk },
                detail: if model_path.is_file() { "モデル取得済み。実際の起動時にランタイムとGPUを確認します。" } else if aligner_path.is_some() { "初回利用時にネイティブ推論ランタイムと日本語音声・位置合わせモデルを取得します（合計約2.5GB）。" } else { "初回利用時にネイティブ推論ランタイムと約1.37GBの日本語モデルを取得します。" }.into() },
            Self::System { .. } => ProviderCapability { name: self.name().into(), available: cfg!(windows), precision: BoundaryPrecision::Word,
                detail: "Windowsのインストール済み音声を使います。Irodoriとは別の音声です。".into() },
            _ => ProviderCapability { name: self.name().into(), available: true, precision: BoundaryPrecision::Chunk,
                detail: "接続設定を使用します。単語時刻がない場合は文単位で表示します。".into() }
        }
    }
}

fn enabled_by_default() -> bool {
    true
}

fn default_voice_seed() -> u64 {
    42
}
fn default_kokoro_voice() -> String {
    "jf_alpha".into()
}
pub fn kokoro_voices() -> &'static [(&'static str, &'static str)] {
    &[
        ("jf_alpha", "Alpha"),
        ("jf_gongitsune", "Gongitsune"),
        ("jf_nezumi", "Nezumi"),
        ("jf_tebukuro", "Tebukuro"),
        ("jm_kumo", "Kumo"),
    ]
}

#[derive(Clone, Debug)]
pub struct ProviderCapability {
    pub name: String,
    pub available: bool,
    pub precision: BoundaryPrecision,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpeechConfig {
    pub provider: ProviderConfig,
    pub cache_dir: PathBuf,
    pub speed: f32,
    #[serde(default)]
    pub pronunciations: BTreeMap<String, String>,
}
impl Default for SpeechConfig {
    fn default() -> Self {
        Self {
            provider: ProviderConfig::default(),
            cache_dir: data_dir().join("speech").join("cache"),
            speed: 1.0,
            pronunciations: BTreeMap::new(),
        }
    }
}

pub fn data_dir() -> PathBuf {
    directories::ProjectDirs::from("jp", "Liesegang", "epub-reader")
        .map(|p| p.data_local_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("epub-reader"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeechState {
    Preparing,
    Playing,
    Paused,
    Stopped,
    Finished,
}

#[derive(Clone, Debug)]
pub struct SpeechEvent {
    pub generation: u64,
    pub kind: SpeechEventKind,
}
#[derive(Clone, Debug)]
pub enum SpeechEventKind {
    State(SpeechState),
    Boundary {
        unit_ids: Vec<u64>,
        ranges: Vec<UnitRange>,
        precision: BoundaryPrecision,
    },
    Progress(String),
    Error(String),
}

#[derive(Clone)]
pub struct Cancellation {
    epoch: Arc<AtomicU64>,
    generation: u64,
    supersession: Option<(Arc<AtomicU64>, u64)>,
}
impl Cancellation {
    /// For direct provider calls that are not owned by a controller generation.
    pub fn uncancelled() -> Self {
        Self::new(Arc::new(AtomicU64::new(0)), 0)
    }
    pub(crate) fn new(epoch: Arc<AtomicU64>, generation: u64) -> Self {
        Self {
            epoch,
            generation,
            supersession: None,
        }
    }
    pub(crate) fn with_supersession(mut self, epoch: Arc<AtomicU64>, generation: u64) -> Self {
        self.supersession = Some((epoch, generation));
        self
    }
    pub fn is_cancelled(&self) -> bool {
        self.epoch.load(Ordering::Acquire) != self.generation
            || self
                .supersession
                .as_ref()
                .is_some_and(|(epoch, generation)| epoch.load(Ordering::Acquire) != *generation)
    }
    pub fn check(&self) -> Result<()> {
        ensure!(!self.is_cancelled(), "Speech request cancelled");
        Ok(())
    }
}

pub(crate) fn unit_ranges(units: &[SpeechUnit]) -> Vec<UnitRange> {
    units
        .iter()
        .map(|u| UnitRange {
            id: u.id,
            start: 0,
            end: u.text.len(),
        })
        .collect()
}

pub(crate) fn ranges_for_text(units: &[SpeechUnit], from: usize, to: usize) -> Vec<UnitRange> {
    let mut offset = 0;
    let mut result = Vec::new();
    for unit in units {
        let end = offset + unit.text.len();
        if from < end && to > offset {
            let start = from.saturating_sub(offset).min(unit.text.len());
            let stop = to.saturating_sub(offset).min(unit.text.len());
            if unit.text.is_char_boundary(start) && unit.text.is_char_boundary(stop) {
                result.push(UnitRange {
                    id: unit.id,
                    start,
                    end: stop,
                });
            }
        }
        offset = end;
    }
    result
}

fn unit_gain() -> f32 {
    1.
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prefetch_supersession_does_not_cancel_playback() {
        let playback_epoch = Arc::new(AtomicU64::new(1));
        let prefetch_epoch = Arc::new(AtomicU64::new(7));
        let playback = Cancellation::new(playback_epoch.clone(), 1);
        let prefetch = playback
            .clone()
            .with_supersession(prefetch_epoch.clone(), 7);
        assert!(!prefetch.is_cancelled());
        prefetch_epoch.store(8, Ordering::Release);
        assert!(prefetch.is_cancelled());
        assert!(!playback.is_cancelled());
        playback_epoch.store(2, Ordering::Release);
        assert!(playback.is_cancelled());
    }
    #[test]
    fn ruby_spoken_once() {
        let units = normalize_runs(&[SpeechRun {
            id: 9,
            text: "東京".into(),
            ruby: Some("とうきょう".into()),
        }]);
        assert_eq!(units[0].text, "とうきょう");
    }
    #[test]
    fn source_ranges_keep_utf8_boundaries() {
        let units = vec![
            SpeechUnit {
                id: 1,
                text: "東京".into(),
            },
            SpeechUnit {
                id: 2,
                text: "です".into(),
            },
        ];
        assert_eq!(
            ranges_for_text(&units, 3, 9),
            vec![
                UnitRange {
                    id: 1,
                    start: 3,
                    end: 6
                },
                UnitRange {
                    id: 2,
                    start: 0,
                    end: 3
                }
            ]
        );
    }
    #[test]
    fn stale_generation_revokes_work() {
        let epoch = Arc::new(AtomicU64::new(5));
        let token = Cancellation::new(epoch.clone(), 5);
        assert!(token.check().is_ok());
        epoch.store(6, Ordering::Release);
        assert!(token.check().is_err());
    }
    #[test]
    fn invalid_timing_is_rejected() {
        let audio = SynthesizedAudio {
            samples: vec![0.0; 100],
            sample_rate: 24_000,
            boundaries: vec![TimedBoundary {
                start_sample: 90,
                end_sample: 110,
                ranges: vec![],
                precision: BoundaryPrecision::Word,
            }],
        };
        assert!(audio.validate().is_err());
    }
    #[test]
    fn legacy_irodori_profiles_get_a_stable_voice_seed_without_losing_paths() {
        let config:ProviderConfig=serde_json::from_str(r#"{"kind":"irodori","runtime_dir":"runtime","model_path":"model.gguf","backend":"cpu","auto_download":false}"#).unwrap();
        assert!(
            matches!(config,ProviderConfig::Irodori{seed:42,reference_audio:None,voice_instruction,..} if voice_instruction.is_empty())
        );
    }
    #[test]
    fn kokoro_default_uses_a_packaged_japanese_voice() {
        let ProviderConfig::Kokoro {
            voice,
            allow_nvidia_download,
            ..
        } = ProviderConfig::kokoro_default()
        else {
            panic!()
        };
        assert!(!allow_nvidia_download);
        assert_eq!(voice, "jf_alpha");
        assert!(kokoro_voices().iter().any(|(id, _)| id == &voice));
        assert_eq!(kokoro_voices().len(), 5);
    }
    #[test]
    fn existing_kokoro_profiles_do_not_implicitly_accept_nvidia_terms() {
        let config: ProviderConfig = serde_json::from_str(
            r#"{"kind":"kokoro","runtime_dir":"runtime","model_path":"model.onnx","backend":"auto","auto_download":true,"voice":"jm_kumo"}"#,
        ).unwrap();
        assert!(
            matches!(config, ProviderConfig::Kokoro { allow_nvidia_download:false, auto_download:true, voice, .. } if voice == "jm_kumo")
        );
    }
}
