//! Binary PCM cache and disk-backed handoff. Synthesis never waits for the DAC.
use crate::{SynthesizedAudio, TimedBoundary};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{BufReader, BufWriter, Read, Write},
    path::Path,
};

#[derive(Serialize, Deserialize)]
struct Metadata {
    sample_rate: u32,
    boundaries: Vec<TimedBoundary>,
}
const MAGIC: &[u8; 8] = b"YOMIPCM3";

pub(crate) fn write(path: &Path, audio: &SynthesizedAudio) -> Result<()> {
    let metadata = serde_json::to_vec(&Metadata {
        sample_rate: audio.sample_rate,
        boundaries: audio.boundaries.clone(),
    })?;
    let mut file = BufWriter::new(File::create(path)?);
    file.write_all(MAGIC)?;
    file.write_all(&(metadata.len() as u64).to_le_bytes())?;
    file.write_all(&metadata)?;
    file.write_all(&(audio.samples.len() as u64).to_le_bytes())?;
    for sample in &audio.samples {
        file.write_all(&sample.to_le_bytes())?;
    }
    file.flush()?;
    Ok(())
}
pub(crate) fn read(path: &Path) -> Result<SynthesizedAudio> {
    let mut file = BufReader::new(File::open(path)?);
    let mut eight = [0; 8];
    file.read_exact(&mut eight)?;
    ensure!(&eight == MAGIC, "Unknown speech cache");
    file.read_exact(&mut eight)?;
    let length = u64::from_le_bytes(eight);
    ensure!(length <= 4 * 1024 * 1024, "Oversize speech metadata");
    let mut metadata = vec![0; length as usize];
    file.read_exact(&mut metadata)?;
    let metadata: Metadata = serde_json::from_slice(&metadata)?;
    file.read_exact(&mut eight)?;
    let frames = u64::from_le_bytes(eight);
    ensure!(
        (8000..=192000).contains(&metadata.sample_rate)
            && frames <= metadata.sample_rate as u64 * 600,
        "Oversize cached PCM"
    );
    let mut bytes = vec![
        0;
        (frames as usize)
            .checked_mul(4)
            .ok_or_else(|| anyhow::anyhow!("Invalid PCM length"))?
    ];
    file.read_exact(&mut bytes)?;
    let samples = bytes
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
        .collect();
    let audio = SynthesizedAudio {
        samples,
        sample_rate: metadata.sample_rate,
        boundaries: metadata.boundaries,
    };
    audio.validate()?;
    Ok(audio)
}

pub(crate) struct PreparedAudio {
    file: tempfile::NamedTempFile,
}
impl PreparedAudio {
    pub fn new(cache: &Path, audio: &SynthesizedAudio) -> Result<Self> {
        let root = cache.join("prepared");
        std::fs::create_dir_all(&root)?;
        let file = tempfile::NamedTempFile::new_in(root)?;
        write(file.path(), audio)?;
        Ok(Self { file })
    }
    pub fn load(self) -> Result<SynthesizedAudio> {
        read(self.file.path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binary_handoff_preserves_pcm_bits_and_drops_its_temporary_file() {
        let root = tempfile::tempdir().unwrap();
        let original = SynthesizedAudio {
            samples: vec![0., 0.25, -0.5, f32::MIN_POSITIVE],
            sample_rate: 24000,
            boundaries: vec![TimedBoundary {
                start_sample: 0,
                end_sample: 4,
                ranges: vec![crate::UnitRange {
                    id: 734,
                    start: 0,
                    end: 6,
                }],
                precision: crate::BoundaryPrecision::AlignedGroup,
            }],
        };
        let prepared = PreparedAudio::new(root.path(), &original).unwrap();
        let path = prepared.file.path().to_owned();
        let decoded = prepared.load().unwrap();
        assert_eq!(decoded.samples, original.samples);
        assert_eq!(decoded.boundaries[0].ranges, original.boundaries[0].ranges);
        assert!(!path.exists());
    }
    #[test]
    fn corrupt_length_is_rejected_without_allocating_declared_pcm() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("corrupt.pcm");
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&u64::MAX.to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        assert!(read(&path).is_err());
    }
}
