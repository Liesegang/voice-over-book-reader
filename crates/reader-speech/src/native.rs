//! Pinned audio.cpp C ABI integration. Native inference stays warm across requests.
use crate::{providers::TtsProvider, types::*};
use anyhow::{Context, Result, bail, ensure};
use libloading::Library;
use sha2::{Digest, Sha256};
use std::{
    ffi::{CStr, CString, c_char, c_void},
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    ptr,
    sync::Arc,
    time::Instant,
};

const MODEL_URL: &str = "https://huggingface.co/audio-cpp/audio.cpp-gguf/resolve/462b4ce/Irodori-TTS-v4-Small-GGUF/irodori-tts-v4.1-small-mf-q8_0.gguf";
const MODEL_SHA: &str = "a450c277a29b771bde3ceb21fd753bfb9a61c93aef952e0a3b20ad8d3facbbcd";
const MODEL_SIZE: u64 = 1_368_276_320;
const ALIGNER_URL: &str = "https://huggingface.co/audio-cpp/audio.cpp-gguf/resolve/462b4ce/Qwen3-ForcedAligner-0.6B-GGUF/qwen3-forced-aligner-0.6b-q8_0.gguf";
const ALIGNER_SHA: &str = "bf4fec616f77a20d89a649cd1c97bda269644eeb3cf906d1f7488a52d71f7cb2";
const ALIGNER_SIZE: u64 = 1_129_969_856;
const KOKORO_URL: &str = "https://huggingface.co/audio-cpp/audio.cpp-gguf/resolve/462b4cec041e06ffb3d8fcf0d8c655051b1cd9f5/Kokoro-82M-GGUF/kokoro-82m-multilingual-q8_0.gguf";
const KOKORO_SHA: &str = "c6864b5b66e94c83b94c45ad71f6cd1a445f25db19cfc0e59de28c694424ab18";
const KOKORO_SIZE: u64 = 932_617_696;
pub const IRODORI_VOICE_SAMPLE_TEXT: &str =
    "これから本を読みます。窓の外には、やわらかな朝の光が広がっています。";

/// The same generated voice is used across sentences, books, and application restarts.
pub fn irodori_reference_path(config: &ProviderConfig) -> Option<PathBuf> {
    let ProviderConfig::Irodori {
        model_path,
        reference_audio,
        voice_instruction,
        seed,
        ..
    } = config
    else {
        return None;
    };
    if let Some(path) = reference_audio {
        return Some(path.clone());
    }
    let mut hash = Sha256::new();
    hash.update(b"yomi-fixed-irodori-voice-v1");
    hash.update(model_path.to_string_lossy().as_bytes());
    hash.update(voice_instruction.as_bytes());
    hash.update(seed.to_le_bytes());
    Some(
        data_dir()
            .join("speech/voices")
            .join(format!("{:x}.wav", hash.finalize())),
    )
}

#[repr(C)]
struct ModelConfig {
    family: *const c_char,
    config: *const c_char,
    weight: *const c_char,
    model_spec: *const c_char,
}
#[repr(C)]
struct BackendConfig {
    backend: *const c_char,
    device: i32,
    threads: i32,
}
type Handle = *mut c_void;

struct Api {
    _library: Library,
    registry_create: unsafe extern "C" fn(*const c_char, *mut Handle) -> i32,
    registry_free: unsafe extern "C" fn(Handle),
    model_load:
        unsafe extern "C" fn(Handle, *const c_char, *const ModelConfig, Handle, *mut Handle) -> i32,
    model_free: unsafe extern "C" fn(Handle),
    session_create: unsafe extern "C" fn(
        Handle,
        *const c_char,
        *const c_char,
        *const BackendConfig,
        Handle,
        *mut Handle,
    ) -> i32,
    session_free: unsafe extern "C" fn(Handle),
    request_create: unsafe extern "C" fn() -> Handle,
    request_free: unsafe extern "C" fn(Handle),
    request_text: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> i32,
    request_text_language: Option<unsafe extern "C" fn(Handle, *const c_char) -> i32>,
    request_option: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> i32,
    request_audio: unsafe extern "C" fn(Handle, *const f32, usize, i32, i32) -> i32,
    request_voice: unsafe extern "C" fn(Handle, *const f32, usize, i32, i32) -> i32,
    session_run: unsafe extern "C" fn(Handle, Handle, *mut Handle) -> i32,
    result_free: unsafe extern "C" fn(Handle),
    result_audio:
        unsafe extern "C" fn(Handle, *mut *const f32, *mut usize, *mut i32, *mut i32) -> i32,
    word_count: unsafe extern "C" fn(Handle) -> usize,
    word: unsafe extern "C" fn(
        Handle,
        usize,
        *mut *const c_char,
        *mut i64,
        *mut i64,
        *mut f32,
    ) -> i32,
    last_error: unsafe extern "C" fn() -> *const c_char,
}

impl Api {
    fn load(path: &Path) -> Result<Arc<Self>> {
        // Dependencies are loaded from the selected, hash-verified runtime's DLL directory.
        #[cfg(windows)]
        let library: Library = unsafe {
            libloading::os::windows::Library::load_with_flags(path, 0x00000100 | 0x00001000)?.into()
        };
        #[cfg(not(windows))]
        let library = unsafe { Library::new(path)? };
        macro_rules! symbol {
            ($name:literal, $ty:ty) => {
                unsafe {
                    *library
                        .get::<$ty>(concat!($name, "\0").as_bytes())
                        .context(concat!("Missing audio.cpp API: ", $name))?
                }
            };
        }
        let abi = symbol!("audiocpp_abi_version", unsafe extern "C" fn() -> u32);
        ensure!((unsafe { abi() } >> 16) == 0, "Unsupported audio.cpp ABI");
        Ok(Arc::new(Self {
            registry_create: symbol!(
                "audiocpp_registry_create",
                unsafe extern "C" fn(*const c_char, *mut Handle) -> i32
            ),
            registry_free: symbol!("audiocpp_registry_free", unsafe extern "C" fn(Handle)),
            model_load: symbol!(
                "audiocpp_model_load",
                unsafe extern "C" fn(
                    Handle,
                    *const c_char,
                    *const ModelConfig,
                    Handle,
                    *mut Handle,
                ) -> i32
            ),
            model_free: symbol!("audiocpp_model_free", unsafe extern "C" fn(Handle)),
            session_create: symbol!(
                "audiocpp_session_create",
                unsafe extern "C" fn(
                    Handle,
                    *const c_char,
                    *const c_char,
                    *const BackendConfig,
                    Handle,
                    *mut Handle,
                ) -> i32
            ),
            session_free: symbol!("audiocpp_session_free", unsafe extern "C" fn(Handle)),
            request_create: symbol!("audiocpp_request_create", unsafe extern "C" fn() -> Handle),
            request_free: symbol!("audiocpp_request_free", unsafe extern "C" fn(Handle)),
            request_text: symbol!(
                "audiocpp_request_set_text",
                unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> i32
            ),
            request_text_language: unsafe {
                library
                    .get::<unsafe extern "C" fn(Handle, *const c_char) -> i32>(
                        b"audiocpp_request_set_text_language\0",
                    )
                    .ok()
                    .map(|f| *f)
            },
            request_option: symbol!(
                "audiocpp_request_set_option",
                unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> i32
            ),
            request_audio: symbol!(
                "audiocpp_request_set_audio",
                unsafe extern "C" fn(Handle, *const f32, usize, i32, i32) -> i32
            ),
            request_voice: symbol!(
                "audiocpp_request_set_voice_audio",
                unsafe extern "C" fn(Handle, *const f32, usize, i32, i32) -> i32
            ),
            session_run: symbol!(
                "audiocpp_session_run",
                unsafe extern "C" fn(Handle, Handle, *mut Handle) -> i32
            ),
            result_free: symbol!("audiocpp_result_free", unsafe extern "C" fn(Handle)),
            result_audio: symbol!(
                "audiocpp_result_audio",
                unsafe extern "C" fn(
                    Handle,
                    *mut *const f32,
                    *mut usize,
                    *mut i32,
                    *mut i32,
                ) -> i32
            ),
            word_count: symbol!(
                "audiocpp_result_word_count",
                unsafe extern "C" fn(Handle) -> usize
            ),
            word: symbol!(
                "audiocpp_result_word",
                unsafe extern "C" fn(
                    Handle,
                    usize,
                    *mut *const c_char,
                    *mut i64,
                    *mut i64,
                    *mut f32,
                ) -> i32
            ),
            last_error: symbol!(
                "audiocpp_last_error",
                unsafe extern "C" fn() -> *const c_char
            ),
            _library: library,
        }))
    }
    fn check(&self, status: i32) -> Result<()> {
        if status != 0 {
            let message = unsafe { CStr::from_ptr((self.last_error)()) }.to_string_lossy();
            bail!("audio.cpp ({status}): {message}");
        }
        Ok(())
    }
}

struct Session {
    api: Arc<Api>,
    registry: Handle,
    model: Handle,
    session: Handle,
}
// A session has exactly one owner, the synthesis thread; no concurrent calls or handles escape it.
unsafe impl Send for Session {}
impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            (self.api.session_free)(self.session);
            (self.api.model_free)(self.model);
            (self.api.registry_free)(self.registry);
        }
    }
}
struct Request {
    api: Arc<Api>,
    handle: Handle,
}
impl Drop for Request {
    fn drop(&mut self) {
        unsafe { (self.api.request_free)(self.handle) }
    }
}
struct Output {
    api: Arc<Api>,
    handle: Handle,
}
impl Drop for Output {
    fn drop(&mut self) {
        unsafe { (self.api.result_free)(self.handle) }
    }
}

impl Session {
    fn new(
        api: Arc<Api>,
        model_path: &Path,
        family: &str,
        task: &str,
        backend: &str,
    ) -> Result<Self> {
        let mut value = Self {
            api,
            registry: ptr::null_mut(),
            model: ptr::null_mut(),
            session: ptr::null_mut(),
        };
        let path = CString::new(model_path.to_string_lossy().as_bytes())?;
        let family = CString::new(family)?;
        let task = CString::new(task)?;
        let backend_name = CString::new(backend)?;
        let config = ModelConfig {
            family: family.as_ptr(),
            config: ptr::null(),
            weight: ptr::null(),
            model_spec: ptr::null(),
        };
        let backend = BackendConfig {
            backend: backend_name.as_ptr(),
            device: 0,
            threads: std::thread::available_parallelism().map_or(4, |n| n.get().min(8)) as i32,
        };
        unsafe {
            value.api.check((value.api.registry_create)(
                ptr::null(),
                &mut value.registry,
            ))?;
            value.api.check((value.api.model_load)(
                value.registry,
                path.as_ptr(),
                &config,
                ptr::null_mut(),
                &mut value.model,
            ))?;
            value.api.check((value.api.session_create)(
                value.model,
                task.as_ptr(),
                c"offline".as_ptr(),
                &backend,
                ptr::null_mut(),
                &mut value.session,
            ))?;
        }
        Ok(value)
    }
    fn request(&self, text: &str, language: &str, alignment: bool) -> Result<Request> {
        let handle = unsafe { (self.api.request_create)() };
        ensure!(!handle.is_null(), "audio.cpp could not allocate request");
        let request = Request {
            api: self.api.clone(),
            handle,
        };
        let text = CString::new(text)?;
        let language = CString::new(language)?;
        unsafe {
            self.api.check((self.api.request_text)(
                handle,
                text.as_ptr(),
                if alignment {
                    ptr::null()
                } else {
                    language.as_ptr()
                },
            ))?;
            if alignment {
                let set_language = self
                    .api
                    .request_text_language
                    .context("audio.cppランタイムにalignment言語APIがありません")?;
                self.api.check(set_language(handle, language.as_ptr()))?;
            }
        }
        Ok(request)
    }
    fn option(&self, request: &Request, key: &str, value: &str) -> Result<()> {
        let key = CString::new(key)?;
        let value = CString::new(value)?;
        self.api.check(unsafe {
            (self.api.request_option)(request.handle, key.as_ptr(), value.as_ptr())
        })
    }
    fn run(&self, request: &Request) -> Result<Output> {
        let mut handle = ptr::null_mut();
        self.api
            .check(unsafe { (self.api.session_run)(self.session, request.handle, &mut handle) })?;
        ensure!(!handle.is_null(), "audio.cpp returned a null result");
        Ok(Output {
            api: self.api.clone(),
            handle,
        })
    }
}

pub(crate) struct IrodoriProvider {
    config: ProviderConfig,
    speech: Option<Session>,
    aligner: Option<Session>,
    reference: Option<(Vec<f32>, u32)>,
}
impl IrodoriProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            config,
            speech: None,
            aligner: None,
            reference: None,
        }
    }
}
impl TtsProvider for IrodoriProvider {
    fn name(&self) -> &str {
        "Irodori TTS v4.1 Small MeanFlow"
    }
    fn synthesize(
        &mut self,
        units: &[SpeechUnit],
        cancel: &Cancellation,
        progress: &dyn Fn(String),
    ) -> Result<SynthesizedAudio> {
        let ProviderConfig::Irodori {
            runtime_dir,
            model_path,
            backend,
            auto_download,
            aligner_path,
            reference_audio: _,
            voice_instruction,
            seed,
        } = &self.config
        else {
            unreachable!()
        };
        cancel.check()?;
        if self.speech.is_none() {
            if *auto_download {
                prepare_assets(runtime_dir, model_path, cancel, progress)?;
                if let Some(path) = aligner_path.as_ref().filter(|p| !p.is_file()) {
                    download(
                        ALIGNER_URL,
                        path,
                        ALIGNER_SIZE,
                        ALIGNER_SHA,
                        cancel,
                        progress,
                        "日本語の音声位置合わせモデル",
                    )?;
                }
            }
            ensure!(
                model_path.is_file(),
                "Irodoriモデルがありません: {}",
                model_path.display()
            );
            let library_path = find_library(runtime_dir)
                .context("audio.cppランタイムがありません。モデル準備を実行してください")?;
            progress("Irodoriの日本語モデルを読み込んでいます".into());
            let api = Api::load(&library_path).with_context(|| {
                format!(
                    "音声ランタイムをロードできません: {}",
                    library_path.display()
                )
            })?;
            let session = Session::new(api.clone(), model_path, "irodori_tts", "tts", backend)
                .or_else(|error| {
                    if backend == "cpu" {
                        return Err(error);
                    }
                    progress(format!("{backend}を利用できません。CPUで読み込みます"));
                    Session::new(api.clone(), model_path, "irodori_tts", "tts", "cpu")
                })?;
            if let Some(path) = aligner_path.as_ref().filter(|p| p.is_file()) {
                progress("日本語の音声位置合わせモデルを読み込んでいます".into());
                self.aligner = Some(Session::new(
                    api,
                    path,
                    "qwen3_forced_aligner",
                    "align",
                    backend,
                )?);
            }
            self.speech = Some(session);
        }
        cancel.check()?;
        if self.reference.is_none() {
            let reference_path = irodori_reference_path(&self.config).unwrap();
            let _reference_lock = if matches!(
                &self.config,
                ProviderConfig::Irodori {
                    reference_audio: None,
                    ..
                }
            ) {
                std::fs::create_dir_all(reference_path.parent().unwrap())?;
                let lock = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(reference_path.with_extension("voice.lock"))?;
                lock.lock()?;
                Some(lock)
            } else {
                None
            };
            cancel.check()?;
            if reference_path.is_file() {
                ensure!(
                    reference_path.metadata()?.len() <= 128 * 1024 * 1024,
                    "参照音声が大きすぎます（最大128MiB）"
                );
                let (mut samples, rate) =
                    crate::providers::decode_wav(&std::fs::read(&reference_path)?)?;
                samples.truncate(rate as usize * 120);
                ensure!(
                    samples.len() >= rate as usize / 2,
                    "参照音声は0.5秒以上のWAVを指定してください"
                );
                self.reference = Some((samples, rate));
            } else {
                if matches!(
                    &self.config,
                    ProviderConfig::Irodori {
                        reference_audio: Some(_),
                        ..
                    }
                ) {
                    bail!("参照音声がありません: {}", reference_path.display());
                }
                progress("選択した声のサンプルを一度だけ準備しています".into());
                let session = self.speech.as_ref().unwrap();
                let request = session.request(IRODORI_VOICE_SAMPLE_TEXT, "ja", false)?;
                session.option(&request, "no_ref", "true")?;
                session.option(&request, "seed", &seed.to_string())?;
                if !voice_instruction.trim().is_empty() {
                    session.option(&request, "instruct", voice_instruction)?;
                }
                let result = session.run(&request)?;
                let (samples, rate) = result_pcm(session, &result)?;
                write_voice_wav(&reference_path, &samples, rate)?;
                self.reference = Some((samples, rate));
            }
        }
        cancel.check()?;
        progress("Irodoriで音声を生成しています".into());
        let text: String = units.iter().map(|u| u.text.as_str()).collect();
        let session = self.speech.as_ref().unwrap();
        let request = session.request(&text, "ja", false)?;
        session.option(&request, "seed", &seed.to_string())?;
        session.option(&request, "no_ref", "false")?;
        let (reference, rate) = self.reference.as_ref().unwrap();
        session.api.check(unsafe {
            (session.api.request_voice)(
                request.handle,
                reference.as_ptr(),
                reference.len(),
                *rate as i32,
                1,
            )
        })?;
        let output = session.run(&request)?;
        let mut samples = ptr::null();
        let mut frames = 0;
        let mut sample_rate = 0;
        let mut channels = 0;
        session.api.check(unsafe {
            (session.api.result_audio)(
                output.handle,
                &mut samples,
                &mut frames,
                &mut sample_rate,
                &mut channels,
            )
        })?;
        ensure!(
            !samples.is_null()
                && channels > 0
                && channels <= 8
                && (8000..=192000).contains(&sample_rate)
                && frames > 0
                && frames <= sample_rate as usize * 600,
            "Irodori returned invalid PCM"
        );
        let interleaved =
            unsafe { std::slice::from_raw_parts(samples, frames * channels as usize) };
        let mono: Vec<f32> = interleaved
            .chunks_exact(channels as usize)
            .map(|f| f.iter().sum::<f32>() / channels as f32)
            .collect();
        let mut audio = SynthesizedAudio {
            samples: mono,
            sample_rate: sample_rate as u32,
            boundaries: vec![TimedBoundary {
                start_sample: 0,
                end_sample: frames as u64,
                ranges: unit_ranges(units),
                precision: BoundaryPrecision::Chunk,
            }],
        };
        if let Some(aligner) = &self.aligner {
            progress("生成した音声と本文を位置合わせしています".into());
            // Upstream supplies real group timings. This path does not claim Nagisa/word parity.
            // Provider group labels are located monotonically in the *spoken* text only.
            match align_groups(aligner, &text, units, &audio) {
                Ok(boundaries) if !boundaries.is_empty() => audio.boundaries = boundaries,
                Ok(_) => progress("細かい時刻を取得できないため、文単位でハイライトします".into()),
                Err(error) => progress(format!(
                    "位置合わせを利用できないため文単位で表示します: {error}"
                )),
            }
        }
        audio.validate()?;
        Ok(audio)
    }
}

/// Match the pinned audio.cpp processor's clean_token(), preserving original
/// UTF-8 source bytes. Japanese kana groups may cross removed punctuation.
/// https://github.com/0xShug0/audio.cpp/blob/v0.9.3/src/models/qwen3_forced_aligner/processor.cpp
struct AlignableText {
    text: String,
    source_starts: Vec<usize>,
    source_ends: Vec<usize>,
}
impl AlignableText {
    fn new(source: &str) -> Self {
        let mut normalized = Self {
            text: String::new(),
            source_starts: Vec::new(),
            source_ends: Vec::new(),
        };
        for (start, character) in source.char_indices() {
            let code = character as u32;
            let kept = character == '\''
                || character.is_ascii_alphanumeric()
                || matches!(code,
                0x00C0..=0x02AF | 0x0370..=0x03FF | 0x0400..=0x052F |
                0x0E00..=0x0E7F | 0x3040..=0x30FF | 0xAC00..=0xD7AF |
                0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x20000..=0x2A6DF |
                0x2A700..=0x2B73F | 0x2B740..=0x2B81F | 0x2B820..=0x2CEAF |
                0xF900..=0xFAFF);
            if kept {
                normalized.text.push(character);
                for _ in 0..character.len_utf8() {
                    normalized.source_starts.push(start);
                    normalized.source_ends.push(start + character.len_utf8());
                }
            }
        }
        normalized
    }
    fn source_range(&self, start: usize, end: usize) -> Result<std::ops::Range<usize>> {
        ensure!(
            start < end
                && end <= self.text.len()
                && self.text.is_char_boundary(start)
                && self.text.is_char_boundary(end),
            "Alignment returned an invalid normalized text range"
        );
        Ok(self.source_starts[start]..self.source_ends[end - 1])
    }
}

fn align_groups(
    session: &Session,
    text: &str,
    units: &[SpeechUnit],
    audio: &SynthesizedAudio,
) -> Result<Vec<TimedBoundary>> {
    let request = session.request(text, "Japanese", true)?;
    let aligned_pcm = crate::audio::resample(&audio.samples, audio.sample_rate, 16_000);
    session.api.check(unsafe {
        (session.api.request_audio)(
            request.handle,
            aligned_pcm.as_ptr(),
            aligned_pcm.len(),
            16_000,
            1,
        )
    })?;
    let output = session.run(&request)?;
    let count = unsafe { (session.api.word_count)(output.handle) };
    ensure!(count <= 10_000, "Too many alignment groups");
    let normalized = AlignableText::new(text);
    let mut offset = 0;
    let mut previous_end = 0;
    let mut boundaries = Vec::new();
    for i in 0..count {
        let mut word = ptr::null();
        let mut start = 0;
        let mut end = 0;
        let mut confidence = 0.0;
        session.api.check(unsafe {
            (session.api.word)(
                output.handle,
                i,
                &mut word,
                &mut start,
                &mut end,
                &mut confidence,
            )
        })?;
        let word = unsafe { CStr::from_ptr(word) }.to_str()?;
        let relative = normalized.text[offset..].find(word).with_context(|| {
            format!("Alignment label {word:?} did not match spoken text at byte {offset}")
        })?;
        let from = offset + relative;
        let to = from + word.len();
        offset = to;
        let source = normalized.source_range(from, to)?;
        // This model returns timestamps in its 16kHz sample clock.
        ensure!(
            start >= previous_end && end > start && end <= aligned_pcm.len() as i64,
            "Alignment group has invalid or nonmonotonic timing"
        );
        previous_end = end;
        boundaries.push(TimedBoundary {
            start_sample: start as u64 * audio.sample_rate as u64 / 16_000,
            end_sample: (end as u64 * audio.sample_rate as u64 / 16_000)
                .min(audio.samples.len() as u64),
            ranges: ranges_for_text(units, source.start, source.end),
            precision: BoundaryPrecision::AlignedGroup,
        });
    }
    Ok(boundaries)
}

fn result_pcm(session: &Session, output: &Output) -> Result<(Vec<f32>, u32)> {
    let mut data = ptr::null();
    let mut frames = 0;
    let mut rate = 0;
    let mut channels = 0;
    session.api.check(unsafe {
        (session.api.result_audio)(
            output.handle,
            &mut data,
            &mut frames,
            &mut rate,
            &mut channels,
        )
    })?;
    ensure!(
        !data.is_null()
            && channels > 0
            && channels <= 8
            && (8000..=192000).contains(&rate)
            && frames > 0
            && frames <= rate as usize * 600,
        "音声モデルが不正なPCMを返しました"
    );
    let samples = unsafe { std::slice::from_raw_parts(data, frames * channels as usize) };
    Ok((
        samples
            .chunks_exact(channels as usize)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect(),
        rate as u32,
    ))
}

pub(crate) struct NativeAligner {
    session: Session,
}
impl NativeAligner {
    pub fn load(runtime: &Path, model: &Path) -> Result<Self> {
        let api = Api::load(&find_library(runtime).context("位置合わせランタイムがありません")?)?;
        let backend = if cfg!(windows) {
            "vulkan"
        } else if cfg!(target_os = "macos") {
            "metal"
        } else {
            "cpu"
        };
        let session = Session::new(api.clone(), model, "qwen3_forced_aligner", "align", backend)
            .or_else(|_| Session::new(api, model, "qwen3_forced_aligner", "align", "cpu"))?;
        Ok(Self { session })
    }
    pub fn align(
        &self,
        text: &str,
        units: &[SpeechUnit],
        audio: &SynthesizedAudio,
    ) -> Result<Vec<TimedBoundary>> {
        align_groups(&self.session, text, units, audio)
    }
}

pub(crate) fn prepare_aligner(
    runtime: &Path,
    model: &Path,
    auto: bool,
    cancel: &Cancellation,
    progress: &dyn Fn(String),
) -> Result<()> {
    if auto {
        prepare_runtime(runtime, cancel, progress)?;
        if !model.is_file() {
            download(
                ALIGNER_URL,
                model,
                ALIGNER_SIZE,
                ALIGNER_SHA,
                cancel,
                progress,
                "日本語の音声位置合わせモデル",
            )?;
        }
    }
    Ok(())
}

fn write_voice_wav(path: &Path, samples: &[f32], rate: u32) -> Result<()> {
    let parent = path.parent().context("Voice path needs a parent")?;
    std::fs::create_dir_all(parent)?;
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut wav = hound::WavWriter::create(
        temporary.path(),
        hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )?;
    for sample in samples {
        wav.write_sample(*sample)?;
    }
    wav.finalize()?;
    temporary.persist(path)?;
    Ok(())
}

pub(crate) struct KokoroProvider {
    config: ProviderConfig,
    speech: Option<Session>,
    aligner: Option<Session>,
}
impl KokoroProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            config,
            speech: None,
            aligner: None,
        }
    }
}
impl TtsProvider for KokoroProvider {
    fn name(&self) -> &str {
        "Kokoro 82M · native Japanese"
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
            voice,
            aligner_path,
            ..
        } = &self.config
        else {
            unreachable!()
        };
        ensure!(
            kokoro_voices().iter().any(|(id, _)| id == voice),
            "Kokoroの日本語の声を選択してください"
        );
        cancel.check()?;
        if self.speech.is_none() {
            if *auto_download {
                crate::kokoro_notices::retain(runtime_dir)?;
                prepare_runtime(runtime_dir, cancel, progress)?;
                if !model_path.is_file() {
                    download(
                        KOKORO_URL,
                        model_path,
                        KOKORO_SIZE,
                        KOKORO_SHA,
                        cancel,
                        progress,
                        "Kokoroの日本語音声モデル",
                    )?;
                }
                if let Some(path) = aligner_path.as_ref().filter(|path| !path.is_file()) {
                    download(
                        ALIGNER_URL,
                        path,
                        ALIGNER_SIZE,
                        ALIGNER_SHA,
                        cancel,
                        progress,
                        "日本語の音声位置合わせモデル",
                    )?;
                }
            }
            ensure!(
                model_path.is_file(),
                "Kokoroモデルがありません: {}",
                model_path.display()
            );
            prepare_mecab(runtime_dir, *auto_download, cancel, progress)?;
            progress("Kokoroの音声モデルを読み込んでいます".into());
            let api =
                Api::load(&find_library(runtime_dir).context("Kokoro推論ランタイムがありません")?)?;
            self.speech = Some(Session::new(
                api.clone(),
                model_path,
                "kokoro_tts",
                "tts",
                backend,
            )?);
            if let Some(path) = aligner_path.as_ref().filter(|path| path.is_file()) {
                let alignment_backend = if cfg!(windows) {
                    "vulkan"
                } else if cfg!(target_os = "macos") {
                    "metal"
                } else {
                    "cpu"
                };
                self.aligner = Some(Session::new(
                    api,
                    path,
                    "qwen3_forced_aligner",
                    "align",
                    alignment_backend,
                )?);
            }
        }
        cancel.check()?;
        let text: String = units.iter().map(|unit| unit.text.as_str()).collect();
        let session = self.speech.as_ref().unwrap();
        let request = session.request(&text, "ja", false)?;
        session.option(&request, "voice_id", voice)?;
        session.option(&request, "seed", "42")?;
        session.option(&request, "text_chunk_size", "100")?;
        let output = session.run(&request)?;
        let (samples, sample_rate) = result_pcm(session, &output)?;
        cancel.check()?;
        let mut audio = SynthesizedAudio {
            boundaries: vec![TimedBoundary {
                start_sample: 0,
                end_sample: samples.len() as u64,
                ranges: unit_ranges(units),
                precision: BoundaryPrecision::Chunk,
            }],
            samples,
            sample_rate,
        };
        if let Some(aligner) = &self.aligner
            && let Ok(boundaries) = align_groups(aligner, &text, units, &audio)
            && !boundaries.is_empty()
        {
            audio.boundaries = boundaries;
        }
        audio.validate()?;
        cancel.check()?;
        Ok(audio)
    }
}

pub(crate) fn prepare_mecab(
    runtime_dir: &Path,
    auto_download: bool,
    cancel: &Cancellation,
    progress: &dyn Fn(String),
) -> Result<()> {
    #[cfg(windows)]
    {
        if let Some(path) = std::env::var_os("AUDIOCPP_MECAB_LIBRARY") {
            return set_native_mecab_environment(Path::new(&path));
        }
        let root = runtime_dir.join("mecab-1.5.2");
        std::fs::create_dir_all(&root)?;
        let setup = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join("prepare.lock"))?;
        setup.lock()?;
        cancel.check()?;
        let library = root.join("libmecab.dll");
        if !library.is_file() && auto_download {
            let archive = root.join("fugashi-1.5.2-win-amd64.whl");
            download(
                "https://files.pythonhosted.org/packages/d7/ce/b18879c94c6267981a65792045321a1d71b849893b40d7e8356e0b55542c/fugashi-1.5.2-cp312-cp312-win_amd64.whl",
                &archive,
                508_407,
                "936d710166c5b05064ec2ce0eb347fff7a0cf102c33989012fad205346943402",
                cancel,
                progress,
                "日本語の発音ライブラリ",
            )?;
            extract_mecab(&archive, &root, &library)?;
        }
        ensure!(
            library.is_file(),
            "Kokoro日本語用のMeCabライブラリがありません"
        );
        set_native_mecab_environment(&library)?;
    }
    #[cfg(not(windows))]
    {
        let _ = (runtime_dir, auto_download, cancel, progress);
    }
    Ok(())
}

/// The Rust frontend takes an explicit library path, so Unix never needs to
/// mutate the process environment while other reader threads are running.
pub(crate) fn kokoro_mecab_library(
    runtime: &Path,
    auto: bool,
    cancel: &Cancellation,
    progress: &dyn Fn(String),
) -> Result<PathBuf> {
    #[cfg(windows)]
    {
        prepare_mecab(runtime, auto, cancel, progress)?;
        Ok(std::env::var_os("AUDIOCPP_MECAB_LIBRARY")
            .context("MeCab library not prepared")?
            .into())
    }
    #[cfg(not(windows))]
    {
        if let Some(path) = std::env::var_os("AUDIOCPP_MECAB_LIBRARY") {
            return Ok(path.into());
        }
        let (name, size, sha, url, filename) = if cfg!(all(
            target_os = "linux",
            target_arch = "x86_64"
        )) {
            (
                "fugashi-1.5.2-linux-x64.whl",
                694_924,
                "83bc7bf08f81a3c3992bf10b8c681720898a826c6c3dffa80e1296e005f4bfb8",
                "https://files.pythonhosted.org/packages/cd/30/436dd468ac8e08940f0414384a5808596c2ed8cbfd721dde09d5b78e8ec5/fugashi-1.5.2-cp312-cp312-manylinux2014_x86_64.manylinux_2_17_x86_64.whl",
                "libmecab.so.2",
            )
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            (
                "fugashi-1.5.2-macos-arm64.whl",
                455_098,
                "8c1f64345a7a13b229fb755b567cbc993adb43b5b617ad4089521e5dd4d27b91",
                "https://files.pythonhosted.org/packages/42/63/e5e02d885d3ea3eeba7f3be371164eb35f618155faa473f0950cbba2d276/fugashi-1.5.2-cp312-cp312-macosx_11_0_arm64.whl",
                "libmecab.2.dylib",
            )
        } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            (
                "fugashi-1.5.2-macos-x64.whl",
                457_118,
                "3d2bb28cc6c6eec1c50729bb2dda44007a45599f0471b14c8fda57b0dde36d50",
                "https://files.pythonhosted.org/packages/65/e2/d8fbb71b3e04fe8e99bd7b2653ac638484d8ab46b22cab6ea64250a020ce/fugashi-1.5.2-cp312-cp312-macosx_10_13_x86_64.whl",
                "libmecab.2.dylib",
            )
        } else {
            return Ok("libmecab.so.2".into());
        };
        let root = runtime.join("mecab-1.5.2");
        std::fs::create_dir_all(&root)?;
        let setup = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join("prepare.lock"))?;
        setup.lock()?;
        cancel.check()?;
        let library = root.join(filename);
        if !library.is_file() {
            ensure!(
                auto,
                "MeCabがありません。自動取得を有効にするかAUDIOCPP_MECAB_LIBRARYを指定してください"
            );
            let archive = root.join(name);
            download(
                url,
                &archive,
                size,
                sha,
                cancel,
                progress,
                "日本語の発音ライブラリ",
            )?;
            extract_mecab(&archive, &root, &library)?;
        }
        Ok(library)
    }
}

fn extract_mecab(archive: &Path, root: &Path, library: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(File::open(archive)?)?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let name = entry.name();
        let target = if (name.starts_with("fugashi.libs/") || name.starts_with("fugashi/.dylibs/"))
            && name.contains("libmecab")
        {
            Some(library.to_owned())
        } else if name.ends_with("licenses/LICENSE.mecab") {
            Some(root.join("LICENSE.mecab"))
        } else if name.ends_with("licenses/LICENSE") {
            Some(root.join("LICENSE.fugashi"))
        } else {
            None
        };
        if let Some(target) = target {
            ensure!(entry.size() <= 16 * 1024 * 1024, "Oversize MeCab library");
            std::fs::create_dir_all(root)?;
            let mut temporary = tempfile::NamedTempFile::new_in(root)?;
            std::io::copy(&mut entry, &mut temporary)?;
            temporary.persist(target)?;
        }
    }
    ensure!(
        library.is_file(),
        "MeCabのライブラリがパッケージにありません"
    );
    Ok(())
}

#[cfg(windows)]
fn set_native_mecab_environment(path: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    type SetEnvironment = unsafe extern "C" fn(*const u16, *const u16) -> i32;
    let name: Vec<u16> = "AUDIOCPP_MECAB_LIBRARY"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let value: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // The native library reads the UCRT environment, whereas std::env on
    // Windows changes the Win32 environment only. Synchronize both once before
    // loading the phonemizer. Unix uses its installed soname; no setenv there.
    unsafe {
        std::env::set_var("AUDIOCPP_MECAB_LIBRARY", path);
        // Update the native DLL's shared UCRT, even when the Rust application
        // uses a static CRT. An import-library link would mix both CRTs in the
        // application and fail when CEF's /MT wrapper is linked into the DLL.
        let runtime = libloading::os::windows::Library::load_with_flags(
            "ucrtbase.dll",
            libloading::os::windows::LOAD_LIBRARY_SEARCH_SYSTEM32,
        )?;
        let set_environment = runtime.get::<SetEnvironment>(b"_wputenv_s\0")?;
        ensure!(
            set_environment(name.as_ptr(), value.as_ptr()) == 0,
            "MeCabの環境設定を初期化できません"
        );
    }
    Ok(())
}

fn find_library(root: &Path) -> Option<PathBuf> {
    let compatible = compatible_runtime_dir(root);
    let name = if cfg!(windows) {
        "audiocpp.dll"
    } else if cfg!(target_os = "macos") {
        "libaudiocpp.dylib"
    } else {
        "libaudiocpp.so"
    };
    fn visit(root: &Path, name: &str, depth: u8) -> Option<PathBuf> {
        if depth > 5 {
            return None;
        }
        for entry in std::fs::read_dir(root).ok()?.flatten() {
            let path = entry.path();
            if path.is_file() && path.file_name().is_some_and(|n| n == name) {
                return Some(path);
            }
            if path.is_dir()
                && let Some(found) = visit(&path, name, depth + 1)
            {
                return Some(found);
            }
        }
        None
    }
    visit(&compatible, name, 0)
}

fn compatible_runtime_dir(root: &Path) -> PathBuf {
    // The upstream nonportable Windows library emits instructions unavailable
    // on some AVX2 CPUs (including Ryzen 3900X). Migrate the former default
    // without deleting the user's existing runtime or custom runtimes.
    if cfg!(windows)
        && root
            .file_name()
            .is_some_and(|name| name == "audio.cpp-v0.9.3")
    {
        root.with_file_name("audio.cpp-v0.9.3-portable")
    } else {
        root.to_path_buf()
    }
}

pub(crate) fn prepare_runtime(
    runtime_dir: &Path,
    cancel: &Cancellation,
    progress: &dyn Fn(String),
) -> Result<()> {
    let compatible = compatible_runtime_dir(runtime_dir);
    let runtime_dir = compatible.as_path();
    std::fs::create_dir_all(runtime_dir)?;
    let runtime_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(runtime_dir.join("runtime.lock"))?;
    runtime_lock.lock()?;
    if find_library(runtime_dir).is_none() {
        let (archive, sha, size) = runtime_package()?;
        let url =
            format!("https://github.com/0xShug0/audio.cpp/releases/download/v0.9.3/{archive}");
        std::fs::create_dir_all(runtime_dir)?;
        let path = runtime_dir.join(archive);
        download(&url, &path, size, sha, cancel, progress, "音声ランタイム")?;
        progress("音声ランタイムを展開しています".into());
        if archive.ends_with(".zip") {
            let mut zip = zip::ZipArchive::new(File::open(&path)?)?;
            for i in 0..zip.len() {
                cancel.check()?;
                let mut item = zip.by_index(i)?;
                let relative = item
                    .enclosed_name()
                    .context("Runtime archive contains an invalid path")?;
                let target = runtime_dir.join(relative);
                if item.is_dir() {
                    std::fs::create_dir_all(&target)?;
                } else {
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    let mut file = File::create(target)?;
                    std::io::copy(&mut item, &mut file)?;
                }
            }
        } else {
            let decoder = flate2::read::GzDecoder::new(File::open(&path)?);
            let mut tar = tar::Archive::new(decoder);
            tar.unpack(runtime_dir)?;
        }
        ensure!(
            find_library(runtime_dir).is_some(),
            "Runtime archive did not contain audio.cpp library"
        );
    }
    Ok(())
}

pub(crate) fn prepare_assets(
    runtime_dir: &Path,
    model_path: &Path,
    cancel: &Cancellation,
    progress: &dyn Fn(String),
) -> Result<()> {
    prepare_runtime(runtime_dir, cancel, progress)?;
    if !model_path.is_file() {
        download(
            MODEL_URL,
            model_path,
            MODEL_SIZE,
            MODEL_SHA,
            cancel,
            progress,
            "Irodori日本語モデル",
        )?;
    }
    ensure!(
        model_path.metadata()?.len() == MODEL_SIZE,
        "Irodoriモデルのサイズが不正です。ファイルを取り直してください"
    );
    Ok(())
}

fn runtime_package() -> Result<(&'static str, &'static str, u64)> {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Ok((
            "audio-v0.9.3-lib-windows-x64-vulkan-portable.zip",
            "b4e5a66e0bda38daabfd00c0b0273a23f9789766a777b5247203f1d1ba2c1572",
            29_850_039,
        ))
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Ok((
            "audio-v0.9.3-lib-macos-arm64-metal.tar.gz",
            "871d5e981be33abfc45aebd3cfe4858997ee298fd022a606fdb250ae1854495d",
            13_469_329,
        ))
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Ok((
            "audio-v0.9.3-lib-macos-x64-metal.tar.gz",
            "7328284cd69b4e1953eca811b906850320e6001beb4eea28e69b54cfd5c07106",
            14_349_788,
        ))
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Ok((
            "audio-v0.9.3-lib-ubuntu-x64-cpu-portable.tar.gz",
            "38c7b7875ebd748b4ece5c865812c7ad9c83ded37ef21172abc1c2b506d3fbb0",
            26_336_991,
        ))
    } else {
        bail!("このプラットフォームの推論ランタイム自動取得はまだ対応していません")
    }
}

pub(crate) fn download(
    url: &str,
    target: &Path,
    size: u64,
    sha: &str,
    cancel: &Cancellation,
    progress: &dyn Fn(String),
    label: &str,
) -> Result<()> {
    cancel.check()?;
    let parent = target
        .parent()
        .context("Asset path needs a parent directory")?;
    std::fs::create_dir_all(parent)?;
    let download_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(target.with_extension("asset.lock"))?;
    download_lock.lock()?;
    // Another reader/preview may have completed the same verified download.
    if target
        .metadata()
        .is_ok_and(|metadata| metadata.len() == size)
    {
        let mut existing = File::open(target)?;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 128 * 1024];
        loop {
            cancel.check()?;
            let amount = existing.read(&mut buffer)?;
            if amount == 0 {
                break;
            }
            hash.update(&buffer[..amount]);
        }
        if format!("{:x}", hash.finalize()) == sha {
            return Ok(());
        }
    }
    let partial = target.with_extension("download");
    let mut count = partial.metadata().map(|m| m.len()).unwrap_or(0);
    if count > size {
        count = 0;
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 128 * 1024];
    if count > 0 {
        let mut existing = File::open(&partial)?;
        loop {
            cancel.check()?;
            let length = existing.read(&mut buffer)?;
            if length == 0 {
                break;
            }
            hasher.update(&buffer[..length]);
        }
    }
    if count == size && format!("{:x}", hasher.clone().finalize()) == sha {
        install_download(&partial, target)?;
        return Ok(());
    }
    if count == size {
        count = 0;
        hasher = Sha256::new();
    }
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(20 * 60))
        .user_agent("epub-reader/0.1")
        .build()?;
    let mut request = client.get(url);
    if count > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={count}-"));
    }
    let mut response = request.send()?.error_for_status()?;
    let resume = count > 0 && response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    if !resume {
        count = 0;
        hasher = Sha256::new();
    }
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .append(resume)
        .truncate(!resume)
        .open(&partial)?;
    let mut last = Instant::now();
    progress(format!(
        "{label}をダウンロードしています（{}%）",
        count * 100 / size
    ));
    loop {
        cancel.check()?;
        let length = response.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        count += length as u64;
        ensure!(count <= size, "Downloaded asset exceeds expected size");
        output.write_all(&buffer[..length])?;
        hasher.update(&buffer[..length]);
        if last.elapsed().as_millis() >= 400 {
            progress(format!(
                "{label}をダウンロードしています（{}%）",
                count * 100 / size
            ));
            last = Instant::now();
        }
    }
    ensure!(
        count == size,
        "Asset download is incomplete ({count}/{size} bytes)"
    );
    ensure!(
        format!("{:x}", hasher.finalize()) == sha,
        "Asset SHA-256 verification failed"
    );
    output.sync_all()?;
    drop(output);
    install_download(&partial, target)?;
    Ok(())
}

fn install_download(partial: &Path, target: &Path) -> Result<()> {
    // Persist uses atomic replacement on Windows as well as Unix, without
    // copying a multi-gigabyte model. Keep a verified partial if install fails.
    match tempfile::TempPath::try_from_path(partial)?.persist(target) {
        Ok(()) => Ok(()),
        Err(error) => {
            let reason = error.error;
            let _ = error.path.keep();
            Err(reason.into())
        }
    }
}

#[cfg(test)]
mod voice_tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn native_mecab_environment_reaches_win32_and_shared_ucrt() {
        use std::os::windows::ffi::OsStringExt;
        if std::env::var_os("VOBR_TEST_NATIVE_CRT").as_deref() != Some(std::ffi::OsStr::new("1")) {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "native::voice_tests::native_mecab_environment_reaches_win32_and_shared_ucrt",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("VOBR_TEST_NATIVE_CRT", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "isolated UCRT test failed: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary
            .path()
            .join("音声ライブラリ𠮷野")
            .join("libmecab.dll");
        set_native_mecab_environment(&path).unwrap();
        assert_eq!(
            std::env::var_os("AUDIOCPP_MECAB_LIBRARY").unwrap(),
            path.as_os_str()
        );
        let name: Vec<u16> = "AUDIOCPP_MECAB_LIBRARY"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // The isolated child has no concurrent environment mutations. Keep the
        // shared UCRT loaded while inspecting its NUL-terminated wide string.
        unsafe {
            let runtime = libloading::os::windows::Library::load_with_flags(
                "ucrtbase.dll",
                libloading::os::windows::LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
            .unwrap();
            let get = runtime
                .get::<unsafe extern "C" fn(*const u16) -> *mut u16>(b"_wgetenv\0")
                .unwrap();
            let actual = get(name.as_ptr());
            assert!(
                !actual.is_null(),
                "shared UCRT did not receive MeCab environment"
            );
            let mut length = 0;
            while *actual.add(length) != 0 {
                length += 1;
            }
            assert_eq!(
                PathBuf::from(std::ffi::OsString::from_wide(std::slice::from_raw_parts(
                    actual, length
                ))),
                path
            );
        }
    }

    #[test]
    fn qwen_punctuation_cleaning_keeps_exact_source_ranges_across_units() {
        let units = [
            SpeechUnit {
                id: 42,
                text: "　とうきょうで読みます。".into(),
            },
            SpeechUnit {
                id: 99,
                text: "まどから光が入る。".into(),
            },
        ];
        let source = units
            .iter()
            .map(|unit| unit.text.as_str())
            .collect::<String>();
        let normalized = AlignableText::new(&source);
        let label = "みますまどから";
        let start = normalized.text.find(label).unwrap();
        let range = normalized.source_range(start, start + label.len()).unwrap();
        assert_eq!(&source[range.clone()], "みます。まどから");
        let ranges = ranges_for_text(&units, range.start, range.end);
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0].id, 42);
        assert_eq!(&units[0].text[ranges[0].start..ranges[0].end], "みます。");
        assert_eq!(ranges[1].id, 99);
        assert_eq!(&units[1].text[ranges[1].start..ranges[1].end], "まどから");
    }

    #[test]
    fn qwen_cleaning_handles_quotes_ascii_spaces_and_astral_kanji_without_byte_drift() {
        let source = "「𠮷野」 A, B! かな。かな";
        let normalized = AlignableText::new(source);
        assert_eq!(normalized.text, "𠮷野ABかなかな");
        let range = normalized.source_range(0, "𠮷".len()).unwrap();
        assert_eq!(&source[range], "𠮷");
        let start = normalized.text.find("かなかな").unwrap();
        let range = normalized
            .source_range(start, normalized.text.len())
            .unwrap();
        assert_eq!(&source[range], "かな。かな");
        assert!(normalized.source_range(1, 4).is_err());
    }
    #[test]
    fn same_size_assets_are_verified_and_completed_partials_replace_corrupt_files() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("asset.bin");
        let expected = b"verified";
        let hash = format!("{:x}", Sha256::digest(expected));
        std::fs::write(&target, expected).unwrap();
        download(
            "http://127.0.0.1:1/unreachable",
            &target,
            expected.len() as u64,
            &hash,
            &Cancellation::uncancelled(),
            &|_| {},
            "fixture",
        )
        .unwrap();
        std::fs::write(&target, b"corrupt!").unwrap();
        std::fs::write(target.with_extension("download"), expected).unwrap();
        download(
            "http://127.0.0.1:1/unreachable",
            &target,
            expected.len() as u64,
            &hash,
            &Cancellation::uncancelled(),
            &|_| {},
            "fixture",
        )
        .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), expected);
        assert!(!target.with_extension("download").exists());
    }
    #[test]
    fn native_mecab_extraction_preserves_licenses_without_loading_python() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("mecab.whl");
        let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
        for (name, bytes) in [
            ("fugashi.libs/libmecab-test.so.2", b"library".as_slice()),
            (
                "fugashi-1.5.2.dist-info/licenses/LICENSE.mecab",
                b"mecab notice",
            ),
            (
                "fugashi-1.5.2.dist-info/licenses/LICENSE",
                b"fugashi notice",
            ),
            ("fugashi/__init__.py", b"never loaded"),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        let out = root.path().join("out");
        let library = out.join("libmecab.so.2");
        extract_mecab(&archive, &out, &library).unwrap();
        assert_eq!(std::fs::read(library).unwrap(), b"library");
        assert!(out.join("LICENSE.mecab").is_file());
        assert!(out.join("LICENSE.fugashi").is_file());
        assert!(!out.join("__init__.py").exists());
    }
    #[test]
    fn selected_voice_reference_survives_new_text_and_a_new_provider() {
        let config = ProviderConfig::default();
        let initial = irodori_reference_path(&config).unwrap();
        assert_eq!(irodori_reference_path(&config.clone()).unwrap(), initial);
        let mut changed = config.clone();
        if let ProviderConfig::Irodori { seed, .. } = &mut changed {
            *seed = 99;
        }
        assert_ne!(irodori_reference_path(&changed).unwrap(), initial);
        if let ProviderConfig::Irodori {
            reference_audio, ..
        } = &mut changed
        {
            *reference_audio = Some(initial.clone());
        }
        assert_eq!(irodori_reference_path(&changed).unwrap(), initial);
    }
}
