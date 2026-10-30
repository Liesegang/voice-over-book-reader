use reader_core::{Book, open, parse_html};
use std::io::Write;
use zip::write::SimpleFileOptions;

fn linked_fixture() -> tempfile::NamedTempFile {
    let mut file = tempfile::Builder::new().suffix(".epub").tempfile().unwrap();
    let mut zip = zip::ZipWriter::new(file.as_file_mut());
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut put = |name: &str, contents: &str| {
        zip.start_file(name, options).unwrap();
        zip.write_all(contents.as_bytes()).unwrap();
    };
    put(
        "META-INF/container.xml",
        r#"<container><rootfiles><rootfile full-path="OPS/package.opf"/></rootfiles></container>"#,
    );
    put(
        "OPS/package.opf",
        r#"<package><metadata><title>Original link fixture</title></metadata><manifest>
        <item id="main" href="Text/main.xhtml" media-type="application/xhtml+xml"/>
        <item id="second" href="Text/second.xhtml" media-type="application/xhtml+xml"/>
        <item id="note" href="Notes/注釈%201.xhtml" media-type="application/xhtml+xml"/>
        <item id="outside" href="Notes/outside.xhtml" media-type="application/xhtml+xml"/>
        </manifest><spine><itemref idref="main"/><itemref idref="note" linear="no"/><itemref idref="second"/></spine></package>"#,
    );
    put(
        "OPS/Text/main.xhtml",
        r#"<html xmlns:epub="http://www.idpf.org/2007/ops" id="root"><body><p id="本文">  本文<a epub:type="noteref" href="../Notes/注釈%201.xhtml?read=1#注一"><em>補足</em>を見る</a>と続きます。</p></body></html>"#,
    );
    put(
        "OPS/Text/second.xhtml",
        r#"<html><body><p id="後半">後半の本文です。</p></body></html>"#,
    );
    put(
        "OPS/Notes/注釈 1.xhtml",
        r#"<html xmlns:epub="http://www.idpf.org/2007/ops"><body><aside epub:type="footnote" id="注一"><p>
        注釈専用語の説明です。<a href="../Text/main.xhtml?back=1#本文">本文へ</a></p></aside><p id="注/#?">別の注釈です。</p></body></html>"#,
    );
    put(
        "OPS/Notes/outside.xhtml",
        r#"<html><body><p id="outside">非spine注記です。</p></body></html>"#,
    );
    put(
        "OPS/not-in-manifest.xhtml",
        r#"<html><body><p id="hidden">未登録の内容</p></body></html>"#,
    );
    zip.finish().unwrap();
    file
}

#[test]
fn lazy_linear_no_and_non_spine_notes_load_without_polluting_reading_order() {
    let file = linked_fixture();
    let original = std::fs::read(file.path()).unwrap();
    let book = open(file.path()).unwrap();
    assert_eq!(book.chapters.len(), 2);
    assert!(book.chapters[0].loaded && !book.chapters[1].loaded);
    assert_eq!(book.archive_stats().unwrap().chapters_parsed, 1);
    assert!(
        book.chapters[0]
            .atoms()
            .iter()
            .any(|atom| atom.text.contains("補足") && atom.link_is_note)
    );

    let second = book
        .load_link_target(0, "second.xhtml?mode=read#%E5%BE%8C%E5%8D%8A")
        .unwrap()
        .unwrap();
    assert_eq!(second.spine_index, Some(1));
    assert_eq!(
        second.chapter.text_at_anchor(second.locator).as_deref(),
        Some("後半の本文です。")
    );
    assert!(
        !book.chapters[1].loaded,
        "worker parsing must not mutate foreground state"
    );

    let note = book
        .load_link_target(
            0,
            "../Notes/%E6%B3%A8%E9%87%88%201.xhtml?read=1#%E6%B3%A8%E4%B8%80",
        )
        .unwrap()
        .unwrap();
    assert_eq!(note.spine_index, None);
    assert!(note.chapter.id >= book.chapters.len() as u32);
    assert_eq!(note.locator.chapter, note.chapter.id);
    assert!(
        note.chapter
            .text_at_anchor(note.locator)
            .unwrap()
            .contains("注釈専用語")
    );
    assert!(
        book.text_at_anchor(note.locator)
            .unwrap()
            .contains("注釈専用語")
    );
    assert_eq!(book.chapters.len(), 2);

    let back = book
        .load_link_target(note.chapter.id, "../Text/main.xhtml?back=1#本文")
        .unwrap()
        .unwrap();
    assert_eq!(back.spine_index, Some(0));
    assert_eq!(back.locator, book.chapters[0].anchors["本文"]);
    let unusual = book
        .load_link_target(note.chapter.id, "#%E6%B3%A8%2F%23%3F")
        .unwrap()
        .unwrap();
    assert_eq!(
        unusual.chapter.text_at_anchor(unusual.locator).as_deref(),
        Some("別の注釈です。")
    );

    let outside = book
        .load_link_target(0, "../Notes/outside.xhtml#outside")
        .unwrap()
        .unwrap();
    assert_eq!(outside.spine_index, None);
    assert_eq!(
        outside.chapter.text_at_anchor(outside.locator).as_deref(),
        Some("非spine注記です。")
    );
    assert!(
        book.load_link_target(0, "../not-in-manifest.xhtml#hidden")
            .unwrap()
            .is_none()
    );
    assert!(book.search_all("注釈専用語").unwrap().is_empty());
    assert_eq!(std::fs::read(file.path()).unwrap(), original);
}

#[test]
fn normalized_local_references_reject_remote_schemes_and_root_traversal_before_loading() {
    let file = linked_fixture();
    let book = open(file.path()).unwrap();
    let reads = book.archive_stats().unwrap().resource_reads;
    for href in [
        "https://example.invalid/note.xhtml",
        "//example.invalid/note.xhtml",
        "%2F%2Fexample.invalid/note.xhtml",
        "file:///secret",
        "%6Aavascript%3Aalert(1)",
        "\\\\server\\share",
    ] {
        assert!(book.load_link_target(0, href).unwrap().is_none(), "{href}");
    }
    for href in [
        "../../../escape.xhtml",
        "%2E%2E/%2E%2E/%2E%2E/escape.xhtml",
        "%FF.xhtml",
    ] {
        assert!(book.load_link_target(0, href).is_err(), "{href}");
    }
    assert_eq!(book.archive_stats().unwrap().resource_reads, reads);
    assert_eq!(
        book.resolve_link(0, "#top").unwrap(),
        book.chapters[0].start_locator()
    );
    assert_eq!(
        book.resolve_link(0, "?mode=read#").unwrap(),
        book.chapters[0].start_locator()
    );
}

#[test]
fn note_semantics_and_trimmed_named_anchors_preserve_actual_source_targets() {
    let mut book = Book::demo();
    book.chapters=vec![parse_html(r##"<html xmlns:epub="http://www.idpf.org/2007/ops" id="whole"><body><p><a name="old"></a>  文頭です。</p><p><a href="#old" epub:type="noteref"><ruby>注<rt>ちゅう</rt></ruby></a><a href="#old" role="doc-noteref"><em>補足</em></a><a href="#old">普通のリンク</a></p></body></html>"##,0,"main.xhtml","").unwrap()];
    let target = book.resolve_link(0, "#old").unwrap().unwrap();
    assert_eq!(book.text_at_anchor(target).as_deref(), Some("文頭です。"));
    assert!(
        target.offset > 0,
        "anchor must point past collapsed leading whitespace"
    );
    assert_eq!(book.resolve_link(0, "#whole").unwrap(), Some(target));
    let atoms = book.chapters[0].atoms();
    assert!(
        atoms
            .iter()
            .any(|atom| atom.text == "注" && atom.link_is_note)
    );
    assert!(
        atoms
            .iter()
            .any(|atom| atom.text == "補足" && atom.link_is_note)
    );
    assert!(
        atoms
            .iter()
            .any(|atom| atom.text == "普通のリンク" && !atom.link_is_note)
    );
}
