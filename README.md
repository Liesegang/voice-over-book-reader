# Voice Over Book Reader

[日本語](README.ja.md) · [Downloads](https://github.com/Liesegang/voice-over-book-reader/releases) · [Speech engines](docs/speech.md)

A Rust desktop EPUB reader for Japanese books and spoken reading.
The interface and current book renderer are native.
Dictionary definitions use a separate WebView.

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
reader-dictionary-popup.exe
reader-voiceroid-bridge.exe
licenses/ and the accompanying notices/documentation
```

Dictionary display requires Microsoft Edge WebView2 Runtime.
Local neural speech downloads additional assets on first use.
Model weights and third-party inference runtimes are separate from the installer and ZIP.

**macOS:** choose the Apple Silicon or Intel ZIP and extract its `.app` bundle.

**Linux x64:** extract the ZIP.
Install GTK 3, WebKitGTK 4.1, ALSA, and the platform display libraries.

CI produces builds for each platform.
Interactive native testing has primarily been performed on Windows.

## Build

Use Rust **1.97.0**.
Windows needs Visual Studio C++ Build Tools; macOS needs Xcode Command Line Tools.
On Ubuntu:

```sh
sudo apt-get install build-essential pkg-config libgtk-3-dev libwebkit2gtk-4.1-dev \
  libasound2-dev libudev-dev libxkbcommon-dev libwayland-dev \
  libx11-dev libxi-dev libxcursor-dev libxrandr-dev libvulkan1 fonts-noto-cjk
```

```sh
cargo build --workspace --bins --locked
cargo run -p voice-over-book-reader --locked -- --demo
cargo test --workspace --locked
```

To open a file, pass its path as a quoted argument. `--data-dir PATH` selects a separate application-data directory for development. Windows VOICEROID2 needs the matching x86 host:

```sh
rustup target add i686-pc-windows-msvc
cargo build -p reader-voiceroid-bridge --release --locked --target i686-pc-windows-msvc
```

Place that executable beside the reader.
[CI](.github/workflows/ci.yml) builds and tests the native targets.
[Release automation](.github/workflows/release.yml) checks version tags and packages every target.
After all targets succeed, it publishes the Windows installer, portable archives, and checksums.

## Formats and limits

EPUB, HTML/XHTML, Markdown, TXT, FB2, CBZ, and PDF are supported. PDFium is acquired separately when needed. MOBI/AZW/AZW3 use an installed Calibre `ebook-convert`. Recorded MP3/M4A/M4B/WAV audio can also be played.

The native renderer implements a subset of EPUB HTML/CSS.
It does not claim complete EPUB conformance.
Complex layouts and scripted books have limitations.
The separate Chromium renderer prototype is disabled in the released native reader.
See [EPUB support](docs/epub-support.md), [layout](docs/layout.md), and [dictionary details](docs/dictionaries.md).

Calibre libraries, source books, and user dictionaries are read-only. No DRM removal is provided.

## License

Original project code is [MIT](LICENSE-MIT).
Third-party code, fonts, dictionary data, models, voices, and runtimes retain their own licenses.
See the [notices](COPYING-THIRD-PARTY.txt) and [TTS license review](docs/license-audit.md).
The project license does not relicense external assets.
