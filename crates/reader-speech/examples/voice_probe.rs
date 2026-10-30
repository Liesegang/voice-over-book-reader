//! Native voice smoke: generates WAVs only; never starts an audio device.
use anyhow::Result;
use reader_speech::*;
use sha2::{Digest, Sha256};
use std::{path::PathBuf, time::Instant};
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let root = PathBuf::from(args.get(1).expect("Pass a QA output directory"));
    std::fs::create_dir_all(&root)?;
    let mut config = if args.iter().any(|s| s == "--voiceroid2") {
        ProviderConfig::voiceroid2_default()
    } else if args.iter().any(|s| s == "--irodori") {
        ProviderConfig::default()
    } else {
        ProviderConfig::kokoro_default()
    };
    if args.iter().any(|s| s == "--coarse") {
        match &mut config {
            ProviderConfig::Irodori { aligner_path, .. }
            | ProviderConfig::Kokoro { aligner_path, .. } => *aligner_path = None,
            _ => {}
        }
    }
    if let Some(voice) = args
        .iter()
        .position(|s| s == "--voice")
        .and_then(|i| args.get(i + 1))
        && let ProviderConfig::Kokoro { voice: stored, .. }
        | ProviderConfig::Voiceroid2 { voice: stored, .. } = &mut config
    {
        *stored = voice.clone();
    }
    if let Some(value) = args
        .iter()
        .position(|s| s == "--backend")
        .and_then(|i| args.get(i + 1))
    {
        match &mut config {
            ProviderConfig::Kokoro { backend, .. } | ProviderConfig::Irodori { backend, .. } => {
                *backend = value.clone()
            }
            _ => {}
        }
    }
    let token = Cancellation::uncancelled();
    let mut provider = create_provider(&config)?;
    let long = "窓辺で本を開くと、静かな物語が始まりました。今日は天気がよいので、庭の花も明るく見えます。ゆっくり読みながら、気になった言葉を辞書で調べます。".repeat(2).chars().take(100).collect::<String>();
    let texts = if let Some(text) = args
        .iter()
        .position(|s| s == "--text")
        .and_then(|i| args.get(i + 1))
    {
        vec![text.clone()]
    } else if args.iter().any(|s| s == "--long") {
        vec![long.clone(), long.clone(), long]
    } else {
        vec![
            "今日はいい天気です。本を読みましょう。".into(),
            "窓辺で猫が、静かに眠っています。".into(),
            "今日はいい天気です。本を読みましょう。".into(),
        ]
    };
    for (index, text) in texts.into_iter().enumerate() {
        let started = Instant::now();
        let audio = provider.synthesize(
            &[SpeechUnit {
                id: 73,
                text: text.clone(),
            }],
            &token,
            &|message| println!("{message}"),
        )?;
        let path = root.join(format!("voice-{index}.wav"));
        let mut wav = hound::WavWriter::create(
            path,
            hound::WavSpec {
                channels: 1,
                sample_rate: audio.sample_rate,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )?;
        let mut hash = Sha256::new();
        for sample in &audio.samples {
            wav.write_sample(*sample)?;
            hash.update(sample.to_le_bytes());
        }
        wav.finalize()?;
        println!(
            "utterance={index} chars={} elapsed_ms={} duration_ms={} boundaries={} precision={:?} pcm_sha={:x}",
            text.chars().count(),
            started.elapsed().as_millis(),
            audio.samples.len() * 1000 / audio.sample_rate as usize,
            audio.boundaries.len(),
            audio.boundaries[0].precision,
            hash.finalize()
        );
    }
    if let Some(path) = irodori_reference_path(&config) {
        println!("fixed_reference={}", path.display());
    }
    Ok(())
}
