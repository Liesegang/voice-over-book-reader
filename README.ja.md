# Voice Over Book Reader

[English](README.md) · [ダウンロード](https://github.com/Liesegang/voice-over-book-reader/releases) · [音声エンジン](docs/speech.md)

日本語の書籍と読み上げに対応した、Rust製のデスクトップEPUBリーダーです。
画面と現在の本文描画にはネイティブ実装を使い、辞書の定義は別プロセスのWebViewで表示します。

## 読書

- 縦書きと横書き、ルビ、行内画像、書籍のページ配置指定、画像の見開きに対応します。
- 本文と最下部のスライダーを表示し、メニューは本文の上に重ねます。
  開いていたサイドバーはメニューを再表示しても保持します。
- 階層付き目次、ブックマーク、検索、注釈、内部リンク、脚注を使えます。
- 現在の文字設定と画面サイズでページ数を計算します。
  ジャンプ履歴は原文の位置で保持し、組版が変わってもスライダー上のドットへ反映します。
- 本棚とリーダーは別ウィンドウです。
  複数冊を開け、Calibre連携、表紙表示と行リスト表示、自分の本棚を使えます。
- ライト、ダーク、セピアのほか、Solarized Light／Dark、One Dark、Nord、Gruvboxを選べます。
  背景色、文字色、アクセント色も調整できます。
  上下左右とページ間の余白は、それぞれ設定できます。
- MDX／MDD辞書の結果を縦に並べ、右の一覧から移動できます。
  辞書ウィンドウは移動、サイズ変更、固定に対応します。

## 読み上げ

選択した位置や本文の右クリックメニューから読み始められます。
音声エンジンの位置情報、または生成音声への位置合わせを使って本文をハイライトします。
再生速度は0.5〜10倍に対応し、先読みとキャッシュを使えます。
登録した読みは、書籍のルビより優先できます。
置き換えた読みも原文の位置へ対応付けます。

| エンジン | 用意するもの |
| --- | --- |
| Irodori | ローカルのネイティブ推論。モデルとランタイムは別途取得し、モデル固有の条件が適用されます。 |
| Kokoro | ローカルのONNX推論。日本語の発音データなどを別途取得します。 |
| AivisSpeech 組み込み | 対応AIVMXモデルとBERTを別途用意し、Rustで推論します。 |
| AivisSpeech Engine | 別途導入したEngineへ接続し、声とスタイルを選びます。 |
| Windows音声 | OSにインストールされた声を使います。 |
| VOICEROID2 | 正規インストール済み製品を、Windows用の同梱x86 DLLホストから使います。 |
| VOICEVOX・OpenAI互換・外部コマンド | 好きなエンジンやサービスへ接続します。 |

再生速度と音声の生成速度は異なります。
未生成の音声をCPUで作りながら10倍速を維持できるとは限りません。
声やモデルごとに利用条件も異なります。
[音声の詳細](docs/speech.md)と[ライセンス確認](docs/license-audit.md)を参照してください。

## インストール

[Releases](https://github.com/Liesegang/voice-over-book-reader/releases)から取得します。

**Windows x64:** `voice-over-book-reader-*-windows-x64-setup.exe`を実行します。
管理者権限なしで、現在のユーザー用にインストールします。
EPUBの登録を選ぶと、エクスプローラーの「プログラムから開く」とWindowsの「既定のアプリ」に追加されます。
完了画面からWindows設定を開き、EPUBの既定アプリとして選べます。
同じインストーラーで更新でき、アンインストール後も本、設定、辞書、取得したモデルは残ります。

ZIP版はフォルダー全体を展開し、次のファイルを同じ場所に置いてください。

```text
voice-over-book-reader.exe
reader-dictionary-popup.exe
reader-voiceroid-bridge.exe
licenses/ と同梱のライセンス表示・説明書
```

辞書の表示にはMicrosoft Edge WebView2 Runtimeが必要です。
ローカル音声の初回利用時には追加データを取得します。
アプリのインストーラーやZIPにモデルの重みや外部の推論ランタイムは含めません。

**macOS:** Apple Silicon用またはIntel用のZIPを選び、`.app`を展開します。
**Linux x64:** ZIPを展開し、GTK 3、WebKitGTK 4.1、ALSA、画面表示用のライブラリを用意します。
各OS向けのビルドはCIで作成します。
実画面を使ったネイティブ動作検証は、主にWindowsで行っています。

## ビルド

Rust **1.97.0**を使います。
WindowsはVisual Studio C++ Build Tools、macOSはXcode Command Line Toolsが必要です。
Ubuntuでは次を導入します。

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

書籍のパスを引用符で囲んだ引数として渡すと、その本を開けます。
開発時は`--data-dir PATH`で別のアプリデータ保存先を選べます。
WindowsのVOICEROID2には、対応するx86ホストも必要です。

```sh
rustup target add i686-pc-windows-msvc
cargo build -p reader-voiceroid-bridge --release --locked --target i686-pc-windows-msvc
```

生成した実行ファイルをリーダーと同じ場所に置きます。
[CI](.github/workflows/ci.yml)で各OSのビルドとテストを行い、[リリース処理](.github/workflows/release.yml)ではバージョンタグを確認してから全対象を作成します。
すべて成功すると、Windowsインストーラー、各OSのZIP、チェックサムを公開します。

## 形式と制約

EPUB、HTML／XHTML、Markdown、TXT、FB2、CBZ、PDFを読み込めます。
PDFiumは必要時に別途取得します。
MOBI／AZW／AZW3は導入済みのCalibre `ebook-convert`で変換します。
MP3／M4A／M4B／WAVの録音済み音声も再生できます。

現在のネイティブ本文描画が扱うEPUBのHTMLとCSSには制約があり、完全準拠とは表明していません。
複雑な組版やスクリプトを含む本には未対応の領域があります。
専用Chromium描画の試作は別に置いており、配布するネイティブ版では有効にしていません。
[EPUBの対応状況](docs/epub-support.md)、[表示指定](docs/layout.md)、[辞書の詳細](docs/dictionaries.md)に記載しています。

Calibreの本棚、原本、ユーザー辞書は読み取り専用です。
DRMの解除は行いません。

## ライセンス

プロジェクト自身のコードは[MIT](LICENSE-MIT)です。
外部のコード、フォント、辞書データ、モデル、声、ランタイムは、それぞれのライセンスに従います。
[第三者ライセンス表示](COPYING-THIRD-PARTY.txt)と[TTSのライセンス確認](docs/license-audit.md)を参照してください。
プロジェクトのMITは外部資産を再ライセンスするものではありません。
