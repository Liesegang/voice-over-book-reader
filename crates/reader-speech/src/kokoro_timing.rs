//! Expose the duration predictor already used to expand Kokoro's phonemes.
//! No acoustic aligner or text-length timing interpolation is involved.
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};

pub(crate) const DURATION_OUTPUT: &str = "/encoder/Cast_output_0";

fn varint(bytes: &[u8], at: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *bytes.get(*at).context("Truncated ONNX protobuf")?;
        *at += 1;
        ensure!(shift < 63 || byte <= 1, "Invalid ONNX varint");
        value |= u64::from(byte & 127) << shift;
        if byte < 128 {
            return Ok(value);
        }
    }
    anyhow::bail!("Invalid ONNX varint")
}
fn put(value: u64, output: &mut Vec<u8>) {
    let mut value = value;
    while value >= 128 {
        output.push((value as u8) | 128);
        value >>= 7;
    }
    output.push(value as u8);
}
fn message(field: u64, value: &[u8], output: &mut Vec<u8>) {
    put((field << 3) | 2, output);
    put(value.len() as u64, output);
    output.extend_from_slice(value);
}
fn field(bytes: &[u8], at: &mut usize) -> Result<(u64, std::ops::Range<usize>)> {
    let key = varint(bytes, at)?;
    let kind = key & 7;
    let begin = *at;
    let range = match kind {
        0 => {
            varint(bytes, at)?;
            begin..*at
        }
        1 => {
            *at = at.checked_add(8).context("Invalid ONNX offset")?;
            begin..*at
        }
        2 => {
            let length = usize::try_from(varint(bytes, at)?)?;
            let start = *at;
            *at = at.checked_add(length).context("Invalid ONNX size")?;
            start..*at
        }
        5 => {
            *at = at.checked_add(4).context("Invalid ONNX offset")?;
            begin..*at
        }
        _ => anyhow::bail!("Unsupported ONNX protobuf wire type"),
    };
    ensure!(*at <= bytes.len(), "Truncated ONNX protobuf");
    Ok((key >> 3, range))
}

/// Append only a graph output declaration. All operators, weights and original
/// outputs are preserved byte-for-byte in this derived local copy.
fn expose(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut at = 0;
    let mut graph = None;
    while at < bytes.len() {
        let start = at;
        let (number, range) = field(bytes, &mut at)?;
        if number == 7 {
            graph = Some((start, range, at));
            break;
        }
    }
    let (start, range, end) = graph.context("ONNX has no graph")?;
    let graph = &bytes[range];
    // The pinned export's Cast(int64) output is fed directly into duration
    // expansion. Require its producer instead of silently guessing names.
    let mut at = 0;
    let mut found = false;
    while at < graph.len() {
        let (number, range) = field(graph, &mut at)?;
        if number != 1 {
            continue;
        }
        let node = &graph[range];
        let mut next = 0;
        while next < node.len() {
            let (number, range) = field(node, &mut next)?;
            if number == 2 && &node[range] == DURATION_OUTPUT.as_bytes() {
                found = true;
            }
        }
    }
    ensure!(
        found,
        "Kokoro model has no supported duration predictor output"
    );
    // ValueInfo(name, tensor_type(INT64, shape[unknown])).
    let mut tensor = vec![8, 7];
    let mut shape = Vec::new();
    message(1, &[], &mut shape);
    message(2, &shape, &mut tensor);
    let mut value_type = Vec::new();
    message(1, &tensor, &mut value_type);
    let mut value = Vec::new();
    message(1, DURATION_OUTPUT.as_bytes(), &mut value);
    message(2, &value_type, &mut value);
    let mut extended = graph.to_vec();
    message(12, &value, &mut extended);
    let mut output = Vec::with_capacity(bytes.len() + value.len() + 32);
    output.extend_from_slice(&bytes[..start]);
    message(7, &extended, &mut output);
    output.extend_from_slice(&bytes[end..]);
    Ok(output)
}

pub(crate) fn timed_model(model: &Path) -> Result<PathBuf> {
    let source = std::fs::metadata(model)?;
    ensure!(source.len() < 1024 * 1024 * 1024, "Oversize Kokoro model");
    let name = model
        .file_stem()
        .context("Invalid Kokoro model path")?
        .to_string_lossy();
    let output = model.with_file_name(format!("{name}.duration-v1.onnx"));
    let current = || {
        output.metadata().is_ok_and(|derived| {
            derived.len() > source.len() && derived.modified().ok() >= source.modified().ok()
        })
    };
    if current() {
        return Ok(output);
    }
    let parent = output.parent().context("Invalid Kokoro directory")?;
    std::fs::create_dir_all(parent)?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(output.with_extension("lock"))?;
    lock.lock()?;
    if current() {
        return Ok(output);
    }
    let bytes = std::fs::read(model)?;
    ensure!(bytes.len() < 1024 * 1024 * 1024, "Oversize Kokoro model");
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    temporary.write_all(&expose(&bytes)?)?;
    temporary.persist(&output)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_output_declaration_is_added() {
        let mut node = Vec::new();
        message(2, DURATION_OUTPUT.as_bytes(), &mut node);
        let mut graph = Vec::new();
        message(1, &node, &mut graph);
        message(5, b"unchanged-weights", &mut graph);
        let mut model = vec![8, 8];
        message(7, &graph, &mut model);
        message(14, b"untouched", &mut model);
        let output = expose(&model).unwrap();
        assert!(output.windows(graph.len()).any(|part| part == graph));
        assert!(output.ends_with(b"untouched"));
        assert!(output.len() > model.len());
        assert!(expose(&[0x3a, 0xff]).is_err());
    }
}
