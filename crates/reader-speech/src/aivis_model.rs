//! Read-only AIVMX metadata/catalog and validated, atomic app-owned imports.
//! AIVMX is an ONNX protobuf with the MIT AIVM specification's metadata keys.
//! No engine or Style-Bert-VITS2 implementation is included here.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

const MAX_MODEL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AivisStyle {
    pub local_id: u32,
    pub name: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AivisSpeaker {
    pub local_id: u32,
    pub name: String,
    pub styles: Vec<AivisStyle>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AivisModelInfo {
    pub uuid: String,
    pub name: String,
    pub version: String,
    pub architecture: String,
    pub license: Option<String>,
    pub speakers: Vec<AivisSpeaker>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AivisModelEntry {
    pub path: PathBuf,
    pub info: AivisModelInfo,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AivisModelDiagnostic {
    pub path: PathBuf,
    pub message: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AivisModelCatalog {
    pub models: Vec<AivisModelEntry>,
    pub diagnostics: Vec<AivisModelDiagnostic>,
}

#[derive(Deserialize)]
struct Manifest {
    manifest_version: String,
    uuid: String,
    name: String,
    version: String,
    model_architecture: String,
    model_format: String,
    license: Option<String>,
    speakers: Vec<AivisSpeaker>,
}
#[derive(Debug)]
pub(crate) struct ModelData {
    pub info: AivisModelInfo,
    pub sample_rate: u32,
    pub hop_length: u32,
    pub add_blank: bool,
    pub styles: Vec<Vec<f32>>,
}
pub(crate) fn initialize_runtime(library: &Path) -> Result<()> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ort::init_from(library.to_string_lossy()).with_name("voice-over-book-reader").commit()
    })).map_err(|_|anyhow::anyhow!("ONNX Runtimeを初期化できません。ライブラリの互換性またはORT_DYLIB_PATHを確認し、読書ウィンドウを開き直してください"))??;
    Ok(())
}

fn varint(reader: &mut impl Read) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let mut b = [0];
        reader.read_exact(&mut b)?;
        ensure!(shift < 63 || b[0] <= 1, "Invalid protobuf integer");
        value |= u64::from(b[0] & 127) << shift;
        if b[0] & 128 == 0 {
            return Ok(value);
        }
    }
    bail!("Invalid protobuf integer")
}
fn bytes(reader: &mut impl Read, limit: u64) -> Result<Vec<u8>> {
    let length = varint(reader)?;
    ensure!(length <= limit, "AIVMX metadata exceeds supported size");
    let mut data = vec![0; length as usize];
    reader.read_exact(&mut data)?;
    Ok(data)
}
fn skip(reader: &mut (impl Read + Seek), wire: u64, remaining: u64) -> Result<()> {
    let start = reader.stream_position()?;
    let length = match wire {
        0 => {
            varint(reader)?;
            return Ok(());
        }
        1 => 8,
        2 => varint(reader)?,
        5 => 4,
        _ => bail!("Unsupported protobuf wire type"),
    };
    let consumed = reader.stream_position()? - start;
    ensure!(
        length <= remaining.saturating_sub(consumed),
        "Truncated ONNX field"
    );
    reader.seek(SeekFrom::Current(i64::try_from(length)?))?;
    Ok(())
}
fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(n, c)| {
            if [8, 13, 18, 23].contains(&n) {
                c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}
fn metadata(path: &Path) -> Result<std::collections::HashMap<String, String>> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    ensure!(
        (16..=MAX_MODEL_BYTES).contains(&length),
        "AIVMX model size is outside the supported range"
    );
    let mut result = std::collections::HashMap::new();
    let mut graph = false;
    let mut ir = false;
    let mut total = 0u64;
    while file.stream_position()? < length {
        let tag = varint(&mut file)?;
        ensure!(tag >> 3 != 0, "Invalid ONNX field tag");
        match (tag >> 3, tag & 7) {
            (1, 0) => {
                let version = varint(&mut file)?;
                ensure!((3..=20).contains(&version), "Unsupported ONNX IR version");
                ir = true;
            }
            (7, 2) => {
                let size = varint(&mut file)?;
                ensure!(
                    size > 0 && size <= length - file.stream_position()?,
                    "Truncated ONNX graph"
                );
                file.seek(SeekFrom::Current(i64::try_from(size)?))?;
                graph = true;
            }
            (14, 2) => {
                let data = bytes(&mut file, MAX_METADATA_BYTES)?;
                total += data.len() as u64;
                ensure!(total <= MAX_METADATA_BYTES, "Too much AIVMX metadata");
                let mut cursor = std::io::Cursor::new(data);
                let mut key = None;
                let mut value = None;
                while cursor.position() < cursor.get_ref().len() as u64 {
                    let tag = varint(&mut cursor)?;
                    match (tag >> 3, tag & 7) {
                        (1, 2) => key = Some(String::from_utf8(bytes(&mut cursor, 1024)?)?),
                        (2, 2) => {
                            value =
                                Some(String::from_utf8(bytes(&mut cursor, MAX_METADATA_BYTES)?)?)
                        }
                        _ => {
                            let remaining = cursor.get_ref().len() as u64 - cursor.position();
                            skip(&mut cursor, tag & 7, remaining)?;
                        }
                    }
                }
                if let (Some(key), Some(value)) = (key, value)
                    && key.starts_with("aivm_")
                {
                    ensure!(
                        result.insert(key, value).is_none(),
                        "Duplicate AIVM metadata key"
                    );
                }
            }
            (_, wire) => {
                let remaining = length - file.stream_position()?;
                skip(&mut file, wire, remaining)?;
            }
        }
    }
    ensure!(ir && graph, "File is not a complete ONNX model");
    Ok(result)
}
fn parse_info(raw: &str) -> Result<AivisModelInfo> {
    let manifest: Manifest = serde_json::from_str(raw).context("Invalid AIVM manifest")?;
    ensure!(
        manifest.manifest_version == "1.0",
        "Unsupported AIVM manifest version"
    );
    ensure!(uuid(&manifest.uuid), "Invalid AIVM model UUID");
    ensure!(
        manifest.model_architecture == "Style-Bert-VITS2 (JP-Extra)",
        "Only Style-Bert-VITS2 (JP-Extra) models are supported"
    );
    ensure!(
        manifest.model_format == "ONNX",
        "AIVMX requires an ONNX model"
    );
    ensure!(
        !manifest.name.trim().is_empty() && manifest.name.chars().count() <= 255,
        "Invalid model name"
    );
    ensure!(
        !manifest.version.is_empty() && manifest.version.len() <= 128,
        "Invalid model version"
    );
    ensure!(
        !manifest.speakers.is_empty() && manifest.speakers.len() <= 1024,
        "Invalid speaker catalog"
    );
    let mut speakers = std::collections::HashSet::new();
    for speaker in &manifest.speakers {
        ensure!(
            speakers.insert(speaker.local_id) && !speaker.name.trim().is_empty(),
            "Duplicate speaker ID or empty speaker name"
        );
        ensure!(
            !speaker.styles.is_empty() && speaker.styles.len() <= 4096,
            "Invalid voice styles"
        );
        let mut styles = std::collections::HashSet::new();
        for style in &speaker.styles {
            ensure!(
                styles.insert(style.local_id) && !style.name.trim().is_empty(),
                "Duplicate style ID or empty style name"
            );
        }
    }
    Ok(AivisModelInfo {
        uuid: manifest.uuid.to_ascii_lowercase(),
        name: manifest.name,
        version: manifest.version,
        architecture: manifest.model_architecture,
        license: manifest.license.filter(|s| !s.trim().is_empty()),
        speakers: manifest.speakers,
    })
}
pub fn inspect_model(path: &Path) -> Result<AivisModelInfo> {
    let metadata = metadata(path)?;
    parse_info(
        metadata
            .get("aivm_manifest")
            .context("ONNX file has no AIVM manifest")?,
    )
}
pub fn list_models(directory: &Path) -> Result<AivisModelCatalog> {
    if !directory.exists() {
        return Ok(AivisModelCatalog::default());
    }
    let mut paths = std::fs::read_dir(directory)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("aivmx"))
        })
        .collect::<Vec<_>>();
    paths.sort();
    let mut catalog = AivisModelCatalog::default();
    for path in paths {
        match inspect_model(&path) {
            Ok(info) => catalog.models.push(AivisModelEntry { path, info }),
            Err(error) => catalog.diagnostics.push(AivisModelDiagnostic {
                path,
                message: format!("{error:#}"),
            }),
        }
    }
    Ok(catalog)
}

fn npy(bytes: &[u8]) -> Result<Vec<Vec<f32>>> {
    ensure!(
        bytes.starts_with(b"\x93NUMPY") && bytes.len() >= 10,
        "Invalid style-vector NPY header"
    );
    let (prefix, length) = match bytes[6] {
        1 => (10, usize::from(u16::from_le_bytes([bytes[8], bytes[9]]))),
        2 | 3 => {
            ensure!(bytes.len() >= 12, "Truncated NPY header");
            (12, u32::from_le_bytes(bytes[8..12].try_into()?) as usize)
        }
        _ => bail!("Unsupported NPY version"),
    };
    let header = std::str::from_utf8(
        bytes
            .get(prefix..prefix + length)
            .context("Truncated NPY header")?,
    )?;
    ensure!(
        header.contains("'<f4'") || header.contains("\"<f4\""),
        "Style vectors must be little-endian float32"
    );
    ensure!(
        header
            .split("fortran_order")
            .nth(1)
            .and_then(|value| value.split_once(':'))
            .is_some_and(|(_, value)| value.trim_start().starts_with("False")),
        "Fortran style vectors are unsupported"
    );
    let shape = header
        .split("shape")
        .nth(1)
        .and_then(|s| s.split_once('('))
        .and_then(|(_, s)| s.split_once(')'))
        .map(|(s, _)| s)
        .context("NPY shape is missing")?;
    let shape = shape
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::parse::<usize>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    ensure!(
        shape.len() == 2 && shape[0] > 0 && shape[0] <= 4096 && shape[1] == 256,
        "Style vectors must be [styles,256]"
    );
    let data = &bytes[prefix + length..];
    ensure!(
        data.len() == shape[0] * 256 * 4,
        "Style-vector payload size mismatch"
    );
    let values = data
        .chunks_exact(4)
        .map(|v| f32::from_le_bytes(v.try_into().unwrap()))
        .collect::<Vec<_>>();
    ensure!(
        values.iter().all(|v| v.is_finite()),
        "Style vectors contain nonfinite values"
    );
    Ok(values.chunks_exact(256).map(<[f32]>::to_vec).collect())
}
pub(crate) fn model_data(path: &Path) -> Result<ModelData> {
    use base64::Engine;
    let metadata = metadata(path)?;
    let info = parse_info(
        metadata
            .get("aivm_manifest")
            .context("Missing AIVM manifest")?,
    )?;
    let hyper: serde_json::Value = serde_json::from_str(
        metadata
            .get("aivm_hyper_parameters")
            .context("Missing model hyperparameters")?,
    )?;
    let sample_rate = hyper["data"]["sampling_rate"]
        .as_u64()
        .context("Missing sample rate")?;
    let hop_length = hyper["data"]["hop_length"]
        .as_u64()
        .context("Missing hop length")?;
    ensure!(
        (8000..=192000).contains(&sample_rate) && (1..=4096).contains(&hop_length),
        "Unsupported audio geometry"
    );
    ensure!(
        hyper["data"]["use_jp_extra"].as_bool() == Some(true),
        "Model is not JP-Extra"
    );
    let styles = npy(&base64::engine::general_purpose::STANDARD.decode(
        metadata
            .get("aivm_style_vectors")
            .context("Missing model styles")?,
    )?)?;
    let speaker_count = hyper["data"]["n_speakers"]
        .as_u64()
        .context("Missing speaker count")?;
    for speaker in &info.speakers {
        ensure!(
            u64::from(speaker.local_id) < speaker_count,
            "Speaker index exceeds the model embedding"
        );
        for style in &speaker.styles {
            ensure!(
                (style.local_id as usize) < styles.len(),
                "Style index exceeds embedded vectors"
            );
        }
    }
    Ok(ModelData {
        info,
        sample_rate: sample_rate as u32,
        hop_length: hop_length as u32,
        add_blank: hyper["data"]["add_blank"].as_bool().unwrap_or(false),
        styles,
    })
}

pub(crate) fn validate_session(session: &ort::session::Session) -> Result<()> {
    use ort::{tensor::TensorElementType, value::ValueType};
    let expected = [
        ("x_tst", TensorElementType::Int64, 2),
        ("x_tst_lengths", TensorElementType::Int64, 1),
        ("sid", TensorElementType::Int64, 1),
        ("tones", TensorElementType::Int64, 2),
        ("language", TensorElementType::Int64, 2),
        ("bert", TensorElementType::Float32, 3),
        ("style_vec", TensorElementType::Float32, 2),
        ("length_scale", TensorElementType::Float32, 0),
        ("sdp_ratio", TensorElementType::Float32, 0),
        ("noise_scale", TensorElementType::Float32, 0),
        ("noise_scale_w", TensorElementType::Float32, 0),
    ];
    ensure!(
        session.inputs.len() == expected.len(),
        "Unsupported AIVMX input count"
    );
    for (name, element, rank) in expected {
        let input = session
            .inputs
            .iter()
            .find(|input| input.name == name)
            .with_context(|| format!("Missing AIVMX input {name}"))?;
        let ValueType::Tensor { ty, shape, .. } = &input.input_type else {
            bail!("Non-tensor AIVMX input {name}")
        };
        ensure!(
            *ty == element && shape.len() == rank,
            "Unsupported AIVMX input type for {name}"
        );
        if name == "bert" {
            ensure!(shape[1] == 1024, "Unsupported BERT feature width");
        }
        if name == "style_vec" {
            ensure!(shape[1] == 256, "Unsupported style-vector width");
        }
    }
    ensure!(
        session.outputs.iter().any(|output| output.name == "output"),
        "AIVMX has no audio output"
    );
    Ok(())
}

/// Copies to a private temporary file first, validates those exact bytes, then
/// atomically publishes a content-addressed name. Source files are never changed.
pub fn import_model(
    source: &Path,
    directory: &Path,
    runtime_library: &Path,
) -> Result<AivisModelEntry> {
    std::fs::create_dir_all(directory)?;
    let mut input = File::open(source)?;
    ensure!(
        input.metadata()?.is_file(),
        "AIVMX source must be a regular file"
    );
    ensure!(
        input.metadata()?.len() <= MAX_MODEL_BYTES,
        "AIVMX exceeds 2 GiB"
    );
    let mut pending = tempfile::NamedTempFile::new_in(directory)?;
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        ensure!(total <= MAX_MODEL_BYTES, "AIVMX exceeds 2 GiB");
        hash.update(&buffer[..count]);
        pending.write_all(&buffer[..count])?;
    }
    pending.flush()?;
    pending.as_file().sync_all()?;
    // ONNX Runtime opens the file without FILE_SHARE_DELETE on Windows. Close
    // the temporary file handle before validation, keeping its cleanup guard.
    let pending = pending.into_temp_path();
    let data = model_data(&pending)?;
    initialize_runtime(runtime_library)?;
    let session = ort::session::Session::builder()?
        .with_intra_threads(2)?
        .commit_from_file(&pending)?;
    validate_session(&session)?;
    drop(session);
    let digest = format!("{:x}", hash.finalize());
    let path = directory.join(format!("{}-{digest}.aivmx", data.info.uuid));
    match pending.persist_noclobber(&path) {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            let mut existing = File::open(&path)?;
            let mut hasher = Sha256::new();
            loop {
                let count = existing.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                hasher.update(&buffer[..count]);
            }
            ensure!(
                format!("{:x}", hasher.finalize()) == digest,
                "An existing imported model is corrupt; remove that app-owned copy before importing again"
            );
        }
        Err(error) => return Err(error.error.into()),
    }
    Ok(AivisModelEntry {
        path,
        info: data.info,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn manifest() -> serde_json::Value {
        serde_json::json!({"manifest_version":"1.0","uuid":"12345678-1234-1234-1234-123456789abc","name":"Original voice","version":"1.0.0","model_architecture":"Style-Bert-VITS2 (JP-Extra)","model_format":"ONNX","license":"Original fixture terms","speakers":[{"name":"Speaker","local_id":0,"styles":[{"name":"Normal","local_id":0}]}]})
    }
    #[test]
    fn validates_architecture_and_never_infers_missing_license() {
        let mut m = manifest();
        m["license"] = serde_json::Value::Null;
        assert!(parse_info(&m.to_string()).unwrap().license.is_none());
        m["model_architecture"] = "other".into();
        assert!(parse_info(&m.to_string()).is_err());
    }
    #[test]
    fn rejects_path_like_uuid_and_duplicate_style_ids() {
        let mut m = manifest();
        m["uuid"] = "../../voice".into();
        assert!(parse_info(&m.to_string()).is_err());
        let mut m = manifest();
        m["speakers"][0]["styles"] =
            serde_json::json!([{"name":"A","local_id":0},{"name":"B","local_id":0}]);
        assert!(parse_info(&m.to_string()).is_err());
    }
    #[test]
    fn malformed_files_are_diagnostics_not_silent_catalog_omissions() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("bad.aivmx"), b"not-an-onnx-model").unwrap();
        let catalog = list_models(temp.path()).unwrap();
        assert!(catalog.models.is_empty());
        assert_eq!(catalog.diagnostics.len(), 1);
    }
    #[test]
    fn invalid_import_keeps_source_and_cleans_pending_copy_before_runtime_load() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("original.aivmx");
        let target = temp.path().join("models");
        let bytes = b"original invalid fixture";
        std::fs::write(&source, bytes).unwrap();
        assert!(import_model(&source, &target, &temp.path().join("absent-runtime")).is_err());
        assert_eq!(std::fs::read(source).unwrap(), bytes);
        assert_eq!(std::fs::read_dir(target).unwrap().count(), 0);
    }
    #[test]
    fn protobuf_skip_rejects_length_that_exceeds_remaining_bytes() {
        let mut reader = std::io::Cursor::new(vec![3, 0, 0]);
        assert!(skip(&mut reader, 2, 3).is_err());
    }
    #[test]
    fn rejects_truncated_or_nonfinite_styles() {
        assert!(npy(b"bad").is_err());
        let header = b"{'descr': '<f4', 'fortran_order': False, 'shape': (1, 256), }\n";
        let mut data = b"\x93NUMPY\x01\x00".to_vec();
        data.extend((header.len() as u16).to_le_bytes());
        data.extend(header);
        data.extend(vec![0; 1024]);
        assert_eq!(npy(&data).unwrap()[0].len(), 256);
        let end = data.len();
        data[end - 4..].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(npy(&data).is_err());
    }
}
