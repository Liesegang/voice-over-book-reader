# EPUB対応状況と残っている課題

2026-10-30時点の開発中ソースを確認した結果です。
現在の独自ネイティブ描画系は、EPUB 3.3完全準拠を達成していません。
自作テストの成功、仕様の一部の実装、公式テストの合格を区別します。
公式EPUBテスト一式の実行結果はまだありません。

## 基準と検証の扱い

利用者が示した[W3C EPUBの案内](https://www.w3.org/publishing/epub3/)から、[EPUB 3.3](https://www.w3.org/TR/epub-33/)と[EPUB Reading Systems 3.3](https://www.w3.org/TR/epub-rs-33/)を基準にしています。
仕様には必須、推奨、任意、条件付きの要件があり、例えばスクリプトを実行しない場合も代替内容の処理を別途確認する必要があります。
全機能を有効化しても、準拠したことにはなりません。

[公式テストの案内](https://w3c.github.io/epub-tests/index.html)は調査時点で3.4向けへ移っています。
3.3の比較には[保存された3.3テスト表](https://w3c.github.io/epub-tests/epub33/index.html)を使います。
そこから各テストのソースと仕様の対応箇所を辿れます。
以下の公式IDは、実装と比較するテストを示します。
本アプリの合格を示すものではありません。

## 現在の実装と不足

### OCF/ZIP・URL

実装と自作テスト: ZIPの中央ディレクトリを使い、必要なファイルだけ展開。
相対パス、ルート相対、UTF-8パーセント符号化、ルート外への移動拒否をテスト

制約と公式テスト: ZIP64、複数パッケージ、圧縮・暗号化の全要件は公式試験未実施。
`ocf-package_multiple`、`ocf-zip-comp`、`ocf-zip-mult`、`ocf-url_*`

### Package/manifest/spine

実装と自作テスト: EPUB 2/3の主要メタデータ、主spine順、XHTML/SVGの遅延ロード。
補助文書は通常の読書順へ混ぜずリンク先として登録

制約と公式テスト: 外部リソースを含むmanifest、foreign-resource fallbackの連鎖、重複spineのすべての挙動、全メタデータの意味処理は不足。
`pkg-spine-*`、`pkg-manifest-*`、`pub-foreign_*`

### 内部リンク・注記

実装と自作テスト: 別章、未ロード章、`linear="no"`、非spine注記へのリンクをcoreでテスト。
`../`、query付きpath、日本語path/fragment、旧nameアンカー、注記から本文へのリンクも確認。
noteref/ARIA doc-noterefの意味を保持

制約と公式テスト: EPUB CFIや全fragment方式は未対応。
GUIでのすべての非spine移動と、CSSで隠した注記の扱いは追加検証が必要。
`pkg-spine-nonlinear-activation`、`pkg-spine-duplicate-item-hyperlink`、`ocf-url_link-*`

### Navigation

実装と自作テスト: 階層TOC、NCX、アンカーを読む。
ネイティブ目次を表示

制約と公式テスト: page-list/landmarksの独立UI、画像だけのnavラベル、非spine対象TOC、nav文書の全表示要件は不足。
`nav-access`、`nav-activation`、`nav-non-text_img*`、`nav-spine_*`

### XHTML/HTML

実装と自作テスト: 段落、見出し、引用、リスト、表、ルビ、リンク、画像などを独自モデルへ変換

制約と公式テスト: 完全なHTMLのDOM、要素の意味、フォーム、埋め込み、レイアウトの互換性はない。
`cnt-xhtml-support`を合格扱いにしない

### CSS

標準スタイルは`assets/default-reader.css`をユーザーエージェント起源のCSSとして読みます。
通常の著者CSSは、詳細度が低い`*`指定でも標準スタイルより優先します。
標準のh1は1.4emで後余白0.8em、h2は1.25emで後余白0.65emです。
h2には太字も指定します。

文字サイズはpx、em、rem、%、一般キーワードを扱い、親からの継承を計算してBlock/Atomに保持します。
組版時には、計算したサイズを一度だけ適用します。
`margin-block`とそのstart/end指定、物理方向のmarginと省略記法も、書字方向に応じて適用します。
CSSの変更はページ数キャッシュにも反映します。
標準CSSはソースに埋め込むため、変更には再ビルドが必要です。
設定画面にCSSエディターを実装したものではありません。

実装と自作テスト: 対応プロパティのカスケード、詳細度、inline、important、インデント、画像寸法、縦書き、強調などの自作テスト

制約と公式テスト: CSS全般、flex/grid、float、absolute配置、複雑なセレクター、変数、メディアクエリー、各種分割、禁則、ハイフネーションなどを網羅しない。
`cnt-xhtml-support`、`css-epub-*`

### フォント

実装と自作テスト: アプリ内フォント、OSのフォント選択、字形・縦書き形の形成と描画

制約と公式テスト: EPUB内の`@font-face`、TrueType/OpenType/WOFF/WOFF2、フォント難読化解除は未対応。
OSフォントを選べることは埋込フォント対応の代わりにならない。
`cnt-css-fonts`、`ocf-font_obfuscation*`

### 言語・方向

実装と自作テスト: 日本語ルビ、縦書き、縦中横、縦書き字形、左右のページ進行方向を実装・自作検証

制約と公式テスト: Unicode Bidi、HTML dir/bdi/bdo、アラビア語等の混在方向処理全般、全Unicode組版は未検証。
`pkg-dir_*`、`pkg-lang_but_not_content`、`pkg-spine-progression_*`

### 固定レイアウト

実装と自作テスト: package/spineのlayout/spread/配置、XHTML viewport、SVG viewBoxを読む。
自作fixtureと単体テストあり

制約と公式テスト: 任意の固定XHTML/CSSを原著どおり描く完全なエンジンではない。
orientation指定、すべてのviewport構文・優先順位は網羅試験が必要。
`fxl-*`、`lay-fxl-*`

### 画像

実装と自作テスト: PNG/JPEG/GIF/WebPと静的SVGの読み込み、インライン外字、縦横比、寸法、先読みを自作テスト

制約と公式テスト: EPUB内data URL画像は現在の通常のimg処理で未対応。
アニメーション、形式の全公式試験も未実施。
`pub-cmt-*`、`pub-data-urls_*`

### SVG

実装と自作テスト: resvgで静的SVGを描画。
外部画像解決とSVG spine、viewBox寸法を実装

制約と公式テスト: SVG DOM、スクリプト、動的挙動、ホストCSSとの全相互作用、すべてのSVG/CSS要件は未検証。
SVG内アンカーはページ単位。
`cnt-svg-*`

### MathML

実装と自作テスト: 元のMathMLを数式として保持し、対応する数式をネイティブ描画する自作テスト

制約と公式テスト: 全MathML表現とアクセシビリティは未検証。
`cnt-mathml-support`の合格宣言はしない

### JavaScript

実装と自作テスト: 本文のscriptを実行しない

制約と公式テスト: DOMイベント、epubReadingSystem、origin/storage、scripted-content fallbackの包括処理は未対応。
実行しないことだけではfallback要件の確認にならない。
`scr-support-fallback`、`scr-readingsystem-*`、`scr-support*`

### 埋込メディア

実装と自作テスト: 録音済みのMP3/AAC/ALAC/WAVは別のオーディオブック機能で再生

制約と公式テスト: EPUB内audio/video要素、全core audio形式、動画や字幕との連携は別途未実装。
`pub-cmt-mp3`、`pub-cmt-mp4`、`pub-cmt-opus`

### Media overlays

実装と自作テスト: アプリのTTSは実音声の境界から本文を強調

制約と公式テスト: EPUBのSMIL、media-overlay属性、録音と本文の同期、skippability/escapabilityは未実装。
独自TTSはMedia Overlays対応の代わりにならない。
`mol-*`

### 構造・アクセシビリティ

実装と自作テスト: 一部の見出し、表、alt、ruby、noterefをモデルに保持し、GUIのキー操作とラベルを実装

制約と公式テスト: 本文の完全なアクセシビリティツリー、ARIAの全意味、読み順、スクリーンリーダー、EPUB Accessibility 1.1は未検証。
準拠とは表明しない

### セキュリティ・ネットワーク

実装と自作テスト: 書籍を変更せず、ZIPルート外や外部protocolを内部リンクとして読まない自作試験あり

制約と公式テスト: 外部リンク同意、remote resource、XML/HTMLフォールバック、originと権限の全要件の監査は未完了。
`pub-file-urls`、`pub-xml-*`、`pub-external-links*`、`ocf-url_origin`

## 再実行できる自作テスト

次のコマンドで自作テストを実行します。

```sh
cargo test -p reader-core --all-targets --locked
```

対象は`tests/documents.rs`、`tests/lazy_epub.rs`です。
`tests/presentation.rs`と`tests/internal_links.rs`も含みます。
内部リンクの追加テストは、元ファイルが変わらないことと補助文書が本の通常の章・検索順へ入り込まないことも確認します。
GUIの画像、組版、見開きは`reader-app`側のテストと実画面確認を併用します。

これらは公式試験の代替ではありません。
表示エンジンを変更する場合は、公式テスト表の版、テストID、viewport、設定、OS、実際の結果を記録し、未実行を合格へ数えないことを検証の基準にします。

## 実装方針として残る判断

CSS/HTML/SVG全般や埋込フォント、JavaScript、アクセシビリティへの対応には、表示エンジンの検討が必要です。
現在の独自組版を拡張する案と、Servo等の文書エンジンを組み込む案では、実装量と動作の性質が大きく変わります。
WebViewを本文に使わないという利用者の条件、エンジンの実現可能性、ライセンス、ページ分割、選択位置や読み上げ位置との対応を確認して決める必要があります。
この文書は現在の実装を整理したもので、エンジンの変更や完全対応の達成を示すものではありません。
