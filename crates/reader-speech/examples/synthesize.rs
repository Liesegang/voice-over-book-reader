//! Generate a real WAV without playing it. --system uses the installed Windows voice.
//! Default uses native Irodori and automatically verifies/downloads its assets.
use anyhow::Result;
use reader_speech::{Cancellation, ProviderConfig, SpeechUnit, create_provider};
fn main() -> Result<()> {
    let arguments: Vec<String> = std::env::args().collect();
    let config = if arguments.iter().any(|a| a == "--system") {
        ProviderConfig::System { voice: None }
    } else {
        ProviderConfig::default()
    };
    let mut provider = create_provider(&config)?;
    let audio = provider.synthesize(
        &[
            SpeechUnit {
                id: 1,
                text: "今日は".into(),
            },
            SpeechUnit {
                id: 2,
                text: "いい天気です。".into(),
            },
            SpeechUnit {
                id: 3,
                text: "本を読みましょう。".into(),
            },
        ],
        &Cancellation::uncancelled(),
        &|message| eprintln!("{message}"),
    )?;
    let output = arguments
        .iter()
        .position(|a| a == "--output")
        .and_then(|i| arguments.get(i + 1))
        .map(String::as_str)
        .unwrap_or("target/speech-probe.wav");
    if let Some(parent) = std::path::Path::new(output).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut writer = hound::WavWriter::create(
        output,
        hound::WavSpec {
            channels: 1,
            sample_rate: audio.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )?;
    for sample in &audio.samples {
        writer.write_sample((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;
    println!(
        "provider={} rate={} frames={} duration={:.2}s boundaries={} file={output}",
        provider.name(),
        audio.sample_rate,
        audio.samples.len(),
        audio.samples.len() as f64 / audio.sample_rate as f64,
        audio.boundaries.len()
    );
    for boundary in audio.boundaries {
        println!(
            "{:?} {}..{} {:?}",
            boundary.precision, boundary.start_sample, boundary.end_sample, boundary.ranges
        );
    }
    Ok(())
}
