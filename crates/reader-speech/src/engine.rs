use crate::{
    audio::{self, Player, TimeMap},
    cache::PreparedAudio,
    providers::create_provider,
    types::*,
};
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

enum Command {
    Speak {
        generation: u64,
        units: Vec<SpeechUnit>,
        start: usize,
        byte_offset: usize,
    },
    Prefetch(u64, Vec<SpeechUnit>),
    Append(u64, Vec<SpeechUnit>),
    Stop(u64),
    Pause,
    Resume,
    Speed(f32),
    Provider(u64, ProviderConfig),
    Shutdown,
}
struct Job {
    generation: u64,
    units: Vec<SpeechUnit>,
    target: Option<(u64, usize)>,
    playback: bool,
    config: SpeechConfig,
}
enum Produced {
    Ready(u64, PreparedAudio),
    Progress(u64, String),
    Done(u64),
    Error(u64, String),
}

pub struct SpeechController {
    commands: Sender<Command>,
    events: Receiver<SpeechEvent>,
    epoch: Arc<AtomicU64>,
}
/// A chapter loader can feed speech without relying on GUI repaint callbacks.
/// Capturing the epoch makes an old loader harmless after stop or point-seek.
#[derive(Clone)]
pub struct SpeechFeeder {
    commands: Sender<Command>,
    epoch: Arc<AtomicU64>,
    generation: u64,
}
impl SpeechFeeder {
    pub fn is_current(&self) -> bool {
        self.generation > 0 && self.epoch.load(Ordering::Acquire) == self.generation
    }
    pub fn append(&self, units: Vec<SpeechUnit>) -> Result<u64> {
        ensure!(self.is_current(), "読み上げ要求は置き換えられました");
        ensure!(
            units.iter().any(|unit| !unit.text.trim().is_empty()),
            "読み上げる本文がありません"
        );
        self.commands
            .send(Command::Append(self.generation, units))?;
        Ok(self.generation)
    }
}
impl SpeechController {
    /// Starts idle native workers. The audio device and model are opened only on use.
    pub fn new(config: SpeechConfig) -> Result<Self> {
        crate::validate_pronunciations(&config.pronunciations)?;
        ensure!(
            config.speed.is_finite() && (0.5..=10.0).contains(&config.speed),
            "読み上げ速度は0.5〜10.0倍です"
        );
        let (commands, rx) = mpsc::channel();
        let (tx, events) = mpsc::channel();
        let epoch = Arc::new(AtomicU64::new(0));
        let worker_epoch = epoch.clone();
        thread::Builder::new()
            .name("reader-speech-playback".into())
            .spawn(move || run(rx, tx, worker_epoch, config))?;
        Ok(Self {
            commands,
            events,
            epoch,
        })
    }
    /// All later requests revoke this generation. start_index is a source unit,
    /// usually the word or atomic ruby group selected by native hit testing.
    pub fn speak(&self, units: Vec<SpeechUnit>, start_index: usize) -> Result<u64> {
        self.speak_at(units, start_index, 0)
    }
    /// Start at a UTF-8 source byte within a unit. For ruby, callers select the
    /// whole atomic reading and pass zero. Event offsets still refer to the
    /// original, uncut unit so hit testing and highlighting share a locator.
    pub fn speak_at(
        &self,
        units: Vec<SpeechUnit>,
        start_index: usize,
        byte_offset: usize,
    ) -> Result<u64> {
        ensure!(
            start_index < units.len(),
            "読み上げ開始位置が本文の範囲外です"
        );
        ensure!(
            byte_offset < units[start_index].text.len()
                && units[start_index].text.is_char_boundary(byte_offset),
            "読み上げ開始文字が不正です"
        );
        ensure!(
            units.iter().any(|u| !u.text.trim().is_empty()),
            "読み上げる本文がありません"
        );
        let generation = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        self.commands.send(Command::Speak {
            generation,
            units,
            start: start_index,
            byte_offset,
        })?;
        Ok(generation)
    }
    pub fn stop(&self) {
        let generation = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        let _ = self.commands.send(Command::Stop(generation));
    }
    pub fn pause(&self) {
        let _ = self.commands.send(Command::Pause);
    }
    pub fn resume(&self) {
        let _ = self.commands.send(Command::Resume);
    }
    pub fn set_speed(&self, speed: f32) -> Result<()> {
        ensure!(
            speed.is_finite() && (0.5..=10.0).contains(&speed),
            "読み上げ速度は0.5〜10.0倍です"
        );
        self.commands.send(Command::Speed(speed))?;
        Ok(())
    }
    pub fn set_provider(&self, provider: ProviderConfig) -> Result<()> {
        let generation = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        self.commands
            .send(Command::Provider(generation, provider))?;
        Ok(())
    }
    pub fn try_recv(&self) -> Option<SpeechEvent> {
        while let Ok(event) = self.events.try_recv() {
            if event.generation == self.epoch.load(Ordering::Acquire) {
                return Some(event);
            }
        }
        None
    }
    pub fn generation(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }
    /// Warm visible text without playing it. Invoke after an explicit speech-tool
    /// action; a missing built-in model may be downloaded. A later speak/stop
    /// revokes outstanding prefetch work, while completed cache entries survive.
    pub fn prefetch(&self, units: Vec<SpeechUnit>) -> Result<()> {
        self.commands
            .send(Command::Prefetch(self.epoch.load(Ordering::Acquire), units))?;
        Ok(())
    }
    /// Supply an already loaded following chapter without replacing the active
    /// voice, clock, or cancellation generation. Call ahead of the chapter end.
    pub fn append(&self, units: Vec<SpeechUnit>) -> Result<u64> {
        self.feeder().append(units)
    }
    pub fn feeder(&self) -> SpeechFeeder {
        SpeechFeeder {
            commands: self.commands.clone(),
            epoch: self.epoch.clone(),
            generation: self.generation(),
        }
    }
}
impl Drop for SpeechController {
    fn drop(&mut self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        let _ = self.commands.send(Command::Shutdown);
    }
}

fn emit(events: &Sender<SpeechEvent>, generation: u64, kind: SpeechEventKind) {
    let _ = events.send(SpeechEvent { generation, kind });
}
fn state(events: &Sender<SpeechEvent>, generation: u64, value: SpeechState) {
    emit(events, generation, SpeechEventKind::State(value));
}

struct Playing {
    generation: u64,
    original: SynthesizedAudio,
    map: TimeMap,
    source_offset: u64,
    device_len: u64,
    last_boundary: Option<usize>,
}
impl Playing {
    fn source_position(&self, player: &Player) -> u64 {
        self.source_offset + self.map.source_at(player.position())
    }
    fn begin(
        original: SynthesizedAudio,
        source_offset: u64,
        speed: f32,
        player: &Player,
        generation: u64,
    ) -> Result<Self> {
        let source_offset = source_offset.min(original.samples.len() as u64);
        let (device_audio, map) = render_audio(
            &original.samples[source_offset as usize..],
            original.sample_rate,
            speed,
            player.sample_rate,
        )?;
        let device_len = device_audio.len() as u64;
        player.play(device_audio, generation);
        Ok(Self {
            generation,
            original,
            map,
            source_offset,
            device_len,
            last_boundary: None,
        })
    }
    fn append(&mut self, mut next: SynthesizedAudio, speed: f32, player: &Player) -> Result<()> {
        if next.sample_rate != self.original.sample_rate {
            let old_rate = next.sample_rate;
            let new_rate = self.original.sample_rate;
            next.samples = audio::resample(&next.samples, old_rate, new_rate);
            for boundary in &mut next.boundaries {
                boundary.start_sample = boundary.start_sample * new_rate as u64 / old_rate as u64;
                boundary.end_sample = boundary.end_sample * new_rate as u64 / old_rate as u64;
            }
            next.sample_rate = new_rate;
        }
        let (device_audio, map) =
            render_audio(&next.samples, next.sample_rate, speed, player.sample_rate)?;
        let source_shift = self.map.source_len;
        let destination_shift = self.map.output_len;
        let raw_shift = self.original.samples.len() as u64;
        self.map.spans.extend(map.spans.into_iter().map(|mut span| {
            span.src_start += source_shift;
            span.src_end += source_shift;
            span.dst_start += destination_shift;
            span.dst_end += destination_shift;
            span
        }));
        self.map.source_len += map.source_len;
        self.map.output_len += map.output_len;
        self.device_len += device_audio.len() as u64;
        self.original.samples.extend_from_slice(&next.samples);
        for mut boundary in next.boundaries {
            boundary.start_sample += raw_shift;
            boundary.end_sample += raw_shift;
            self.original.boundaries.push(boundary);
        }
        player.append(&device_audio);
        Ok(())
    }
    fn discard_history(&mut self, player: &Player) {
        let position = self.source_position(player);
        let rate = self.original.sample_rate as u64;
        if position < rate * 30 {
            return;
        }
        let retain_from = position - rate * 10;
        let Some(span) = self
            .map
            .spans
            .iter()
            .rev()
            .find(|s| {
                s.src_start + self.source_offset <= retain_from && s.dst_start <= player.position()
            })
            .cloned()
        else {
            return;
        };
        if span.dst_start == 0 {
            return;
        }
        let raw_cut = self.source_offset + span.src_start;
        self.original.samples.drain(..raw_cut as usize);
        self.original.boundaries.retain(|b| b.end_sample > raw_cut);
        for boundary in &mut self.original.boundaries {
            boundary.start_sample = boundary.start_sample.saturating_sub(raw_cut);
            boundary.end_sample -= raw_cut;
        }
        self.map.spans.retain(|s| s.src_end > span.src_start);
        for part in &mut self.map.spans {
            part.src_start -= span.src_start;
            part.src_end -= span.src_start;
            part.dst_start -= span.dst_start;
            part.dst_end -= span.dst_start;
        }
        self.map.source_len -= span.src_start;
        self.map.output_len -= span.dst_start;
        self.device_len -= span.dst_start;
        self.source_offset = 0;
        self.last_boundary = None;
        player.discard_prefix(span.dst_start);
    }
}

pub(crate) fn render_audio(
    samples: &[f32],
    rate: u32,
    speed: f32,
    device_rate: u32,
) -> Result<(Vec<f32>, TimeMap)> {
    render_audio_channels(samples, rate, 1, speed, device_rate)
}
pub(crate) fn render_audio_channels(
    samples: &[f32],
    rate: u32,
    channels: usize,
    speed: f32,
    device_rate: u32,
) -> Result<(Vec<f32>, TimeMap)> {
    let (stretched, mut map) = audio::stretch_channels(samples, rate, channels, speed)?;
    let device_audio = audio::resample_channels(&stretched, rate, device_rate, channels);
    for span in &mut map.spans {
        span.dst_start = span.dst_start * device_rate as u64 / rate as u64;
        span.dst_end = span.dst_end * device_rate as u64 / rate as u64;
    }
    map.spans.retain(|span| span.dst_end > span.dst_start);
    map.output_len = (device_audio.len() / channels) as u64;
    if let Some(last) = map.spans.last_mut() {
        last.dst_end = map.output_len;
    }
    Ok((device_audio, map))
}

fn run(
    commands: Receiver<Command>,
    events: Sender<SpeechEvent>,
    epoch: Arc<AtomicU64>,
    mut config: SpeechConfig,
) {
    let (jobs, job_rx) = mpsc::channel();
    // Only small file handles cross this unbounded channel. PCM lives in the
    // cache, so synthesis can finish the chapter even while playback is paused.
    let (produced_tx, produced) = mpsc::channel();
    let synth_epoch = epoch.clone();
    let _ = thread::Builder::new()
        .name("reader-speech-synthesis".into())
        .spawn(move || synthesize_jobs(job_rx, produced_tx, synth_epoch));
    let mut generation = 0;
    let mut queue = VecDeque::new();
    let mut active: Option<Playing> = None;
    let mut player: Option<Player> = None;
    let mut paused = false;
    let mut producer_done = true;
    let mut pending_jobs = 0usize;
    loop {
        match commands.recv_timeout(Duration::from_millis(10)) {
            Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(Command::Speak {
                generation: next,
                units,
                start,
                byte_offset,
            }) => {
                generation = next;
                active = None;
                queue.clear();
                paused = false;
                producer_done = false;
                pending_jobs = 1;
                if let Some(player) = &player {
                    player.stop();
                }
                state(&events, generation, SpeechState::Preparing);
                let target = Some((units[start].id, byte_offset));
                if jobs
                    .send(Job {
                        generation,
                        units,
                        target,
                        playback: true,
                        config: config.clone(),
                    })
                    .is_err()
                {
                    emit(
                        &events,
                        generation,
                        SpeechEventKind::Error("音声ワーカーが終了しました".into()),
                    );
                    producer_done = true;
                    pending_jobs = 0;
                }
            }
            Ok(Command::Prefetch(id, units)) => {
                let _ = jobs.send(Job {
                    generation: id,
                    units,
                    target: None,
                    playback: false,
                    config: config.clone(),
                });
            }
            Ok(Command::Append(id, units)) if id == epoch.load(Ordering::Acquire) => {
                if generation == 0 {
                    generation = id;
                }
                producer_done = false;
                pending_jobs += 1;
                if jobs
                    .send(Job {
                        generation: id,
                        units,
                        target: None,
                        playback: true,
                        config: config.clone(),
                    })
                    .is_err()
                {
                    pending_jobs = pending_jobs.saturating_sub(1);
                    producer_done = pending_jobs == 0;
                }
            }
            Ok(Command::Append(_, _)) => {}
            Ok(Command::Stop(next)) => {
                generation = next;
                active = None;
                queue.clear();
                producer_done = true;
                pending_jobs = 0;
                paused = false;
                if let Some(player) = &player {
                    player.stop();
                }
                state(&events, generation, SpeechState::Stopped);
                generation = 0;
            }
            Ok(Command::Provider(next, provider)) => {
                generation = next;
                config.provider = provider;
                active = None;
                queue.clear();
                producer_done = true;
                pending_jobs = 0;
                if let Some(player) = &player {
                    player.stop();
                }
                state(&events, generation, SpeechState::Stopped);
                generation = 0;
            }
            Ok(Command::Pause) => {
                paused = true;
                if let Some(player) = &player {
                    player.pause();
                }
                state(&events, generation, SpeechState::Paused);
            }
            Ok(Command::Resume) => {
                paused = false;
                if let Some(player) = &player {
                    player.resume();
                }
                if active.is_some() {
                    state(&events, generation, SpeechState::Playing);
                }
            }
            Ok(Command::Speed(speed)) => {
                config.speed = speed;
                if let (Some(old), Some(player)) = (active.take(), player.as_ref()) {
                    let source_position = old.source_position(player);
                    match Playing::begin(
                        old.original,
                        source_position,
                        speed,
                        player,
                        old.generation,
                    ) {
                        Ok(new) => {
                            active = Some(new);
                            if paused {
                                player.pause();
                            }
                        }
                        Err(error) => emit(
                            &events,
                            generation,
                            SpeechEventKind::Error(error.to_string()),
                        ),
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        // A bounded queue keeps chapter length from becoming unbounded PCM RAM.
        if queue.len() < 2 {
            while queue.len() < 2 {
                match produced.try_recv() {
                    Ok(Produced::Ready(id, audio))
                        if id == generation && id == epoch.load(Ordering::Acquire) =>
                    {
                        queue.push_back(audio)
                    }
                    Ok(Produced::Progress(id, message)) if id == generation => {
                        emit(&events, generation, SpeechEventKind::Progress(message))
                    }
                    Ok(Produced::Done(id)) if id == generation => {
                        pending_jobs = pending_jobs.saturating_sub(1);
                        producer_done = pending_jobs == 0;
                    }
                    Ok(Produced::Error(id, message)) if id == generation => {
                        emit(&events, generation, SpeechEventKind::Error(message));
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        }
        if let (Some(playing), Some(output)) = (active.as_mut(), player.as_ref()) {
            if let Some(error) = output.error() {
                emit(&events, generation, SpeechEventKind::Error(error));
                active = None;
                output.stop();
                continue;
            }
            // Queue ahead before the DAC consumes the current sentence. A new
            // sentence must not add another whole device-latency interval.
            let remaining = playing.original.samples.len() as u64 - playing.source_position(output);
            if remaining
                < (playing.original.sample_rate as f32 * 20.0 * config.speed.max(1.0)) as u64
                && let Some(next) = queue.pop_front()
                && let Err(error) = next
                    .load()
                    .and_then(|audio| playing.append(audio, config.speed, output))
            {
                emit(
                    &events,
                    generation,
                    SpeechEventKind::Error(error.to_string()),
                );
            }
            if !paused {
                playing.discard_history(output);
                let position = playing.source_position(output);
                let boundary = playing
                    .original
                    .boundaries
                    .iter()
                    .position(|b| b.start_sample <= position && position < b.end_sample);
                if boundary != playing.last_boundary {
                    playing.last_boundary = boundary;
                    if let Some(index) = boundary {
                        let boundary = &playing.original.boundaries[index];
                        let mut ids: Vec<_> = boundary.ranges.iter().map(|r| r.id).collect();
                        ids.dedup();
                        emit(
                            &events,
                            generation,
                            SpeechEventKind::Boundary {
                                unit_ids: ids,
                                ranges: boundary.ranges.clone(),
                                precision: boundary.precision,
                            },
                        );
                    }
                }
                if output.position() >= playing.device_len {
                    active = None;
                    output.stop();
                }
            }
        }
        if active.is_none() {
            if let Some(audio) = queue.pop_front() {
                let audio = match audio.load() {
                    Ok(audio) => audio,
                    Err(error) => {
                        emit(
                            &events,
                            generation,
                            SpeechEventKind::Error(format!("先読み音声を読み込めません: {error}")),
                        );
                        continue;
                    }
                };
                if player.is_none() {
                    match Player::new(epoch.clone()) {
                        Ok(output) => player = Some(output),
                        Err(error) => {
                            emit(
                                &events,
                                generation,
                                SpeechEventKind::Error(error.to_string()),
                            );
                            producer_done = true;
                            queue.clear();
                            continue;
                        }
                    }
                }
                let output = player.as_ref().unwrap();
                match Playing::begin(audio, 0, config.speed, output, generation) {
                    Ok(playing) => {
                        active = Some(playing);
                        if paused {
                            output.pause();
                        }
                        state(
                            &events,
                            generation,
                            if paused {
                                SpeechState::Paused
                            } else {
                                SpeechState::Playing
                            },
                        );
                    }
                    Err(error) => emit(
                        &events,
                        generation,
                        SpeechEventKind::Error(error.to_string()),
                    ),
                }
            } else if producer_done && generation != 0 {
                // Completion is emitted once, not on each clock tick.
                state(&events, generation, SpeechState::Finished);
                generation = 0;
            }
        }
    }
    if let Some(player) = player {
        player.stop();
    }
}

#[derive(Clone)]
struct Translation {
    original_id: u64,
    byte_offset: usize,
}
#[derive(Clone)]
struct Chunk {
    units: Vec<SpeechUnit>,
    translation: Vec<Translation>,
}

fn chunks(units: Vec<SpeechUnit>) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut chunk = Chunk {
        units: vec![],
        translation: vec![],
    };
    let mut count = 0;
    for unit in units {
        let mut beginning = 0;
        let mut since_cut = 0;
        for (offset, ch) in unit.text.char_indices() {
            since_cut += 1;
            let end = offset + ch.len_utf8();
            if matches!(ch, '。' | '！' | '？' | '\n') || since_cut >= 100 || end == unit.text.len()
            {
                if count + since_cut > 100 && !chunk.units.is_empty() {
                    chunks.push(chunk);
                    chunk = Chunk {
                        units: vec![],
                        translation: vec![],
                    };
                    count = 0;
                }
                let part = &unit.text[beginning..end];
                if !part.trim().is_empty() {
                    let id = chunk.translation.len() as u64;
                    chunk.units.push(SpeechUnit {
                        id,
                        text: part.into(),
                    });
                    chunk.translation.push(Translation {
                        original_id: unit.id,
                        byte_offset: beginning,
                    });
                    count += since_cut;
                }
                beginning = end;
                since_cut = 0;
                // Amortize the acoustic model/aligner setup across short
                // sentences. Keep the native Japanese 100-character window.
                if (ch == '\n' || (matches!(ch, '。' | '！' | '？') && count >= 80) || count >= 100)
                    && !chunk.units.is_empty()
                {
                    chunks.push(chunk);
                    chunk = Chunk {
                        units: vec![],
                        translation: vec![],
                    };
                    count = 0;
                }
            }
        }
    }
    if !chunk.units.is_empty() {
        chunks.push(chunk);
    }
    chunks
}

fn synthesize_jobs(jobs: Receiver<Job>, output: Sender<Produced>, epoch: Arc<AtomicU64>) {
    let mut provider: Option<Box<dyn crate::TtsProvider>> = None;
    let mut provider_key = String::new();
    for job in jobs {
        let cancel = Cancellation::new(epoch.clone(), job.generation);
        if cancel.is_cancelled() {
            continue;
        }
        let key = serde_json::to_string(&job.config.provider).unwrap_or_default();
        if provider.is_none() || key != provider_key {
            match create_provider(&job.config.provider) {
                Ok(value) => {
                    provider = Some(value);
                    provider_key = key;
                }
                Err(error) => {
                    let _ = output.send(Produced::Error(job.generation, error.to_string()));
                    if job.playback {
                        let _ = output.send(Produced::Done(job.generation));
                    }
                    continue;
                }
            }
        }
        let mut reached_target = job.target.is_none();
        for mut chunk in chunks(job.units) {
            let target = if !reached_target {
                let (id, byte) = job.target.unwrap();
                let found = chunk.translation.iter().enumerate().find_map(|(i, t)| {
                    (t.original_id == id
                        && t.byte_offset <= byte
                        && byte < t.byte_offset + chunk.units[i].text.len())
                    .then(|| (i, byte - t.byte_offset))
                });
                let Some(target) = found else {
                    continue;
                };
                reached_target = true;
                Some(crate::pronunciations::pronunciation_target(
                    &chunk.units,
                    &job.config.pronunciations,
                    target.0,
                    target.1,
                ))
            } else {
                None
            };
            if cancel.is_cancelled() {
                break;
            }
            let result = (|| -> Result<SynthesizedAudio> {
                let progress = |text| {
                    if !cancel.is_cancelled() {
                        let _ = output.send(Produced::Progress(job.generation, text));
                    }
                };
                let mut pronounced = crate::pronunciations::PronunciationProvider {
                    provider: provider.as_mut().unwrap().as_mut(),
                    rules: &job.config.pronunciations,
                };
                let cache_key =
                    serde_json::to_string(&(&provider_key, &job.config.pronunciations))?;
                let mut audio = cached_audio(
                    &mut pronounced,
                    &cache_key,
                    &chunk.units,
                    &job.config.cache_dir,
                    job.generation,
                    &cancel,
                    &progress,
                )?;
                if let Some((index, offset)) = target {
                    let boundary = audio.boundaries.iter().find(|b| {
                        b.precision != BoundaryPrecision::Chunk
                            && b.ranges
                                .iter()
                                .any(|r| r.id == index as u64 && r.end > offset)
                    });
                    if let Some(boundary) = boundary {
                        let start = boundary.start_sample;
                        crop_audio(&mut audio, start);
                    } else if index > 0 || offset > 0 {
                        // Audio-only providers cannot seek a word. Generate the
                        // exact suffix instead of inventing a timing ratio.
                        chunk = trim_chunk(&chunk, index, offset);
                        audio = cached_audio(
                            &mut pronounced,
                            &cache_key,
                            &chunk.units,
                            &job.config.cache_dir,
                            job.generation,
                            &cancel,
                            &progress,
                        )?;
                    }
                }
                for boundary in &mut audio.boundaries {
                    for range in &mut boundary.ranges {
                        let translation = chunk
                            .translation
                            .get(range.id as usize)
                            .context("Provider returned an unknown speech unit")?;
                        let text = &chunk.units[range.id as usize].text;
                        ensure!(
                            range.end <= text.len()
                                && text.is_char_boundary(range.start)
                                && text.is_char_boundary(range.end),
                            "Provider returned an invalid source character range"
                        );
                        range.id = translation.original_id;
                        range.start += translation.byte_offset;
                        range.end += translation.byte_offset;
                    }
                }
                Ok(audio)
            })();
            if cancel.is_cancelled() {
                break;
            }
            match result {
                Ok(audio) => {
                    if job.playback {
                        match PreparedAudio::new(&job.config.cache_dir, &audio) {
                            Ok(prepared) => {
                                if output
                                    .send(Produced::Ready(job.generation, prepared))
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            Err(error) => {
                                let _ =
                                    output.send(Produced::Error(job.generation, error.to_string()));
                                break;
                            }
                        }
                    }
                }
                Err(error) => {
                    let _ = output.send(Produced::Error(job.generation, format!("{error:#}")));
                    break;
                }
            }
        }
        if !cancel.is_cancelled() {
            if job.playback {
                let _ = output.send(Produced::Done(job.generation));
            } else {
                let _ = output.send(Produced::Progress(
                    job.generation,
                    "音声の先読みが完了しました".into(),
                ));
            }
        }
    }
}

fn cached_audio(
    provider: &mut dyn crate::TtsProvider,
    key: &str,
    units: &[SpeechUnit],
    cache: &std::path::Path,
    generation: u64,
    cancel: &Cancellation,
    progress: &dyn Fn(String),
) -> Result<SynthesizedAudio> {
    cancel.check()?;
    let mut hasher = Sha256::new();
    hasher.update(b"reader-speech-cache-v8-pronunciations");
    hasher.update(key.as_bytes());
    hasher.update(serde_json::to_vec(units)?);
    let path = cache.join(format!("{:x}.pcm", hasher.finalize()));
    if let Ok(audio) = crate::cache::read(&path) {
        return Ok(audio);
    }
    let audio = provider.synthesize(units, cancel, progress)?;
    audio.validate()?;
    std::fs::create_dir_all(cache)?;
    // A native inference can finish after cancellation. Keep its completed,
    // validated PCM for a later request, while never publishing the old epoch.
    let temporary = tempfile::Builder::new()
        .prefix(&format!("{generation}-"))
        .tempfile_in(cache)?;
    crate::cache::write(temporary.path(), &audio)?;
    let _ = temporary.persist(&path);
    cancel.check()?;
    Ok(audio)
}

fn crop_audio(audio: &mut SynthesizedAudio, start: u64) {
    audio.samples = audio.samples[start.min(audio.samples.len() as u64) as usize..].to_vec();
    audio.boundaries.retain(|b| b.end_sample > start);
    for boundary in &mut audio.boundaries {
        boundary.start_sample = boundary.start_sample.saturating_sub(start);
        boundary.end_sample -= start;
    }
}
fn trim_chunk(chunk: &Chunk, index: usize, offset: usize) -> Chunk {
    let mut result = Chunk {
        units: chunk.units[index..].to_vec(),
        translation: chunk.translation[index..].to_vec(),
    };
    result.units[0].text = result.units[0].text[offset..].to_owned();
    result.translation[0].byte_offset += offset;
    for (id, unit) in result.units.iter_mut().enumerate() {
        unit.id = id as u64;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    #[ignore = "manual timing probe: requires an installed Japanese Windows SAPI voice"]
    fn system_voice_resample_timing_probe() {
        let mut provider = create_provider(&ProviderConfig::System { voice: None }).unwrap();
        let text="　今日はいい天気です。東京で本を読みながら、次の文章を自然な速度で確認します。4003冊の本と日本語の言葉を、ゆっくり読み上げましょう。".chars().cycle().take(100).collect::<String>();
        let audio = provider
            .synthesize(
                &[SpeechUnit { id: 7, text }],
                &Cancellation::uncancelled(),
                &|_| {},
            )
            .unwrap();
        for speed in [1.0, 2.0, 10.0] {
            let started = std::time::Instant::now();
            let (rendered, map) =
                render_audio(&audio.samples, audio.sample_rate, speed, 48_000).unwrap();
            eprintln!(
                "SAPI-DSP speed={speed} source_rate={} source_ms={} marks={} render_ms={} output_ms={}",
                audio.sample_rate,
                audio.samples.len() * 1000 / audio.sample_rate as usize,
                audio.boundaries.len(),
                started.elapsed().as_millis(),
                rendered.len() * 1000 / 48_000
            );
            assert_eq!(map.source_at(map.output_len), audio.samples.len() as u64);
        }
    }
    #[test]
    fn completed_cancelled_inference_is_cached_but_never_returned_to_the_old_epoch() {
        struct CompletingProvider {
            epoch: Arc<AtomicU64>,
            calls: usize,
        }
        impl crate::TtsProvider for CompletingProvider {
            fn name(&self) -> &str {
                "completed fixture"
            }
            fn synthesize(
                &mut self,
                units: &[SpeechUnit],
                _: &Cancellation,
                _: &dyn Fn(String),
            ) -> Result<SynthesizedAudio> {
                self.calls += 1;
                self.epoch.store(2, Ordering::Release);
                Ok(SynthesizedAudio {
                    samples: vec![0.1; 240],
                    sample_rate: 24000,
                    boundaries: vec![TimedBoundary {
                        start_sample: 0,
                        end_sample: 240,
                        ranges: crate::types::unit_ranges(units),
                        precision: BoundaryPrecision::Chunk,
                    }],
                })
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let epoch = Arc::new(AtomicU64::new(1));
        let mut provider = CompletingProvider {
            epoch: epoch.clone(),
            calls: 0,
        };
        let units = [SpeechUnit {
            id: 0,
            text: "再利用する完成音声。".into(),
        }];
        assert!(
            cached_audio(
                &mut provider,
                "fixed voice",
                &units,
                directory.path(),
                1,
                &Cancellation::new(epoch.clone(), 1),
                &|_| {}
            )
            .is_err()
        );
        assert_eq!(provider.calls, 1);
        let audio = cached_audio(
            &mut provider,
            "fixed voice",
            &units,
            directory.path(),
            2,
            &Cancellation::new(epoch.clone(), 2),
            &|_| {},
        )
        .unwrap();
        assert_eq!(audio.samples.len(), 240);
        assert_eq!(
            provider.calls, 1,
            "completed PCM must survive the cancelled generation"
        );
        let cached = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|ext| ext == "pcm"))
            .unwrap();
        std::fs::write(&cached, b"corrupt cached PCM").unwrap();
        cached_audio(
            &mut provider,
            "fixed voice",
            &units,
            directory.path(),
            2,
            &Cancellation::new(epoch.clone(), 2),
            &|_| {},
        )
        .unwrap();
        assert_eq!(provider.calls, 2);
        cached_audio(
            &mut provider,
            "fixed voice",
            &units,
            directory.path(),
            2,
            &Cancellation::new(epoch, 2),
            &|_| {},
        )
        .unwrap();
        assert_eq!(
            provider.calls, 2,
            "a damaged cache must be replaced by the newly validated PCM"
        );
    }
    #[test]
    fn a_background_feeder_cannot_append_after_a_new_point_request() {
        let (commands, received) = mpsc::channel();
        let epoch = Arc::new(AtomicU64::new(7));
        let feeder = SpeechFeeder {
            commands,
            epoch: epoch.clone(),
            generation: 7,
        };
        feeder
            .clone()
            .append(vec![SpeechUnit {
                id: 40,
                text: "次の章です。".into(),
            }])
            .unwrap();
        assert!(matches!(
            received.try_recv().unwrap(),
            Command::Append(7, _)
        ));
        epoch.store(8, Ordering::Release);
        assert!(!feeder.is_current());
        assert!(
            feeder
                .append(vec![SpeechUnit {
                    id: 41,
                    text: "古い章です。".into()
                }])
                .is_err()
        );
        assert!(received.try_recv().is_err());
    }
    #[test]
    fn sentence_chunks_preserve_source_byte_offsets() {
        let list = chunks(vec![SpeechUnit {
            id: 42,
            text: "東京です。次です。".into(),
        }]);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].translation[1].original_id, 42);
        assert_eq!(list[0].translation[1].byte_offset, "東京です。".len());
    }
    #[test]
    fn native_windows_group_short_sentences_but_never_exceed_one_hundred_characters() {
        let original = "短い文です。".repeat(50);
        let list = chunks(vec![SpeechUnit {
            id: 43,
            text: original.clone(),
        }]);
        assert!(list.len() < 10);
        assert!(list.iter().all(|chunk| {
            chunk
                .units
                .iter()
                .map(|unit| unit.text.chars().count())
                .sum::<usize>()
                <= 100
        }));
        assert_eq!(
            list.iter()
                .flat_map(|chunk| chunk.units.iter())
                .map(|unit| unit.text.as_str())
                .collect::<String>(),
            original
        );
    }
    #[test]
    fn synthesis_finishes_a_chapter_without_the_playback_receiver_draining_pcm() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            time::Instant,
        };
        let directory = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut wav = hound::WavWriter::new(
                &mut cursor,
                hound::WavSpec {
                    channels: 1,
                    sample_rate: 24000,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                },
            )
            .unwrap();
            for _ in 0..240 {
                wav.write_sample(1000i16).unwrap();
            }
            wav.finalize().unwrap();
        }
        let wav = cursor.into_inner();
        let epoch = Arc::new(AtomicU64::new(1));
        let server_epoch = epoch.clone();
        let server = thread::spawn(move || {
            let start = Instant::now();
            let mut count = 0;
            while count < 12
                && start.elapsed() < Duration::from_secs(10)
                && server_epoch.load(Ordering::Acquire) == 1
            {
                let (mut socket, _) = match listener.accept() {
                    Ok(socket) => socket,
                    Err(_) => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                loop {
                    let amount = socket.read(&mut buffer).unwrap();
                    assert!(amount > 0);
                    request.extend_from_slice(&buffer[..amount]);
                    if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&request[..end]);
                        let length = header
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|value| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",wav.len()).unwrap();
                socket.write_all(&wav).unwrap();
                count += 1;
            }
            count
        });
        let config = SpeechConfig {
            provider: ProviderConfig::OpenAi {
                base_url: format!("http://{address}/v1"),
                api_key: String::new(),
                model: "test".into(),
                voice: "fixed".into(),
            },
            cache_dir: directory.path().join("cache"),
            speed: 1.0,
            pronunciations: Default::default(),
        };
        let (jobs, rx) = mpsc::channel();
        let (output, results) = mpsc::channel();
        let (finished, finish) = mpsc::channel();
        let worker_epoch = epoch.clone();
        let worker = thread::spawn(move || {
            synthesize_jobs(rx, output, worker_epoch);
            let _ = finished.send(());
        });
        jobs.send(Job {
            generation: 1,
            units: (0..12)
                .map(|index| SpeechUnit {
                    id: index,
                    text: format!("第{index}章。{}\n", "あ".repeat(80)),
                })
                .collect(),
            target: None,
            playback: true,
            config,
        })
        .unwrap();
        drop(jobs);
        let completed = finish.recv_timeout(Duration::from_secs(8)).is_ok();
        if !completed {
            epoch.store(2, Ordering::Release);
            drop(results);
            worker.join().unwrap();
            server.join().unwrap();
            panic!("synthesis was back-pressured by an undrained playback receiver");
        }
        worker.join().unwrap();
        assert_eq!(server.join().unwrap(), 12);
        let results: Vec<_> = results.into_iter().collect();
        assert_eq!(
            results
                .iter()
                .filter(|value| matches!(value, Produced::Ready(1, _)))
                .count(),
            12
        );
        assert!(
            results
                .iter()
                .all(|value| !matches!(value, Produced::Error(..)))
        );
        for result in results {
            if let Produced::Ready(_, prepared) = result {
                assert_eq!(prepared.load().unwrap().samples.len(), 240);
            }
        }
    }
    #[test]
    fn every_generation_change_drops_stale_boundary_events() {
        let (tx, rx) = mpsc::channel();
        let (command, _) = mpsc::channel();
        let epoch = Arc::new(AtomicU64::new(2));
        tx.send(SpeechEvent {
            generation: 1,
            kind: SpeechEventKind::Error("stale".into()),
        })
        .unwrap();
        tx.send(SpeechEvent {
            generation: 2,
            kind: SpeechEventKind::State(SpeechState::Playing),
        })
        .unwrap();
        let controller = SpeechController {
            commands: command,
            events: rx,
            epoch,
        };
        assert!(matches!(
            controller.try_recv().unwrap().kind,
            SpeechEventKind::State(SpeechState::Playing)
        ));
        assert!(controller.try_recv().is_none());
    }
    #[test]
    fn pronunciation_cache_depends_on_both_rules_and_original_spelling() {
        struct Provider {
            calls: usize,
        }
        impl crate::TtsProvider for Provider {
            fn name(&self) -> &str {
                "pronunciation fixture"
            }
            fn synthesize(
                &mut self,
                units: &[SpeechUnit],
                _: &Cancellation,
                _: &dyn Fn(String),
            ) -> Result<SynthesizedAudio> {
                self.calls += 1;
                Ok(SynthesizedAudio {
                    samples: vec![0.1; 240],
                    sample_rate: 24000,
                    boundaries: vec![TimedBoundary {
                        start_sample: 17,
                        end_sample: 220,
                        ranges: crate::types::unit_ranges(units),
                        precision: BoundaryPrecision::Word,
                    }],
                })
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let mut provider = Provider { calls: 0 };
        for (word, reading) in [
            ("日本橋", "にっぽんばし"),
            ("日本橋", "にっぽんばし"),
            ("日本橋", "にほんばし"),
            ("日", "にっぽんばし"),
        ] {
            let rules = [(word.into(), reading.into())].into();
            let key = serde_json::to_string(&("fixed provider", &rules)).unwrap();
            let mut pronounced = crate::pronunciations::PronunciationProvider {
                provider: &mut provider,
                rules: &rules,
            };
            let audio = cached_audio(
                &mut pronounced,
                &key,
                &[SpeechUnit {
                    id: 0,
                    text: word.into(),
                }],
                directory.path(),
                1,
                &Cancellation::uncancelled(),
                &|_| {},
            )
            .unwrap();
            assert_eq!(
                audio.boundaries[0].ranges,
                vec![UnitRange {
                    id: 0,
                    start: 0,
                    end: word.len()
                }]
            );
            assert_eq!(
                (
                    audio.boundaries[0].start_sample,
                    audio.boundaries[0].end_sample
                ),
                (17, 220)
            );
        }
        assert_eq!(
            provider.calls, 3,
            "only the identical source and rule set may reuse PCM"
        );
    }
    #[test]
    fn point_seek_uses_real_pcm_boundary_and_keeps_source_offsets() {
        let mut audio = SynthesizedAudio {
            samples: vec![0.2; 24000],
            sample_rate: 24000,
            boundaries: vec![
                TimedBoundary {
                    start_sample: 100,
                    end_sample: 5000,
                    ranges: vec![UnitRange {
                        id: 0,
                        start: 0,
                        end: 3,
                    }],
                    precision: BoundaryPrecision::Word,
                },
                TimedBoundary {
                    start_sample: 7000,
                    end_sample: 15000,
                    ranges: vec![UnitRange {
                        id: 0,
                        start: 3,
                        end: 6,
                    }],
                    precision: BoundaryPrecision::Word,
                },
            ],
        };
        crop_audio(&mut audio, 7000);
        assert_eq!(audio.samples.len(), 17000);
        assert_eq!(audio.boundaries.len(), 1);
        assert_eq!(audio.boundaries[0].start_sample, 0);
        assert_eq!(audio.boundaries[0].ranges[0].start, 3);
    }
}
