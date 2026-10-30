# Speech playback and validation

The app runs local Japanese Irodori inference through a pinned native audio.cpp C ABI. A separate Qwen3 ForcedAligner derives actual speech/text group timings. The GUI and book renderer remain native. HTTP serves only chosen external TTS providers and asset downloads.

## Engines

### Irodori v4.1 Small MeanFlow + Qwen3

Setup: Automatic verified native runtime and model download; about 2.5 GB of model weights

Highlight precision: Real aligned Japanese groups, at the aligner's 80 ms grid

### Built-in Kokoro 82M

Setup: In-process ONNX Runtime. Supported Windows NVIDIA GPUs use CUDA FP32; CPU uses quantization. Japanese voice, G2P, and UniDic resources are pinned.

Highlight precision: Actual phoneme duration frames map to lexical source groups. No Qwen download.

### Native AivisSpeech models

Setup: Compatible AIVMX and Japanese BERT assets; independent Rust inference.

Highlight precision: Recognized ONNX attention groups; chunk fallback for unknown timing exports.

### AivisSpeech Engine

Setup: Installed local engine with real speakers/styles and lazy loopback auto-start. Uses shared, verified Qwen aligner assets.

Highlight precision: Actual acoustic aligned groups. Coarse chunk fallback when alignment is unavailable.

### Windows system voice

Setup: Installed Japanese Windows voice, or an explicit voice name

Highlight precision: Actual System.Speech word events

### VOICEROID2

Setup: Existing licensed AHS installation; headless matching-bitness Rust DLL host

Highlight precision: Actual AUTO_BOOKMARK group-start ticks and source ranges

### OpenAI-compatible endpoint

Setup: Base URL, model, voice, optional API key

Highlight precision: Chunk timing unless the adapter supplies actual finer marks

### VOICEVOX endpoint

Setup: Base URL and speaker ID

Highlight precision: Chunk timing

### External command

Setup: Executable and argument array; JSON on stdin, WAV and optional boundary JSON returned

Highlight precision: Declared actual marks, otherwise chunk timing

Irodori and system voices are separate choices. The app labels system voices separately from Irodori. It never derives timestamps from character-count ratios. Ruby replaces its parent text once for synthesis. The reader maps its spoken range back to the whole parent group when a finer mapping is ambiguous.

The current upstream audio.cpp Japanese aligner frontend separates Han characters and kana runs. These are reported as **AlignedGroup**. They do not claim Nagisa word parity or exact character-level acoustic boundaries. Invalid timings or unmatched labels fall back to an explicitly coarse chunk highlight.

Windows System.Speech word events use a separate verified PCM clock. The .NET Framework file writer can divide converted output bytes by the engine's input byte rate. Haruka's 16 kHz engine with default 22.05 kHz WAV output produced word marks 37.8% too late.

The adapter checks negotiated input/output byte rates and the framework's duration calculation. It corrects only that identified conversion error. A fixed runtime keeps scale one; unfamiliar internals fall back to chunk highlights. No character counts or estimated speech durations determine this correction. The behavior is grounded in Microsoft's [AudioFileOut implementation](https://github.com/dotnet/runtime/blob/main/src/libraries/System.Speech/src/Internal/Synthesis/AudioFileOut.cs) and [EngineSite event creation](https://github.com/dotnet/runtime/blob/main/src/libraries/System.Speech/src/Internal/Synthesis/EngineSite.cs).

Each event's UTF-16 range is checked against its original text. It is then mapped to the reader's UTF-8 units. These are the voice's reported word starts. They are not independently detected acoustic phoneme boundaries.

## Native AivisSpeech models

The `aivis_native` provider runs compatible JP-Extra AIVMX models inside the Rust process.
It uses ONNX Runtime, Hugging Face tokenizers, and JPreprocess with NAIST-JDIC.
It neither launches AivisSpeech Engine nor links its or Style-Bert-VITS2's Python implementation.
The existing `aivis` HTTP provider remains a separate choice.

Native setup can read existing Engine model and BERT files without modifying them.
Otherwise, it downloads pinned assets directly: まお 1.2.0 and the audited Japanese BERT model.
The まお AIVMX is 258,037,076 bytes, under ACML 1.0; its SHA-256 is:

```text
f87ccea2e8e2de0e0bfe52e803945af903b4086bf25621a015111628f00e4119
```

BERT is about 653 MB, under CC-BY-SA-4.0, at revision:

```text
d701ec67708287b20d2063270f6b535e6eed09ab
```

Its exact weight checksum is recorded in the Engine asset list below and native source constants.
User-selected AIVMX imports validate headers, metadata, and the ONNX interface.
Import copies the model atomically into app-data and preserves its embedded manifest/license.
Each model's terms remain separate from the reader's license.

Recognized まお and コハク exports supply real attention boundaries mapped as **AlignedGroup**.
An export without recognized timing output reports **Chunk** precision.
Independent Japanese G2P/accent processing may differ from AivisSpeech Engine.
Pronunciation parity is not claimed.

The native provider generated 2.775 seconds of audio in local tests.
CPU synthesis took 9.47 seconds cold and 1.54 seconds for repeated text.

CUDA synthesis took 10.52 seconds cold and 114 ms for repeated text.
A 100-character passage yielded 18.506 seconds of audio and 60 actual aligned groups.
CUDA synthesis took 227 ms when repeating that passage.

The repeated-input figures do not represent synthesis of a changing book passage.
A later test kept one native provider with forced CUDA and did not play the audio.
Inputs varied across 15, 35, 70, and 100 characters.
After the first call, changing text took about 0.5 to 0.9 seconds per request.
A consecutive repeat of the same 100-character input took 237 ms in the control run.
These call times include observed cuDNN retry overhead and exclude asset downloads.

Another CPU probe ran alongside the earlier repeated-input tests.
These figures are local observations.
They do not establish performance on other devices or sustained playback throughput.
Five representative sentences matched the Engine's 256-phoneme sequence.
Two sentences with numbers/quotes differed in six binary tone values.
Full pronunciation parity remains unverified, and no listening-equivalence claim is made.
See the [native license review](license-audit-aivis-voicevox.md#native-aivis-model-inference).

## Local AivisSpeech Engine

AivisSpeech has its own URL and signed 32-bit style ID. It also has an optional executable path and auto-start setting. Settings retrieves the engine's actual `/speakers` catalog in a background worker. It never replaces an unavailable AivisSpeech engine with another synthesizer.

A managed installation is discovered from `speech/aivis-engine.json`. The executable runs hidden and bound to loopback. It starts only when AivisSpeech is selected for setup or synthesis.

The shared server can outlive an individual reading window. Remote endpoints can be used explicitly, but are never auto-launched.

Following the [official Engine API notes](https://github.com/Aivis-Project/AivisSpeech-Engine/tree/1.2.0#readme), the adapter obtains `/audio_query`. It then puts the original spoken text in `kana` before `/synthesis`. It preserves the query's pitch, style strength, tempo, and volume defaults.

The engine's mora durations are dummy values. They never supply highlight timestamps. The shared native Qwen ForcedAligner processes generated PCM and ruby-normalized speech units.

Aligned groups retain source locator mapping and the DAC clock. Playback uses the same pitch-preserving 0.5 to 10x processing as other voices.

The Windows installation exercised locally uses official Engine **1.2.0**:

- [Windows x64 archive](https://github.com/Aivis-Project/AivisSpeech-Engine/releases/tag/1.2.0), SHA-256 `bfbceba2e14dc7f23c7f3695f9ac0381baf91b15d6544e98384574eaadd271f3`.
- Japanese BERT [model repository](https://huggingface.co/tsukumijima/deberta-v2-large-japanese-char-wwm-onnx/tree/d701ec67708287b20d2063270f6b535e6eed09ab), revision:

  ```text
  d701ec67708287b20d2063270f6b535e6eed09ab
  ```

  `model_fp16.onnx` SHA-256:

  ```text
  23f633ae7c5900ff82b35a428b67a54e7e7911d5d6a6dcfc77967be8f1c94dc6
  ```
- The engine installed its current defaults: コハク 1.1.0 and まお 1.2.0. Style IDs are read from the running engine, including まお・ノーマル `888753760`.

The downloaded engine retains its terms and dependency notices. The installation's `licenses` directory records both models' metadata and ACML 1.0 text. These come from `/aivm_models`. Engine LGPL terms and voice model licenses are separate.

The BERT model card declares CC-BY-SA-4.0. Its original card, source/revision attribution, and license text are retained. These third-party assets are not bundled into the reader executable.

On this Windows environment, ONNX Runtime could not read a Hugging Face cache symlink. Replacing the snapshot entry with the verified regular model file resolved startup. macOS/Linux AivisSpeech lifecycle behavior has not been exercised locally.

A Windows/Ryzen 3900X/RTX 3090 test used CPU AivisSpeech and Vulkan Qwen alignment. It generated roughly 7.9 seconds of Japanese audio with **23 real aligned groups**. One provider instance took 5.40 seconds for its first request. The next two took 4.00 and 4.50 seconds. A separate cold test restarted the engine and initialized both model stacks in 46 seconds. These measurements exclude the initial asset download. Prepared audio plays at 10x with pitch preserved. Measured uncached generation cannot sustain continuous 10x.

## Point-to-speak, clock, and speed

The reader assigns stable IDs and source locators to speech units. `speak_at` accepts a unit index and UTF-8 byte offset. For ruby, the reader supplies the whole atomic reading with offset zero. `prefetch` can prepare visible sentences without playing them. A cached point request seeks to a real timing boundary. Audio-only providers synthesize the requested suffix without guessing a waveform position.

Raw PCM and its boundary metadata are cached independently of playback speed. Sonic changes speed from 0.5 to 10x while preserving pitch. The vendored extension records actual copy, skip, and insert operation boundaries. An overlap-add operation combines several source positions into one output sample. Interpolation within it therefore gives a nominal timeline. Device resampling is included in that map. CPAL's predicted DAC playback time drives the current source position.

Prepared sentences are appended ahead of the DAC. This avoids adding device latency to every sentence. The queue and retained playback history are bounded. The controller and output callback both check epochs. Cancelled or superseded work cannot write old audio after a later request. An already running native offline inference may finish in its worker. Validated PCM and fixed speaker references remain reusable in the cache. An old epoch cannot publish that PCM to playback. Cache and reference writes use unique temporary files and atomic installation. Fixed reference creation is locked across reader processes.

First use includes model download and loading. Cold generation and sustained 10x throughput depend on the device. Prefetch and cached point starts reduce delay. Instant uncached synthesis is not guaranteed on every CPU.

## Fixed voices and ahead-of-playback preparation

Irodori uses a stable speaker reference. A supplied reference WAV is used for every request. Otherwise, the app generates a fixed sample with the selected voice instruction and seed. The default seed is 42. It saves the WAV under `speech/voices`. This reference is reused across sentences, books, and restarts.

The model, instruction, and seed determine the reference path. Changing the instruction or seed prepares a new candidate. Accepting the sample stores its path as the explicit reference. Irodori still has upstream pronunciation and tail-generation limits when using a reference.

Kokoro offers `jf_alpha`, `jf_gongitsune`, `jf_nezumi`, `jf_tebukuro`, and `jm_kumo`. The selected style embedding stays fixed. It does not require an OpenAI-compatible server or Python for inference. The full multilingual GGUF supplies the Japanese G2P tables and UniDic. Pinned fugashi wheels supply only native MeCab and license notices. Python code is never loaded. Pinned MeCab and ONNX Runtime downloads cover Windows x64, macOS arm64/x64, and Linux x64. macOS/Linux still need native validation. `AUDIOCPP_MECAB_LIBRARY` and `ORT_DYLIB_PATH` can select explicit local libraries.

Kokoro's pinned ONNX export exposes the duration predictor already used to expand phonemes. One frame is exactly 600 PCM samples at 24 kHz. This relationship is checked before returning word marks.

Japanese normalization and lexical grouping retain UTF-8 source ranges. This includes numerals and technical acronyms. Quotes and parentheses are prosody marks. Known Latin words use UniDic's Japanese reading. Unknown ASCII letters use Japanese letter names.

These are model expansion times. They do not establish independent acoustic alignment or single-character boundaries. An incompatible custom model falls back to coarse highlights.

Japanese phoneme spacing follows the default Cutlet path in [Misaki 0.9.4](https://github.com/hexgrad/misaki/blob/main/misaki/cutlet.py). The native frontend joins sokuon across lexical boundaries. Thus `あっと` stays joined instead of becoming `aʔ to`. Likewise, `なっちゃった` does not become `naʔ ʨaʔ ta`. Closing quotes have no extra preceding space, and parentheses retain the model's actual IDs 12/13. Short-phrase goldens use the same UniDic as the installed native frontend. Punctuation remains available for phrase pauses. These changes remove false boundaries. The selected voice and model stay unchanged, and no unsupported pitch tokens are added. Correct lexical accent is not guaranteed for every Japanese phrase. Old pre-fix PCM is not reused by the new cache epoch.

The CPU ONNX model is about 92 MB; CUDA uses the 326 MB FP32 model. The shared Japanese resource download is 933 MB. Windows ONNX Runtime adds about 72 MB for CPU or 313 MB for CUDA. CUDA setups lacking compatible local libraries can download pinned NVIDIA CUDA/cuDNN wheels. These total about 1.25 GB. Only native DLLs and their notices are extracted. The package licenses are retained locally. Old `directml`, `vulkan`, or `metal` Kokoro profiles use CPU instead of the removed DirectML path. `auto` and `cuda` try supported NVIDIA CUDA and fall back to CPU. ONNX Runtime stays loaded for the reader process lifetime. If CPU initialized before GPU runtime installation, reopen the reader window to enable CUDA. GPU inference can produce slightly different PCM for identical requests. The selected speaker embedding stays fixed.

The worker groups short sentences into native synthesis windows of at most 100 characters. It does not start a model run at every full stop. Completed PCM and its actual timing metadata use a binary cache. Disk-backed handles pass prepared audio to the playback worker. Pausing or slow playback therefore does not stop generation after a few sentences. Playback retains a bounded amount of decoded PCM. Disk preparation may run ahead through the supplied chapter.

The player keeps roughly 20 wall-clock seconds ahead, scaling the source buffer with playback speed. `SpeechController::append` accepts a loaded following chapter. It preserves the current voice and cancellation generation.

The former random-voice JSON cache is deliberately not reused. This removes avoidable sentence setup and producer back-pressure. A measured real-time factor above 1 still prevents live 10x synthesis. Uncached sections still need preparation. Cold model loading is separate from native book rendering.

Native Windows assets now use the portable v0.9.3 Vulkan build. The old nonportable build failed a Kokoro CPU probe on Ryzen 3900X with `STATUS_ILLEGAL_INSTRUCTION`. The former default runtime directory resolves to its portable sibling. Existing files and explicit custom directories are retained.

## Installed VOICEROID2

The reader calls the installed native `aitalked.dll` directly. It does not control the editor UI, inject into other processes, or modify the installation. AHS engines, licenses, and voices are not redistributed. The inspected installation has PE32 x86 DLLs. A bundled headless 32-bit Rust process hosts them while the GUI stays 64-bit. A JSON/stdin protocol carries requests and WAV paths. The host retains the selected voice between sentences. Kansai voices use the installed Kansai DLL and language dictionary.

The wrapper uses TextToKana followed by KanaToSpeech and retains the engine's AUTO_BOOKMARK events. Their positions are exclusive ends of source groups, while their ticks mark those groups' starts. Shift-JIS positions map back to original UTF-8 boundaries before synthesis. The parent maps ranges to speech units and reports **AlignedGroup**. These are not per-character acoustic timings. Invalid/incomplete timing uses a coarse highlight.

Characters outside native Shift-JIS become spaces and are reported. They receive no invented pronunciation. Common Sonic playback speed and DAC clocks apply to this provider too.

All four inspected voices generated real 44.1 kHz WAVs: Yukari, Aoi, Akane, and Akari. Yukari produced 3.237 seconds of audio with five actual position groups. The same text before and after a different sentence produced identical PCM. A cached point start at 10x emitted four group events, the first at 87.8 ms.

Playback completed in 403.5 ms on that local output device. These are measurements from one installation, not platform-wide guarantees.

The ABI/callback implementation refers to [pyvcroid2](https://github.com/Nkyoku/pyvcroid2) (MIT; notice retained). It was cross-checked against [aitalked](https://github.com/yanorei32/aitalked). Build the helper with:

```text
cargo build -p reader-voiceroid-bridge --release --target i686-pc-windows-msvc --locked
``` Packaging verifies x86 PE headers and rejects the 64-bit workspace stub.

## Recorded audiobooks

`AudiobookPlayer` is a separate native playback path for MP3, M4A/M4B (AAC-LC or ALAC), and PCM WAV. Symphonia decodes the selected audio track in approximately two-second batches. The output queue keeps a few seconds ahead and discards old PCM. Opening an hours-long recording does not decode it all into RAM. Stereo is preserved; recordings with more channels are downmixed to mono.

The API exposes available title, artist, album, duration, and MP4/ID3 chapter marks. It supports pause, resume, and accurate decoder seek. Speed remains pitch-preserving from 0.5 to 10x, using the same DAC-driven clock. Codec and container duration metadata can include encoder padding. Unsupported codecs, DRM-protected media, and decoder errors are reported. This path makes no text alignment claim and does not label an existing recording as TTS. It does not invoke FFmpeg or an external media player.

## Asset pins and licenses

- [audio.cpp v0.9.3](https://github.com/0xShug0/audio.cpp/releases/tag/v0.9.3), Apache-2.0: native C ABI packages.
- [Irodori v4.1 Small MeanFlow](https://huggingface.co/Aratako/Irodori-TTS-v4.1-Small-MF), MIT with the model card's additional voice-use restrictions.
- [Converted GGUF assets](https://huggingface.co/audio-cpp/audio.cpp-gguf/tree/462b4ce): revision `462b4ce`. The Irodori package includes its tokenizer and DACVAE codec.
- Irodori Q8: 1,368,276,320 bytes. SHA-256:

  ```text
  a450c277a29b771bde3ceb21fd753bfb9a61c93aef952e0a3b20ad8d3facbbcd
  ```
- [Qwen3 ForcedAligner](https://huggingface.co/Qwen/Qwen3-ForcedAligner-0.6B), Apache-2.0. Q8: 1,129,969,856 bytes. SHA-256:

  ```text
  bf4fec616f77a20d89a649cd1c97bda269644eeb3cf906d1f7488a52d71f7cb2
  ```
- [Kokoro ONNX](https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX), Apache-2.0; revision `468588286ebb2dd77c25b9771e5d165896538cce`. CPU: 92,361,116 bytes. SHA-256:

  ```text
  fbae9257e1e05ffc727e951ef9b9c98418e6d79f1c9b6b13bd59f5c9028a1478
  ```

  FP32: 325,532,232 bytes. SHA-256:

  ```text
  8fbea51ea711f2af382e88c833d9e288c6dc82ce5e98421ea61c058ce21a34cb
  ```
- [ONNX Runtime v1.22.0](https://github.com/microsoft/onnxruntime/releases/tag/v1.22.0), MIT; native packages retain LICENSE and ThirdPartyNotices. Exact sizes and SHA-256 pins are in `kokoro_ort_assets.rs`.
- [Misaki 0.9.4](https://pypi.org/project/misaki/0.9.4/), Apache-2.0. Cutlet-derived rules also retain the original [Cutlet MIT notice](https://github.com/polm/cutlet/blob/master/LICENSE). The verified resource archive supplies Misaki and UniDic package/dictionary notices. UniDic's included BSD option applies to dictionary data. The retained `g2p/versions.json` identifies Misaki 0.9.4 and unidic-py 1.1.0. This package version differs from the dictionary dataset version.
- [NVIDIA CUDA runtime](https://pypi.org/project/nvidia-cuda-runtime-cu12/12.8.90/), [cuBLAS](https://pypi.org/project/nvidia-cublas-cu12/12.8.4.1/), and [cuDNN](https://pypi.org/project/nvidia-cudnn-cu12/9.8.0.87/) retain their NVIDIA license notices. These optional proprietary GPU runtimes download separately. They are outside the reader's MIT license. New downloads need explicit permission in Kokoro settings. First review the [CUDA EULA](https://docs.nvidia.com/cuda/archive/12.8.1/eula/index.html) and [cuDNN agreement](https://docs.nvidia.com/deeplearning/cudnn/backend/v9.8.0/reference/eula.html). Legacy profiles default to no permission; CPU or compatible installed libraries remain available.
- [Sonic](https://github.com/waywardgeek/sonic), Apache-2.0; license retained in the source and release notices.

The [Kokoro asset audit](license-audit-kokoro.md) records exact runtime pins, original notices, and source-card snapshots. It also covers the separate NVIDIA permission. Managed setup saves complete notices and cards offline under the runtime's `licenses/kokoro/`. This also applies when upgrading existing assets.

Downloads live in application data, support resuming, and are verified before being installed. EPUBs, Calibre files, and dictionary sources are not modified. Release ZIPs include the native reader and dictionary processes and notices. Windows packages also include the 32-bit VOICEROID2 DLL host. The original AHS installation and voice data are not included. Neural models and selected inference runtimes are acquired separately from platform ZIPs.

## Checks and current evidence

`cargo test -p reader-speech --lib` covers:

- Cancellation and stale event suppression.
- UTF-8/UTF-16 mapping and ruby.
- Actual Sonic operation maps at six playback speeds.
- PCM timing validation and cached point seeks.
- Local HTTP fixtures for both network TTS adapters. Small MP3/AAC fixtures cover native decoding, chapters, and seek. WAV tests cover stereo and bounded decoding. Default tests require no audio device or neural model.

Manual local checks:

```text
cargo run -p reader-speech --example synthesize -- --system
cargo run -p reader-speech --example synthesize
cargo run -p reader-speech --example playback_probe -- --irodori
cargo run -p reader-speech --example audiobook_probe -- recording.m4b --play --speed 10 --seek 60
cargo test -p reader-speech hardware_pause_seek_and_speed_use_the_audible_media_clock -- --ignored
cargo test -p reader-speech system_voice_pcm_clock_keeps_final_words_after_ruby_numbers_and_pauses -- --ignored --nocapture
```

The synthesize examples save WAVs without playing them. The playback probe uses the real output device at 10x and reports boundary events and completion. The System.Speech PCM regression includes leading ideographic whitespace and a ruby reading. It also includes punctuation, several source units, and the spoken number 4003.

Haruka's 8.666-second WAV previously reported its final word at 10.297 seconds. After correction, all 21 word marks remain, with the last starting at 7.472 seconds. The original UTF-16 event text matches throughout. A local Windows / RTX 3090 / Vulkan test generated 3.64 seconds of Irodori audio at 48 kHz. Qwen supplied ten real group timings. Local prefetched point starts produced their first boundary in about 131 to 234 ms.

These measurements come from one environment and do not establish a universal SLA.

On that RTX 3090, Kokoro generated 18.75 seconds of Japanese audio from 100 characters. It returned 60 model duration boundaries. Warm CUDA FP32 synthesis took 1.26 to 1.28 seconds, about 14.7 to 14.9 times playback speed. The first cold request took 10.31 seconds, including model/resource SHA verification.

Ryzen 3900X CPU quantized synthesis used an old `directml` profile. It took 24.3 to 25.0 seconds for 18.6 seconds of the same passage, or 29.33 seconds cold. Prepared audio works with CPU inference. Measured uncached generation cannot sustain live 10x in that environment. Neither result includes an asset download.

The native C ABI packages are configured for Windows x64, macOS arm64/x64, and Linux x64. macOS/Linux speech and device behavior still need native validation.

A local test opened an AAC M4B with chapter metadata and sought to six seconds. The native decoder and output device then played it to completion at 10x.

The CI workflow builds and tests those four targets using Rust 1.97.0. Its normal test run does not invoke the manual synthesis/device probes or download neural models. A configured CI matrix does not establish that its jobs passed. Report platform support from completed job logs and native smoke results.

Runner labels are based on [GitHub's hosted runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners). Linux installs GTK3 and WebKitGTK 4.1 for dictionary display. Native playback uses ALSA development libraries. These follow [Wry's platform requirements](https://github.com/tauri-apps/wry#linux-dependencies).
