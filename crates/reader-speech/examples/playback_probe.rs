//! Hardware smoke test: a short Japanese phrase at 10x, through the actual DAC clock.
use anyhow::{Result, bail};
use reader_speech::*;
use std::time::{Duration, Instant};
fn main() -> Result<()> {
    let mut config = SpeechConfig::default();
    if std::env::args().any(|s| s == "--voiceroid2") {
        config.provider = ProviderConfig::voiceroid2_default();
    } else if !std::env::args().any(|s| s == "--irodori") {
        config.provider = ProviderConfig::System { voice: None };
    }
    config.speed = 10.0;
    let controller = SpeechController::new(config)?;
    let units = vec![SpeechUnit {
        id: 7,
        text: "今日はいい天気です。本を読みましょう。".into(),
    }];
    controller.prefetch(units.clone())?;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if Instant::now() > deadline {
            bail!("Prefetch timed out");
        }
        if let Some(event) = controller.try_recv() {
            match event.kind {
                SpeechEventKind::Progress(message) if message == "音声の先読みが完了しました" =>
                {
                    break;
                }
                SpeechEventKind::Error(error) => bail!(error),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let started = Instant::now();
    let generation = controller.speak_at(units, 0, "今日は".len())?;
    let mut count = 0;
    let mut first = None;
    loop {
        if started.elapsed() > Duration::from_secs(15) {
            bail!("Playback clock did not complete");
        }
        while let Some(event) = controller.try_recv() {
            assert_eq!(event.generation, generation);
            match event.kind {
                SpeechEventKind::Boundary {
                    precision, ranges, ..
                } => {
                    let elapsed = started.elapsed();
                    first.get_or_insert(elapsed);
                    count += 1;
                    println!("{elapsed:?} {precision:?} {ranges:?}");
                }
                SpeechEventKind::State(SpeechState::Finished) => {
                    println!(
                        "completed={:?} first_boundary={first:?} boundary_events={count}",
                        started.elapsed()
                    );
                    if count == 0 {
                        bail!("No boundary events");
                    }
                    return Ok(());
                }
                SpeechEventKind::Error(error) => bail!(error),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
