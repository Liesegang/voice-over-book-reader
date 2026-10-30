//! Standalone recorded-audio playback. This path has no TTS or book-text alignment.
use crate::{
    audio::{Player, TimeMap},
    engine::render_audio_channels,
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};
use symphonia::core::{
    audio::SampleBuffer,
    codecs::{CODEC_TYPE_NULL, Decoder, DecoderOptions},
    errors::Error as DecodeError,
    formats::{FormatOptions, FormatReader, SeekMode, SeekTo},
    io::MediaSourceStream,
    meta::{MetadataOptions, StandardTagKey},
    probe::Hint,
    units::{Time, TimeBase},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudiobookChapter {
    pub title: String,
    pub start: Duration,
    pub end: Option<Duration>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudiobookInfo {
    pub path: PathBuf,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<Duration>,
    pub sample_rate: u32,
    pub channels: u16,
    pub chapters: Vec<AudiobookChapter>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudiobookState {
    Stopped,
    Preparing,
    Playing,
    Paused,
    Ended,
}
#[derive(Clone, Debug)]
pub enum AudiobookEvent {
    State(AudiobookState),
    Position {
        position: Duration,
        duration: Option<Duration>,
    },
    Error(String),
}
#[derive(Clone, Copy)]
enum Command {
    Play,
    Pause,
    Stop(u64),
    Seek(u64, Duration),
    Speed(u64, f32),
    Close,
}

pub struct AudiobookPlayer {
    info: AudiobookInfo,
    commands: Sender<Command>,
    events: Receiver<(u64, AudiobookEvent)>,
    epoch: Arc<AtomicU64>,
    position: Arc<AtomicU64>,
}
impl AudiobookPlayer {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let decoder = FileDecoder::open(path.as_ref())?;
        let info = decoder.info.clone();
        let (commands, rx) = mpsc::channel();
        let (tx, events) = mpsc::channel();
        let epoch = Arc::new(AtomicU64::new(1));
        let position = Arc::new(AtomicU64::new(0));
        let worker_epoch = epoch.clone();
        let worker_position = position.clone();
        thread::Builder::new()
            .name("reader-audiobook".into())
            .spawn(move || playback_loop(decoder, rx, tx, worker_epoch, worker_position))?;
        Ok(Self {
            info,
            commands,
            events,
            epoch,
            position,
        })
    }
    pub fn info(&self) -> &AudiobookInfo {
        &self.info
    }
    pub fn position(&self) -> Duration {
        Duration::from_millis(self.position.load(Ordering::Acquire))
    }
    pub fn play(&self) {
        let _ = self.commands.send(Command::Play);
    }
    pub fn pause(&self) {
        let _ = self.commands.send(Command::Pause);
    }
    pub fn stop(&self) {
        let generation = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        self.position.store(0, Ordering::Release);
        let _ = self.commands.send(Command::Stop(generation));
    }
    pub fn seek(&self, position: Duration) -> Result<()> {
        let position = self
            .info
            .duration
            .map_or(position, |duration| position.min(duration));
        let generation = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        self.position.store(
            position.as_millis().min(u64::MAX as u128) as u64,
            Ordering::Release,
        );
        self.commands.send(Command::Seek(generation, position))?;
        Ok(())
    }
    pub fn set_speed(&self, speed: f32) -> Result<()> {
        ensure!(
            speed.is_finite() && (0.5..=10.0).contains(&speed),
            "再生速度は0.5〜10.0倍です"
        );
        let generation = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        self.commands.send(Command::Speed(generation, speed))?;
        Ok(())
    }
    pub fn try_recv(&self) -> Option<AudiobookEvent> {
        while let Ok((generation, event)) = self.events.try_recv() {
            if generation == self.epoch.load(Ordering::Acquire) {
                return Some(event);
            }
        }
        None
    }
}
impl Drop for AudiobookPlayer {
    fn drop(&mut self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        let _ = self.commands.send(Command::Close);
    }
}

pub fn inspect_audiobook(path: impl AsRef<Path>) -> Result<AudiobookInfo> {
    Ok(FileDecoder::open(path.as_ref())?.info)
}

struct FileDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    time_base: TimeBase,
    start_ts: u64,
    info: AudiobookInfo,
    skip_before: f64,
    eof: bool,
    decoded_end: f64,
    playback_channels: usize,
}
struct AudioChunk {
    samples: Vec<f32>,
    start: f64,
}
impl FileDecoder {
    fn open(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .with_context(|| format!("音声ファイルを開けません: {}", path.display()))?;
        let source = MediaSourceStream::new(Box::new(File::open(&path)?), Default::default());
        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(|s| s.to_str()) {
            hint.with_extension(extension);
        }
        let mut probed = symphonia::default::get_probe()
            .format(
                &hint,
                source,
                &FormatOptions {
                    enable_gapless: true,
                    ..Default::default()
                },
                &MetadataOptions::default(),
            )
            .context("MP3/M4A/M4B/WAVの音声を読み取れません")?;
        let track = probed
            .format
            .default_track()
            .filter(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .or_else(|| {
                probed
                    .format
                    .tracks()
                    .iter()
                    .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            })
            .context("再生可能な音声トラックがありません")?;
        let params = track.codec_params.clone();
        let track_id = track.id;
        let sample_rate = params
            .sample_rate
            .context("音声サンプルレートを読み取れません")?;
        let channels = params.channels.map_or(1, |c| c.count()) as u16;
        let time_base = params.time_base.unwrap_or(TimeBase::new(1, sample_rate));
        let duration = params
            .n_frames
            .map(|frames| time_duration(time_base.calc_time(frames)));
        let decoder = symphonia::default::get_codecs()
            .make(&params, &DecoderOptions::default())
            .context("この音声コーデックには対応していません")?;
        let mut info = AudiobookInfo {
            path: path.clone(),
            title: path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
            artist: None,
            album: None,
            duration,
            sample_rate,
            channels,
            chapters: vec![],
        };
        if let Some(metadata) = probed.metadata.get().and_then(|m| m.current().cloned()) {
            read_tags(&mut info, metadata.tags());
        }
        if let Some(metadata) = probed.format.metadata().current() {
            read_tags(&mut info, metadata.tags());
        }
        for cue in probed.format.cues() {
            let title = cue
                .tags
                .iter()
                .find(|t| t.std_key == Some(StandardTagKey::TrackTitle))
                .map(|t| t.value.to_string())
                .unwrap_or_else(|| format!("Chapter {}", cue.index + 1));
            info.chapters.push(AudiobookChapter {
                title,
                start: time_duration(time_base.calc_time(cue.start_ts)),
                end: None,
            });
        }
        read_container_tags(&mut info);
        info.chapters.sort_by_key(|c| c.start);
        info.chapters.dedup_by_key(|c| c.start);
        for i in 0..info.chapters.len() {
            info.chapters[i].end = info.chapters.get(i + 1).map(|c| c.start).or(info.duration);
        }
        let playback_channels = if channels == 2 { 2 } else { 1 };
        Ok(Self {
            format: probed.format,
            decoder,
            track_id,
            time_base,
            start_ts: params.start_ts,
            info,
            skip_before: 0.0,
            eof: false,
            decoded_end: 0.0,
            playback_channels,
        })
    }
    fn seek(&mut self, position: Duration) -> Result<()> {
        // Container seek tables commonly reject the exact EOF timestamp. An
        // explicit endpoint seek needs no decoding; a later seek resets it.
        if self
            .info
            .duration
            .is_some_and(|duration| position >= duration)
        {
            self.decoder.reset();
            self.skip_before = position.as_secs_f64();
            self.decoded_end = position.as_secs_f64();
            self.eof = true;
            return Ok(());
        }
        self.format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: Time::from(position.as_secs_f64()),
                    track_id: Some(self.track_id),
                },
            )
            .context("この音声の指定位置へ移動できません")?;
        self.decoder.reset();
        self.skip_before = position.as_secs_f64();
        self.eof = false;
        self.decoded_end = position.as_secs_f64();
        Ok(())
    }
    /// At most roughly two seconds plus one codec packet. Never decodes the file
    /// into an unbounded Vec, even when the audiobook is many hours long.
    fn next_chunk(&mut self) -> Result<Option<AudioChunk>> {
        if self.eof {
            return Ok(None);
        }
        let target = self.info.sample_rate as usize * 2 * self.playback_channels;
        let mut samples = Vec::with_capacity(target + 8192);
        let mut start = None;
        while samples.len() < target {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(DecodeError::IoError(error))
                    if error.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    self.eof = true;
                    break;
                }
                Err(error) => return Err(error.into()),
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let packet_start = time_seconds(
                self.time_base
                    .calc_time(packet.ts().saturating_sub(self.start_ts)),
            );
            let decoded = self.decoder.decode(&packet)?;
            let rate = decoded.spec().rate;
            let channels = decoded.spec().channels.count();
            ensure!(
                rate == self.info.sample_rate,
                "音声の途中でサンプルレートが変わりました"
            );
            ensure!(channels > 0 && channels <= 32, "音声チャンネル数が不正です");
            ensure!(
                decoded.capacity() <= rate as usize * 10,
                "音声の1パケットが大きすぎます"
            );
            let mut converted =
                SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
            converted.copy_interleaved_ref(decoded);
            let frames = converted.samples().len() / channels;
            self.decoded_end = packet_start + frames as f64 / rate as f64;
            let skip = ((self.skip_before - packet_start).max(0.0) * rate as f64).round() as usize;
            if skip >= frames {
                continue;
            }
            start.get_or_insert(packet_start + skip as f64 / rate as f64);
            for frame in converted.samples().chunks_exact(channels).skip(skip) {
                if channels == 2 && self.playback_channels == 2 {
                    samples.extend_from_slice(frame);
                } else {
                    samples.push(frame.iter().sum::<f32>() / channels as f32);
                }
            }
        }
        if samples.is_empty() {
            return Ok(None);
        }
        ensure!(
            samples.iter().all(|s| s.is_finite()),
            "音声デコーダが不正なPCMを返しました"
        );
        Ok(Some(AudioChunk {
            samples,
            start: start.unwrap_or(self.decoded_end),
        }))
    }
}
fn time_seconds(time: Time) -> f64 {
    time.seconds as f64 + time.frac
}
fn time_duration(time: Time) -> Duration {
    Duration::from_secs_f64(time_seconds(time).max(0.0))
}
fn read_tags(info: &mut AudiobookInfo, tags: &[symphonia::core::meta::Tag]) {
    for tag in tags {
        match tag.std_key {
            Some(StandardTagKey::TrackTitle) => info.title = tag.value.to_string(),
            Some(StandardTagKey::Artist) => info.artist = Some(tag.value.to_string()),
            Some(StandardTagKey::Album) => info.album = Some(tag.value.to_string()),
            _ => {}
        }
    }
}
fn read_container_tags(info: &mut AudiobookInfo) {
    let extension = info
        .path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(extension.as_str(), "m4a" | "m4b" | "mp4") {
        let config = mp4ameta::ReadConfig {
            read_image_data: false,
            ..Default::default()
        };
        if let Ok(tag) = mp4ameta::Tag::read_with_path(&info.path, &config) {
            if let Some(title) = tag.title() {
                info.title = title.into();
            }
            if let Some(artist) = tag.artist() {
                info.artist = Some(artist.into());
            }
            if let Some(album) = tag.album() {
                info.album = Some(album.into());
            }
            if info.duration.is_none() && !tag.duration().is_zero() {
                info.duration = Some(tag.duration());
            }
            let chapters = if tag.chapter_track().is_empty() {
                tag.chapter_list()
            } else {
                tag.chapter_track()
            };
            if !chapters.is_empty() {
                info.chapters = chapters
                    .iter()
                    .map(|chapter| AudiobookChapter {
                        title: chapter.title.clone(),
                        start: chapter.start,
                        end: None,
                    })
                    .collect();
            }
        }
    } else if extension == "mp3" {
        use id3::TagLike;
        if let Ok(tag) = id3::Tag::read_from_path(&info.path) {
            if let Some(title) = tag.title() {
                info.title = title.into();
            }
            if let Some(artist) = tag.artist() {
                info.artist = Some(artist.into());
            }
            if let Some(album) = tag.album() {
                info.album = Some(album.into());
            }
            let chapters: Vec<_> = tag
                .chapters()
                .map(|chapter| AudiobookChapter {
                    title: chapter.title().unwrap_or(&chapter.element_id).into(),
                    start: Duration::from_millis(chapter.start_time as u64),
                    end: (chapter.end_time != u32::MAX)
                        .then(|| Duration::from_millis(chapter.end_time as u64)),
                })
                .collect();
            if !chapters.is_empty() {
                info.chapters = chapters;
            }
        }
    }
}

struct Buffered {
    map: TimeMap,
    origin: f64,
    rate: u32,
    channels: usize,
}
impl Buffered {
    fn position(&self, player: &Player) -> Duration {
        Duration::from_secs_f64(
            (self.origin + self.map.source_at(player.position()) as f64 / self.rate as f64)
                .max(0.0),
        )
    }
    fn append(&mut self, chunk: AudioChunk, speed: f32, player: &Player) -> Result<()> {
        let (samples, map) = render_audio_channels(
            &chunk.samples,
            self.rate,
            self.channels,
            speed,
            player.sample_rate,
        )?;
        let source = self.map.source_len;
        let destination = self.map.output_len;
        self.map.spans.extend(map.spans.into_iter().map(|mut span| {
            span.src_start += source;
            span.src_end += source;
            span.dst_start += destination;
            span.dst_end += destination;
            span
        }));
        self.map.source_len += map.source_len;
        self.map.output_len += map.output_len;
        player.append(&samples);
        Ok(())
    }
    fn discard_history(&mut self, player: &Player) {
        let source = self.map.source_at(player.position());
        if source < self.rate as u64 * 12 {
            return;
        }
        let retain = source - self.rate as u64 * 4;
        let Some(cut) = self
            .map
            .spans
            .iter()
            .rev()
            .find(|s| s.src_start <= retain && s.dst_start <= player.position())
            .cloned()
        else {
            return;
        };
        if cut.dst_start == 0 {
            return;
        }
        self.map.spans.retain(|span| span.src_end > cut.src_start);
        for span in &mut self.map.spans {
            span.src_start -= cut.src_start;
            span.src_end -= cut.src_start;
            span.dst_start -= cut.dst_start;
            span.dst_end -= cut.dst_start;
        }
        self.map.source_len -= cut.src_start;
        self.map.output_len -= cut.dst_start;
        self.origin += cut.src_start as f64 / self.rate as f64;
        player.discard_prefix(cut.dst_start);
    }
}
fn send(events: &Sender<(u64, AudiobookEvent)>, generation: u64, event: AudiobookEvent) {
    let _ = events.send((generation, event));
}
fn playback_loop(
    mut decoder: FileDecoder,
    commands: Receiver<Command>,
    events: Sender<(u64, AudiobookEvent)>,
    epoch: Arc<AtomicU64>,
    position: Arc<AtomicU64>,
) {
    let mut generation = epoch.load(Ordering::Acquire);
    let mut player: Option<Player> = None;
    let mut buffered: Option<Buffered> = None;
    let mut playing = false;
    let mut ended = false;
    let mut speed = 1.0;
    let mut last_event = Instant::now();
    loop {
        let command = match commands.recv_timeout(Duration::from_millis(10)) {
            Ok(command) => Some(command),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(_) => break,
        };
        let action = (|| -> Result<()> {
            match command {
                Some(Command::Close) => return Ok(()),
                Some(Command::Play) => {
                    if ended {
                        decoder.seek(Duration::ZERO)?;
                        buffered = None;
                        position.store(0, Ordering::Release);
                        ended = false;
                    }
                    playing = true;
                    if let Some(player) = &player {
                        player.resume();
                    }
                    send(
                        &events,
                        generation,
                        AudiobookEvent::State(if buffered.is_some() {
                            AudiobookState::Playing
                        } else {
                            AudiobookState::Preparing
                        }),
                    );
                }
                Some(Command::Pause) => {
                    playing = false;
                    if let Some(player) = &player {
                        player.pause();
                    }
                    send(
                        &events,
                        generation,
                        AudiobookEvent::State(AudiobookState::Paused),
                    );
                }
                Some(Command::Stop(next)) => {
                    generation = next;
                    playing = false;
                    ended = false;
                    if let Some(player) = &player {
                        player.stop();
                    }
                    buffered = None;
                    decoder.seek(Duration::ZERO)?;
                    position.store(0, Ordering::Release);
                    send(
                        &events,
                        generation,
                        AudiobookEvent::State(AudiobookState::Stopped),
                    );
                }
                Some(Command::Seek(next, target)) => {
                    generation = next;
                    ended = false;
                    if let Some(player) = &player {
                        player.stop();
                    }
                    buffered = None;
                    decoder.seek(target)?;
                    position.store(
                        target.as_millis().min(u64::MAX as u128) as u64,
                        Ordering::Release,
                    );
                }
                Some(Command::Speed(next, value)) => {
                    let current = buffered
                        .as_ref()
                        .zip(player.as_ref())
                        .map(|(b, p)| b.position(p))
                        .unwrap_or_else(|| Duration::from_millis(position.load(Ordering::Acquire)));
                    generation = next;
                    speed = value;
                    if let Some(player) = &player {
                        player.stop();
                    }
                    buffered = None;
                    decoder.seek(current)?;
                }
                None => {}
            }
            if !playing {
                return Ok(());
            }
            if player.is_none() {
                player = Some(Player::new(epoch.clone())?);
            }
            let output = player.as_ref().unwrap();
            if let Some(error) = output.error() {
                bail!(error);
            }
            let need_audio = buffered.as_ref().is_none_or(|b| {
                b.map
                    .source_len
                    .saturating_sub(b.map.source_at(output.position()))
                    < decoder.info.sample_rate as u64 * 6
            });
            if need_audio
                && !decoder.eof
                && let Some(chunk) = decoder.next_chunk()?
            {
                if generation != epoch.load(Ordering::Acquire) {
                    return Ok(());
                }
                if let Some(buffer) = &mut buffered {
                    buffer.append(chunk, speed, output)?;
                } else {
                    let (samples, map) = render_audio_channels(
                        &chunk.samples,
                        decoder.info.sample_rate,
                        decoder.playback_channels,
                        speed,
                        output.sample_rate,
                    )?;
                    output.play_interleaved(samples, decoder.playback_channels, generation);
                    buffered = Some(Buffered {
                        map,
                        origin: chunk.start,
                        rate: decoder.info.sample_rate,
                        channels: decoder.playback_channels,
                    });
                    send(
                        &events,
                        generation,
                        AudiobookEvent::State(AudiobookState::Playing),
                    );
                }
            }
            if let Some(buffer) = &mut buffered {
                let current = buffer.position(output);
                position.store(
                    current.as_millis().min(u64::MAX as u128) as u64,
                    Ordering::Release,
                );
                if last_event.elapsed() >= Duration::from_millis(50) {
                    send(
                        &events,
                        generation,
                        AudiobookEvent::Position {
                            position: current,
                            duration: decoder.info.duration,
                        },
                    );
                    last_event = Instant::now();
                }
                buffer.discard_history(output);
                if decoder.eof && output.position() >= buffer.map.output_len {
                    playing = false;
                    ended = true;
                    output.stop();
                    send(
                        &events,
                        generation,
                        AudiobookEvent::State(AudiobookState::Ended),
                    );
                }
            } else if decoder.eof {
                playing = false;
                ended = true;
                send(
                    &events,
                    generation,
                    AudiobookEvent::State(AudiobookState::Ended),
                );
            }
            Ok(())
        })();
        if matches!(command, Some(Command::Close)) {
            break;
        }
        if let Err(error) = action {
            playing = false;
            if let Some(player) = &player {
                player.stop();
            }
            send(
                &events,
                generation,
                AudiobookEvent::Error(format!("{error:#}")),
            );
        }
    }
    if let Some(player) = player {
        player.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(seconds: u32) -> tempfile::NamedTempFile {
        let file = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        let mut writer = hound::WavWriter::create(
            file.path(),
            hound::WavSpec {
                channels: 1,
                sample_rate: 8000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for i in 0..8000 * seconds {
            writer
                .write_sample(if i / 8000 < 5 { 1000i16 } else { -1000 })
                .unwrap();
        }
        writer.finalize().unwrap();
        file
    }
    #[test]
    fn recorded_audio_metadata_and_seek_use_real_frames() {
        let file = fixture(20);
        let original = std::fs::read(file.path()).unwrap();
        let mut decoder = FileDecoder::open(file.path()).unwrap();
        assert_eq!(decoder.info.duration, Some(Duration::from_secs(20)));
        assert_eq!(decoder.info.sample_rate, 8000);
        let chunk = decoder.next_chunk().unwrap().unwrap();
        assert!(chunk.samples.len() <= 8000 * 3);
        assert!(chunk.samples[0] > 0.0);
        decoder.seek(Duration::from_secs(7)).unwrap();
        let chunk = decoder.next_chunk().unwrap().unwrap();
        assert!((chunk.start - 7.0).abs() < 0.001);
        assert!(chunk.samples[0] < 0.0);
        assert_eq!(std::fs::read(file.path()).unwrap(), original);
    }
    #[test]
    fn decoder_keeps_small_batches_for_longer_input() {
        let file = fixture(90);
        let mut decoder = FileDecoder::open(file.path()).unwrap();
        let chunk = decoder.next_chunk().unwrap().unwrap();
        assert!(chunk.samples.len() < 8000 * 4);
        assert!(!decoder.eof);
        assert!(decoder.decoded_end < 4.0);
    }
    #[test]
    fn stereo_file_preserves_channels_without_doubling_media_clock() {
        let file = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        let mut writer = hound::WavWriter::create(
            file.path(),
            hound::WavSpec {
                channels: 2,
                sample_rate: 8000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for _ in 0..8000 * 10 {
            writer.write_sample(2000i16).unwrap();
            writer.write_sample(-4000i16).unwrap();
        }
        writer.finalize().unwrap();
        let mut decoder = FileDecoder::open(file.path()).unwrap();
        assert_eq!(decoder.info.channels, 2);
        assert_eq!(decoder.info.duration, Some(Duration::from_secs(10)));
        decoder.seek(Duration::from_secs(3)).unwrap();
        let chunk = decoder.next_chunk().unwrap().unwrap();
        assert!((chunk.start - 3.0).abs() < 0.001);
        assert!(chunk.samples.len() <= 8000 * 2 * 3);
        assert!(
            chunk
                .samples
                .chunks_exact(2)
                .all(|frame| frame[0] > 0.0 && frame[1] < 0.0)
        );
    }
    #[test]
    fn mp3_and_aac_chapter_fixtures_decode_and_seek_without_external_tools() {
        let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
        for filename in ["tone.mp3", "chapters.m4b"] {
            let path = data.join(filename);
            let original = std::fs::read(&path).unwrap();
            let mut decoder = FileDecoder::open(&path).unwrap();
            assert_eq!(decoder.info.sample_rate, 24_000);
            assert!((decoder.info.duration.unwrap().as_secs_f64() - 2.0).abs() < 0.1);
            if filename.ends_with("m4b") {
                assert_eq!(decoder.info.title, "Native chapter fixture");
                assert_eq!(decoder.info.chapters.len(), 2);
                assert_eq!(decoder.info.chapters[1].title, "Second");
                assert_eq!(decoder.info.chapters[1].start, Duration::from_secs(1));
            } else {
                assert_eq!(decoder.info.title, "Native MP3 fixture");
            }
            decoder.seek(Duration::from_secs(1)).unwrap();
            let chunk = decoder.next_chunk().unwrap().unwrap();
            assert!(
                (chunk.start - 1.0).abs() < 1.0 / 24_000.0,
                "{filename}: {}",
                chunk.start
            );
            assert!(chunk.samples.iter().any(|sample| sample.abs() > 0.001));
            assert!((chunk.samples.len() as f64 / 24_000.0 - 1.0).abs() < 0.1);
            decoder.seek(decoder.info.duration.unwrap()).unwrap();
            assert!(
                decoder
                    .next_chunk()
                    .unwrap()
                    .is_none_or(|chunk| chunk.samples.len() <= 2048)
            );
            assert_eq!(std::fs::read(&path).unwrap(), original);
        }
    }
    #[test]
    #[ignore = "manual probe: requires an actual audio output device"]
    fn hardware_pause_seek_and_speed_use_the_audible_media_clock() {
        let file = fixture(20);
        let player = AudiobookPlayer::open(file.path()).unwrap();
        player.play();
        let deadline = Instant::now() + Duration::from_secs(5);
        while player.position() < Duration::from_millis(100) {
            if let Some(AudiobookEvent::Error(error)) = player.try_recv() {
                panic!("{error}");
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        player.pause();
        thread::sleep(Duration::from_millis(100));
        let paused = player.position();
        thread::sleep(Duration::from_millis(100));
        assert_eq!(player.position(), paused);
        player.seek(Duration::from_secs(18)).unwrap();
        player.set_speed(10.0).unwrap();
        player.play();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut ended = false;
        while Instant::now() < deadline && !ended {
            while let Some(event) = player.try_recv() {
                match event {
                    AudiobookEvent::Error(error) => panic!("{error}"),
                    AudiobookEvent::State(AudiobookState::Ended) => ended = true,
                    _ => {}
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(ended);
        assert!(player.position() >= Duration::from_millis(19_900));
        player.stop();
        assert_eq!(player.position(), Duration::ZERO);
    }
}
