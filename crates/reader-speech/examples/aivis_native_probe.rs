//! Manual, local-only native AIVMX probe. No model or runtime is downloaded.
use anyhow::{Context, Result};
use reader_speech::{Cancellation, ProviderConfig, SpeechUnit};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut config = ProviderConfig::aivis_native_default();
    let ProviderConfig::AivisNative {
        backend,
        model_path,
        bert_dir,
        speaker,
        style,
        auto_download,
        ..
    } = &mut config
    else {
        unreachable!()
    };
    *auto_download = false;
    let mut output = PathBuf::from("aivis-native.wav");
    let mut text = "こんにちは。今日は本を読みます。".to_owned();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--cpu" => *backend = "cpu".into(),
            "--cuda" => *backend = "cuda".into(),
            "--model" => *model_path = arguments.next().context("--model PATH")?.into(),
            "--bert" => *bert_dir = arguments.next().context("--bert DIR")?.into(),
            "--speaker" => *speaker = arguments.next().context("--speaker ID")?.parse()?,
            "--style" => *style = arguments.next().context("--style ID")?.parse()?,
            "--text" => text = arguments.next().context("--text TEXT")?,
            "--output" => output = arguments.next().context("--output FILE.wav")?.into(),
            _ => anyhow::bail!("Unknown argument {argument}"),
        }
    }
    let mut provider = reader_speech::create_provider(&config)?;
    let units = [SpeechUnit { id: 7, text }];
    for attempt in 1..=2 {
        let started = std::time::Instant::now();
        let audio = provider.synthesize(&units, &Cancellation::uncancelled(), &|message| {
            eprintln!("{message}")
        })?;
        let seconds = audio.samples.len() as f64 / f64::from(audio.sample_rate);
        println!(
            "attempt={attempt} elapsed_ms={} audio_seconds={seconds:.3} marks={} precision={:?}",
            started.elapsed().as_millis(),
            audio.boundaries.len(),
            audio.boundaries.first().map(|value| value.precision)
        );
        for mark in &audio.boundaries {
            for range in &mark.ranges {
                anyhow::ensure!(
                    units[0].text.is_char_boundary(range.start)
                        && units[0].text.is_char_boundary(range.end),
                    "Invalid UTF-8 timing range"
                );
                println!(
                    "samples={}..{} source={}..{}",
                    mark.start_sample, mark.end_sample, range.start, range.end
                );
            }
        }
        let mut file = hound::WavWriter::create(
            &output,
            hound::WavSpec {
                channels: 1,
                sample_rate: audio.sample_rate,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )?;
        for sample in audio.samples {
            file.write_sample(sample)?;
        }
        file.finalize()?;
    }
    Ok(())
}
