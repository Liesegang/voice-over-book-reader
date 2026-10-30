#![cfg_attr(windows, windows_subsystem = "windows")]
//! Headless 32-bit bridge to the user's installed VOICEROID2 native DLL.
//! No editor process, window automation, injection, or installed-file changes.
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, Write},
    path::PathBuf,
};
#[cfg(all(windows, target_arch = "x86"))]
mod backend;

#[derive(Deserialize)]
pub struct Request {
    pub install_dir: PathBuf,
    pub voice: String,
    pub text: String,
    pub output_wav: PathBuf,
    #[serde(default = "one")]
    pub volume: f32,
    #[serde(default = "one")]
    pub pitch: f32,
    #[serde(default = "one")]
    pub range: f32,
}
fn one() -> f32 {
    1.
}
#[derive(Clone, Serialize)]
pub struct Mark {
    pub tick_ms: u64,
    pub byte_offset: usize,
}
#[derive(Serialize)]
pub struct Response {
    pub sample_rate: u32,
    pub samples: usize,
    pub marks: Vec<Mark>,
    pub replaced_characters: usize,
}

fn main() {
    #[cfg(all(windows, target_arch = "x86"))]
    let mut engine: Option<backend::Engine> = None;
    let input = std::io::stdin();
    let mut output = std::io::stdout().lock();
    for line in input.lock().lines() {
        let reply = (|| -> anyhow::Result<serde_json::Value> {
            let line = line?;
            anyhow::ensure!(line.len() <= 1024 * 1024, "VOICEROID2 request exceeds 1MiB");
            let request: Request = serde_json::from_str(&line)?;
            #[cfg(all(windows, target_arch = "x86"))]
            {
                if engine.as_ref().is_none_or(|e| !e.matches(&request)) {
                    drop(engine.take()); // End the previous global DLL instance before Init.
                    engine = Some(backend::Engine::new(&request)?);
                }
                let response = engine.as_mut().unwrap().synthesize(&request)?;
                Ok(serde_json::json!({"ok":true,"audio":response}))
            }
            #[cfg(not(all(windows, target_arch = "x86")))]
            {
                let _ = request;
                anyhow::bail!("VOICEROID2 bridge must be built for i686-pc-windows-msvc")
            }
        })();
        let failed = reply.is_err();
        let value = reply
            .unwrap_or_else(|error| serde_json::json!({"ok":false,"error":format!("{error:#}")}));
        if writeln!(output, "{value}")
            .and_then(|_| output.flush())
            .is_err()
        {
            break;
        }
        if failed {
            break;
        } // Retire native state after an error; the parent can restart safely.
    }
}
