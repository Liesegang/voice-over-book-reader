//! Local AivisSpeech Engine lifecycle and its distinct VOICEVOX-compatible API.
use crate::{Cancellation, ProviderConfig, SpeechUnit, SynthesizedAudio, TtsProvider};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AivisVoice {
    pub id: i32,
    pub speaker_name: String,
    pub style_name: String,
    pub speaker_uuid: String,
}
impl AivisVoice {
    pub fn label(&self) -> String {
        format!("{} · {}", self.speaker_name, self.style_name)
    }
}

pub(crate) fn installed_engine() -> Option<PathBuf> {
    let manifest = crate::data_dir().join("speech/aivis-engine.json");
    let saved = std::fs::read(manifest)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| value["engine_path"].as_str().map(PathBuf::from));
    let mut candidates = saved.into_iter().collect::<Vec<_>>();
    candidates.push(
        crate::data_dir().join("speech/engines/AivisSpeech-Engine-1.2.0/Windows-x64/run.exe"),
    );
    for variable in ["LOCALAPPDATA", "ProgramFiles"] {
        if let Some(path) = std::env::var_os(variable) {
            let root = PathBuf::from(path);
            candidates.push(if variable == "LOCALAPPDATA" {
                root.join("Programs/AivisSpeech/AivisSpeech-Engine/run.exe")
            } else {
                root.join("AivisSpeech/AivisSpeech-Engine/run.exe")
            });
        }
    }
    candidates
        .push("/Applications/AivisSpeech.app/Contents/Resources/AivisSpeech-Engine/run".into());
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(
            PathBuf::from(home)
                .join("Applications/AivisSpeech.app/Contents/Resources/AivisSpeech-Engine/run"),
        );
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn client(timeout: Duration) -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .timeout(timeout)
        .build()?)
}
fn ready(base: &reqwest::Url) -> Result<bool> {
    let response = match client(Duration::from_millis(800))?
        .get(crate::providers::endpoint(base, "engine_manifest")?)
        .send()
    {
        Ok(response) => response,
        Err(_) => return Ok(false),
    };
    if !response.status().is_success() {
        return Ok(false);
    }
    let manifest: serde_json::Value = response.json()?;
    ensure!(
        manifest["name"]
            .as_str()
            .is_some_and(|name| name.contains("AivisSpeech")),
        "接続先はAivisSpeech Engineではありません。URLを確認してください"
    );
    Ok(true)
}

/// Lazily start only a loopback engine. One server is shared across reader
/// processes; it remains available after an individual book window closes.
pub fn prepare_aivis(
    config: &ProviderConfig,
    cancel: &Cancellation,
    progress: &dyn Fn(String),
) -> Result<()> {
    let ProviderConfig::AivisSpeech {
        base_url,
        engine_path,
        auto_start,
        ..
    } = config
    else {
        bail!("AivisSpeech設定ではありません");
    };
    let base = crate::validate_endpoint(base_url)?;
    cancel.check()?;
    if ready(&base)? {
        return Ok(());
    }
    ensure!(
        *auto_start,
        "AivisSpeech Engineに接続できません。エンジンを起動してください"
    );
    let host = base.host_str().unwrap_or("").trim_matches(['[', ']']);
    ensure!(
        host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback()),
        "自動起動はlocalhostのAivisSpeech Engineだけに対応しています"
    );
    ensure!(
        base.path().trim_matches('/').is_empty(),
        "自動起動にはエンジンのルートURLを指定してください"
    );
    ensure!(
        base.scheme() == "http",
        "AivisSpeech Engineの自動起動にはhttpのローカルURLを指定してください"
    );
    let directory = crate::data_dir().join("speech");
    std::fs::create_dir_all(&directory)?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("aivis-start.lock"))?;
    let waiting = Instant::now();
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => {
                cancel.check()?;
                ensure!(
                    waiting.elapsed() < Duration::from_secs(600),
                    "別の読書ウィンドウによるAivisSpeechの起動を待機できませんでした"
                );
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
    if ready(&base)? {
        return Ok(());
    }
    let engine = if let Some(path) = engine_path {
        ensure!(
            path.is_file(),
            "指定したAivisSpeech Engineが見つかりません: {}",
            path.display()
        );
        path.clone()
    } else {
        installed_engine()
            .context("AivisSpeech Engineが見つかりません。設定からrun.exeを選んでください")?
    };
    let log = File::options()
        .create(true)
        .append(true)
        .open(directory.join("aivis-engine.log"))?;
    let mut command = Command::new(&engine);
    command
        .args([
            "--host",
            if host == "localhost" {
                "127.0.0.1"
            } else {
                host
            },
            "--port",
            &base.port_or_known_default().unwrap_or(10101).to_string(),
            "--no-use_gpu",
            "--output_log_utf8",
            "--disable_sentry",
        ])
        .current_dir(engine.parent().context("Invalid AivisSpeech engine path")?)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    crate::providers::hide_window(&mut command);
    let mut child = command
        .spawn()
        .with_context(|| format!("AivisSpeech Engineを起動できません: {}", engine.display()))?;
    progress("AivisSpeech Engineを準備しています。初回はモデルの取得に数分かかります".into());
    let started = Instant::now();
    let mut last = Instant::now();
    while started.elapsed() < Duration::from_secs(600) {
        cancel.check()?;
        if ready(&base)? {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            bail!(
                "AivisSpeech Engineが終了しました({status})。ログ: {}",
                directory.join("aivis-engine.log").display()
            );
        }
        if last.elapsed() > Duration::from_secs(5) {
            progress("AivisSpeech Engineの起動を待っています".into());
            last = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    bail!(
        "AivisSpeech Engineの起動が時間内に完了しませんでした。ログ: {}",
        directory.join("aivis-engine.log").display()
    )
}

pub fn aivis_voices(
    config: &ProviderConfig,
    cancel: &Cancellation,
    progress: &dyn Fn(String),
) -> Result<Vec<AivisVoice>> {
    prepare_aivis(config, cancel, progress)?;
    let ProviderConfig::AivisSpeech { base_url, .. } = config else {
        unreachable!()
    };
    let base = crate::validate_endpoint(base_url)?;
    #[derive(Deserialize)]
    struct Speaker {
        name: String,
        speaker_uuid: String,
        styles: Vec<Style>,
    }
    #[derive(Deserialize)]
    struct Style {
        name: String,
        id: i32,
    }
    let speakers: Vec<Speaker> = client(Duration::from_secs(30))?
        .get(crate::providers::endpoint(&base, "speakers")?)
        .send()?
        .error_for_status()?
        .json()?;
    let voices = speakers
        .into_iter()
        .flat_map(|speaker| {
            speaker.styles.into_iter().map(move |style| AivisVoice {
                id: style.id,
                speaker_name: speaker.name.clone(),
                style_name: style.name,
                speaker_uuid: speaker.speaker_uuid.clone(),
            })
        })
        .collect::<Vec<_>>();
    ensure!(
        !voices.is_empty(),
        "AivisSpeech Engineに話者がありません。音声モデルを追加してください"
    );
    Ok(voices)
}

pub(crate) struct AivisProvider {
    config: ProviderConfig,
    aligner: Option<crate::native::NativeAligner>,
}
impl AivisProvider {
    pub(crate) fn new(config: ProviderConfig) -> Self {
        Self {
            config,
            aligner: None,
        }
    }
}
impl TtsProvider for AivisProvider {
    fn name(&self) -> &str {
        "AivisSpeech"
    }
    fn synthesize(
        &mut self,
        units: &[SpeechUnit],
        cancel: &Cancellation,
        progress: &dyn Fn(String),
    ) -> Result<SynthesizedAudio> {
        prepare_aivis(&self.config, cancel, progress)?;
        let mut audio = request_audio(&self.config, units, cancel, progress)?;
        if !cancel.is_cancelled() {
            let aligned = (|| -> Result<_> {
                if self.aligner.is_none() {
                    let assets = crate::data_dir().join("speech/assets");
                    let runtime = assets.join("audio.cpp-v0.9.3-portable");
                    let model = assets.join("qwen3-forced-aligner-0.6b-q8_0.gguf");
                    crate::native::prepare_aligner(&runtime, &model, true, cancel, progress)?;
                    self.aligner = Some(crate::native::NativeAligner::load(&runtime, &model)?);
                }
                cancel.check()?;
                progress("AivisSpeechの実音声から読み上げ位置を合わせています".into());
                let text = units
                    .iter()
                    .map(|unit| unit.text.as_str())
                    .collect::<String>();
                self.aligner.as_ref().unwrap().align(&text, units, &audio)
            })();
            match aligned {
                Ok(boundaries) if !boundaries.is_empty() => audio.boundaries = boundaries,
                Ok(_) => progress(
                    "音声の位置合わせが得られなかったため、この文は文単位で表示します".into(),
                ),
                Err(error) if !cancel.is_cancelled() => progress(format!(
                    "音声の位置合わせを利用できないため、この文は文単位で表示します: {error:#}"
                )),
                Err(_) => {}
            }
        }
        Ok(audio)
    }
}

fn request_audio(
    config: &ProviderConfig,
    units: &[SpeechUnit],
    cancel: &Cancellation,
    progress: &dyn Fn(String),
) -> Result<SynthesizedAudio> {
    let ProviderConfig::AivisSpeech {
        base_url, speaker, ..
    } = config
    else {
        unreachable!()
    };
    let base = crate::validate_endpoint(base_url)?;
    let text = units
        .iter()
        .map(|unit| unit.text.as_str())
        .collect::<String>();
    let client = client(Duration::from_secs(180))?;
    let mut query: serde_json::Value = client
        .post(crate::providers::endpoint(&base, "audio_query")?)
        .query(&[("text", text.clone()), ("speaker", speaker.to_string())])
        .send()?
        .error_for_status()?
        .json()?;
    ensure!(
        query.is_object(),
        "AivisSpeech Engineから正しい音声クエリを取得できません"
    );
    // Unlike VOICEVOX, kana is the natural original text. Do not replace
    // it with mora readings or alter the engine's style/pitch defaults.
    query["kana"] = serde_json::Value::String(text);
    cancel.check()?;
    progress("AivisSpeechで音声を生成しています".into());
    use std::io::Read;
    let response = client
        .post(crate::providers::endpoint(&base, "synthesis")?)
        .query(&[("speaker", speaker)])
        .json(&query)
        .send()?
        .error_for_status()?;
    let mut bytes = Vec::new();
    response.take(128 * 1024 * 1024).read_to_end(&mut bytes)?;
    // Upstream mora duration fields are dummy zeros. Chunk marks are honest;
    // completed PCM may still be cached when its request was superseded.
    crate::providers::chunk_audio(&bytes, units)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BoundaryPrecision;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    fn receive(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&bytes[..end]);
                let length = header
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|value| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if bytes.len() >= end + 4 + length {
                    return String::from_utf8(bytes).unwrap();
                }
            }
        }
    }
    fn respond(stream: &mut TcpStream, content_type: &str, body: &[u8]) {
        write!(stream,"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",body.len()).unwrap();
        stream.write_all(body).unwrap();
    }
    fn config(url: String) -> ProviderConfig {
        ProviderConfig::AivisSpeech {
            base_url: url,
            speaker: -123,
            engine_path: None,
            auto_start: false,
        }
    }
    #[test]
    fn original_text_in_kana_and_engine_style_defaults_are_preserved() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let configuration = config(format!("http://{}", listener.local_addr().unwrap()));
        let server = std::thread::spawn(move || {
            let (mut query, _) = listener.accept().unwrap();
            let request = receive(&mut query);
            assert!(request.starts_with("POST /audio_query?"));
            assert!(request.contains("speaker=-123"));
            respond(&mut query,"application/json",br#"{"kana":"wrong mora reconstruction","pitchScale":0.0,"intonationScale":1.0,"tempoDynamicsScale":1.0,"volumeScale":1.0,"accent_phrases":[]}"#);
            let (mut synth, _) = listener.accept().unwrap();
            let request = receive(&mut synth);
            assert!(request.starts_with("POST /synthesis?speaker=-123 "));
            let query: serde_json::Value =
                serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
            assert_eq!(query["kana"], "　とうきょうで読みます。");
            assert_eq!(query["pitchScale"], 0.0);
            assert_eq!(query["intonationScale"], 1.0);
            assert_eq!(query["tempoDynamicsScale"], 1.0);
            assert_eq!(query["volumeScale"], 1.0);
            let mut wav = std::io::Cursor::new(Vec::new());
            {
                let mut writer = hound::WavWriter::new(
                    &mut wav,
                    hound::WavSpec {
                        channels: 1,
                        sample_rate: 24_000,
                        bits_per_sample: 16,
                        sample_format: hound::SampleFormat::Int,
                    },
                )
                .unwrap();
                for _ in 0..480 {
                    writer.write_sample(8192i16).unwrap();
                }
                writer.finalize().unwrap();
            }
            respond(&mut synth, "audio/wav", &wav.into_inner());
        });
        let units = [
            SpeechUnit {
                id: 42,
                text: "　とうきょう".into(),
            },
            SpeechUnit {
                id: 99,
                text: "で読みます。".into(),
            },
        ];
        let audio = request_audio(
            &configuration,
            &units,
            &Cancellation::uncancelled(),
            &|_| {},
        )
        .unwrap();
        assert_eq!(audio.samples.len(), 480);
        assert_eq!(audio.boundaries.len(), 1);
        assert_eq!(audio.boundaries[0].precision, BoundaryPrecision::Chunk);
        assert_eq!(audio.boundaries[0].ranges.len(), 2);
        assert_eq!(audio.boundaries[0].ranges[0].id, 42);
        assert_eq!(audio.boundaries[0].ranges[1].id, 99);
        server.join().unwrap();
    }
    #[test]
    fn actual_speaker_styles_keep_signed_identifiers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let configuration = config(format!("http://{}", listener.local_addr().unwrap()));
        let server = std::thread::spawn(move || {
            let (mut manifest, _) = listener.accept().unwrap();
            assert!(receive(&mut manifest).starts_with("GET /engine_manifest "));
            respond(
                &mut manifest,
                "application/json",
                br#"{"name":"AivisSpeech Engine"}"#,
            );
            let (mut speakers, _) = listener.accept().unwrap();
            assert!(receive(&mut speakers).starts_with("GET /speakers "));
            respond(&mut speakers,"application/json",br#"[{"name":"Test Voice","speaker_uuid":"test-uuid","styles":[{"name":"Normal","id":-123},{"name":"Calm","id":456}]}]"#);
        });
        let voices = aivis_voices(&configuration, &Cancellation::uncancelled(), &|_| {}).unwrap();
        assert_eq!(
            voices.iter().map(|voice| voice.id).collect::<Vec<_>>(),
            [-123, 456]
        );
        assert_eq!(voices[0].speaker_uuid, "test-uuid");
        assert_eq!(voices[0].label(), "Test Voice · Normal");
        server.join().unwrap();
    }
    #[test]
    fn another_voicevox_compatible_engine_is_not_used_as_aivis() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let configuration = config(format!("http://{}", listener.local_addr().unwrap()));
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            receive(&mut stream);
            respond(
                &mut stream,
                "application/json",
                br#"{"name":"VOICEVOX Engine"}"#,
            );
        });
        assert!(
            prepare_aivis(&configuration, &Cancellation::uncancelled(), &|_| {})
                .unwrap_err()
                .to_string()
                .contains("AivisSpeech Engineではありません")
        );
        server.join().unwrap();
    }

    #[test]
    #[ignore = "Requires an installed running/lazy-startable AivisSpeech and real Qwen assets"]
    fn installed_aivis_audio_has_real_groups_and_speed_mapping() {
        let units = [
            SpeechUnit {
                id: 42,
                text: "　とうきょうの図書館で、新しい本を読みます。".into(),
            },
            SpeechUnit {
                id: 99,
                text: "まどから入る光が、紙の上を静かに照らしています。".into(),
            },
        ];
        let mut provider = AivisProvider::new(ProviderConfig::aivis_default());
        let audio = provider
            .synthesize(&units, &Cancellation::uncancelled(), &|message| {
                eprintln!("{message}")
            })
            .unwrap();
        assert!(
            audio.boundaries.len() > 5,
            "actual alignment must not silently become one chunk"
        );
        assert!(
            audio
                .boundaries
                .iter()
                .all(|boundary| boundary.precision == BoundaryPrecision::AlignedGroup)
        );
        for id in [42, 99] {
            assert!(
                audio
                    .boundaries
                    .iter()
                    .any(|boundary| boundary.ranges.iter().any(|range| range.id == id))
            );
        }
        for speed in [0.5, 1., 2., 10.] {
            let (_, map) =
                crate::engine::render_audio(&audio.samples, audio.sample_rate, speed, 48_000)
                    .unwrap();
            for boundary in &audio.boundaries {
                assert!(
                    boundary.start_sample < boundary.end_sample
                        && boundary.end_sample <= audio.samples.len() as u64
                );
                let frame = map.destination_at(boundary.start_sample);
                assert!(
                    map.source_at(frame).abs_diff(boundary.start_sample)
                        < audio.sample_rate as u64 / 20
                );
            }
        }
    }
}
