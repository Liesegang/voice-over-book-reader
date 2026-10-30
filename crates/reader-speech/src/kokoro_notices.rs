//! Original model cards and upstream notices accompany managed Kokoro assets.
use anyhow::{Context, Result};
use std::path::Path;

const NOTICES: &[(&str, &[u8])] = &[
    (
        "NOTICE.txt",
        include_bytes!("../licenses/kokoro/NOTICE.txt"),
    ),
    (
        "SOURCES.json",
        include_bytes!("../licenses/kokoro/SOURCES.json"),
    ),
    (
        "Kokoro-ONNX-MODEL-CARD.md",
        include_bytes!("../licenses/kokoro/Kokoro-ONNX-MODEL-CARD.md"),
    ),
    (
        "Kokoro-BASE-MODEL-CARD.md",
        include_bytes!("../licenses/kokoro/Kokoro-BASE-MODEL-CARD.md"),
    ),
    (
        "audio.cpp-LICENSE.txt",
        include_bytes!("../licenses/kokoro/audio.cpp-LICENSE.txt"),
    ),
    (
        "Misaki-LICENSE.txt",
        include_bytes!("../licenses/kokoro/Misaki-LICENSE.txt"),
    ),
    (
        "Cutlet-LICENSE.txt",
        include_bytes!("../licenses/kokoro/Cutlet-LICENSE.txt"),
    ),
    (
        "NVIDIA-CUDA-cuBLAS-LICENSE.txt",
        include_bytes!("../licenses/kokoro/NVIDIA-CUDA-cuBLAS-LICENSE.txt"),
    ),
    (
        "NVIDIA-cuDNN-LICENSE.txt",
        include_bytes!("../licenses/kokoro/NVIDIA-cuDNN-LICENSE.txt"),
    ),
    (
        "NVIDIA-cuDNN-ACKNOWLEDGEMENTS.html",
        include_bytes!("../licenses/kokoro/NVIDIA-cuDNN-ACKNOWLEDGEMENTS.html"),
    ),
];

/// Exact terms from the pinned wheels, available before any NVIDIA download.
pub fn nvidia_runtime_terms() -> [(&'static str, &'static str); 2] {
    [
        (
            "CUDA / cuBLAS",
            include_str!("../licenses/kokoro/NVIDIA-CUDA-cuBLAS-LICENSE.txt"),
        ),
        (
            "cuDNN",
            include_str!("../licenses/kokoro/NVIDIA-cuDNN-LICENSE.txt"),
        ),
    ]
}

/// Offline: upgrades existing installations as well as new downloads, without
/// depending on a mutable upstream license URL during model startup.
pub(crate) fn retain(runtime: &Path) -> Result<()> {
    let directory = runtime.join("licenses/kokoro");
    std::fs::create_dir_all(&directory).context("Cannot create Kokoro notices directory")?;
    for (name, content) in NOTICES {
        let path = directory.join(name);
        if std::fs::read(&path).is_ok_and(|existing| existing == *content) {
            continue;
        }
        let mut file = tempfile::NamedTempFile::new_in(&directory)?;
        std::io::Write::write_all(&mut file, content)?;
        file.persist(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn managed_assets_keep_complete_original_notices_and_cards_offline() {
        let root = tempfile::tempdir().unwrap();
        retain(root.path()).unwrap();
        let notices = root.path().join("licenses/kokoro");
        let misaki = std::fs::read_to_string(notices.join("Misaki-LICENSE.txt")).unwrap();
        assert!(
            misaki.contains("Apache License") && misaki.contains("END OF TERMS AND CONDITIONS")
        );
        let cutlet = std::fs::read_to_string(notices.join("Cutlet-LICENSE.txt")).unwrap();
        assert!(
            cutlet.contains("2020 Paul O'Leary McCann")
                && cutlet.contains("THE SOFTWARE IS PROVIDED")
        );
        let card = std::fs::read_to_string(notices.join("Kokoro-BASE-MODEL-CARD.md")).unwrap();
        assert!(card.contains("Creative Commons Attribution") && card.contains("Koniwa"));
        std::fs::write(
            notices.join("Cutlet-LICENSE.txt"),
            "incomplete legacy notice",
        )
        .unwrap();
        retain(root.path()).unwrap();
        assert_eq!(
            std::fs::read(notices.join("Cutlet-LICENSE.txt")).unwrap(),
            NOTICES
                .iter()
                .find(|(name, _)| *name == "Cutlet-LICENSE.txt")
                .unwrap()
                .1
        );
        retain(root.path()).unwrap();
    }
    #[test]
    fn exact_pinned_nvidia_terms_are_available_before_download() {
        let terms = nvidia_runtime_terms();
        assert!(terms[0].1.contains("CUDA Toolkit"));
        assert!(terms[1].1.contains("cuDNN SUPPLEMENT"));
        assert!(
            terms
                .iter()
                .all(|(_, text)| text.contains("do not download, install or use"))
        );
    }
}
