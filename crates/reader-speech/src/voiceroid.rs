//! Native installed VOICEROID2 integration through a headless 32-bit Rust host.
use crate::providers::decode_wav;
use crate::{providers::TtsProvider, types::*};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct VoiceroidVoice {
    pub id: String,
    pub label: String,
}
pub fn voiceroid2_voices(install: &Path) -> Vec<VoiceroidVoice> {
    let Ok(entries) = std::fs::read_dir(install.join("Voice")) else {
        return vec![];
    };
    let mut voices = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let id = e.file_name().to_str()?.to_owned();
            if !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return None;
            }
            let label = match id.as_str() {
                "yukari_emo_44" => "結月ゆかり",
                "aoi_emo_44" => "琴葉 葵",
                "akane_west_emo_44" => "琴葉 茜",
                "akari_44" => "紲星あかり",
                _ => &id,
            }
            .to_owned();
            Some(VoiceroidVoice { id, label })
        })
        .collect::<Vec<_>>();
    voices.sort_by(|a, b| a.id.cmp(&b.id));
    voices
}
fn is_x86_executable(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    (|| -> std::io::Result<bool> {
        let mut file = std::fs::File::open(path)?;
        let mut dos = [0; 64];
        file.read_exact(&mut dos)?;
        if &dos[..2] != b"MZ" {
            return Ok(false);
        }
        let pe = u32::from_le_bytes(dos[60..64].try_into().unwrap());
        if pe > 1024 * 1024 {
            return Ok(false);
        }
        file.seek(SeekFrom::Start(pe as u64))?;
        let mut header = [0; 6];
        file.read_exact(&mut header)?;
        Ok(&header[..4] == b"PE\0\0" && u16::from_le_bytes([header[4], header[5]]) == 0x14c)
    })()
    .unwrap_or(false)
}
pub fn voiceroid2_helper_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("VOBR_VOICEROID_BRIDGE")
        .map(PathBuf::from)
        .filter(|path| is_x86_executable(path))
    {
        return Some(path);
    }
    let exe = std::env::current_exe().ok()?;
    let directory = exe.parent()?;
    let direct = directory.join("reader-voiceroid-bridge.exe");
    if is_x86_executable(&direct) {
        return Some(direct);
    }
    for parent in directory.ancestors().take(4) {
        for profile in ["debug", "release"] {
            let path = parent
                .join("i686-pc-windows-msvc")
                .join(profile)
                .join("reader-voiceroid-bridge.exe");
            if is_x86_executable(&path) {
                return Some(path);
            }
        }
    }
    None
}
struct Host {
    child: Child,
    replies: mpsc::Receiver<String>,
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
pub(crate) struct VoiceroidProvider {
    config: ProviderConfig,
    host: Option<Host>,
}
impl VoiceroidProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self { config, host: None }
    }
    fn start(&mut self, install: &Path) -> Result<()> {
        if self
            .host
            .as_mut()
            .is_some_and(|host| host.child.try_wait().is_ok_and(|s| s.is_none()))
        {
            return Ok(());
        }
        self.host = None;
        let helper=voiceroid2_helper_path().context("32bitのreader-voiceroid-bridge.exeがありません。配布フォルダーの構成を確認してください")?;
        let mut command = Command::new(helper);
        command
            .current_dir(install)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        crate::providers::hide_window(&mut command);
        let mut child = command
            .spawn()
            .context("VOICEROID2のDLLホストを起動できません")?;
        let stdout = child
            .stdout
            .take()
            .context("VOICEROID2ホストの出力がありません")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) if line.len() <= 128 * 1024 => {
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                    _ => break,
                }
            }
        });
        self.host = Some(Host { child, replies: rx });
        Ok(())
    }
}
#[derive(Deserialize)]
struct Reply {
    ok: bool,
    error: Option<String>,
    audio: Option<AudioReply>,
}
#[derive(Deserialize)]
struct AudioReply {
    sample_rate: u32,
    samples: usize,
    marks: Vec<Position>,
    replaced_characters: usize,
}
#[derive(Clone, Deserialize)]
struct Position {
    tick_ms: u64,
    byte_offset: usize,
}
impl TtsProvider for VoiceroidProvider {
    fn name(&self) -> &str {
        "VOICEROID2 · native DLL"
    }
    fn synthesize(
        &mut self,
        units: &[SpeechUnit],
        cancel: &Cancellation,
        progress: &dyn Fn(String),
    ) -> Result<SynthesizedAudio> {
        ensure!(cfg!(windows), "VOICEROID2はWindowsで利用できます");
        cancel.check()?;
        let ProviderConfig::Voiceroid2 {
            install_dir,
            voice,
            volume,
            pitch,
            range,
        } = self.config.clone()
        else {
            bail!("Invalid VOICEROID2 profile");
        };
        ensure!(
            install_dir.join("aitalked.dll").is_file(),
            "VOICEROID2のインストール先が見つかりません"
        );
        self.start(&install_dir)?;
        let temp = tempfile::tempdir()?;
        let wav = temp.path().join("speech.wav");
        let text = units
            .iter()
            .map(|unit| unit.text.as_str())
            .collect::<String>();
        let request = serde_json::json!({"install_dir":install_dir,"voice":voice,"text":text,"output_wav":wav,"volume":volume,"pitch":pitch,"range":range});
        let input = self
            .host
            .as_mut()
            .unwrap()
            .child
            .stdin
            .as_mut()
            .context("VOICEROID2ホストが終了しました")?;
        if let Err(error) = writeln!(input, "{request}").and_then(|_| input.flush()) {
            self.host = None;
            return Err(error).context("VOICEROID2へ文章を渡せません");
        }
        progress("VOICEROID2の音声DLLで生成しています".into());
        let started = Instant::now();
        let reply = loop {
            if cancel.is_cancelled() {
                self.host = None;
                cancel.check()?;
            }
            if started.elapsed() > Duration::from_secs(130) {
                self.host = None;
                bail!("VOICEROID2の生成がタイムアウトしました");
            }
            match self
                .host
                .as_ref()
                .unwrap()
                .replies
                .recv_timeout(Duration::from_millis(20))
            {
                Ok(line) => {
                    if let Ok(reply) = serde_json::from_str::<Reply>(&line) {
                        break reply;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.host = None;
                    bail!("VOICEROID2のDLLホストが終了しました");
                }
            }
        };
        if !reply.ok {
            self.host = None;
            bail!(
                "VOICEROID2: {}",
                reply.error.unwrap_or_else(|| "音声を生成できません".into())
            );
        }
        cancel.check()?;
        let info = reply.audio.context("VOICEROID2の生成結果がありません")?;
        let (samples, sample_rate) = decode_wav(&std::fs::read(&wav)?)?;
        ensure!(
            sample_rate == info.sample_rate && samples.len() == info.samples,
            "VOICEROID2のPCM情報が一致しません"
        );
        let mut boundaries =
            position_boundaries(&info.marks, units, sample_rate, samples.len()).unwrap_or_default();
        if boundaries.is_empty() {
            boundaries.push(TimedBoundary {
                start_sample: 0,
                end_sample: samples.len() as u64,
                ranges: unit_ranges(units),
                precision: BoundaryPrecision::Chunk,
            });
        }
        if info.replaced_characters > 0 {
            progress(format!(
                "VOICEROID2の文字コードにない{}文字は空白として扱いました",
                info.replaced_characters
            ));
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
fn position_boundaries(
    marks: &[Position],
    units: &[SpeechUnit],
    rate: u32,
    samples: usize,
) -> Result<Vec<TimedBoundary>> {
    let text = units
        .iter()
        .map(|unit| unit.text.as_str())
        .collect::<String>();
    let mut from = 0;
    let mut boundaries = vec![];
    for (index, mark) in marks.iter().enumerate() {
        ensure!(
            mark.byte_offset >= from && text.is_char_boundary(mark.byte_offset),
            "Invalid VOICEROID2 source position"
        );
        let start = mark
            .tick_ms
            .checked_mul(rate as u64)
            .context("Invalid VOICEROID2 tick")?
            / 1000;
        let end = marks
            .get(index + 1)
            .map(|next| next.tick_ms.saturating_mul(rate as u64) / 1000)
            .unwrap_or(samples as u64);
        ensure!(
            start <= end && end <= samples as u64,
            "VOICEROID2 tick exceeds PCM"
        );
        if start < end && from < mark.byte_offset {
            boundaries.push(TimedBoundary {
                start_sample: start,
                end_sample: end,
                ranges: ranges_for_text(units, from, mark.byte_offset),
                precision: BoundaryPrecision::AlignedGroup,
            });
        }
        from = mark.byte_offset;
    }
    ensure!(from == text.len(), "Incomplete VOICEROID2 position events");
    Ok(boundaries)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_auto_bookmarks_are_end_offsets_at_group_start_ticks() {
        let units = [
            SpeechUnit {
                id: 1,
                text: "今日は".into(),
            },
            SpeechUnit {
                id: 2,
                text: "いい天気です。".into(),
            },
        ];
        let marks = [
            Position {
                tick_ms: 0,
                byte_offset: 9,
            },
            Position {
                tick_ms: 394,
                byte_offset: 15,
            },
            Position {
                tick_ms: 606,
                byte_offset: 30,
            },
        ];
        let boundaries = position_boundaries(&marks, &units, 44100, 44100).unwrap();
        assert_eq!(boundaries.len(), 3);
        assert_eq!(
            boundaries[0].ranges,
            vec![UnitRange {
                id: 1,
                start: 0,
                end: 9
            }]
        );
        assert_eq!(
            boundaries[1].ranges,
            vec![UnitRange {
                id: 2,
                start: 0,
                end: 6
            }]
        );
        assert_eq!(
            boundaries[2].ranges,
            vec![UnitRange {
                id: 2,
                start: 6,
                end: 21
            }]
        );
        assert_eq!(boundaries[1].start_sample, 17375);
        assert!(
            position_boundaries(
                &[Position {
                    tick_ms: 0,
                    byte_offset: 1
                }],
                &units,
                44100,
                44100
            )
            .is_err()
        );
    }
}
