//! Asset discovery and verified downloads for the independent native backend.
//! Installed Engine assets are read-only; no Engine process is launched here.
use crate::{Cancellation, ProviderConfig, aivis_model, aivis_native::AivisNativePaths};
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const MAO_UUID: &str = "a59cb814-0083-4369-8542-f51a29e72af7";
const BERT_REV: &str = "d701ec67708287b20d2063270f6b535e6eed09ab";
const BERT_REPO: &str = "tsukumijima/deberta-v2-large-japanese-char-wwm-onnx";
const NOTICES: &[(&str, &[u8])] = &[
    (
        "JPreprocess-LICENSE.txt",
        include_bytes!("../licenses/aivis-native/JPreprocess-LICENSE.txt"),
    ),
    (
        "OpenJTalk-NOTICE.txt",
        include_bytes!("../licenses/aivis-native/OpenJTalk-NOTICE.txt"),
    ),
    (
        "NAIST-JDIC-NOTICE.txt",
        include_bytes!("../licenses/aivis-native/NAIST-JDIC-NOTICE.txt"),
    ),
    (
        "BERT-MODEL-CARD.md",
        include_bytes!("../licenses/aivis-native/BERT-MODEL-CARD.md"),
    ),
    (
        "CC-BY-SA-4.0.txt",
        include_bytes!("../licenses/aivis-native/CC-BY-SA-4.0.txt"),
    ),
    (
        "SOURCES.json",
        include_bytes!("../licenses/aivis-native/SOURCES.json"),
    ),
];
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if std::fs::read(path).is_ok_and(|existing| existing == bytes) {
        return Ok(());
    }
    let parent = path.parent().context("Notice file requires a parent")?;
    std::fs::create_dir_all(parent)?;
    let mut pending = tempfile::NamedTempFile::new_in(parent)?;
    std::io::Write::write_all(&mut pending, bytes)?;
    pending.persist(path)?;
    Ok(())
}
fn retain_notices(directory: &Path) -> Result<()> {
    for (name, content) in NOTICES {
        atomic_write(&directory.join(name), content)?;
    }
    Ok(())
}

fn managed_root() -> PathBuf {
    crate::data_dir().join("speech/aivis-native")
}
fn engine_data() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|base| base.data_dir().join("AivisSpeech-Engine"))
}
pub(crate) fn default_paths() -> (PathBuf, PathBuf) {
    let root = managed_root();
    let existing = engine_data();
    let model = existing
        .as_ref()
        .map(|path| path.join("Models").join(format!("{MAO_UUID}.aivmx")))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| root.join("models/mao-1.2.0.aivmx"));
    let bert = existing
        .map(|path| {
            path.join("BertModelCaches").join(format!(
                "models--{}/snapshots/{BERT_REV}",
                BERT_REPO.replace('/', "--")
            ))
        })
        .filter(|path| {
            path.join("model_fp16.onnx").is_file() && path.join("tokenizer.json").is_file()
        })
        .unwrap_or_else(|| root.join(format!("bert/{BERT_REV}")));
    (model, bert)
}

/// Cataloging reads only bounded model metadata, never ONNX weights or samples.
pub fn aivis_native_catalog(config: &ProviderConfig) -> Result<aivis_model::AivisModelCatalog> {
    let ProviderConfig::AivisNative { model_path, .. } = config else {
        anyhow::bail!("Aivis組み込み設定ではありません")
    };
    let mut paths = vec![managed_root().join("models")];
    if let Some(root) = engine_data() {
        paths.push(root.join("Models"));
    }
    let mut catalog = aivis_model::AivisModelCatalog::default();
    for path in paths {
        if path.is_dir() {
            let found = aivis_model::list_models(&path)?;
            catalog.models.extend(found.models);
            catalog.diagnostics.extend(found.diagnostics);
        }
    }
    if model_path.is_file()
        && !catalog
            .models
            .iter()
            .any(|entry| same_path(&entry.path, model_path))
    {
        match aivis_model::inspect_model(model_path) {
            Ok(info) => catalog.models.push(aivis_model::AivisModelEntry {
                path: model_path.clone(),
                info,
            }),
            Err(error) => catalog.diagnostics.push(aivis_model::AivisModelDiagnostic {
                path: model_path.clone(),
                message: format!("{error:#}"),
            }),
        }
    }
    catalog
        .models
        .sort_by(|a, b| a.info.name.cmp(&b.info.name).then(a.path.cmp(&b.path)));
    catalog.models.dedup_by(|a, b| same_path(&a.path, &b.path));
    Ok(catalog)
}
fn same_path(a: &Path, b: &Path) -> bool {
    a == b
        || a.canonicalize()
            .ok()
            .zip(b.canonicalize().ok())
            .is_some_and(|(a, b)| a == b)
}

/// Validate a user-selected AIVMX, then copy atomically into application data.
/// This never deletes or changes the selected file.
pub fn import_aivis_native_model(
    source: &Path,
    config: &ProviderConfig,
    cancel: &Cancellation,
    progress: &dyn Fn(String),
) -> Result<aivis_model::AivisModelEntry> {
    let ProviderConfig::AivisNative {
        runtime_dir,
        auto_download,
        ..
    } = config
    else {
        anyhow::bail!("Aivis組み込み設定ではありません")
    };
    cancel.check()?;
    retain_notices(&managed_root().join("licenses/native"))?;
    let library = crate::kokoro_ort_assets::prepare(
        runtime_dir,
        *auto_download,
        false,
        false,
        cancel,
        progress,
    )?;
    progress("AIVMXの形式とモデル入力を検証しています".into());
    let model = aivis_model::import_model(source, &managed_root().join("models"), &library)?;
    retain_model_license(&model.info)?;
    Ok(model)
}

fn retain_model_license(info: &aivis_model::AivisModelInfo) -> Result<()> {
    let metadata = serde_json::to_vec_pretty(info)?;
    // Distinct versions/custom terms of the same UUID must not overwrite each
    // other's retained license. The digest is safe as a filesystem component.
    let edition = format!("{:x}", Sha256::digest(&metadata));
    let directory = managed_root()
        .join("licenses")
        .join(&info.uuid)
        .join(edition);
    std::fs::create_dir_all(&directory)?;
    atomic_write(&directory.join("model-info.json"), &metadata)?;
    atomic_write(&directory.join("MODEL-LICENSE.txt"),info.license.as_deref().unwrap_or("The model author did not supply a license in the AIVM manifest. Do not infer permission from this reader's MIT license.").as_bytes())?;
    Ok(())
}

pub(crate) fn prepare(
    config: &ProviderConfig,
    cancel: &Cancellation,
    progress: &dyn Fn(String),
) -> Result<AivisNativePaths> {
    let ProviderConfig::AivisNative {
        runtime_dir,
        model_path,
        bert_dir,
        auto_download,
        ..
    } = config
    else {
        anyhow::bail!("Aivis組み込み設定ではありません")
    };
    cancel.check()?;
    retain_notices(&managed_root().join("licenses/native"))?;
    let default = managed_root().join("models/mao-1.2.0.aivmx");
    if !model_path.is_file() || (*auto_download && same_path(model_path, &default)) {
        ensure!(
            *auto_download && same_path(model_path, &default),
            "AIVMXがありません。モデルを追加するかファイルを選択してください"
        );
        crate::native::download(
            &format!(
                "https://api.aivis-project.com/v1/aivm-models/{MAO_UUID}/download?model_type=AIVMX"
            ),
            model_path,
            258_037_076,
            "f87ccea2e8e2de0e0bfe52e803945af903b4086bf25621a015111628f00e4119",
            cancel,
            progress,
            "Aivis まお（ACML 1.0）",
        )?;
    }
    let info = aivis_model::inspect_model(model_path).context("AIVMXメタデータを読み取れません")?;
    retain_model_license(&info)?;
    let bert_model = bert_dir.join("model_fp16.onnx");
    let tokenizer = bert_dir.join("tokenizer.json");
    for (path, name, size, sha) in [
        (
            &bert_model,
            "model_fp16.onnx",
            653_075_699,
            "23f633ae7c5900ff82b35a428b67a54e7e7911d5d6a6dcfc77967be8f1c94dc6",
        ),
        (
            &tokenizer,
            "tokenizer.json",
            430_981,
            "21a17e4d0032739e82dc60f155b4ae37e94519103e0d399657fdd249ec3e7905",
        ),
    ] {
        if !path.is_file() || (*auto_download && path.starts_with(managed_root())) {
            ensure!(
                *auto_download && path.starts_with(managed_root()),
                "日本語BERTの{name}がありません。model_fp16.onnxとtokenizer.jsonを含むフォルダーを選択してください。既存Engineのフォルダーへは書き込みません"
            );
            crate::native::download(
                &format!("https://huggingface.co/{BERT_REPO}/resolve/{BERT_REV}/{name}"),
                path,
                size,
                sha,
                cancel,
                progress,
                "Aivis 日本語BERT",
            )?;
        }
    }
    let notices = managed_root().join("licenses");
    std::fs::create_dir_all(&notices)?;
    atomic_write(
        &notices.join("BERT-SOURCE.txt"),
        format!(
            "Japanese BERT: {BERT_REPO}\nRevision: {BERT_REV}\nLicense: Creative Commons Attribution-ShareAlike 4.0 International\nhttps://creativecommons.org/licenses/by-sa/4.0/\nSource: https://huggingface.co/{BERT_REPO}/tree/{BERT_REV}\nModel weights are downloaded unchanged; no model is included in the reader distribution.\n"
        ).as_bytes(),
    )?;
    Ok(AivisNativePaths {
        model: model_path.clone(),
        bert_model,
        tokenizer,
        runtime_library: runtime_dir.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pinned_native_notices_are_retained_offline_and_upgraded_atomically() {
        let root = tempfile::tempdir().unwrap();
        retain_notices(root.path()).unwrap();
        let target = root.path().join("NAIST-JDIC-NOTICE.txt");
        let original = std::fs::read(&target).unwrap();
        assert!(String::from_utf8_lossy(&original).contains("Nara Institute"));
        assert!(
            std::fs::read_to_string(root.path().join("OpenJTalk-NOTICE.txt"))
                .unwrap()
                .contains("Nagoya")
        );
        assert!(
            std::fs::read_to_string(root.path().join("CC-BY-SA-4.0.txt"))
                .unwrap()
                .contains("Attribution-ShareAlike")
        );
        std::fs::write(&target, "incomplete").unwrap();
        retain_notices(root.path()).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), original);
        assert_eq!(
            std::fs::read_dir(root.path()).unwrap().count(),
            NOTICES.len()
        );
    }
    #[test]
    fn unrelated_provider_cannot_import_or_trigger_native_assets() {
        let config = ProviderConfig::System { voice: None };
        assert!(aivis_native_catalog(&config).is_err());
        assert!(
            import_aivis_native_model(
                Path::new("nonexistent.aivmx"),
                &config,
                &Cancellation::uncancelled(),
                &|_| panic!("no asset preparation")
            )
            .is_err()
        );
    }
}
