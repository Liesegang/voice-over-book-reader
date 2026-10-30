//! Explicit, opt-in local AivisSpeech synthesis/alignment probe; no device playback.
use anyhow::Result;
use reader_speech::{Cancellation, ProviderConfig, SpeechUnit, aivis_voices, create_provider};
use std::{path::PathBuf, time::Instant};

fn main() -> Result<()> {
    let directory = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("output directory argument");
    std::fs::create_dir_all(&directory)?;
    let config = ProviderConfig::aivis_default();
    let cancel = Cancellation::uncancelled();
    let voices = aivis_voices(&config, &cancel, &|message| eprintln!("{message}"))?;
    let mut provider = create_provider(&config)?;
    let units = [
        SpeechUnit {
            id: 42,
            text: "　とうきょうの図書館で、新しい本を読みます。".into(),
        },
        SpeechUnit {
            id: 44,
            text: "まどから入る光が、紙の上を静かに照らしています。".into(),
        },
    ];
    let mut measurements = Vec::new();
    for attempt in 1..=3 {
        let started = Instant::now();
        let audio = provider.synthesize(&units, &cancel, &|message| eprintln!("{message}"))?;
        let elapsed = started.elapsed().as_secs_f64();
        let duration = audio.samples.len() as f64 / audio.sample_rate as f64;
        let measurement = serde_json::json!({"attempt":attempt,"elapsed_seconds":elapsed,"audio_seconds":duration,"playback_multiple":duration/elapsed,"sample_rate":audio.sample_rate,"boundary_count":audio.boundaries.len(),"boundaries":audio.boundaries});
        println!(
            "{}",
            serde_json::json!({"attempt":attempt,"elapsed_seconds":elapsed,"audio_seconds":duration,"playback_multiple":duration/elapsed,"boundary_count":audio.boundaries.len(),"precision":audio.boundaries.first().map(|mark|mark.precision)})
        );
        let mut writer = hound::WavWriter::create(
            directory.join(format!("aivis-{attempt}.wav")),
            hound::WavSpec {
                channels: 1,
                sample_rate: audio.sample_rate,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )?;
        for sample in audio.samples {
            writer.write_sample((sample.clamp(-1., 1.) * i16::MAX as f32) as i16)?;
        }
        writer.finalize()?;
        measurements.push(measurement);
    }
    std::fs::write(
        directory.join("aivis-probe.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"config":config,"voices":voices,"units":units,"measurements":measurements}),
        )?,
    )?;
    Ok(())
}
