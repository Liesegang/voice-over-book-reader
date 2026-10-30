use crate::{native, types::Cancellation};
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    path::{Path, PathBuf},
};

pub(crate) fn cuda_available() -> bool {
    #[cfg(windows)]
    {
        unsafe {
            let Ok(library) =
                libloading::os::windows::Library::load_with_flags("nvcuda.dll", 0x800)
            else {
                return false;
            };
            let Ok(init) = library.get::<unsafe extern "C" fn(u32) -> i32>(b"cuInit\0") else {
                return false;
            };
            init(0) == 0
        }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub(crate) fn prepare(
    root: &Path,
    auto: bool,
    cuda: bool,
    allow_nvidia_download: bool,
    cancel: &Cancellation,
    progress: &dyn Fn(String),
) -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("ORT_DYLIB_PATH") {
        let path = PathBuf::from(path);
        ensure!(
            path.is_file(),
            "ORT_DYLIB_PATHのライブラリがありません: {}",
            path.display()
        );
        return Ok(path);
    }
    std::fs::create_dir_all(root)?;
    let setup = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("onnxruntime-prepare.lock"))?;
    setup.lock()?;
    cancel.check()?;
    #[cfg(all(windows, target_arch = "x86_64"))]
    {
        // Check existing libraries before fetching even the MIT ORT GPU package.
        // Declining NVIDIA downloads is a normal CPU fallback, not consent.
        if cuda {
            prepare_cuda(root, auto, allow_nvidia_download, cancel, progress)?;
        }
        // ort owns one process-wide runtime. Prefer an already installed GPU
        // runtime even for CPU sessions, so changing profiles can enable CUDA.
        if !cuda {
            let installed = root
                .join("onnxruntime-cuda-1.22.0/onnxruntime-win-x64-gpu-1.22.0/lib/onnxruntime.dll");
            if installed.is_file() {
                return Ok(installed);
            }
        }
        let (name, size, sha) = if cuda {
            (
                "onnxruntime-win-x64-gpu-1.22.0.zip",
                312_740_706,
                "5b5241716b2628c1ab5e79ee620be767531021149ee68f30fc46c16263fb94dd",
            )
        } else {
            (
                "onnxruntime-win-x64-1.22.0.zip",
                72_368_545,
                "174c616efc0271194488642a72f1a514e01487da4dfe84c49296d66e40ebe0da",
            )
        };
        let directory = root.join(if cuda {
            "onnxruntime-cuda-1.22.0"
        } else {
            "onnxruntime-1.22.0"
        });
        let prefix = name.trim_end_matches(".zip");
        let library = directory.join(prefix).join("lib/onnxruntime.dll");
        if !library.is_file() {
            ensure!(
                auto,
                "ONNX Runtimeがありません。自動取得を有効にしてください"
            );
            let archive = directory.join(name);
            native::download(
                &format!(
                    "https://github.com/microsoft/onnxruntime/releases/download/v1.22.0/{name}"
                ),
                &archive,
                size,
                sha,
                cancel,
                progress,
                "高速音声ランタイム",
            )?;
            extract_zip(&archive, &directory, None)?;
        }
        Ok(library)
    }
    #[cfg(all(
        any(target_os = "macos", target_os = "linux"),
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    {
        let _ = (cuda, allow_nvidia_download);
        let (name, size, sha, library_name) =
            if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
                (
                    "onnxruntime-linux-x64-1.22.0.tgz",
                    7_798_730,
                    "8344d55f93d5bc5021ce342db50f62079daf39aaafb5d311a451846228be49b3",
                    "libonnxruntime.so.1.22.0",
                )
            } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
                (
                    "onnxruntime-osx-arm64-1.22.0.tgz",
                    25_943_843,
                    "cab6dcbd77e7ec775390e7b73a8939d45fec3379b017c7cb74f5b204c1a1cc07",
                    "libonnxruntime.1.22.0.dylib",
                )
            } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
                (
                    "onnxruntime-osx-x86_64-1.22.0.tgz",
                    27_889_590,
                    "e4ec94a7696de74fb1b12846569aa94e499958af6ffa186022cfde16c9d617f0",
                    "libonnxruntime.1.22.0.dylib",
                )
            } else {
                anyhow::bail!("このプラットフォームではORT_DYLIB_PATHを指定してください")
            };
        let directory = root.join("onnxruntime-1.22.0");
        let library = directory
            .join(name.trim_end_matches(".tgz"))
            .join("lib")
            .join(library_name);
        if !library.is_file() {
            ensure!(
                auto,
                "ONNX Runtimeがありません。自動取得を有効にしてください"
            );
            let archive = directory.join(name);
            native::download(
                &format!(
                    "https://github.com/microsoft/onnxruntime/releases/download/v1.22.0/{name}"
                ),
                &archive,
                size,
                sha,
                cancel,
                progress,
                "高速音声ランタイム",
            )?;
            extract_tar(&archive, &directory)?;
        }
        Ok(library)
    }
    #[cfg(not(any(
        all(windows, target_arch = "x86_64"),
        all(
            any(target_os = "macos", target_os = "linux"),
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    )))]
    {
        let _ = (root, auto, cuda, allow_nvidia_download, cancel, progress);
        anyhow::bail!("ONNX Runtimeを用意しORT_DYLIB_PATHを指定してください")
    }
}

#[cfg(any(not(windows), test))]
fn extract_tar(archive: &Path, directory: &Path) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(File::open(archive)?));
    let mut total = 0u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        total = total
            .checked_add(entry.size())
            .context("Runtime archive size overflow")?;
        ensure!(
            entry.size() <= 1024 * 1024 * 1024 && total <= 4 * 1024 * 1024 * 1024,
            "Oversize runtime archive"
        );
        ensure!(entry.unpack_in(directory)?, "Invalid runtime archive path");
    }
    Ok(())
}

#[cfg(any(windows, test))]
fn extract_zip(archive: &Path, directory: &Path, flatten_dlls: Option<&str>) -> Result<()> {
    let mut zip = zip::ZipArchive::new(File::open(archive)?)?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let relative = entry
            .enclosed_name()
            .context("Invalid runtime archive path")?;
        if entry.is_dir() {
            continue;
        }
        ensure!(
            entry.size() <= 1024 * 1024 * 1024,
            "Oversize runtime archive entry"
        );
        let target = if let Some(label) = flatten_dlls {
            let filename = relative.file_name().context("Invalid runtime file")?;
            if entry.name().ends_with(".dll") {
                directory.join(filename)
            } else if ["license", "notice", "copying", "copyright", "metadata"]
                .iter()
                .any(|part| entry.name().to_ascii_lowercase().contains(part))
            {
                directory
                    .join("licenses")
                    .join(format!("{label}-{}", filename.to_string_lossy()))
            } else {
                continue;
            }
        } else {
            directory.join(relative)
        };
        std::fs::create_dir_all(target.parent().unwrap())?;
        let mut temporary = tempfile::NamedTempFile::new_in(target.parent().unwrap())?;
        std::io::copy(&mut entry, &mut temporary)?;
        temporary.persist(target)?;
    }
    Ok(())
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn prepare_cuda(
    root: &Path,
    auto: bool,
    allow_nvidia_download: bool,
    cancel: &Cancellation,
    progress: &dyn Fn(String),
) -> Result<()> {
    let own = root.join("cuda-12.8-cudnn-9.8");
    let mut roots = vec![own.clone()];
    for variable in [
        "ORT_CUDA_DLL_DIR",
        "CUDA_PATH",
        "VIRTUAL_ENV",
        "CONDA_PREFIX",
    ] {
        if let Some(path) = std::env::var_os(variable) {
            let path = PathBuf::from(path);
            roots.extend([
                path.clone(),
                path.join("bin"),
                path.join("Lib/site-packages/torch/lib"),
            ]);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&path) {
            roots.extend([
                path.clone(),
                path.join("Lib/site-packages/torch/lib"),
                path.join("../Lib/site-packages/torch/lib"),
            ]);
        }
    }
    let find = |name: &str| {
        roots
            .iter()
            .map(|root| root.join(name))
            .find(|path| path.is_file())
    };
    let packages = [
        (
            "cudart64_12.dll",
            "cudart.whl",
            "https://files.pythonhosted.org/packages/30/a5/a515b7600ad361ea14bfa13fb4d6687abf500adc270f19e89849c0590492/nvidia_cuda_runtime_cu12-12.8.90-py3-none-win_amd64.whl",
            944_318,
            "c0c6027f01505bfed6c3b21ec546f69c687689aad5f1a377554bc6ca4aa993a8",
        ),
        (
            "cublas64_12.dll",
            "cublas.whl",
            "https://files.pythonhosted.org/packages/70/61/7d7b3c70186fb651d0fbd35b01dbfc8e755f69fd58f817f3d0f642df20c3/nvidia_cublas_cu12-12.8.4.1-py3-none-win_amd64.whl",
            567_544_208,
            "47e9b82132fa8d2b4944e708049229601448aaad7e6f296f630f2d1a32de35af",
        ),
        (
            "cudnn64_9.dll",
            "cudnn.whl",
            "https://files.pythonhosted.org/packages/39/6a/5e9910b2b2c9dcddee9aaef372b3db0f08b9f7eaf1d462f859461a79caf9/nvidia_cudnn_cu12-9.8.0.87-py3-none-win_amd64.whl",
            684_630_375,
            "b4b5cfddc32aa4180f9d390ee99e9a9f55a89e7087329b41aba4319327e22466",
        ),
    ];
    for (dll, file, url, size, sha) in packages {
        if find(dll).is_none() {
            require_nvidia_download_permission(auto, allow_nvidia_download)?;
            let archive = own.join(file);
            native::download(
                url,
                &archive,
                size,
                sha,
                cancel,
                progress,
                "GPUの音声ライブラリ",
            )?;
            extract_zip(&archive, &own, Some(file))?;
        }
    }
    static LOADED: std::sync::OnceLock<Vec<libloading::Library>> = std::sync::OnceLock::new();
    if LOADED.get().is_none() {
        let mut libraries = Vec::new();
        let mut files = Vec::new();
        for name in [
            "cudart64_12.dll",
            "cublasLt64_12.dll",
            "cublas64_12.dll",
            "cudnn64_9.dll",
        ] {
            let path = find(name).with_context(|| format!("CUDA library missing: {name}"))?;
            let loaded: libloading::Library = unsafe {
                libloading::os::windows::Library::load_with_flags(path, 0x100 | 0x1000)?.into()
            };
            libraries.push(loaded);
        }
        for root in &roots {
            if let Ok(entries) = std::fs::read_dir(root) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.ends_with(".dll")
                        && (name.starts_with("cudnn_") || name.starts_with("nvrtc"))
                    {
                        files.push(entry.path());
                    }
                }
            }
        }
        for path in files {
            if let Ok(library) =
                unsafe { libloading::os::windows::Library::load_with_flags(path, 0x100 | 0x1000) }
            {
                libraries.push(library.into());
            }
        }
        let _ = LOADED.set(libraries);
    }
    Ok(())
}

#[cfg(any(all(windows, target_arch = "x86_64"), test))]
fn require_nvidia_download_permission(auto: bool, accepted: bool) -> Result<()> {
    ensure!(
        auto,
        "CUDA/cuDNNがありません。CPU音声を選ぶか自動取得を有効にしてください"
    );
    ensure!(
        accepted,
        "NVIDIA GPUランタイムの取得には設定で利用条件への同意が必要です"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nvidia_downloads_require_both_auto_download_and_explicit_permission() {
        assert!(require_nvidia_download_permission(true, false).is_err());
        assert!(require_nvidia_download_permission(false, true).is_err());
        assert!(require_nvidia_download_permission(false, false).is_err());
        assert!(require_nvidia_download_permission(true, true).is_ok());
    }
    #[test]
    fn cuda_wheel_expansion_keeps_dlls_and_licenses_and_skips_python() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("cuda.whl");
        let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
        for (name, bytes) in [
            (
                "nvidia/cuda_runtime/bin/cudart64_12.dll",
                b"native dll".as_slice(),
            ),
            (
                "nvidia_cuda_runtime.dist-info/License.txt",
                b"NVIDIA notice",
            ),
            ("nvidia_cuda_runtime.dist-info/NOTICE", b"additional notice"),
            (
                "nvidia_cuda_runtime.dist-info/METADATA",
                b"Name: pinned-runtime",
            ),
            ("nvidia/__init__.py", b"never loaded"),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        let out = root.path().join("out");
        extract_zip(&archive, &out, Some("cudart.whl")).unwrap();
        assert_eq!(
            std::fs::read(out.join("cudart64_12.dll")).unwrap(),
            b"native dll"
        );
        assert!(out.join("licenses/cudart.whl-License.txt").is_file());
        assert_eq!(
            std::fs::read(out.join("licenses/cudart.whl-NOTICE")).unwrap(),
            b"additional notice"
        );
        assert!(out.join("licenses/cudart.whl-METADATA").is_file());
        assert!(!out.join("__init__.py").exists());
    }
    #[test]
    fn unix_runtime_expansion_keeps_library_and_upstream_notices() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("runtime.tgz");
        let encoder = flate2::write::GzEncoder::new(
            File::create(&source).unwrap(),
            flate2::Compression::default(),
        );
        let mut archive = tar::Builder::new(encoder);
        for (name, bytes) in [
            (
                "runtime/lib/libonnxruntime.so.1.22.0",
                b"native library".as_slice(),
            ),
            ("runtime/LICENSE", b"MIT notice"),
            ("runtime/ThirdPartyNotices.txt", b"third party notices"),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive.append_data(&mut header, name, bytes).unwrap();
        }
        archive.into_inner().unwrap().finish().unwrap();
        let expanded = root.path().join("out");
        extract_tar(&source, &expanded).unwrap();
        assert_eq!(
            std::fs::read(expanded.join("runtime/lib/libonnxruntime.so.1.22.0")).unwrap(),
            b"native library"
        );
        assert!(expanded.join("runtime/LICENSE").is_file());
        assert!(expanded.join("runtime/ThirdPartyNotices.txt").is_file());
    }
}
