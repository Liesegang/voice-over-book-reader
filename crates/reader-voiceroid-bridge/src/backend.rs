//! ABI and callback sequence adapted from Nkyoku/pyvcroid2 (MIT).
//! See ../licenses/pyvcroid2-LICENSE. Original installed AHS assets are not bundled.
use crate::{Mark, Request, Response};
use anyhow::{Context, Result, bail, ensure};
use libloading::Library;
use std::{
    ffi::{CStr, CString, c_char, c_void},
    path::PathBuf,
    sync::{Mutex, mpsc},
    time::Duration,
};
type TextCallback = unsafe extern "system" fn(i32, i32, *mut c_void) -> i32;
type RawCallback = unsafe extern "system" fn(i32, i32, u64, *mut c_void) -> i32;
type EventCallback = unsafe extern "system" fn(i32, i32, u64, *const c_char, *mut c_void) -> i32;
#[repr(C, packed)]
struct Config {
    hz: u32,
    voice_dir: *const c_char,
    timeout: u32,
    license: *const c_char,
    auth: *const c_char,
    reserved: u32,
}
#[repr(C, packed)]
struct Job {
    mode: u32,
    user: *mut c_void,
}
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Param {
    size: u32,
    text: Option<TextCallback>,
    raw: Option<RawCallback>,
    event: Option<EventCallback>,
    text_len: u32,
    raw_len: u32,
    volume: f32,
    pause_begin: i32,
    pause_end: i32,
    flags: i32,
    voice: [u8; 80],
    jeita: [u8; 184],
    speakers: u32,
    reserved: i32,
}
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Speaker {
    name: [u8; 80],
    volume: f32,
    speed: f32,
    pitch: f32,
    range: f32,
    pauses: [i32; 3],
    style: [u8; 80],
}
#[derive(Clone, Copy)]
struct Api {
    end: unsafe extern "system" fn() -> i32,
    get_param: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32,
    set_param: unsafe extern "system" fn(*const c_void) -> i32,
    kana: unsafe extern "system" fn(*mut i32, *const Job, *const c_char) -> i32,
    speech: unsafe extern "system" fn(*mut i32, *const Job, *const c_char) -> i32,
    get_kana: unsafe extern "system" fn(i32, *mut u8, u32, *mut u32, *mut u32) -> i32,
    get_data: unsafe extern "system" fn(i32, *mut i16, u32, *mut u32) -> i32,
    close_kana: unsafe extern "system" fn(i32, u32) -> i32,
    close_speech: unsafe extern "system" fn(i32, u32) -> i32,
}
pub struct Engine {
    _library: Library,
    api: Api,
    install: PathBuf,
    voice: String,
    defaults: Vec<u8>,
    _paths: Vec<CString>,
}
impl Drop for Engine {
    fn drop(&mut self) {
        unsafe {
            (self.api.end)();
        }
    }
}
fn check(code: i32, call: &str) -> Result<()> {
    if code == 0 {
        return Ok(());
    }
    let detail = match code {
        -100 => "license absent",
        -101 => "license expired",
        -102 => "license rejected",
        -1001 => "file not found",
        -1002 => "path not found",
        -3 => "invalid argument",
        _ => "native error",
    };
    bail!("{call}: {detail} ({code})")
}
fn sjis(text: &str) -> Result<CString> {
    let (bytes, _, bad) = encoding_rs::SHIFT_JIS.encode(text);
    ensure!(!bad, "Path/name cannot be represented in Shift-JIS");
    Ok(CString::new(bytes.as_ref())?)
}
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
struct Call {
    api: Api,
    kana: Mutex<Vec<u8>>,
    pcm: Mutex<Vec<i16>>,
    events: Mutex<Vec<Mark>>,
    done: mpsc::Sender<()>,
    error: Mutex<Option<String>>,
}
unsafe extern "system" fn text_callback(reason: i32, job: i32, data: *mut c_void) -> i32 {
    if data.is_null() || !(101..=103).contains(&reason) {
        return 0;
    }
    let context = unsafe { &*data.cast::<Call>() };
    let mut bytes = vec![0u8; 65536];
    loop {
        let mut used = 0;
        let mut position = 0;
        let code = unsafe {
            (context.api.get_kana)(
                job,
                bytes.as_mut_ptr(),
                bytes.len() as u32,
                &mut used,
                &mut position,
            )
        };
        if code != 0 {
            break;
        }
        if used as usize > bytes.len() {
            *lock(&context.error) = Some("Invalid native kana length".into());
            break;
        }
        let part = &bytes[..used as usize];
        let end = part.iter().position(|b| *b == 0).unwrap_or(part.len());
        lock(&context.kana).extend_from_slice(&part[..end]);
        if (used as usize) < bytes.len() - 1 {
            break;
        }
    }
    if reason == 103 {
        let _ = context.done.send(());
    }
    0
}
unsafe extern "system" fn raw_callback(
    reason: i32,
    job: i32,
    _tick: u64,
    data: *mut c_void,
) -> i32 {
    if data.is_null() || !(201..=203).contains(&reason) {
        return 0;
    }
    let context = unsafe { &*data.cast::<Call>() };
    let mut pcm = vec![0i16; 65536];
    loop {
        let mut used = 0;
        let code =
            unsafe { (context.api.get_data)(job, pcm.as_mut_ptr(), pcm.len() as u32, &mut used) };
        if code != 0 {
            break;
        }
        if used as usize > pcm.len() {
            *lock(&context.error) = Some("Invalid native PCM length".into());
            break;
        }
        let mut output = lock(&context.pcm);
        if output.len() + used as usize > 64 * 1024 * 1024 {
            *lock(&context.error) = Some("Native PCM exceeded 128MiB".into());
            break;
        }
        output.extend_from_slice(&pcm[..used as usize]);
        drop(output);
        if (used as usize) < pcm.len() {
            break;
        }
    }
    if reason == 203 {
        let _ = context.done.send(());
    }
    0
}
unsafe extern "system" fn event_callback(
    reason: i32,
    _job: i32,
    tick: u64,
    name: *const c_char,
    data: *mut c_void,
) -> i32 {
    if reason != 303 || name.is_null() || data.is_null() {
        return 0;
    }
    let context = unsafe { &*data.cast::<Call>() };
    if let Ok(name) = unsafe { CStr::from_ptr(name) }.to_str()
        && let Ok(offset) = name.parse::<usize>()
    {
        lock(&context.events).push(Mark {
            tick_ms: tick,
            byte_offset: offset,
        });
    }
    0
}
impl Engine {
    pub fn matches(&self, r: &Request) -> bool {
        self.install == r.install_dir && self.voice == r.voice
    }
    pub fn new(r: &Request) -> Result<Self> {
        ensure!(
            r.install_dir.is_absolute(),
            "VOICEROID2 install path must be absolute"
        );
        ensure!(
            r.voice
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                && !r.voice.is_empty(),
            "Invalid VOICEROID2 voice name"
        );
        ensure!(
            r.install_dir.join("Voice").join(&r.voice).is_dir(),
            "Selected VOICEROID2 voice is not installed"
        );
        std::env::set_current_dir(&r.install_dir)?;
        let west = r.voice.contains("_west_");
        let dll = r.install_dir.join(if west {
            "aitalked_kansai.dll"
        } else {
            "aitalked.dll"
        });
        let library: Library = unsafe {
            libloading::os::windows::Library::load_with_flags(&dll, 0x100 | 0x1000)?.into()
        };
        macro_rules! load {
            ($name:literal,$ty:ty) => {
                unsafe { *library.get::<$ty>(concat!($name, "\0").as_bytes())? }
            };
        }
        let init = load!(
            "_AITalkAPI_Init@4",
            unsafe extern "system" fn(*const Config) -> i32
        );
        let lang = load!(
            "_AITalkAPI_LangLoad@4",
            unsafe extern "system" fn(*const c_char) -> i32
        );
        let voice = load!(
            "_AITalkAPI_VoiceLoad@4",
            unsafe extern "system" fn(*const c_char) -> i32
        );
        let api = Api {
            end: load!("_AITalkAPI_End@0", unsafe extern "system" fn() -> i32),
            get_param: load!(
                "_AITalkAPI_GetParam@8",
                unsafe extern "system" fn(*mut c_void, *mut u32) -> i32
            ),
            set_param: load!(
                "_AITalkAPI_SetParam@4",
                unsafe extern "system" fn(*const c_void) -> i32
            ),
            kana: load!(
                "_AITalkAPI_TextToKana@12",
                unsafe extern "system" fn(*mut i32, *const Job, *const c_char) -> i32
            ),
            speech: load!(
                "_AITalkAPI_TextToSpeech@12",
                unsafe extern "system" fn(*mut i32, *const Job, *const c_char) -> i32
            ),
            get_kana: load!(
                "_AITalkAPI_GetKana@20",
                unsafe extern "system" fn(i32, *mut u8, u32, *mut u32, *mut u32) -> i32
            ),
            get_data: load!(
                "_AITalkAPI_GetData@16",
                unsafe extern "system" fn(i32, *mut i16, u32, *mut u32) -> i32
            ),
            close_kana: load!(
                "_AITalkAPI_CloseKana@8",
                unsafe extern "system" fn(i32, u32) -> i32
            ),
            close_speech: load!(
                "_AITalkAPI_CloseSpeech@8",
                unsafe extern "system" fn(i32, u32) -> i32
            ),
        };
        let paths = vec![
            sjis(&r.install_dir.join("Voice").to_string_lossy())?,
            sjis(&r.install_dir.join("aitalk.lic").to_string_lossy())?,
            CString::new("ORXJC6AIWAUKDpDbH2al")?,
        ];
        let config = Config {
            hz: 44100,
            voice_dir: paths[0].as_ptr(),
            timeout: 10000,
            license: paths[1].as_ptr(),
            auth: paths[2].as_ptr(),
            reserved: 0,
        };
        check(unsafe { init(&config) }, "AITalkAPI_Init")?;
        let mut engine = Self {
            _library: library,
            api,
            install: r.install_dir.clone(),
            voice: r.voice.clone(),
            defaults: vec![],
            _paths: paths,
        };
        let language = sjis(
            &r.install_dir
                .join("Lang")
                .join(if west { "standard_kansai" } else { "standard" })
                .to_string_lossy(),
        )?;
        check(unsafe { lang(language.as_ptr()) }, "AITalkAPI_LangLoad")?;
        check(
            unsafe { voice(sjis(&r.voice)?.as_ptr()) },
            "AITalkAPI_VoiceLoad",
        )?;
        let mut size = 0;
        let code = unsafe { (api.get_param)(std::ptr::null_mut(), &mut size) };
        ensure!(
            code == -20 && (312..=1024 * 1024).contains(&size),
            "Invalid VOICEROID2 parameter size: {code}/{size}"
        );
        engine.defaults = vec![0; size as usize];
        unsafe {
            engine
                .defaults
                .as_mut_ptr()
                .cast::<u32>()
                .write_unaligned(size)
        };
        check(
            unsafe { (api.get_param)(engine.defaults.as_mut_ptr().cast(), &mut size) },
            "AITalkAPI_GetParam",
        )?;
        ensure!(
            engine.defaults.len() >= std::mem::size_of::<Param>()
                && (engine.defaults.len() - std::mem::size_of::<Param>())
                    .is_multiple_of(std::mem::size_of::<Speaker>()),
            "Unsupported VOICEROID2 parameter ABI"
        );
        Ok(engine)
    }
    fn parameters(
        &self,
        r: &Request,
        text: Option<TextCallback>,
        raw: Option<RawCallback>,
        event: Option<EventCallback>,
    ) -> Result<Vec<u8>> {
        ensure!(
            r.volume.is_finite() && (0.0..=5.0).contains(&r.volume),
            "Invalid voice volume"
        );
        ensure!(
            r.pitch.is_finite() && (0.5..=2.0).contains(&r.pitch),
            "Invalid voice pitch"
        );
        ensure!(
            r.range.is_finite() && (0.0..=2.0).contains(&r.range),
            "Invalid voice range"
        );
        let mut bytes = self.defaults.clone();
        let ptr = bytes.as_mut_ptr().cast::<Param>();
        unsafe {
            (*ptr).text = text;
            (*ptr).raw = raw;
            (*ptr).event = event;
            (*ptr).flags = 17;
            (*ptr).pause_begin = 0;
            (*ptr).pause_end = 0;
            (*ptr).volume = r.volume;
        }
        let count = (bytes.len() - std::mem::size_of::<Param>()) / std::mem::size_of::<Speaker>();
        for i in 0..count {
            let p = unsafe {
                bytes
                    .as_mut_ptr()
                    .add(std::mem::size_of::<Param>() + i * std::mem::size_of::<Speaker>())
                    .cast::<Speaker>()
            };
            unsafe {
                (*p).speed = 1.;
                (*p).pitch = r.pitch;
                (*p).range = r.range;
            }
        }
        check(
            unsafe { (self.api.set_param)(bytes.as_ptr().cast()) },
            "AITalkAPI_SetParam",
        )?;
        Ok(bytes)
    }
    pub fn synthesize(&mut self, r: &Request) -> Result<Response> {
        ensure!(r.output_wav.is_absolute(), "Output WAV must be absolute");
        let (encoded, map, replaced) = mapped_sjis(&r.text)?;
        let (tx, rx) = mpsc::channel();
        let context = Box::new(Call {
            api: self.api,
            kana: Mutex::new(vec![]),
            pcm: Mutex::new(vec![]),
            events: Mutex::new(vec![]),
            done: tx,
            error: Mutex::new(None),
        });
        let context = Box::into_raw(context); // Native callbacks retain this pointer through CloseSpeech.
        let _parameter = self.parameters(r, Some(text_callback), None, None)?;
        let input = CString::new(encoded)?;
        let mut id = 0;
        let job = Job {
            mode: 21,
            user: context.cast(),
        };
        check(
            unsafe { (self.api.kana)(&mut id, &job, input.as_ptr()) },
            "AITalkAPI_TextToKana",
        )?;
        let received = rx.recv_timeout(Duration::from_secs(60));
        let close = unsafe { (self.api.close_kana)(id, 0) };
        received.context("VOICEROID2 kana conversion timeout")?;
        check(close, "AITalkAPI_CloseKana")?;
        if let Some(error) = lock(unsafe { &(*context).error }).take() {
            bail!("{error}");
        }
        let kana = lock(unsafe { &(*context).kana }).clone();
        let kana = CString::new(remap_bookmarks(&kana, &map)?)?;
        let _parameter = self.parameters(r, None, Some(raw_callback), Some(event_callback))?;
        let job = Job {
            mode: 12,
            user: context.cast(),
        };
        check(
            unsafe { (self.api.speech)(&mut id, &job, kana.as_ptr()) },
            "AITalkAPI_TextToSpeech",
        )?;
        let received = rx.recv_timeout(Duration::from_secs(60));
        let close = unsafe { (self.api.close_speech)(id, 0) };
        received.context("VOICEROID2 speech generation timeout")?;
        check(close, "AITalkAPI_CloseSpeech")?;
        let context = unsafe { Box::from_raw(context) };
        if let Some(error) = lock(&context.error).take() {
            bail!("{error}");
        }
        let pcm = lock(&context.pcm);
        ensure!(!pcm.is_empty(), "VOICEROID2 returned empty PCM");
        let mut wav = hound::WavWriter::create(
            &r.output_wav,
            hound::WavSpec {
                channels: 1,
                sample_rate: 44100,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )?;
        for &sample in pcm.iter() {
            wav.write_sample(sample)?;
        }
        wav.finalize()?;
        let response = Response {
            sample_rate: 44100,
            samples: pcm.len(),
            marks: lock(&context.events).clone(),
            replaced_characters: replaced,
        };
        Ok(response)
    }
}
fn mapped_sjis(text: &str) -> Result<(Vec<u8>, Vec<usize>, usize)> {
    ensure!(
        !text.is_empty() && text.len() <= 256 * 1024,
        "VOICEROID2 text is empty or too large"
    );
    let mut bytes = Vec::new();
    let mut map = Vec::new();
    let mut replaced = 0;
    for (start, ch) in text.char_indices() {
        let character = ch.to_string();
        let (part, _, bad) = encoding_rs::SHIFT_JIS.encode(&character);
        let part = if bad {
            replaced += 1;
            vec![b' ']
        } else {
            part.into_owned()
        };
        map.extend(std::iter::repeat_n(start, part.len()));
        bytes.extend(part);
    }
    ensure!(bytes.len() < 65535, "VOICEROID2 text exceeds native buffer");
    map.push(text.len());
    Ok((bytes, map, replaced))
}
fn remap_bookmarks(kana: &[u8], positions: &[usize]) -> Result<Vec<u8>> {
    const MARK: &[u8] = b"(Irq MARK=_AI@";
    let mut output = vec![];
    let mut at = 0;
    while let Some(relative) = kana[at..].windows(MARK.len()).position(|w| w == MARK) {
        let start = at + relative + MARK.len();
        output.extend_from_slice(&kana[at..start]);
        let end = start
            + kana[start..]
                .iter()
                .position(|b| *b == b')')
                .context("Unclosed VOICEROID2 bookmark")?;
        let position = std::str::from_utf8(&kana[start..end])?.parse::<usize>()?;
        let mapped = positions
            .get(position)
            .context("VOICEROID2 bookmark exceeds source")?;
        output.extend_from_slice(mapped.to_string().as_bytes());
        at = end;
    }
    output.extend_from_slice(&kana[at..]);
    Ok(output)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abi_sizes_and_source_positions_are_real() {
        assert_eq!(std::mem::size_of::<Config>(), 24);
        assert_eq!(std::mem::size_of::<Param>(), 312);
        assert_eq!(std::mem::size_of::<Speaker>(), 188);
        let (bytes, map, bad) = mapped_sjis("漢😀字").unwrap();
        assert_eq!(bad, 1);
        assert_eq!(bytes.len(), 5);
        assert_eq!(map, vec![0, 0, 3, 7, 7, 10]);
        assert_eq!(
            remap_bookmarks(b"(Irq MARK=_AI@3)", &map).unwrap(),
            b"(Irq MARK=_AI@7)"
        );
    }
}
