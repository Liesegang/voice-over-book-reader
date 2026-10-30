# Kokoro assets and license review

Reviewed 2026-10-30 against the 0.2.2 source, the pinned packages, and their original notices. This is an inventory of the files this application uses and preserves. It does not relicense external components as MIT or approve every use of generated speech.

## Components and boundaries

### Kokoro weights

Version/source: 82M v1.0, ONNX conversion revision `468588286ebb2dd77c25b9771e5d165896538cce`

License and handling: [Apache-2.0](https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/blob/468588286ebb2dd77c25b9771e5d165896538cce/README.md). The original conversion and base-model cards are retained with training-data acknowledgements. Weights are acquired separately.

### Native Japanese frontend

Version/source: Adapted Rust rules from [audio.cpp v0.9.3](https://github.com/0xShug0/audio.cpp/tree/v0.9.3), with Misaki rules

License and handling: audio.cpp is Apache-2.0, copyright 2026 ShugoAI LLC. Its complete notice and a description of local Rust/source-mapping changes are retained.

### Misaki

Version/source: **0.9.4**, verified from the GGUF's `g2p/versions.json` and [published wheel](https://pypi.org/project/misaki/0.9.4/)

License and handling: **Apache-2.0, not MIT**. The unmodified license is retained in the installed resource bundle and source notices. Python is not installed or run.

### Cutlet-derived rules

Version/source: `misaki/cutlet.py` in Misaki 0.9.4; no independent Cutlet installation

License and handling: That file identifies its origin in [Cutlet](https://github.com/polm/cutlet) under MIT. The original notice, copyright 2020 Paul O'Leary McCann, is retained in `Cutlet-LICENSE.txt`.

### UniDic package/resources

Version/source: Embedded metadata identifies `unidic` Python package **1.1.0**

License and handling: [Package MIT notice](https://github.com/polm/unidic-py/blob/v1.1.0/LICENSE) retained. This package version is **not** a dictionary dataset version. No mutable `latest` dictionary is fetched. Exact dictionary bytes are fixed by the full GGUF checksum below.

### UniDic dictionary

Version/source: The selected dictionary binary and companion data inside the pinned GGUF

License and handling: Its `COPYING` offers GPL, LGPL, or BSD. This integration uses the included BSD option, copyright 2011–2021 The UniDic Consortium. Original `AUTHORS`, `COPYING`, `BSD`, `GPL`, `LGPL` and dictionary README are retained.

### Native MeCab

Version/source: Native library from pinned [fugashi 1.5.2 wheels](https://pypi.org/project/fugashi/1.5.2/)

License and handling: The wheel's [MeCab BSD notice](https://github.com/polm/fugashi/blob/v1.5.2/LICENSE.mecab) and [fugashi MIT notice](https://github.com/polm/fugashi/blob/v1.5.2/LICENSE) are retained. Only the native library is loaded; Python code is not loaded.

### Rust ONNX binding

Version/source: `ort` **2.0.0-rc.10**, exactly pinned in Cargo

License and handling: [MIT OR Apache-2.0](https://github.com/pykeio/ort/tree/v2.0.0-rc.10). The Cargo release-notice generator includes the resolved crate's original notices. This binding is distinct from the native runtime.

### ONNX Runtime

Version/source: Native **1.22.0** packages

License and handling: [MIT](https://github.com/microsoft/onnxruntime/blob/v1.22.0/LICENSE), with separate third-party components. Complete archive extraction preserves `LICENSE` and `ThirdPartyNotices.txt`.

### Optional NVIDIA runtimes

Version/source: CUDA runtime 12.8.90, cuBLAS 12.8.4.1, cuDNN 9.8.0.87

License and handling: Proprietary NVIDIA terms, separate from both the reader's MIT and ORT's MIT. Original wheel `License.txt` files are retained. These binaries are excluded from the reader's ZIP/installer.

Japanese processing does not extract or run eSpeak-ng, jieba, or other languages' frontends. The full resource GGUF contains other languages' data and notices. All original `licenses/` entries are preserved. Kokoro uses model durations without the optional Qwen alignment model. Qwen is covered in the [main review](license-audit.md).

## Download permission for NVIDIA

The [CUDA 12.8.1 EULA](https://docs.nvidia.com/cuda/archive/12.8.1/eula/index.html) and [cuDNN 9.8 agreement](https://docs.nvidia.com/deeplearning/cudnn/backend/v9.8.0/reference/eula.html) cover SDK components. They set conditions for downloading, installing, using, and redistributing them. Kokoro settings show both links before the separate GPU runtime download permission switch. An expandable offline view contains the actual pinned wheels' original license texts. This is independent of normal Apache/MIT model and runtime acquisition.

`ProviderConfig::Kokoro.allow_nvidia_download` defaults to `false`. Legacy profiles also load without permission. New NVIDIA wheel downloads require both `auto_download` and that explicit permission. Existing compatible local DLLs may be used without downloading copies. If dependencies cannot be acquired, Kokoro uses CPU inference. The permission control itself starts neither downloads nor playback. Revoking permission changes the provider configuration key. It replaces an active synthesizer before the next request.

Permission to download these components does not change their proprietary license. The reader's release does not redistribute them. Separate runtime redistribution requires reviewing NVIDIA's conditions. It also requires retaining the original terms and notices.

## Exact model/resource pins

The model source is [onnx-community/Kokoro-82M-v1.0-ONNX](https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/tree/468588286ebb2dd77c25b9771e5d165896538cce).

Revision:

```text
468588286ebb2dd77c25b9771e5d165896538cce
```

| Asset | Bytes |
| --- | ---: |
| `onnx/model_quantized.onnx` | 92,361,116 |
| `onnx/model.onnx` | 325,532,232 |
| Japanese voice/G2P/UniDic resource GGUF | 932,617,696 |

SHA-256 (in table order):

```text
fbae9257e1e05ffc727e951ef9b9c98418e6d79f1c9b6b13bd59f5c9028a1478  onnx/model_quantized.onnx
8fbea51ea711f2af382e88c833d9e288c6dc82ce5e98421ea61c058ce21a34cb  onnx/model.onnx
c6864b5b66e94c83b94c45ad71f6cd1a445f25db19cfc0e59de28c694424ab18  Japanese voice/G2P/UniDic resource GGUF
```

The GGUF comes from [audio-cpp/audio.cpp-gguf](https://huggingface.co/audio-cpp/audio.cpp-gguf/tree/462b4cec041e06ffb3d8fcf0d8c655051b1cd9f5/Kokoro-82M-GGUF).

Path:

```text
Kokoro-82M-GGUF/kokoro-82m-multilingual-q8_0.gguf
```

Revision:

```text
462b4cec041e06ffb3d8fcf0d8c655051b1cd9f5
``` Extraction includes the needed Japanese resources, five voice tensors, config, and original licenses. Unused neural tensor data stays in the archive. The extracted `g2p/versions.json` is preserved as provenance. Its `unidic: 1.1.0` field identifies the Python package used for conversion. An independently verified dataset release identifier is not supplied. This audit therefore does not assign one.

The local `*.duration-v1.onnx` cache adds a graph output exposing an existing duration prediction. The installed `NOTICE.txt` records this change. It does not retrain or replace the original model. The original verified ONNX is kept separately.

## Exact native runtime pins

ORT archives come from the [official v1.22.0 release](https://github.com/microsoft/onnxruntime/releases/tag/v1.22.0). Source constants are in [`kokoro_ort_assets.rs`](../crates/reader-speech/src/kokoro_ort_assets.rs).

| Archive | Bytes |
| --- | ---: |
| `onnxruntime-win-x64-1.22.0.zip` | 72,368,545 |
| `onnxruntime-win-x64-gpu-1.22.0.zip` | 312,740,706 |
| `onnxruntime-linux-x64-1.22.0.tgz` | 7,798,730 |
| `onnxruntime-osx-arm64-1.22.0.tgz` | 25,943,843 |
| `onnxruntime-osx-x86_64-1.22.0.tgz` | 27,889,590 |

SHA-256 (in table order):

```text
174c616efc0271194488642a72f1a514e01487da4dfe84c49296d66e40ebe0da  onnxruntime-win-x64-1.22.0.zip
5b5241716b2628c1ab5e79ee620be767531021149ee68f30fc46c16263fb94dd  onnxruntime-win-x64-gpu-1.22.0.zip
8344d55f93d5bc5021ce342db50f62079daf39aaafb5d311a451846228be49b3  onnxruntime-linux-x64-1.22.0.tgz
cab6dcbd77e7ec775390e7b73a8939d45fec3379b017c7cb74f5b204c1a1cc07  onnxruntime-osx-arm64-1.22.0.tgz
e4ec94a7696de74fb1b12846569aa94e499958af6ffa186022cfde16c9d617f0  onnxruntime-osx-x86_64-1.22.0.tgz
```

The MeCab inputs are fugashi **1.5.2**, CPython **cp312-cp312** wheel distributions. Only the bundled native library and notices are extracted. All source URLs are fixed in [`native.rs`](../crates/reader-speech/src/native.rs). A platform pin does not establish native validation on that platform.

| Wheel platform tag | Bytes |
| --- | ---: |
| `win_amd64` | 508,407 |
| `manylinux2014_x86_64.manylinux_2_17_x86_64` | 694,924 |
| `macosx_11_0_arm64` | 455,098 |
| `macosx_10_13_x86_64` | 457,118 |

SHA-256 (in table order):

```text
936d710166c5b05064ec2ce0eb347fff7a0cf102c33989012fad205346943402  win_amd64
83bc7bf08f81a3c3992bf10b8c681720898a826c6c3dffa80e1296e005f4bfb8  manylinux2014_x86_64.manylinux_2_17_x86_64
8c1f64345a7a13b229fb755b567cbc993adb43b5b617ad4089521e5dd4d27b91  macosx_11_0_arm64
3d2bb28cc6c6eec1c50729bb2dda44007a45599f0471b14c8fda57b0dde36d50  macosx_10_13_x86_64
```

| Optional Windows NVIDIA wheel | Bytes |
| --- | ---: |
| [`nvidia_cuda_runtime_cu12-12.8.90-py3-none-win_amd64.whl`](https://pypi.org/project/nvidia-cuda-runtime-cu12/12.8.90/#files) | 944,318 |
| [`nvidia_cublas_cu12-12.8.4.1-py3-none-win_amd64.whl`](https://pypi.org/project/nvidia-cublas-cu12/12.8.4.1/#files) | 567,544,208 |
| [`nvidia_cudnn_cu12-9.8.0.87-py3-none-win_amd64.whl`](https://pypi.org/project/nvidia-cudnn-cu12/9.8.0.87/#files) | 684,630,375 |

SHA-256 (in table order):

```text
c0c6027f01505bfed6c3b21ec546f69c687689aad5f1a377554bc6ca4aa993a8  nvidia_cuda_runtime_cu12-12.8.90-py3-none-win_amd64.whl
47e9b82132fa8d2b4944e708049229601448aaad7e6f296f630f2d1a32de35af  nvidia_cublas_cu12-12.8.4.1-py3-none-win_amd64.whl
b4b5cfddc32aa4180f9d390ee99e9a9f55a89e7087329b41aba4319327e22466  nvidia_cudnn_cu12-9.8.0.87-py3-none-win_amd64.whl
```

All three pinned wheel directories and original `License.txt` files were inspected. Their sizes/checksums match the official PyPI metadata. CUDA runtime and cuBLAS share the same 59,262-byte license. The cuDNN agreement is 18,174 bytes. There was no separately named NOTICE/COPYING in these three wheels. Extraction also preserves such notice names and package `METADATA` when supplied. Bounded HTTP ranges allowed inspection without fetching over a gigabyte of unused DLLs.

The cuDNN wheel contains a 2020 SDK supplement with older library filenames. Its text differs from the versioned online 9.8 documentation. The original wheel text and official documentation link are both preserved. A newer agreement does not replace the original text.

NVIDIA's [9.8 acknowledgements](https://docs.nvidia.com/deeplearning/cudnn/backend/v9.8.0/reference/acknowledgements.html) also identify University of Tennessee and Caffe/Berkeley BSD material. The original acknowledgements page is retained as a supplemental notice.

From the old supplement, this review cannot establish blanket permission to redistribute every DLL. The reader downloads from NVIDIA's published package URLs. Its release excludes those DLLs.

## Notice retention and verification

Source snapshots are in [`crates/reader-speech/licenses/kokoro/`](../crates/reader-speech/licenses/kokoro/). `SOURCES.json` records original URLs, revisions and SHA-256 for the cards and notices. The base model-card snapshot has this revision, separate from the ONNX weight pin:

```text
f3ff3571791e39611d31c381e3a41a3af07b4987
```

Managed Kokoro setup saves complete notices and cards before preparing downloads. Offline copies are under `<runtime>/licenses/kokoro/`. This also upgrades existing managed installations. GGUF extraction preserves every embedded `licenses/` entry. It upgrades older extraction markers to retain `g2p/versions.json` too. MeCab and ORT extraction preserves original license files. Wheel extraction keeps DLLs and notices without installing Python packages.

Regression tests cover:

- Legacy permission defaults, independent profile round trips, and explicit permission checks.
- Complete original notices and model-card acknowledgements saved offline.
- Replacement of incomplete legacy notices.
- GGUF extraction upgrades and native archive notice retention. A headless native-GUI test clicks the permission control and saves and reloads its state. It then revokes permission and verifies that the control does not start synthesis. Hardware speech evidence is in [speech documentation](speech.md). This license inventory adds no performance or cross-platform validation.

## 日本語要点

KokoroモデルとMisakiはApache-2.0、元Cutletの該当部分はMITです。
MeCab、UniDic、ONNX Runtimeの告知を分けて保存します。
MisakiをMITとする旧表記は訂正しました。
UniDicのPythonパッケージ版と辞書データ版は区別し、辞書の実体は固定GGUFのハッシュで特定します。

NVIDIAのCUDA/cuBLAS/cuDNNはプロプライエタリです。
Kokoro設定に原文の利用条件リンクと個別の取得許可を置き、旧設定でも既定は未許可にします。
未許可でもCPUで読み上げられ、導入済みの互換ライブラリは利用できます。
モデルやGPUライブラリをGitやアプリ配布物へ同梱する変更は行っていません。
