//! Manual native decoder/DAC probe. No model or speech engine is involved.
use anyhow::{Context, Result, bail};
use reader_speech::{AudiobookEvent, AudiobookPlayer, AudiobookState};
use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(
        args.next()
            .context("Usage: audiobook_probe AUDIO [--play] [--speed N] [--seek SECONDS]")?,
    );
    let mut play = false;
    let mut speed = 1.0;
    let mut seek = 0.0;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--play" => play = true,
            "--speed" => speed = args.next().context("Missing speed")?.parse()?,
            "--seek" => seek = args.next().context("Missing seek")?.parse()?,
            _ => bail!("Unknown argument: {arg}"),
        }
    }
    let player = AudiobookPlayer::open(path)?;
    println!("{}", serde_json::to_string_pretty(player.info())?);
    if !play {
        return Ok(());
    }
    player.set_speed(speed)?;
    player.seek(Duration::from_secs_f64(seek))?;
    player.play();
    let started = Instant::now();
    let mut last = Duration::ZERO;
    while started.elapsed() < Duration::from_secs(30) {
        while let Some(event) = player.try_recv() {
            match event {
                AudiobookEvent::Error(error) => bail!(error),
                AudiobookEvent::State(state) => {
                    println!("{state:?} @ {:?}", player.position());
                    if state == AudiobookState::Ended {
                        return Ok(());
                    }
                }
                AudiobookEvent::Position { position, .. } => {
                    if position.as_secs() != last.as_secs() {
                        println!("Position: {position:?}");
                        last = position;
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
    player.stop();
    bail!("Playback did not finish inside the 30-second probe deadline")
}
