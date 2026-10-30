# AivisSpeech, VOICEVOX and Windows voice license audit

Reviewed 2026-10-30 for the 0.2.2 source and packaging scripts. This audit covers components and distribution. It does not approve every voice, recording, book, or downstream use. VOICEROID2 is outside this audit.

## Current distribution boundary

The HTTP adapters call separately installed AivisSpeech/VOICEVOX engines. Windows voices use the installed Windows speech service. They do not copy engine implementation code into the reader. `scripts/ci-package.py` stages only listed reader/helper executables, fonts, and notices. It excludes app-data, installed engines, BERT files, and voice weights. The installer consumes that package. Publishing these adapters does not redistribute AivisSpeech, VOICEVOX, or Windows voice databases.

AivisSpeech's `prepare_aivis` discovers/starts an existing executable. The reader has no Aivis Engine installer or downloader. The development installation was downloaded separately from the official release. Once started, upstream Engine may obtain its own default voice/BERT assets. The Qwen alignment download is separate; see [speech implementation](speech.md).

No embedded third-party engine or voice files were found in these adapters. None identified here requires changing the reader's `MIT` declaration. This conclusion covers only the current separate processes/services and allowed package contents. It does not approve bundling an engine or providing a modified public speech service. Nor does it approve publishing every resulting recording.

## Native Aivis model inference

The new `aivis_native` provider is an independent Rust JP-Extra AIVMX implementation.
It neither launches AivisSpeech Engine nor links Engine or Style-Bert-VITS2 Python code.
The HTTP adapter and its LGPL/AGPL engine discussion below remain separate.
This does not change the licenses of imported models or BERT weights.
GPU runtime downloads use the shared explicit NVIDIA permission.
See the [separate runtime terms](license-audit-kokoro.md#download-permission-for-nvidia).

Native inference uses ONNX Runtime, Hugging Face tokenizers, and JPreprocess.
[JPreprocess 0.15.0](https://github.com/jpreprocess/jpreprocess/tree/v0.15.0) is BSD-3-Clause.
The original JPreprocess license and Open JTalk/NAIST-JDIC notices are retained.
The native package copies [these original notices](../crates/reader-speech/licenses/aivis-native/).
Resolved Rust dependency notices also retain their actual versions and license texts.

Existing Engine model/BERT files can be read without launching or modifying Engine.
The native downloader separately obtains まお 1.2.0 under ACML 1.0.
It also obtains the same pinned CC-BY-SA-4.0 BERT asset audited below.
The default AIVMX is 258,037,076 bytes; its checksum matches the まお entry below.
No neural weights or AivisSpeech Engine binaries enter the reader's Git repository or release.

Compatible AIVMX imports preserve and display each embedded manifest/license.
Header, metadata, and ONNX validation establish format compatibility, not permission for every use.
Other voices can have ACML-NC or custom terms.
The BERT card, full CC-BY-SA-4.0 text, and exact source pins stay in the native notices directory.
The pinned upstream BERT card contains only its license YAML.
The separate legal text is retained too.

## AivisSpeech Engine and dependencies

The exercised Windows x64 release is [AivisSpeech Engine 1.2.0](https://github.com/Aivis-Project/AivisSpeech-Engine/releases/tag/1.2.0). The setup manifest records this SHA-256 for the official `.7z.001` archive:

```text
bfbceba2e14dc7f23c7f3695f9ac0381baf91b15d6544e98384574eaadd271f3
``` The archive was not downloaded again during this audit. Recomputed SHA-256 of the installed `run.exe`:

```text
14715fff47da5875752a382ac9456adc29eb893626142c8539bb6cd49e4cb9d9
```

The Engine's own code uses [LGPL-3.0](https://github.com/Aivis-Project/AivisSpeech-Engine/blob/1.2.0/LICENSE). The dependency bundle is not LGPL-only. The [Engine dependency declaration](https://github.com/Aivis-Project/AivisSpeech-Engine/blob/1.2.0/pyproject.toml) pins Style-Bert-VITS2 to this revision:

```text
1588a46595141a04652f9aae486f8a625c04ce55
``` That dependency declares [AGPL-3.0-only](https://github.com/tsukumijima/Style-Bert-VITS2/blob/1588a46595141a04652f9aae486f8a625c04ce55/pyproject.toml) and includes its [license text](https://github.com/tsukumijima/Style-Bert-VITS2/blob/1588a46595141a04652f9aae486f8a625c04ce55/LICENSE). The installed Engine's dependency notice contains the AGPL text. Its machine-readable license label is `UNKNOWN`. The label alone must not be treated as permission or a missing notice.

The local upstream distribution keeps its terms and 104 dependency notices here:

```text
resources/engine_manifest_assets/
``` The separate setup record keeps copies in this app-data directory:

```text
speech/engines/AivisSpeech-Engine-1.2.0/licenses/
``` SHA-256 of the audited `ENGINE-DEPENDENCIES.json`:

```text
ac3e3b180a2b23c6035e7ec81bf828760b148b65f53171997acfc0bad5674d31
```

Keep this engine external and replaceable. Redistributing engine binaries would require preserving applicable licenses and notices. Provide the source and build information required by the actual LGPL/AGPL components. An upstream GitHub link alone is not a complete redistribution audit. For modified or hosted AGPL components, review source-offer requirements for network users too. The current reader does neither.

## The installed Aivis voice models

The installed AIVMX files were rehashed. These hashes match the local model metadata and official AivisHub pages:

| Model | Version / UUID | License |
| --- | --- | --- |
| [まお](https://hub.aivis-project.com/aivm-models/a59cb814-0083-4369-8542-f51a29e72af7) | 1.2.0 / `a59cb814-0083-4369-8542-f51a29e72af7` | ACML 1.0 |
| [コハク](https://hub.aivis-project.com/aivm-models/22e8ed77-94fe-4ef2-871f-a86f94e9a579) | 1.1.0 / `22e8ed77-94fe-4ef2-871f-a86f94e9a579` | ACML 1.0 |

SHA-256 (in table order):

```text
f87ccea2e8e2de0e0bfe52e803945af903b4086bf25621a015111628f00e4119  まお
3f5c08b52bb8a64efd361268580c81510f96c927cd6905aa7dbae6851333270a  コハク
```

Their creators are Oz Chat / Trippy / ねゆたろ. Each exact model's metadata and ACML text are retained in the app-data setup's `licenses` directory. [ACML 1.0](https://github.com/Aivis-Project/ACML/blob/master/ACML-1.0.md) permits commercial and noncommercial use, modification, and redistribution. Its prohibited uses still apply. Credit is optional; if provided, the model pages request `AivisSpeech: まお` or `AivisSpeech: コハク`. Redistributing a model requires attaching its license. Restrictions cover deceptive impersonation and specified harmful or abusive uses. Specified political/religious advocacy is also prohibited. ACML is not an unrestricted OSI software license. Its public-application exception still asks developers to comply. It also asks for reasonable efforts against prohibited uses. AivisHub also hosts ACML-NC and custom-license models. Another voice does not inherit these two models' permissions.

## Japanese BERT asset

The installed `model_fp16.onnx` is unchanged from [tsukumijima/deberta-v2-large-japanese-char-wwm-onnx](https://huggingface.co/tsukumijima/deberta-v2-large-japanese-char-wwm-onnx/tree/d701ec67708287b20d2063270f6b535e6eed09ab).

Revision:

```text
d701ec67708287b20d2063270f6b535e6eed09ab
```

Recomputed SHA-256:

```text
23f633ae7c5900ff82b35a428b67a54e7e7911d5d6a6dcfc77967be8f1c94dc6
``` The exact revision's original model card declares **CC-BY-SA-4.0**. The setup keeps that card, a source/revision/no-changes record and the full license locally. The model is not in the reader package or source repository.

Sharing that weight or an adaptation requires following [CC-BY-SA-4.0](https://creativecommons.org/licenses/by-sa/4.0/legalcode.en). This includes attribution, license references, and change notices. Shared adaptations need the required compatible license. Do not relabel it as the reader's MIT license or assume the Engine's software license covers it.

## VOICEVOX

The adapter calls a user-configured `/audio_query` and `/synthesis` endpoint. No VOICEVOX Engine, CORE, voice libraries or images are downloaded or shipped by this reader. The open-source [Engine license](https://github.com/VOICEVOX/voicevox_engine/blob/master/LICENSE) offers LGPL v3 or a separately obtained license. Product binaries, voice libraries, and generated recordings have distinct terms. The Engine's license does not replace those terms.

The official [VOICEVOX terms](https://voicevox.hiroshiba.jp/term/) require visible VOICEVOX credit. They also require following the chosen voice library's conditions. Use the required speaker credit when publishing recordings, commonly `VOICEVOX:キャラクター名`. Check that character's commercial-use and other conditions. See the [official credit FAQ](https://voicevox.hiroshiba.jp/qa/) and [voice pages](https://voicevox.hiroshiba.jp/). This integration does not grant permission to repackage official binaries or publish every voice. No installed VOICEVOX voice catalog was available for this audit. This review makes no commercial-use claim covering all voices.

## Windows system voices

The current System provider is **Windows-only**. Its original wrapper uses Windows PowerShell and `System.Speech.Synthesis`. It selects an enabled, installed voice and synthesizes a local WAV. It does not download, patch, copy, or redistribute voice databases or Windows components. This includes `System.Speech.dll` and SAPI runtimes. [Microsoft documents this installed-voice API](https://learn.microsoft.com/en-us/dotnet/api/system.speech.synthesis.speechsynthesizer.getinstalledvoices?view=netframework-4.8.1).

Installed voices remain subject to the Windows/voice supplier's license. The reader's MIT license does not cover those assets or guarantee commercial recording rights. Before distributing recordings, check the applicable installed product terms. Consult [Microsoft License Terms](https://www.microsoft.com/useterms) or the third-party voice supplier. This audit did not establish a blanket commercial-output grant for all SAPI voices. It also does not cover Edge online voices, Azure Speech or unofficial voice extraction.

## Release handling

Keep engines, voice weights, and generated book recordings out of Git and release archives. Also exclude user profiles, engine logs, and app-data manifests. The latter can contain local paths and reading text. Public documentation should describe Aivis as an optional installed engine. Name the applicable voice terms. Preserve the current package allowlist. Review licenses before adding engine/model mirrors, bundled weights, audio export, or hosted TTS.
