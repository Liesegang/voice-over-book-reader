# TTS license review / TTSライセンス確認

Reviewed 2026-10-30 against source version 0.2.2. The review covers pinned downloads and release/installer contents. VOICEROID2's product license is outside this review, as requested. This review identifies components and what the project distributes. It does not approve every voice, recording, or downstream use.

## Result

The reader's original code can be published under [MIT](../LICENSE-MIT). Existing third-party code and data retain their own terms. Release packages contain the reader, helpers, fonts, and notices. They exclude neural weights, installed voice databases, and NVIDIA libraries. They also exclude the AivisSpeech and VOICEVOX engines. Preserve this distribution boundary.

The components have distinct conditions and one unresolved upstream inconsistency. Neither “every TTS component is MIT” nor “all generated audio is unrestricted” is correct.

### audio.cpp 0.9.3

Apache-2.0; acquired separately. Its archive is extracted with upstream notices intact. Apache-derived portions and vendored Sonic retain their original notices.

### Irodori v4.1 Small MF

Upstream declares MIT with additional voice-use restrictions. The converted GGUF includes a codec. Its ancestor's conflicting license declarations remain unresolved; see below. We do not redistribute those weights.

### Qwen3 ForcedAligner 0.6B

Apache-2.0 model; separately acquired for timing alignment.

### Kokoro, ONNX Runtime, Japanese frontend

Separately licensed; see the [Kokoro review](license-audit-kokoro.md). Misaki is **Apache-2.0**, not MIT.

### Optional NVIDIA GPU libraries

Proprietary terms, not the reader's MIT. New downloads require explicit permission after the terms are shown. Installed libraries and CPU inference remain separate options.

### Native AivisSpeech models

Independent Rust inference reads compatible AIVMX models, with no Engine Python process.
Model-specific terms and the BERT CC-BY-SA-4.0 license still apply.
JPreprocess and Open JTalk/NAIST-JDIC notices are retained separately.
See the [native implementation review](license-audit-aivis-voicevox.md#native-aivis-model-inference).

### AivisSpeech / VOICEVOX / Windows voices

The reader uses existing engines and voices through their interfaces. It does not redistribute them. Aivis's dependency bundle includes AGPL code; it is not LGPL-only. See the [adapter and voice review](license-audit-aivis-voicevox.md).

### OpenAI-compatible HTTP / external commands

The adapter does not distribute a service or the user's chosen engine. The selected service, voice, model, and recording terms still apply.

## Irodori: source and codec distinction

The [pinned Irodori model card](https://huggingface.co/Aratako/Irodori-TTS-v4.1-Small-MF/blob/ccc78f5d480b6e51b69b2d5042a14c4da04fea6e/README.md) declares MIT. It also restricts nonconsensual impersonation and misleading synthetic speech. These are recorded as model conditions, not folded into the reader's MIT license.

The [converted package](https://huggingface.co/audio-cpp/audio.cpp-gguf/blob/462b4cec041e06ffb3d8fcf0d8c655051b1cd9f5/Irodori-TTS-v4-Small-GGUF/README.md) embeds its tokenizer, configuration and DACVAE codec. [Semantic-DACVAE-Japanese-32dim](https://huggingface.co/Aratako/Semantic-DACVAE-Japanese-32dim/blob/47376ee24834d7a05a48ebabfe3cde29b3c5e214/README.md) declares MIT. It identifies `facebook/dacvae-watermarked` as an ancestor. The [ancestor's own card](https://huggingface.co/facebook/dacvae-watermarked/blob/main/README.md) labels its metadata Apache-2.0 but names the SAM License in its body. No model LICENSE file was present in that repository's inspected file list. The separate [DACVAE code repository](https://github.com/facebookresearch/dacvae/blob/main/LICENSE) is Apache-2.0.

Those primary sources conflict. Permission to relicense or redistribute the ancestor-derived weights as pure MIT is not established. The project keeps these weights out of Git and releases and retains upstream links. It makes no unconditional commercial-use claim for Irodori. Seek upstream clarification before redistributing those weights. The same applies before relying on an interpretation of the conflicting terms.

## Runtime, alignment and notices

[audio.cpp v0.9.3](https://github.com/0xShug0/audio.cpp/blob/v0.9.3/LICENSE) and [Qwen3 ForcedAligner](https://huggingface.co/Qwen/Qwen3-ForcedAligner-0.6B) declare Apache-2.0. Conversion does not replace model/component terms. The audio.cpp [license inventory](https://github.com/0xShug0/audio.cpp/blob/v0.9.3/docs/model_licenses.md) states this distinction. Exact download revisions/hashes are in [speech documentation](speech.md) and source constants.

Bundled [Sonic](../crates/reader-speech/vendor/sonic/LICENSE) is Apache-2.0, with its original headers and local-change notices retained. The packager includes its full license. Rust dependency notices give exact versions and source-archive URLs. These include unchanged MPL-covered crates. [Third-party notices](../COPYING-THIRD-PARTY.txt), font OFL notices, and IPADIC notices retain their own terms. The supplementary [Apache text](../licenses/Apache-2.0.txt) is also separate from the project's MIT license.

## 日本語要点

独自コードはMITで公開できますが、外部モデルや声までMITになるわけではありません。
配布物にモデル、AivisSpeech本体、Windows音声データ、NVIDIAランタイムは同梱しません。
第三者ライセンス表示は残します。

MisakiはApache-2.0へ表記を訂正しました。
AivisSpeechの依存にはAGPLがあり、外部プロセスとの接続と、エンジンの直接取り込みや再配布は区別が必要です。
声ごとの条件は[個別確認](license-audit-aivis-voicevox.md)を参照してください。

Irodoriにはモデル固有の利用条件があり、元コーデックの一次資料にも表記の不一致が残ります。
重みの再配布や無条件の商用利用を承認した結果ではありません。
VOICEROID2製品の条件は今回確認していません。
