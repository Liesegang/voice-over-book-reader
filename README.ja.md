# Voice Over Book Reader

[English](README.md) · [ダウンロード](https://github.com/Liesegang/voice-over-book-reader/releases) · [音声エンジン](docs/speech.md)

日本語の書籍と読み上げに対応した、Rust製のデスクトップEPUBリーダーです。
0.3の統合ソースは、実行時の検証を進めている段階です。
GUIはネイティブRustで実装しています。
0.3では本文を同梱のChromium Embedded Framework（CEF）で描画し、書籍のXHTML、CSS、SVG、埋込フォントを使います。
書籍内のスクリプトは実行しません。
辞書の定義は別プロセスのWebViewで表示します。

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
登録した読みは書籍のルビより優先できます。
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
voice-over-book-reader.dll
libcef.dll と同梱のChromium DLL
locales/ とChromiumのリソースファイル
CEF-MANIFEST.json
reader-dictionary-popup.exe
reader-voiceroid-bridge.exe
licenses/ と同梱のライセンス表示・説明書
```

本文描画用のChromiumはインストーラーとZIPに含まれ、別途ブラウザーを取得する必要はありません。
辞書の表示にはMicrosoft Edge WebView2 Runtimeが必要です。
ローカル音声の初回利用時には追加データを取得します。
音声モデルの重みや追加の推論ランタイムは、アプリのインストーラーやZIPに含めません。

**macOS:** Apple Silicon用またはIntel用のZIPを選び、`.app`を展開します。
**Linux x64:** ZIPを展開し、GTK 3、WebKitGTK 4.1、ALSA、画面表示用のライブラリを用意します。
各OS向けのビルドはCIで作成します。
実画面を使ったネイティブ動作検証は、主にWindowsで行っています。

## ビルド

Rust **1.97.1**、Python 3.12以降、CMake、Ninja、C／C++コンパイラーを使います。
WindowsはVisual Studio C++ Build Toolsとlibclang、macOSはXcode Command Line Toolsが必要です。
ビルド時には、バージョンが一致するCEF SDKも用意します。
SDKの準備と各OSの要件は[Chromiumのビルドと同梱](docs/chromium-runtime.md)を参照してください。

各対象の基準となるビルド手順はCIにあります。
固定したSDKを準備して`CEF_PATH`を設定し、実行ファイルとクライアントライブラリをビルドした後、必要なランタイムをまとめます。

```sh
python scripts/prepare-cef-sdk.py --target TARGET --directory CEF_SDK_DIRECTORY
# CEF_PATHに、作成されたcef_<platform>ディレクトリーを設定する。
cargo +1.97.1 build --workspace --release --bins --locked --target TARGET
cargo +1.97.1 build -p voice-over-book-reader --lib --release --locked --target TARGET
python scripts/ci-package.py --target TARGET --cef-path CEF_PATH --output-dir target/packages
```

`TARGET`は対象のターゲットトリプルに、SDKの仮のパスは実際のパスに置き換えてください。
Windowsの配布ビルドは`RUSTFLAGS=-C target-feature=+crt-static`を設定します。
`reader-voiceroid-bridge`は`i686-pc-windows-msvc`向けにもビルドします。
Windowsではパッケージに入った実行ファイルから起動します。
CEF公式の起動プログラムがサンドボックスの情報を保って同名のアプリDLLを読み込みます。
Cargoが作る通常の実行ファイルは、この起動プログラムとは異なります。

書籍のパスを引用符で囲んだ引数として渡すと、その本を開けます。
開発時は`--data-dir PATH`で別のアプリデータ保存先を選べます。

[CI](.github/workflows/ci.yml)で各OSのビルドとテストを行い、[リリース処理](.github/workflows/release.yml)ではバージョンタグを確認してから全対象を作成します。
すべて成功すると、Windowsインストーラー、各OSのZIP、チェックサムを公開します。

## 形式と制約

EPUB、HTML／XHTML、Markdown、TXT、FB2、CBZ、PDFを読み込めます。
PDFiumは必要時に別途取得します。
MOBI／AZW／AZW3は導入済みのCalibre `ebook-convert`で変換します。
MP3／M4A／M4B／WAVの録音済み音声も再生できます。

本文は原文の文書とローカルリソースをChromiumで描画します。
書籍内のスクリプトと外部ネットワークリソースは無効です。
EPUB完全準拠とは表明しておらず、公式の閲覧システム試験とアクセシビリティの検証は別途必要です。
[EPUBの対応状況](docs/epub-support.md)、[Chromiumの配布構成](docs/chromium-runtime.md)、[辞書の詳細](docs/dictionaries.md)を参照してください。

Calibreの本棚、原本、ユーザー辞書は読み取り専用です。
DRMの解除は行いません。

## ライセンス

プロジェクト自身のコードは[MIT](LICENSE-MIT)です。
外部のコード、フォント、辞書データ、モデル、声、ランタイムは、それぞれのライセンスに従います。
配布物にはCEFのBSDライセンス、Chromiumの原典クレジット、変更したweldingのMPL-2.0とソース参照先を含めます。
[第三者ライセンス表示](COPYING-THIRD-PARTY.txt)と[TTSのライセンス確認](docs/license-audit.md)を参照してください。
プロジェクトのMITは外部資産を再ライセンスするものではありません。
