use reader_core::{BlockKind, Book, Locator, WritingMode, open, parse_html};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::SimpleFileOptions};

fn epub_file(entries: &[(&str, &str)]) -> tempfile::NamedTempFile {
    archive_file(".epub", entries)
}

fn archive_file(suffix: &str, entries: &[(&str, &str)]) -> tempfile::NamedTempFile {
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, content) in entries {
        archive
            .start_file(*name, SimpleFileOptions::default())
            .unwrap();
        archive.write_all(content.as_bytes()).unwrap();
    }
    let bytes = archive.finish().unwrap().into_inner();
    let mut file = tempfile::Builder::new().suffix(suffix).tempfile().unwrap();
    file.write_all(&bytes).unwrap();
    file
}

const CONTAINER: &str = r#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="EPUB/package.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;

#[test]
fn ruby_reads_once_and_maps_each_base_node_by_utf8_bytes() {
    let html = r#"<html><body><p>A&amp;<ruby><rb>東</rb><rb>京</rb><rp>(</rp><rt>とうきょう</rt><rp>)</rp></ruby>😺</p></body></html>"#;
    let chapter = parse_html(html, 3, "chapter.xhtml", "").unwrap();
    assert_eq!(chapter.plain_text(), "A&東京😺");
    let ruby = chapter
        .atoms()
        .into_iter()
        .find(|atom| atom.ruby.is_some())
        .unwrap();
    assert_eq!(ruby.spoken_text(), "とうきょう");
    assert_eq!(ruby.text, "東京");
    assert_eq!(ruby.sources.len(), 2);
    assert_eq!(ruby.locator_at(0).unwrap().chapter, 3);
    assert_eq!(
        ruby.locator_at("東".len()).unwrap(),
        ruby.sources[1].start_locator()
    );
    assert_eq!(
        ruby.locator_at("東京".len()).unwrap(),
        ruby.sources[1].end_locator()
    );
    assert!(!chapter.plain_text().contains("とうきょう"));
}

#[test]
fn whitespace_collapse_preserves_source_offsets_and_ideographic_spaces() {
    let chapter = parse_html(
        "<html><body><p>  甲 \n\t <em>乙</em>　丙  </p></body></html>",
        0,
        "x.xhtml",
        "",
    )
    .unwrap();
    assert_eq!(chapter.plain_text(), "甲 乙　丙");
    let atoms = chapter.atoms();
    assert_eq!(atoms[0].sources[0].start, 2);
    assert!(
        atoms
            .iter()
            .any(|atom| atom.text == "乙" && atom.style.italic)
    );
    for atom in atoms {
        assert_eq!(
            atom.text.len(),
            atom.sources.iter().map(|span| span.len()).sum::<usize>()
        );
    }
}

#[test]
fn mono_ruby_boundaries_are_not_fused_into_a_group() {
    for ruby in [
        "<ruby>東<rt>とう</rt>京<rt>きょう</rt></ruby>",
        "<ruby><rb>東</rb><rb>京</rb><rt>とう</rt><rt>きょう</rt></ruby>",
    ] {
        let chapter = parse_html(
            &format!("<html><body><p>{ruby}</p></body></html>"),
            0,
            "x.xhtml",
            "",
        )
        .unwrap();
        let atoms = chapter.atoms();
        assert_eq!(atoms.len(), 2);
        assert_eq!(atoms[0].text, "東");
        assert_eq!(atoms[0].ruby.as_deref(), Some("とう"));
        assert_eq!(atoms[1].text, "京");
        assert_eq!(atoms[1].ruby.as_deref(), Some("きょう"));
    }
}

#[test]
fn code_tables_lists_quote_and_inline_svg_are_structural() {
    let html = r#"<html><body><h1>道具</h1><pre><code>  let x = 1;
    println!("{}", x);
</code></pre><blockquote><p>引用</p></blockquote><ol start="3"><li>一つ</li><li>二つ</li></ol>
    <table><caption>比較</caption><thead><tr><th>名前</th><th>値</th></tr></thead><tbody><tr><td colspan="2">結合</td></tr></tbody></table>
    <svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><title>図形</title><rect width="10" height="10"/></svg>
    </body></html>"#;
    let chapter = parse_html(html, 0, "x.xhtml", "").unwrap();
    let code = chapter
        .blocks
        .iter()
        .find(|block| block.kind == BlockKind::Code)
        .unwrap();
    assert_eq!(
        code.plain_text(),
        "  let x = 1;\n    println!(\"{}\", x);\n"
    );
    assert!(
        chapter
            .blocks
            .iter()
            .any(|block| block.kind == BlockKind::Quote)
    );
    let list: Vec<_> = chapter
        .blocks
        .iter()
        .filter(|block| block.kind == BlockKind::ListItem)
        .collect();
    assert_eq!(list[0].list_marker.as_deref(), Some("3."));
    assert_eq!(list[1].list_marker.as_deref(), Some("4."));
    let table = chapter
        .blocks
        .iter()
        .find_map(|block| block.table.as_ref())
        .unwrap();
    assert_eq!(table.rows.len(), 2);
    assert!(table.rows[0].header);
    assert_eq!(table.rows[1].cells[0].colspan, 2);
    assert!(!table.rows[1].cells[0].blocks[0].atoms[0].sources.is_empty());
    let image = chapter
        .blocks
        .iter()
        .find_map(|block| block.image.as_ref())
        .unwrap();
    assert!(image.inline_svg.as_deref().unwrap().contains("<rect"));
    assert_eq!(image.alt, "図形");
}

#[test]
fn epub3_spine_metadata_cover_toc_css_and_resources() {
    let opf = r#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>日本語の本</dc:title><dc:creator>著者</dc:creator><dc:language>ja</dc:language></metadata><manifest>
    <item id="c1" href="text/first%20chapter.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="text/second.xhtml" media-type="application/xhtml+xml"/>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="css" href="style.css" media-type="text/css"/>
    <item id="cover" href="cover.svg" media-type="image/svg+xml" properties="cover-image"/>
    </manifest><spine page-progression-direction="rtl"><itemref idref="c2"/><itemref idref="c1"/></spine></package>"#;
    let first = r#"<html><head><title>一章</title><link rel="stylesheet" href="../style.css"/></head><body class="vertical"><p><ruby>本<rt>ほん</rt></ruby></p><img src="../cover.svg" alt="表紙"/></body></html>"#;
    let second = r#"<html><head><title>二章</title></head><body><p>先に読む</p></body></html>"#;
    let nav = r#"<html xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><ol><li><a href="text/second.xhtml">二章から</a><ol><li><a href="text/first%20chapter.xhtml#start">一章へ</a></li></ol></li></ol></nav></body></html>"#;
    let file = epub_file(&[
        ("META-INF/container.xml", CONTAINER),
        ("EPUB/package.opf", opf),
        ("EPUB/text/first chapter.xhtml", first),
        ("EPUB/text/second.xhtml", second),
        ("EPUB/nav.xhtml", nav),
        (
            "EPUB/style.css",
            ".vertical { -epub-writing-mode: vertical-rl; }",
        ),
        (
            "EPUB/cover.svg",
            "<svg xmlns='http://www.w3.org/2000/svg'/>",
        ),
    ]);
    let mut book = open(file.path()).unwrap();
    assert!(!book.chapters[1].loaded);
    book.ensure_chapter_loaded(1).unwrap();
    assert_eq!(book.title, "日本語の本");
    assert_eq!(book.authors, ["著者"]);
    assert_eq!(book.language, "ja");
    assert_eq!(book.chapters[0].title, "二章");
    assert_eq!(book.chapters[1].writing_mode, WritingMode::VerticalRl);
    assert_eq!(book.page_progression, reader_core::PageProgression::Rtl);
    assert_eq!(
        book.chapters[1].blocks[0].atoms[0].ruby.as_deref(),
        Some("ほん")
    );
    assert_eq!(book.cover.as_deref(), Some("EPUB/cover.svg"));
    assert!(book.resource("EPUB/cover.svg").is_some());
    assert_eq!(book.toc[0].chapter, 0);
    assert_eq!(book.toc[0].children[0].chapter, 1);
    assert_eq!(book.toc[0].children[0].fragment.as_deref(), Some("start"));
    assert_eq!(open(file.path()).unwrap().id, book.id);
}

#[test]
fn epub2_ncx_and_legacy_cover_metadata() {
    let opf = r#"<package version="2.0"><metadata><title>Legacy</title><meta name="cover" content="cover"/></metadata><manifest><item id="c" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="cover" href="cover.jpg" media-type="image/jpeg"/></manifest><spine toc="ncx"><itemref idref="c"/></spine></package>"#;
    let ncx = r#"<ncx><navMap><navPoint id="one"><navLabel><text>Start here</text></navLabel><content src="chapter.xhtml#part"/></navPoint></navMap></ncx>"#;
    let file = epub_file(&[
        ("META-INF/container.xml", CONTAINER),
        ("EPUB/package.opf", opf),
        (
            "EPUB/chapter.xhtml",
            "<html><body><p>本文</p></body></html>",
        ),
        ("EPUB/toc.ncx", ncx),
        ("EPUB/cover.jpg", "image"),
    ]);
    let book = open(file.path()).unwrap();
    assert_eq!(book.toc[0].title, "Start here");
    assert_eq!(book.toc[0].fragment.as_deref(), Some("part"));
    assert_eq!(book.cover.as_deref(), Some("EPUB/cover.jpg"));
}

#[test]
fn invalid_container_and_traversal_are_errors() {
    let file = epub_file(&[(
        "META-INF/container.xml",
        "<container><rootfile full-path='../escape.opf'/></container>",
    )]);
    let error = open(file.path()).unwrap_err();
    assert!(error.to_string().contains("escapes"));
}

#[test]
fn loose_html_and_embedded_styles_work_without_executing_scripts() {
    let chapter = parse_html("<html><head><style>body { writing-mode:vertical-rl; } .hidden {display:none;}</style></head><body><p>甲<br>乙<p class=hidden>不可視<script>fakeBookText()</script>", 0, "x.html", "").unwrap();
    assert_eq!(chapter.writing_mode, WritingMode::VerticalRl);
    assert_eq!(chapter.plain_text(), "甲\n乙");
}

#[test]
fn search_crosses_inline_boundaries_and_locator_roundtrips() {
    let chapter = parse_html(
        "<html><body><p>読む<strong>ため</strong>の道具</p></body></html>",
        0,
        "x.xhtml",
        "",
    )
    .unwrap();
    let mut book = Book::demo();
    book.chapters = vec![chapter];
    let hits = book.search("むための");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].start.offset, "読".len() as u32);
    assert_ne!(hits[0].start.node, hits[0].end.node);
    let json = serde_json::to_string(&hits[0].start).unwrap();
    assert_eq!(
        serde_json::from_str::<Locator>(&json).unwrap(),
        hits[0].start
    );
    assert!(book.search("").is_empty());
}

#[test]
fn markdown_keeps_code_and_tables() {
    let mut file = tempfile::Builder::new().suffix(".md").tempfile().unwrap();
    write!(
        file,
        "# Heading\n\n```rust\nlet x = 1;\n```\n\n| A | B |\n|---|---|\n| 1 | 2 |\n"
    )
    .unwrap();
    let book = open(file.path()).unwrap();
    assert!(
        book.chapters[0]
            .blocks
            .iter()
            .any(|block| block.kind == BlockKind::Code)
    );
    assert!(
        book.chapters[0]
            .blocks
            .iter()
            .any(|block| block.kind == BlockKind::Table)
    );
}

#[test]
fn css_supported_property_cascade_honors_specificity_inline_and_important() {
    let html = r#"<html><body class="vertical" id="reader" style="writing-mode: horizontal-tb"><p id="start"><strong style="font-weight:normal">本文</strong></p></body></html>"#;
    let css = "/* body orientation */ #reader { -epub-writing-mode: vertical-rl !important; } body { writing-mode: horizontal-tb; } .vertical {writing-mode:horizontal-tb;}";
    let chapter = parse_html(html, 0, "x.xhtml", css).unwrap();
    assert_eq!(chapter.writing_mode, WritingMode::VerticalRl);
    assert!(!chapter.atoms()[0].style.bold);
    assert_eq!(
        chapter.anchors["start"],
        chapter.atoms()[0].sources[0].start_locator()
    );
    let chapter = parse_html(html, 0, "x.xhtml", "#reader{writing-mode:vertical-rl}").unwrap();
    assert_eq!(chapter.writing_mode, WritingMode::Horizontal);
}

#[test]
fn mathml_is_preserved_as_a_typed_formula() {
    let chapter = parse_html(r#"<html><body><p>式</p><math xmlns="http://www.w3.org/1998/Math/MathML"><mfrac><mi>x</mi><mn>2</mn></mfrac></math></body></html>"#, 0, "x.xhtml", "").unwrap();
    assert!(chapter.blocks.iter().any(|block| {
        block
            .mathml
            .as_ref()
            .is_some_and(|math| math.contains("<mfrac>"))
    }));
}

#[test]
fn comic_pages_are_naturally_ordered_and_follow_comicinfo_direction() {
    let comic_info = "<ComicInfo><Title>図の本</Title><Writer>画家</Writer><Manga>YesAndRightToLeft</Manga></ComicInfo>";
    let file = archive_file(
        ".cbz",
        &[
            ("page10.png", "ten"),
            ("page2.png", "two"),
            ("page1.png", "one"),
            ("ComicInfo.xml", comic_info),
        ],
    );
    let book = open(file.path()).unwrap();
    assert_eq!(book.title, "図の本");
    assert_eq!(book.chapters.len(), 3);
    assert_eq!(book.chapters[0].href, "page1.png");
    assert_eq!(book.chapters[1].href, "page2.png");
    assert_eq!(book.page_progression, reader_core::PageProgression::Rtl);
    assert!(book.chapters[0].atoms().is_empty());
}

#[test]
fn fictionbook_keeps_chapters_binary_images_and_author() {
    let xml = r##"<?xml version="1.0" encoding="UTF-8"?><FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0" xmlns:l="http://www.w3.org/1999/xlink">
    <description><title-info><author><first-name>花子</first-name><last-name>山田</last-name></author><book-title>物語</book-title><lang>ja</lang><coverpage><image l:href="#pic"/></coverpage></title-info></description>
    <body><section id="first"><title><p>第一章</p></title><p>海の<emphasis>向こう</emphasis>へ。</p><image l:href="#pic"/></section><section><title><p>第二章</p></title><p>つづき</p></section></body><binary id="pic" content-type="image/png">aW1hZ2U=</binary></FictionBook>"##;
    let mut file = tempfile::Builder::new().suffix(".fb2").tempfile().unwrap();
    file.write_all(xml.as_bytes()).unwrap();
    let before = std::fs::read(file.path()).unwrap();
    let book = open(file.path()).unwrap();
    assert_eq!(book.title, "物語");
    assert_eq!(book.authors, ["花子 山田"]);
    assert_eq!(book.chapters.len(), 2);
    assert!(book.chapters[0].plain_text().contains("海の向こうへ。"));
    assert!(
        book.cover
            .as_ref()
            .is_some_and(|resource| book.resource(resource) == Some(b"image".as_slice()))
    );
    assert_eq!(std::fs::read(file.path()).unwrap(), before);
}

#[test]
fn internal_links_and_footnotes_resolve_without_opening_external_protocols() {
    let mut book = reader_core::Book::demo();
    book.chapters = vec![
        parse_html("<html><body><p id='本文'>本文<a href='../Notes/notes%201.xhtml#note%201'>注</a></p></body></html>", 0, "OPS/Text/ch1.xhtml", "").unwrap(),
        parse_html("<html><body><p id='note 1'>注釈の<em>説明</em>です。</p><table><tr><td><p id='cell'>表の注釈</p></td></tr></table></body></html>", 1, "OPS/Notes/notes 1.xhtml", "").unwrap(),
    ];
    let target = book
        .resolve_link(0, "../Notes/notes%201.xhtml#note%201")
        .unwrap()
        .unwrap();
    assert_eq!(target, book.chapters[1].anchors["note 1"]);
    assert_eq!(
        book.text_at_anchor(target).as_deref(),
        Some("注釈の説明です。")
    );
    let cell = book.resolve_link(1, "#cell").unwrap().unwrap();
    assert_eq!(book.text_at_anchor(cell).as_deref(), Some("表の注釈"));
    assert_eq!(
        book.resolve_link(0, "#%E6%9C%AC%E6%96%87").unwrap(),
        Some(book.chapters[0].anchors["本文"])
    );
    assert_eq!(
        book.resolve_link(0, "?read=1#本文").unwrap(),
        Some(book.chapters[0].anchors["本文"])
    );
    assert_eq!(
        book.resolve_link(0, "../Notes/notes%201.xhtml").unwrap(),
        book.chapters[1].start_locator()
    );
    for external in [
        "https://example.com/a",
        "//example.com/a",
        "javascript:alert(1)",
        "file:///C:/secret",
        "%6Aavascript%3Aalert(1)",
        "\\\\server\\share",
    ] {
        assert_eq!(
            book.resolve_link(0, external).unwrap(),
            None,
            "external link: {external}"
        );
    }
    for traversal in ["../../../escape.xhtml", "%2E%2E/%2E%2E/%2E%2E/escape.xhtml"] {
        assert!(book.resolve_link(0, traversal).is_err());
    }
    assert!(book.resolve_link(0, "#%FF").is_err());
    assert_eq!(book.resolve_link(0, "#missing").unwrap(), None);
}

#[test]
fn supported_inline_css_can_reset_inherited_emphasis_combine_and_script() {
    let chapter = parse_html("<html><body><p style='text-emphasis:dot;text-combine-upright:all;font-weight:bold'><span style='text-emphasis:none;text-combine-upright:none;font-weight:inherit;vertical-align:super'>上</span><sup style='vertical-align:baseline'>基</sup><span style='vertical-align:sub'>下</span></p></body></html>", 0, "reset.xhtml", "").unwrap();
    let atoms = chapter.atoms();
    assert!(!atoms[0].style.emphasis);
    assert!(!atoms[0].style.tate_chu_yoko);
    assert!(atoms[0].style.bold);
    assert!(atoms[0].style.superscript);
    assert!(!atoms[1].style.superscript);
    assert!(atoms[2].style.subscript);
}

#[test]
fn inline_gaiji_images_preserve_text_order_css_dimensions_alt_and_atomic_ruby() {
    let chapter = parse_html("<html><body><p style='width:99%'>甲<img class='gaiji' src='gaiji.png' alt='髙'/>乙<ruby><img class='gaiji' src='g2.png' alt='𠮷'/><rt>よし</rt></ruby>丙</p><img class='figure' src='photo.jpg' width='999' height='888'/></body></html>", 0, "OPS/book.xhtml", ".gaiji{width:1em;height:1em;vertical-align:-.12em}.figure{display:block;width:50%;height:auto;max-height:10rem;aspect-ratio:2/1}").unwrap();
    let atoms = chapter.atoms();
    assert_eq!(
        atoms
            .iter()
            .map(|atom| atom.text.as_str())
            .collect::<String>(),
        "甲髙乙𠮷丙"
    );
    assert_eq!(
        atoms
            .iter()
            .map(|atom| atom.spoken_text())
            .collect::<String>(),
        "甲髙乙よし丙"
    );
    let gaiji = atoms.iter().find(|atom| atom.text == "髙").unwrap();
    let image = gaiji.image.as_ref().unwrap();
    assert_eq!(image.display, reader_core::ImageDisplay::Inline);
    assert_eq!(image.width, Some(reader_core::CssLength::Em(1.0)));
    assert_eq!(image.height, Some(reader_core::CssLength::Em(1.0)));
    assert_eq!(
        image.baseline_offset,
        Some(reader_core::CssLength::Em(-0.12))
    );
    assert_eq!(
        image.source.unwrap().start_locator(),
        gaiji.locator_at(0).unwrap()
    );
    let ruby = atoms.iter().find(|atom| atom.ruby.is_some()).unwrap();
    assert!(ruby.image.is_some());
    let figure = chapter
        .blocks
        .iter()
        .find_map(|block| block.image.as_ref())
        .unwrap();
    assert_eq!(figure.display, reader_core::ImageDisplay::Block);
    assert_eq!(figure.width, Some(reader_core::CssLength::Percent(50.0)));
    assert_eq!(
        figure.height, None,
        "CSS height:auto overrides HTML presentation height"
    );
    assert_eq!(figure.max_height, Some(reader_core::CssLength::Rem(10.0)));
    assert_eq!(figure.aspect_ratio, Some(2.0));
}
