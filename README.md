# Voice Over Book Reader

[日本語](README.ja.md) · [Downloads](https://github.com/Liesegang/voice-over-book-reader/releases) · [Speech engines](docs/speech.md)

A Rust desktop EPUB reader for Japanese books and spoken reading.
The 0.3 source integration is undergoing runtime validation.
The interface is native Rust.
Version 0.3 renders books with bundled Chromium Embedded Framework (CEF).
It uses author XHTML, CSS, SVG and embedded fonts.
Book scripts are disabled. Dictionary definitions use a separate WebView.

## Reading

- Japanese vertical and horizontal text, ruby, and inline illustrations. Publisher page placement and image spreads.
- A reading surface with a persistent page slider. Menus overlay the book. An open sidebar returns when menus reappear.
- Nested contents, bookmarks, search, annotations, internal links, and footnotes.
- Page-based progress calculated for the current typography and window size. Recent jump markers retain source positions through reflow.
- Separate library and reader windows with multiple books open. Calibre, grid/list views, and custom shelves.
- Light, dark, sepia, Solarized Light/Dark, One Dark, Nord, and Gruvbox. Custom paper, text, and accent colors; independent vertical, horizontal, and spread spacing.
- MDX/MDD dictionaries with continuous definitions and an index. Move, resize, or pin the lookup window.

## Spoken reading

Start from selected text or the book's context menu.
Reading highlights follow provider timing or acoustic alignment.
Playback supports 0.5 to 10× speed, with background synthesis and caching.
Registered pronunciations can override readings, including ruby.
Highlights still refer to the original text.

### Irodori

Local native inference; model/runtime assets download separately. Model-specific terms apply.

### Kokoro

Local ONNX inference; Japanese pronunciation resources download separately.

### AivisSpeech

Run compatible AIVMX models natively, or connect to a separately installed Engine.
Select the model's voice/style. Model and BERT assets have separate licenses.

### Windows voices

Use installed system voices.

### VOICEROID2

Use an existing licensed installation through the included x86 DLL host on Windows.

### VOICEVOX, OpenAI-compatible, command

Connect to an engine/service of your choice.

Uncached CPU synthesis may not keep up with 10× playback.
Voice/model licenses and conditions for using recordings differ.
See [speech details](docs/speech.md) and the [license review](docs/license-audit.md).

## Install

Get a build from [Releases](https://github.com/Liesegang/voice-over-book-reader/releases).

**Windows x64:** run `voice-over-book-reader-*-windows-x64-setup.exe`.
Installation is per-user and requires no administrator rights.
Select EPUB registration to add the reader to **Open with** and **Default apps**.
The finish page can open Windows Settings so you can choose your default EPUB reader.
Use the same installer for updates.
Uninstalling preserves books, settings, dictionaries, and downloaded models.

For the portable ZIP, extract the complete folder and keep these together:

```text
voice-over-book-reader.exe
voice-over-book-reader.dll
libcef.dll and the accompanying Chromium DLLs
locales/ and Chromium resource files
CEF-MANIFEST.json
reader-dictionary-popup.exe
reader-voiceroid-bridge.exe
licenses/ and the accompanying notices/documentation
```

The installer and portable ZIP include Chromium.
Book rendering needs no browser download.
Dictionary display requires Microsoft Edge WebView2 Runtime.
Local neural speech downloads additional assets on first use.
Speech models and optional inference runtimes are separate from the installer and ZIP.

**macOS:** choose the Apple Silicon or Intel ZIP and extract its `.app` bundle.

**Linux x64:** extract the ZIP.
Install GTK 3, WebKitGTK 4.1, ALSA, and the platform display libraries.

CI produces builds for each platform.
Interactive native testing has primarily been performed on Windows.

## Build

Use Rust **1.97.1**, Python 3.12 or newer, CMake, Ninja, and a C/C++ toolchain.
Windows needs Visual Studio C++ Build Tools and libclang; macOS needs Xcode Command Line Tools.
The matching CEF SDK is required at build time.
[Chromium build and bundle details](docs/chromium-runtime.md) describe SDK preparation and platform requirements.

CI is the reference build procedure for all four targets.
It prepares the pinned SDK and sets `CEF_PATH`.
It then builds the binaries and client library and packages the runtime:

```sh
python scripts/prepare-cef-sdk.py --target TARGET --directory CEF_SDK_DIRECTORY
# Set CEF_PATH to the resulting cef_<platform> directory.
cargo +1.97.1 build --workspace --release --bins --locked --target TARGET
cargo +1.97.1 build -p voice-over-book-reader --lib --release --locked --target TARGET
python scripts/ci-package.py --target TARGET --cef-path CEF_PATH --output-dir target/packages
```

Replace `TARGET` with the platform target triple and the SDK placeholders with actual paths.
Windows shipping builds use `RUSTFLAGS=-C target-feature=+crt-static`.
Also build `reader-voiceroid-bridge` for `i686-pc-windows-msvc`.
On Windows, launch the packaged executable.
The CEF bootstrap loads the same-named app DLL with its sandbox context.
The ordinary Cargo executable is not that bootstrap.

To open a file, pass its path as a quoted argument to the packaged application.
`--data-dir PATH` selects a separate application-data directory for development.

[CI](.github/workflows/ci.yml) builds and tests the native targets.
[Release automation](.github/workflows/release.yml) checks version tags and packages every target.
After all targets succeed, it publishes the Windows installer, portable archives, and checksums.

## Formats and limits

EPUB, HTML/XHTML, Markdown, TXT, FB2, CBZ, and PDF are supported. PDFium is acquired separately when needed. MOBI/AZW/AZW3 use an installed Calibre `ebook-convert`. Recorded MP3/M4A/M4B/WAV audio can also be played.

Book rendering uses Chromium with the original document and local resources.
Author scripts and external network resources are disabled.
The application does not claim complete EPUB conformance.
Official reading-system tests and accessibility coverage remain separate work.
See [EPUB support](docs/epub-support.md), [Chromium distribution](docs/chromium-runtime.md), and [dictionary details](docs/dictionaries.md).

Calibre libraries, source books, and user dictionaries are read-only. No DRM removal is provided.

## License

Original project code is [MIT](LICENSE-MIT).
Third-party code, fonts, dictionary data, models, voices, and runtimes retain their own licenses.
Packages include the original CEF BSD license and Chromium credits.
They also contain the MPL-2.0 license and source reference for modified welding code.
See the [notices](COPYING-THIRD-PARTY.txt) and [TTS license review](docs/license-audit.md).
The project license does not relicense external assets.
