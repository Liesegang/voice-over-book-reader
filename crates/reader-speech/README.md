# reader-speech

Native Rust speech orchestration and CPAL output. The reader owns source locators; this crate returns stable unit IDs and UTF-8 ranges in those units. Ruby runs use their reading once, with the reader mapping that reading back to the parent group.

- Built-in Irodori v4.1 Small MeanFlow and optional Qwen3 ForcedAligner run through audio.cpp's C ABI. The default configuration enables both. First use downloads about 2.5 GB of models plus the runtime. These go into the application data folder. Downloads are pinned and SHA-256 verified, cancellable, and resumable.
- Windows System.Speech provides installed voices independently of Irodori. It supplies actual `SpeakProgress` timestamps. It is labelled System voice, never Irodori.
- Built-in Kokoro runs directly through ONNX Runtime and native MeCab. Supported Windows NVIDIA GPUs use FP32 CUDA; a quantized CPU path remains available. Five Japanese speaker embeddings are selectable. Actual phoneme duration frames map to lexical source groups. No acoustic aligner download is needed. Native runtime/resource downloads are pinned by size and SHA-256.
- Native AivisSpeech uses compatible AIVMX models through independent Rust inference.
  ONNX Runtime, Hugging Face tokenizers, and JPreprocess run without Engine or Python.
  Recognized attention outputs map to aligned groups; other exports use chunk highlights.
  Model and BERT licenses remain separate; see the [audit](../../docs/license-audit-aivis-voicevox.md#native-aivis-model-inference).
- OpenAI-compatible HTTP, VOICEVOX, and a shell-free JSON/PCM-WAV command protocol allow other voices. Audio-only providers use chunk highlights.
- Point-to-speak uses `speak_at` and real cached timing boundaries. Engines without fine timing synthesize the requested suffix. They do not derive times from character counts. `prefetch` warms visible text without playback.
- 0.5–10x playback preserves pitch through Sonic. A local instrumentation patch records actual copy/skip/insert operation boundaries. Time within overlap-add operations is interpolated; it is not exact acoustic sample provenance. CPAL's predicted DAC timestamp drives the displayed position.
- Generation IDs revoke queued audio and stale UI events. Native inference already in progress may finish in its thread. Completed PCM stays cached, but the revoked epoch cannot play it. The audio device stops independently.
- `AudiobookPlayer` streams MP3, AAC-LC/ALAC M4A/M4B, and PCM WAV separately from TTS. It exposes available metadata and chapter marks. It supports pause, seek, and 0.5 to 10x speed with the same DAC clock. PCM batches and playback history are bounded. This path makes no claim of alignment with EPUB text.

Qwen reports **AlignedGroup** precision. This does not establish word or character certainty. Upstream audio.cpp groups Japanese Han characters and kana runs differently from official Nagisa. It supplies real model timestamps at an 80 ms grid. Invalid alignment falls back visibly to chunk highlighting. No character-count timings or synthetic confidence scores are generated.

## Validate

```text
cargo test -p reader-speech --lib
cargo run -p reader-speech --example synthesize -- --system
cargo run -p reader-speech --example synthesize
cargo run -p reader-speech --example voice_probe -- work/kokoro-qa --backend auto --long
cargo run -p reader-speech --example playback_probe -- --irodori
cargo run -p reader-speech --example audiobook_probe -- recording.m4b --play --speed 10
```

The synthesize examples write WAVs without playing them. The playback probe plays a short phrase at 10x. It checks that source boundary events and completion arrive. Windows is exercised locally. macOS/Linux runtime packages are configured but still need native validation. Inference, model loading, alignment accuracy, and sustained 10x throughput vary by device. None is guaranteed across all devices.

## Sources and licenses

- [Irodori-TTS](https://github.com/Aratako/Irodori-TTS): the model declares MIT with additional voice-use restrictions. The codec ancestor has conflicting license declarations. See the [license review](../../docs/license-audit.md#irodori-source-and-codec-distinction).
- [audio.cpp](https://github.com/0xShug0/audio.cpp), pinned v0.9.3: Apache-2.0.
- [Qwen3 ForcedAligner](https://huggingface.co/Qwen/Qwen3-ForcedAligner-0.6B): Apache-2.0.
- [Kokoro ONNX](https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX): Apache-2.0.
- [ONNX Runtime](https://github.com/microsoft/onnxruntime): MIT; package notices retained.
- [Misaki](https://github.com/hexgrad/misaki): Apache-2.0. Cutlet-derived rules retain the original MIT notice. UniDic and MeCab notices stay with extracted Japanese resources. Optional NVIDIA CUDA/cuDNN DLLs have separate proprietary terms. New downloads require explicit permission; see the [Kokoro audit](../../docs/license-audit-kokoro.md).
- [Sonic](https://github.com/waywardgeek/sonic): Apache-2.0; license is included in `vendor/sonic/LICENSE`. Reader modifications add a timeline callback only.

External TTS endpoints receive the selected text. The built-in engines run locally after their assets have downloaded.
