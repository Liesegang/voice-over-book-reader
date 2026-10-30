use anyhow::{Context, Result, ensure};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    ffi::c_void,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct MapSpan {
    pub src_start: u64,
    pub src_end: u64,
    pub dst_start: u64,
    pub dst_end: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct TimeMap {
    pub spans: Vec<MapSpan>,
    pub source_len: u64,
    pub output_len: u64,
}
impl TimeMap {
    pub fn source_at(&self, output: u64) -> u64 {
        if output >= self.output_len {
            return self.source_len;
        }
        let index = self.spans.partition_point(|s| s.dst_end <= output);
        let Some(span) = self.spans.get(index) else {
            return self.source_len;
        };
        interpolate(
            output,
            span.dst_start,
            span.dst_end,
            span.src_start,
            span.src_end,
        )
        .min(self.source_len)
    }
    #[cfg(test)]
    pub fn destination_at(&self, source: u64) -> u64 {
        if source >= self.source_len {
            return self.output_len;
        }
        let index = self.spans.partition_point(|s| s.src_end <= source);
        let Some(span) = self.spans.get(index) else {
            return self.output_len;
        };
        interpolate(
            source,
            span.src_start,
            span.src_end,
            span.dst_start,
            span.dst_end,
        )
        .min(self.output_len)
    }
}
fn interpolate(value: u64, from: u64, to: u64, begin: u64, end: u64) -> u64 {
    if to <= from {
        return begin;
    }
    begin
        + ((value.saturating_sub(from).min(to - from) as u128 * end.saturating_sub(begin) as u128)
            / (to - from) as u128) as u64
}

unsafe extern "C" {
    fn sonicCreateStream(sample_rate: i32, channels: i32) -> *mut c_void;
    fn sonicDestroyStream(stream: *mut c_void);
    fn sonicSetSpeed(stream: *mut c_void, speed: f32);
    fn sonicSetPitch(stream: *mut c_void, pitch: f32);
    fn sonicSetRate(stream: *mut c_void, rate: f32);
    fn sonicWriteFloatToStream(stream: *mut c_void, samples: *const f32, count: i32) -> i32;
    fn sonicFlushStream(stream: *mut c_void) -> i32;
    fn sonicSamplesAvailable(stream: *mut c_void) -> i32;
    fn sonicReadFloatFromStream(stream: *mut c_void, samples: *mut f32, max: i32) -> i32;
    fn sonicSetTimelineCallback(
        stream: *mut c_void,
        callback: unsafe extern "C" fn(*mut c_void, i64, i64, i64, i64),
        context: *mut c_void,
    );
}
struct Sonic(*mut c_void);
impl Drop for Sonic {
    fn drop(&mut self) {
        unsafe { sonicDestroyStream(self.0) }
    }
}
unsafe extern "C" fn note_span(context: *mut c_void, a: i64, b: i64, c: i64, d: i64) {
    if a >= 0 && b > a && c >= 0 && d > c {
        let spans = unsafe { &mut *(context as *mut Vec<MapSpan>) };
        spans.push(MapSpan {
            src_start: a as u64,
            src_end: b as u64,
            dst_start: c as u64,
            dst_end: d as u64,
        });
    }
}

/// Pitch-preserving speech stretch, with actual Sonic operation boundaries.
/// Inside an overlap-add operation, timeline interpolation is nominal: multiple
/// source samples contribute to one output. No character-count timing is used.
#[cfg(test)]
pub(crate) fn stretch(
    samples: &[f32],
    sample_rate: u32,
    speed: f32,
) -> Result<(Vec<f32>, TimeMap)> {
    stretch_channels(samples, sample_rate, 1, speed)
}
pub(crate) fn stretch_channels(
    samples: &[f32],
    sample_rate: u32,
    channels: usize,
    speed: f32,
) -> Result<(Vec<f32>, TimeMap)> {
    ensure!(
        (1..=2).contains(&channels) && samples.len().is_multiple_of(channels),
        "Invalid PCM channels"
    );
    let source_frames = samples.len() / channels;
    ensure!(
        speed.is_finite() && (0.5..=10.0).contains(&speed),
        "読み上げ速度は0.5〜10.0倍です"
    );
    ensure!(samples.len() < i32::MAX as usize, "Speech chunk too large");
    if (speed - 1.0).abs() < 0.0001 {
        return Ok((
            samples.to_vec(),
            TimeMap {
                spans: vec![MapSpan {
                    src_start: 0,
                    src_end: source_frames as u64,
                    dst_start: 0,
                    dst_end: source_frames as u64,
                }],
                source_len: source_frames as u64,
                output_len: source_frames as u64,
            },
        ));
    }
    let stream = Sonic(unsafe { sonicCreateStream(sample_rate as i32, channels as i32) });
    ensure!(!stream.0.is_null(), "音声速度変換を初期化できませんでした");
    let mut spans = Vec::<MapSpan>::new();
    let clamped: Vec<f32> = samples.iter().map(|s| s.clamp(-1.0, 1.0)).collect();
    unsafe {
        sonicSetSpeed(stream.0, speed);
        sonicSetPitch(stream.0, 1.0);
        sonicSetRate(stream.0, 1.0);
        sonicSetTimelineCallback(stream.0, note_span, &mut spans as *mut _ as *mut c_void);
        ensure!(
            sonicWriteFloatToStream(stream.0, clamped.as_ptr(), source_frames as i32) != 0,
            "音声速度変換中にメモリが不足しました"
        );
        ensure!(
            sonicFlushStream(stream.0) != 0,
            "音声速度変換を完了できませんでした"
        );
    }
    let count = unsafe { sonicSamplesAvailable(stream.0) }.max(0) as usize;
    let mut output = vec![0.0; count * channels];
    let count = unsafe { sonicReadFloatFromStream(stream.0, output.as_mut_ptr(), count as i32) }
        .max(0) as usize;
    output.truncate(count * channels);
    let source_len = source_frames as u64;
    let output_len = count as u64;
    // Flush synthesizes padding. Never expose its imaginary source positions.
    spans.retain(|s| s.src_start < source_len && s.dst_start < output_len);
    for span in &mut spans {
        span.src_end = span.src_end.min(source_len);
        span.dst_end = span.dst_end.min(output_len);
    }
    if let Some(last) = spans.last_mut() {
        last.src_end = source_len;
        last.dst_end = output_len;
    }
    ensure!(
        !spans.is_empty() || samples.is_empty(),
        "速度変換の時間対応表がありません"
    );
    Ok((
        output,
        TimeMap {
            spans,
            source_len,
            output_len,
        },
    ))
}

/// Band-limited windowed-sinc resampling. One clock frame is one mono sample.
pub(crate) fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to {
        return samples.to_vec();
    }
    let length = ((samples.len() as u128 * to as u128) / from as u128) as usize;
    let cutoff = (to as f64 / from as f64).min(1.0) * 0.94;
    let radius = 16i64;
    // Quantized fractional phases avoid millions of trigonometric calls when
    // starting cached audio. The phase error is below 1/1024 input sample.
    const PHASES: usize = 1024;
    let coefficients: Vec<[f64; 32]> = (0..PHASES)
        .map(|phase| {
            std::array::from_fn(|tap| {
                let x = phase as f64 / PHASES as f64 - (tap as i64 - radius + 1) as f64;
                let sinc = if x.abs() < 1e-9 {
                    cutoff
                } else {
                    (std::f64::consts::PI * x * cutoff).sin() / (std::f64::consts::PI * x)
                };
                sinc * (0.5 + 0.5 * (std::f64::consts::PI * x / radius as f64).cos())
            })
        })
        .collect();
    (0..length)
        .map(|i| {
            let position = i as f64 * from as f64 / to as f64;
            let center = position.floor() as i64;
            let phase = ((position - center as f64) * PHASES as f64) as usize;
            let coefficients = &coefficients[phase.min(PHASES - 1)];
            let mut total = 0.0;
            let mut weight = 0.0;
            for (tap, w) in coefficients.iter().copied().enumerate() {
                let j = center - radius + 1 + tap as i64;
                if j < 0 || j >= samples.len() as i64 {
                    continue;
                }
                total += samples[j as usize] as f64 * w;
                weight += w;
            }
            if weight.abs() > 1e-12 {
                (total / weight) as f32
            } else {
                0.0
            }
        })
        .collect()
}

pub(crate) fn resample_channels(samples: &[f32], from: u32, to: u32, channels: usize) -> Vec<f32> {
    if channels == 1 {
        return resample(samples, from, to);
    }
    if from == to {
        return samples.to_vec();
    }
    let planes: Vec<_> = (0..channels)
        .map(|channel| {
            let source: Vec<f32> = samples
                .chunks_exact(channels)
                .map(|frame| frame[channel])
                .collect();
            resample(&source, from, to)
        })
        .collect();
    let frames = planes.first().map_or(0, |p| p.len());
    let mut result = Vec::with_capacity(frames * channels);
    for frame in 0..frames {
        for plane in &planes {
            result.push(plane[frame]);
        }
    }
    result
}

#[derive(Clone, Copy)]
struct Anchor {
    at: Instant,
    frame: u64,
    end: u64,
}
struct OutputState {
    epoch: Arc<AtomicU64>,
    generation: u64,
    samples: Arc<Vec<f32>>,
    source_channels: usize,
    queued: usize,
    paused: bool,
    held: u64,
    anchors: VecDeque<Anchor>,
    error: Option<String>,
}
impl OutputState {
    fn frames(&self) -> usize {
        self.samples.len() / self.source_channels
    }
    fn fill<T: cpal::SizedSample + cpal::FromSample<f32>>(
        &mut self,
        output: &mut [T],
        channels: usize,
    ) -> bool {
        let active = !self.paused && self.generation == self.epoch.load(Ordering::Acquire);
        for frame in output.chunks_mut(channels) {
            let readable = active && self.queued < self.frames();
            let offset = self.queued * self.source_channels;
            for (channel, destination) in frame.iter_mut().enumerate() {
                let value = if !readable {
                    0.0
                } else if self.source_channels == 1 {
                    self.samples[offset]
                } else if channels == 1 {
                    (self.samples[offset] + self.samples[offset + 1]) * 0.5
                } else if channel < 2 {
                    self.samples[offset + channel]
                } else {
                    0.0
                };
                *destination = T::from_sample(value);
            }
            if readable {
                self.queued += 1;
            }
        }
        active
    }
    fn audible(&self, rate: u32) -> u64 {
        if self.paused {
            return self.held;
        }
        let now = Instant::now();
        self.anchors
            .iter()
            .rev()
            .find(|a| a.at <= now)
            .map(|a| {
                (a.frame + (now.duration_since(a.at).as_secs_f64() * rate as f64) as u64).min(a.end)
            })
            .unwrap_or(self.held)
            .min(self.frames() as u64)
    }
}
pub(crate) struct Player {
    _stream: cpal::Stream,
    shared: Arc<Mutex<OutputState>>,
    pub sample_rate: u32,
}
impl Player {
    pub fn new(epoch: Arc<AtomicU64>) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .context("音声出力デバイスがありません")?;
        let supported = device.default_output_config()?;
        let sample_rate = supported.sample_rate().0;
        let channels = supported.channels() as usize;
        let mut config: cpal::StreamConfig = supported.clone().into();
        if let cpal::SupportedBufferSize::Range { min, max } = supported.buffer_size() {
            // A book reader still needs responsive seeking and fast word
            // highlights. Avoid the backend's much larger default buffer.
            config.buffer_size = cpal::BufferSize::Fixed((sample_rate / 100).clamp(*min, *max));
        }
        let shared = Arc::new(Mutex::new(OutputState {
            epoch,
            generation: 0,
            samples: Arc::new(vec![]),
            source_channels: 1,
            queued: 0,
            paused: false,
            held: 0,
            anchors: VecDeque::with_capacity(128),
            error: None,
        }));
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(&device, &config, shared.clone(), channels)?,
            cpal::SampleFormat::I16 => build::<i16>(&device, &config, shared.clone(), channels)?,
            cpal::SampleFormat::U16 => build::<u16>(&device, &config, shared.clone(), channels)?,
            other => anyhow::bail!("未対応の出力音声形式: {other:?}"),
        };
        stream.play()?;
        Ok(Self {
            _stream: stream,
            shared,
            sample_rate,
        })
    }
    pub fn play(&self, samples: Vec<f32>, generation: u64) {
        self.play_interleaved(samples, 1, generation);
    }
    pub fn play_interleaved(&self, samples: Vec<f32>, channels: usize, generation: u64) {
        let mut s = self.shared.lock().unwrap();
        s.samples = Arc::new(samples);
        s.source_channels = channels;
        s.generation = generation;
        s.queued = 0;
        s.paused = false;
        s.held = 0;
        s.anchors.clear();
    }
    pub fn append(&self, samples: &[f32]) {
        let mut state = self.shared.lock().unwrap();
        Arc::make_mut(&mut state.samples).extend_from_slice(samples);
    }
    /// Discard only media already delivered by the DAC. Future callback anchors
    /// are translated with the buffer so the media clock remains continuous.
    pub fn discard_prefix(&self, frames: u64) {
        let mut state = self.shared.lock().unwrap();
        let frames = (frames as usize).min(state.queued).min(state.frames());
        let values = frames * state.source_channels;
        Arc::make_mut(&mut state.samples).drain(..values);
        state.queued -= frames;
        state.held = state.held.saturating_sub(frames as u64);
        for anchor in &mut state.anchors {
            anchor.frame = anchor.frame.saturating_sub(frames as u64);
            anchor.end = anchor.end.saturating_sub(frames as u64);
        }
    }
    pub fn stop(&self) {
        let mut s = self.shared.lock().unwrap();
        s.samples = Arc::new(vec![]);
        s.queued = 0;
        s.held = 0;
        s.anchors.clear();
    }
    pub fn pause(&self) {
        let mut s = self.shared.lock().unwrap();
        s.held = s.audible(self.sample_rate);
        s.paused = true;
        s.queued = s.held as usize;
        s.anchors.clear();
    }
    pub fn resume(&self) {
        let mut s = self.shared.lock().unwrap();
        s.paused = false;
        s.anchors.clear();
    }
    pub fn position(&self) -> u64 {
        self.shared.lock().unwrap().audible(self.sample_rate)
    }
    pub fn error(&self) -> Option<String> {
        self.shared.lock().unwrap().error.take()
    }
}
fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: Arc<Mutex<OutputState>>,
    channels: usize,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let errors = shared.clone();
    let rate = config.sample_rate.0;
    Ok(device.build_output_stream(
        config,
        move |output: &mut [T], info: &cpal::OutputCallbackInfo| {
            let mut state = shared.lock().unwrap();
            let start = state.queued;
            let active = state.fill(output, channels);
            if active {
                let timestamp = info.timestamp();
                let delay = timestamp
                    .playback
                    .duration_since(&timestamp.callback)
                    .unwrap_or(Duration::ZERO);
                let anchor = Anchor {
                    at: Instant::now() + delay,
                    frame: start as u64,
                    end: state.queued as u64,
                };
                if state.anchors.len() >= 128 {
                    state.anchors.pop_front();
                }
                state.anchors.push_back(anchor);
            }
            let _ = rate;
        },
        move |error| {
            if let Ok(mut state) = errors.lock() {
                state.error = Some(format!("音声出力エラー: {error}"));
            }
        },
        None,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn signal() -> Vec<f32> {
        (0..24_000)
            .map(|i| (i as f32 * 220.0 * std::f32::consts::TAU / 24_000.0).sin() * 0.2)
            .collect()
    }
    #[test]
    fn actual_timeline_is_monotonic_at_every_supported_speed() {
        for speed in [0.5, 1.0, 1.25, 2.0, 5.0, 10.0] {
            let input = signal();
            let (out, map) = stretch(&input, 24_000, speed).unwrap();
            assert!(!out.is_empty());
            assert_eq!(map.source_at(out.len() as u64), input.len() as u64);
            assert_eq!(map.destination_at(input.len() as u64), out.len() as u64);
            let positions: Vec<_> = (0..out.len())
                .step_by(50)
                .map(|i| map.source_at(i as u64))
                .collect();
            assert!(positions.windows(2).all(|w| w[0] <= w[1]), "{speed}");
            assert!(
                (out.len() as f32 - input.len() as f32 / speed).abs() < 1_000.0,
                "{speed}"
            );
        }
    }
    #[test]
    fn rate_one_does_not_modify_pcm() {
        let input = signal();
        let (out, map) = stretch(&input, 24_000, 1.0).unwrap();
        assert_eq!(input, out);
        assert_eq!(map.source_at(5_000), 5_000);
    }
    #[test]
    fn resample_preserves_duration_and_dc() {
        let out = resample(&vec![0.25; 48_000], 48_000, 16_000);
        assert_eq!(out.len(), 16_000);
        assert!(out.iter().all(|s| (s - 0.25).abs() < 0.001));
    }
    #[test]
    fn stereo_speed_and_resample_keep_independent_channels_and_frame_clock() {
        let input: Vec<_> = signal()
            .into_iter()
            .flat_map(|value| [value, -value])
            .collect();
        for speed in [0.5, 1.0, 2.0, 10.0] {
            let (stretched, map) = stretch_channels(&input, 24_000, 2, speed).unwrap();
            assert_eq!(map.source_len, 24_000);
            assert_eq!(map.output_len, stretched.len() as u64 / 2);
            let output = resample_channels(&stretched, 24_000, 48_000, 2);
            assert_eq!(output.len(), stretched.len() * 2);
            assert!(
                output
                    .chunks_exact(2)
                    .all(|frame| (frame[0] + frame[1]).abs() < 0.0001)
            );
        }
    }
    #[test]
    fn output_callback_revokes_queued_audio_before_next_worker_command() {
        let epoch = Arc::new(AtomicU64::new(7));
        let mut state = OutputState {
            epoch: epoch.clone(),
            generation: 7,
            samples: Arc::new(vec![0.25, -0.5, 0.5, -0.25]),
            source_channels: 2,
            queued: 0,
            paused: false,
            held: 0,
            anchors: VecDeque::new(),
            error: None,
        };
        let mut frame = [0.0f32; 2];
        assert!(state.fill(&mut frame, 2));
        assert_eq!(frame, [0.25, -0.5]);
        assert_eq!(state.queued, 1);
        epoch.fetch_add(1, Ordering::AcqRel);
        frame.fill(1.0);
        assert!(!state.fill(&mut frame, 2));
        assert_eq!(frame, [0.0, 0.0]);
        assert_eq!(state.queued, 1);
    }
}
