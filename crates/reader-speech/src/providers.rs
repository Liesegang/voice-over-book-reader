use crate::{
    native::{IrodoriProvider, KokoroProvider},
    types::*,
};
use anyhow::{Context, Result, bail, ensure};
use reqwest::{Url, blocking::Client};
use serde::{Deserialize, Serialize};
use std::{
    io::{Cursor, Read, Write},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

pub trait TtsProvider: Send {
    fn name(&self) -> &str;
    fn synthesize(
        &mut self,
        units: &[SpeechUnit],
        cancel: &Cancellation,
        progress: &dyn Fn(String),
    ) -> Result<SynthesizedAudio>;
}

pub fn create_provider(config: &ProviderConfig) -> Result<Box<dyn TtsProvider>> {
    Ok(match config {
        ProviderConfig::Irodori { .. } => Box::new(IrodoriProvider::new(config.clone())),
        ProviderConfig::Kokoro { backend, .. } if backend.starts_with("audio.cpp_") => {
            Box::new(KokoroProvider::new(config.clone()))
        }
        ProviderConfig::Kokoro { .. } => {
            Box::new(crate::kokoro_onnx::KokoroOnnxProvider::new(config.clone()))
        }
        ProviderConfig::AivisNative { .. } => Box::new(
            crate::aivis_native::AivisNativeProvider::new(config.clone()),
        ),
        ProviderConfig::System { voice } => Box::new(SystemProvider {
            voice: voice.clone(),
        }),
        ProviderConfig::OpenAi {
            base_url,
            api_key,
            model,
            voice,
        } => Box::new(HttpProvider {
            client: http_client()?,
            base: validate_endpoint(base_url)?,
            kind: HttpKind::OpenAi {
                api_key: api_key.clone(),
                model: model.clone(),
                voice: voice.clone(),
            },
        }),
        ProviderConfig::Voicevox { base_url, speaker } => Box::new(HttpProvider {
            client: http_client()?,
            base: validate_endpoint(base_url)?,
            kind: HttpKind::Voicevox { speaker: *speaker },
        }),
        ProviderConfig::AivisSpeech { .. } => {
            Box::new(crate::aivis::AivisProvider::new(config.clone()))
        }
        ProviderConfig::Voiceroid2 { .. } => {
            Box::new(crate::voiceroid::VoiceroidProvider::new(config.clone()))
        }
        ProviderConfig::Command { program, args } => Box::new(CommandProvider {
            program: program.clone(),
            args: args.clone(),
        }),
    })
}

pub fn validate_endpoint(value: &str) -> Result<Url> {
    let url = Url::parse(value).context("TTS接続先URLが不正です")?;
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "TTS接続先はhttpまたはhttpsを指定してください"
    );
    ensure!(url.host_str().is_some(), "TTS接続先にホスト名が必要です");
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "URL内の認証情報は使用できません。APIキー設定を使用してください"
    );
    ensure!(
        url.query().is_none() && url.fragment().is_none(),
        "TTS接続先にはクエリ・フラグメントを含められません"
    );
    Ok(url)
}

pub(crate) fn http_client() -> Result<Client> {
    Ok(Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(180))
        .user_agent("epub-reader/0.1")
        .build()?)
}

pub(crate) fn endpoint(base: &Url, suffix: &str) -> Result<Url> {
    let mut url = base.clone();
    let path = format!(
        "{}/{}",
        url.path().trim_end_matches('/'),
        suffix.trim_start_matches('/')
    );
    url.set_path(&path);
    Ok(url)
}

pub(crate) fn decode_wav(bytes: &[u8]) -> Result<(Vec<f32>, u32)> {
    let mut reader =
        hound::WavReader::new(Cursor::new(bytes)).context("TTSはPCM WAVを返す必要があります")?;
    let spec = reader.spec();
    ensure!(spec.channels > 0, "WAV channels must be positive");
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<std::result::Result<_, _>>()?,
        hound::SampleFormat::Int => {
            ensure!(
                spec.bits_per_sample > 0 && spec.bits_per_sample <= 32,
                "Unsupported WAV encoding"
            );
            let scale = 2_f32.powi(spec.bits_per_sample as i32 - 1);
            reader
                .samples::<i32>()
                .map(|r| r.map(|v| v as f32 / scale))
                .collect::<std::result::Result<_, _>>()?
        }
    };
    let mono = samples
        .chunks_exact(spec.channels as usize)
        .map(|frame| frame.iter().sum::<f32>() / spec.channels as f32)
        .collect();
    Ok((mono, spec.sample_rate))
}

pub(crate) fn chunk_audio(bytes: &[u8], units: &[SpeechUnit]) -> Result<SynthesizedAudio> {
    let (samples, sample_rate) = decode_wav(bytes)?;
    let end = samples.len() as u64;
    let audio = SynthesizedAudio {
        samples,
        sample_rate,
        boundaries: vec![TimedBoundary {
            start_sample: 0,
            end_sample: end,
            ranges: unit_ranges(units),
            precision: BoundaryPrecision::Chunk,
        }],
    };
    audio.validate()?;
    Ok(audio)
}

struct HttpProvider {
    client: Client,
    base: Url,
    kind: HttpKind,
}
enum HttpKind {
    OpenAi {
        api_key: String,
        model: String,
        voice: String,
    },
    Voicevox {
        speaker: u32,
    },
}
impl TtsProvider for HttpProvider {
    fn name(&self) -> &str {
        match self.kind {
            HttpKind::OpenAi { .. } => "OpenAI互換TTS",
            HttpKind::Voicevox { .. } => "VOICEVOX",
        }
    }
    fn synthesize(
        &mut self,
        units: &[SpeechUnit],
        cancel: &Cancellation,
        _: &dyn Fn(String),
    ) -> Result<SynthesizedAudio> {
        cancel.check()?;
        let text: String = units.iter().map(|u| u.text.as_str()).collect();
        let response = match &self.kind {
            HttpKind::OpenAi {
                api_key,
                model,
                voice,
            } => {
                let suffix = if self.base.path().trim_end_matches('/').ends_with("/v1") {
                    "audio/speech"
                } else {
                    "v1/audio/speech"
                };
                let mut request = self.client.post(endpoint(&self.base, suffix)?).json(&serde_json::json!({
                    "input": text, "model": model, "voice": voice, "response_format": "wav", "speed": 1.0 }));
                if !api_key.is_empty() {
                    request = request.bearer_auth(api_key);
                }
                request.send()?.error_for_status()?
            }
            HttpKind::Voicevox { speaker } => {
                let query: serde_json::Value = self
                    .client
                    .post(endpoint(&self.base, "audio_query")?)
                    .query(&[("text", text.clone()), ("speaker", speaker.to_string())])
                    .send()?
                    .error_for_status()?
                    .json()?;
                cancel.check()?;
                self.client
                    .post(endpoint(&self.base, "synthesis")?)
                    .query(&[("speaker", speaker)])
                    .json(&query)
                    .send()?
                    .error_for_status()?
            }
        };
        let mut bytes = Vec::new();
        response.take(128 * 1024 * 1024).read_to_end(&mut bytes)?;
        cancel.check()?;
        chunk_audio(&bytes, units)
    }
}

struct CommandProvider {
    program: std::path::PathBuf,
    args: Vec<String>,
}
#[derive(Serialize)]
struct CommandRequest<'a> {
    text: String,
    units: &'a [SpeechUnit],
    output_wav: &'a Path,
}
#[derive(Deserialize, Default)]
struct CommandResponse {
    #[serde(default)]
    boundaries: Vec<TimedBoundary>,
}

impl TtsProvider for CommandProvider {
    fn name(&self) -> &str {
        "外部コマンドTTS"
    }
    fn synthesize(
        &mut self,
        units: &[SpeechUnit],
        cancel: &Cancellation,
        _: &dyn Fn(String),
    ) -> Result<SynthesizedAudio> {
        let temp = tempfile::tempdir()?;
        let wav = temp.path().join("speech.wav");
        let out = std::fs::File::create(temp.path().join("result.json"))?;
        let error = std::fs::File::create(temp.path().join("stderr.log"))?;
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(out)
            .stderr(error);
        hide_window(&mut command);
        let mut child = command
            .spawn()
            .context("TTSコマンドを起動できませんでした")?;
        let input = serde_json::to_vec(&CommandRequest {
            text: units.iter().map(|u| u.text.as_str()).collect(),
            units,
            output_wav: &wav,
        })?;
        child
            .stdin
            .take()
            .context("TTS stdin is unavailable")?
            .write_all(&input)?;
        loop {
            if cancel.is_cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                cancel.check()?;
            }
            if let Some(status) = child.try_wait()? {
                ensure!(status.success(), "TTSコマンドが失敗しました: {status}");
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let bytes =
            std::fs::read(&wav).context("TTSコマンドがoutput_wavへWAVを書き込みませんでした")?;
        let mut audio = chunk_audio(&bytes, units)?;
        let result = std::fs::read(temp.path().join("result.json"))?;
        if !result.iter().all(u8::is_ascii_whitespace) {
            let response: CommandResponse = serde_json::from_slice(&result)
                .context("TTSコマンドのstdoutはJSONである必要があります")?;
            if !response.boundaries.is_empty() {
                audio.boundaries = response.boundaries;
                audio.validate()?;
            }
        }
        Ok(audio)
    }
}

struct SystemProvider {
    voice: Option<String>,
}

#[derive(Deserialize)]
struct SapiMark {
    position: usize,
    length: usize,
    seconds: f64,
    #[serde(default)]
    text: Option<String>,
}

fn sapi_boundaries(
    text: &str,
    units: &[SpeechUnit],
    frames: usize,
    rate: u32,
    marks: &[SapiMark],
) -> Vec<TimedBoundary> {
    let utf16 = text.encode_utf16().count();
    // Reject an unverified stream clock as a whole instead of silently losing
    // trailing words (the former 16k/22.05k System.Speech bug did exactly that).
    if marks.iter().any(|mark| {
        !mark.seconds.is_finite()
            || mark.seconds < 0.0
            || mark.seconds * rate as f64 >= frames as f64
            || mark.position.saturating_add(mark.length) > utf16
    }) || marks
        .windows(2)
        .any(|pair| pair[1].seconds < pair[0].seconds)
    {
        return Vec::new();
    }
    let mut boundaries = Vec::new();
    for (index, mark) in marks.iter().enumerate() {
        let from = utf16_to_utf8(text, mark.position);
        let to = utf16_to_utf8(text, mark.position.saturating_add(mark.length));
        if from >= to
            || mark
                .text
                .as_ref()
                .is_some_and(|event_text| event_text != &text[from..to])
        {
            continue;
        }
        let start = (mark.seconds * rate as f64).round() as u64;
        let end = marks
            .get(index + 1)
            .map(|next| (next.seconds * rate as f64).round() as u64)
            .unwrap_or(frames as u64)
            .min(frames as u64);
        let ranges = ranges_for_text(units, from, to);
        if start < end && !ranges.is_empty() {
            boundaries.push(TimedBoundary {
                start_sample: start,
                end_sample: end,
                ranges,
                precision: BoundaryPrecision::Word,
            });
        }
    }
    boundaries
}
impl TtsProvider for SystemProvider {
    fn name(&self) -> &str {
        "Windows システム音声"
    }
    fn synthesize(
        &mut self,
        units: &[SpeechUnit],
        cancel: &Cancellation,
        progress: &dyn Fn(String),
    ) -> Result<SynthesizedAudio> {
        if !cfg!(windows) {
            bail!(
                "システム音声は現在Windowsで利用できます。Irodoriまたは外部TTSを選択してください"
            );
        }
        progress("Windowsの音声で合成しています".into());
        let temp = tempfile::tempdir()?;
        let wav = temp.path().join("speech.wav");
        let request = temp.path().join("request.json");
        let result_path = temp.path().join("timings.json");
        let error_path = temp.path().join("error.txt");
        let script = temp.path().join("sapi.ps1");
        let text: String = units.iter().map(|u| u.text.as_str()).collect();
        std::fs::write(
            &request,
            serde_json::to_vec(
                &serde_json::json!({"text":text,"voice":self.voice,"wav":wav,"result":result_path,"error":error_path}),
            )?,
        )?;
        // Windows PowerShell 5 reads a BOM-less script using the legacy ANSI
        // code page. A Japanese message inside Add-Type can then corrupt C#
        // syntax before synthesis even starts.
        std::fs::write(
            &script,
            [
                b"\xef\xbb\xbf".as_slice(),
                include_bytes!("../scripts/sapi.ps1"),
            ]
            .concat(),
        )?;
        let mut cmd = Command::new("powershell.exe");
        let log = std::fs::File::create(temp.path().join("sapi.log"))?;
        cmd.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script)
        .arg(&request)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
        hide_window(&mut cmd);
        let mut child = cmd
            .spawn()
            .context("Windows音声サービスを起動できませんでした")?;
        loop {
            if cancel.is_cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                cancel.check()?;
            }
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    let detail = std::fs::read_to_string(&error_path)
                        .ok()
                        .filter(|text| !text.trim().is_empty())
                        .unwrap_or_else(|| {
                            String::from_utf8_lossy(
                                &std::fs::read(temp.path().join("sapi.log")).unwrap_or_default(),
                            )
                            .trim()
                            .to_owned()
                        });
                    bail!("Windows音声で合成できませんでした ({status}): {detail}");
                }
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        cancel.check()?;
        let (samples, sample_rate) = decode_wav(&std::fs::read(wav)?)?;
        let marks: Vec<SapiMark> = serde_json::from_slice(&std::fs::read(result_path)?)?;
        let mut boundaries = sapi_boundaries(&text, units, samples.len(), sample_rate, &marks);
        if boundaries.is_empty() {
            boundaries.push(TimedBoundary {
                start_sample: 0,
                end_sample: samples.len() as u64,
                ranges: unit_ranges(units),
                precision: BoundaryPrecision::Chunk,
            });
        }
        let audio = SynthesizedAudio {
            samples,
            sample_rate,
            boundaries,
        };
        audio.validate()?;
        Ok(audio)
    }
}

fn utf16_to_utf8(text: &str, target: usize) -> usize {
    let mut utf16 = 0;
    for (byte, ch) in text.char_indices() {
        if utf16 >= target {
            return byte;
        }
        utf16 += ch.len_utf16();
    }
    text.len()
}

pub(crate) fn hide_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    #[cfg(not(windows))]
    let _ = command;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn url_rejects_secrets_and_non_http_schemes() {
        assert!(validate_endpoint("file:///tmp/a").is_err());
        assert!(validate_endpoint("https://token:secret@example.test").is_err());
        assert!(validate_endpoint("http://127.0.0.1:50021").is_ok());
        assert!(validate_endpoint("https://example.test/v1/").is_ok());
    }
    #[test]
    fn utf16_event_offsets_preserve_supplementary_characters() {
        assert_eq!(utf16_to_utf8("漢😀字", 3), 7);
    }
    #[test]
    fn sapi_pcm_clock_and_utf16_marks_cover_multiple_units_without_partial_bad_clocks() {
        let units = [
            SpeechUnit {
                id: 41,
                text: "　今日😀".into(),
            },
            SpeechUnit {
                id: 42,
                text: "とうきょう".into(),
            },
            SpeechUnit {
                id: 43,
                text: "です。".into(),
            },
        ];
        let text = units
            .iter()
            .map(|unit| unit.text.as_str())
            .collect::<String>();
        let marks = vec![
            SapiMark {
                position: 1,
                length: 2,
                seconds: 0.1,
                text: Some("今日".into()),
            },
            SapiMark {
                position: 5,
                length: 5,
                seconds: 0.4,
                text: Some("とうきょう".into()),
            },
            SapiMark {
                position: 10,
                length: 2,
                seconds: 0.8,
                text: Some("です".into()),
            },
        ];
        let boundaries = sapi_boundaries(&text, &units, 22_050, 22_050, &marks);
        assert_eq!(boundaries.len(), 3);
        assert_eq!(boundaries[0].start_sample, 2205);
        assert_eq!(
            boundaries[0].ranges[0],
            UnitRange {
                id: 41,
                start: 3,
                end: 9
            }
        );
        assert_eq!(
            boundaries[1].ranges[0],
            UnitRange {
                id: 42,
                start: 0,
                end: 15
            }
        );
        assert_eq!(
            boundaries[2].ranges[0],
            UnitRange {
                id: 43,
                start: 0,
                end: 6
            }
        );
        let mut invalid = marks;
        invalid[2].seconds = 1.2;
        assert!(
            sapi_boundaries(&text, &units, 22_050, 22_050, &invalid).is_empty(),
            "do not retain misleading initial marks from a drifting clock"
        );
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "Requires an installed Japanese Windows SAPI voice"]
    fn system_voice_falls_back_from_a_foreign_provider_name_with_real_word_marks() {
        let units = [SpeechUnit {
            id: 123,
            text: "今日はいい天気です。本を読みましょう。".into(),
        }];
        let mut provider = SystemProvider {
            voice: Some("jf_alpha".into()),
        };
        let audio = provider
            .synthesize(&units, &Cancellation::uncancelled(), &|_| {})
            .unwrap();
        audio.validate().unwrap();
        assert!(audio.samples.iter().any(|sample| sample.abs() > 0.01));
        assert!(audio.boundaries.len() > 1);
        assert!(audio.boundaries.iter().all(|boundary| {
            boundary.precision == BoundaryPrecision::Word
                && boundary.ranges.iter().all(|range| range.id == 123)
        }));
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "manual PCM timing regression: requires an installed Japanese Windows SAPI voice"]
    fn system_voice_pcm_clock_keeps_final_words_after_ruby_numbers_and_pauses() {
        let units = [
            SpeechUnit {
                id: 41,
                text: "　今日はいい天気です。".into(),
            },
            SpeechUnit {
                id: 42,
                text: "とうきょう".into(),
            },
            SpeechUnit {
                id: 43,
                text: "で本を読みましょう。".into(),
            },
            SpeechUnit {
                id: 44,
                text: "4003冊の本と、次の文章です。".into(),
            },
        ];
        let mut provider = SystemProvider {
            voice: Some("Microsoft Haruka Desktop".into()),
        };
        let audio = provider
            .synthesize(&units, &Cancellation::uncancelled(), &|_| {})
            .unwrap();
        assert!(
            audio.boundaries.len() >= 20,
            "the old converted-PCM clock silently lost trailing marks"
        );
        assert!(
            audio
                .boundaries
                .iter()
                .all(|boundary| boundary.precision == BoundaryPrecision::Word)
        );
        assert!(audio.boundaries.iter().any(|boundary| {
            boundary
                .ranges
                .iter()
                .any(|range| range.id == 42 && range.start == 0 && range.end == 15)
        }));
        let final_range = audio.boundaries.last().unwrap().ranges.last().unwrap();
        assert_eq!(final_range.id, 44);
        assert_eq!(final_range.end, units[3].text.len() - "。".len());
        let last_seconds =
            audio.boundaries.last().unwrap().start_sample as f64 / audio.sample_rate as f64;
        let total_seconds = audio.samples.len() as f64 / audio.sample_rate as f64;
        eprintln!(
            "SAPI verified PCM: rate={} duration={total_seconds:.3}s marks={} last_mark={last_seconds:.3}s",
            audio.sample_rate,
            audio.boundaries.len()
        );
        assert!(last_seconds < total_seconds);
        for speed in [0.5, 1.0, 2.0, 10.0] {
            let (_, map) =
                crate::engine::render_audio(&audio.samples, audio.sample_rate, speed, 48_000)
                    .unwrap();
            for boundary in &audio.boundaries {
                let device_frame = map.destination_at(boundary.start_sample);
                let source_frame = map.source_at(device_frame);
                assert!(
                    source_frame.abs_diff(boundary.start_sample) < audio.sample_rate as u64 / 20,
                    "SAPI word start was lost by the real Sonic/resampler timeline at {speed}x"
                );
            }
        }
    }
    fn test_wave() -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(
                &mut cursor,
                hound::WavSpec {
                    channels: 1,
                    sample_rate: 24_000,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                },
            )
            .unwrap();
            for _ in 0..480 {
                writer.write_sample(8_192i16).unwrap();
            }
            writer.finalize().unwrap();
        }
        cursor.into_inner()
    }
    fn receive_http(stream: &mut std::net::TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 2048];
        loop {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(header_end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&bytes[..header_end]);
                let length = header
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|n| n.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if bytes.len() >= header_end + 4 + length {
                    return String::from_utf8(bytes).unwrap();
                }
            }
        }
    }
    fn respond(stream: &mut std::net::TcpStream, content_type: &str, body: &[u8]) {
        write!(stream,"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",body.len()).unwrap();
        stream.write_all(body).unwrap();
    }
    #[test]
    fn openai_compatible_provider_uses_wav_and_keeps_real_chunk_capability() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = receive_http(&mut stream);
            assert!(request.starts_with("POST /v1/audio/speech "));
            let body = request.split("\r\n\r\n").nth(1).unwrap();
            let value: serde_json::Value = serde_json::from_str(body).unwrap();
            assert_eq!(value["input"], "東京です。");
            assert_eq!(value["response_format"], "wav");
            respond(&mut stream, "audio/wav", &test_wave());
        });
        let mut provider = create_provider(&ProviderConfig::OpenAi {
            base_url: format!("http://{address}/v1"),
            api_key: "test-key".into(),
            model: "local-model".into(),
            voice: "voice".into(),
        })
        .unwrap();
        let audio = provider
            .synthesize(
                &[SpeechUnit {
                    id: 21,
                    text: "東京です。".into(),
                }],
                &Cancellation::uncancelled(),
                &|_| {},
            )
            .unwrap();
        assert_eq!(audio.samples.len(), 480);
        assert_eq!(audio.boundaries[0].precision, BoundaryPrecision::Chunk);
        assert_eq!(audio.boundaries[0].ranges[0].id, 21);
        server.join().unwrap();
    }
    #[test]
    fn voicevox_provider_passes_query_to_synthesis_without_inventing_word_marks() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut query, _) = listener.accept().unwrap();
            let request = receive_http(&mut query);
            assert!(request.starts_with("POST /audio_query?"));
            assert!(request.contains("speaker=3"));
            respond(
                &mut query,
                "application/json",
                br#"{"accent_phrases":[],"speedScale":1.0}"#,
            );
            let (mut synth, _) = listener.accept().unwrap();
            let request = receive_http(&mut synth);
            assert!(request.starts_with("POST /synthesis?speaker=3 "));
            assert!(request.contains("accent_phrases"));
            respond(&mut synth, "audio/wav", &test_wave());
        });
        let mut provider = create_provider(&ProviderConfig::Voicevox {
            base_url: format!("http://{address}"),
            speaker: 3,
        })
        .unwrap();
        let audio = provider
            .synthesize(
                &[SpeechUnit {
                    id: 9,
                    text: "本です。".into(),
                }],
                &Cancellation::uncancelled(),
                &|_| {},
            )
            .unwrap();
        assert_eq!(audio.boundaries.len(), 1);
        assert_eq!(audio.boundaries[0].precision, BoundaryPrecision::Chunk);
        server.join().unwrap();
    }
}
