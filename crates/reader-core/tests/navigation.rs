use reader_core::{Book, TocEntry, TocTarget, open};
use std::io::Write;
use zip::write::SimpleFileOptions;

fn publication(navigation: &str, ncx: bool) -> tempfile::NamedTempFile {
    let mut file = tempfile::Builder::new().suffix(".epub").tempfile().unwrap();
    let mut zip = zip::ZipWriter::new(file.as_file_mut());
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let nav_item = if ncx {
        r#"<item id="nav" href="Navigation/toc.ncx" media-type="application/x-dtbncx+xml"/>"#
    } else {
        r#"<item id="nav" href="Navigation/nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>"#
    };
    let package = format!(
        r#"<package><metadata><title>Deep navigation fixture</title></metadata><manifest>
    <item id="first" href="Text/main.xhtml" media-type="application/xhtml+xml"/>
    <item id="second" href="Text/second.xhtml" media-type="application/xhtml+xml"/>
    <item id="note" href="Notes/補足%201.xhtml" media-type="application/xhtml+xml"/>{nav_item}
    </manifest><spine><itemref idref="first"/><itemref idref="note" linear="no"/><itemref idref="second"/></spine></package>"#
    );
    for (name, content) in [
        (
            "META-INF/container.xml",
            r#"<container><rootfiles><rootfile full-path="OPS/package.opf"/></rootfiles></container>"#,
        ),
        ("OPS/package.opf", package.as_str()),
        (
            if ncx {
                "OPS/Navigation/toc.ncx"
            } else {
                "OPS/Navigation/nav.xhtml"
            },
            navigation,
        ),
        (
            "OPS/Text/main.xhtml",
            r#"<html><body><p id="parent">最初の本文です。</p></body></html>"#,
        ),
        (
            "OPS/Text/second.xhtml",
            r#"<html><body><p id="深い/#?">深い節です。</p><p id="leaf">最深の本文です。</p></body></html>"#,
        ),
        (
            "OPS/Notes/補足 1.xhtml",
            r#"<html><body><p id="注/一">補足の内容です。</p></body></html>"#,
        ),
    ] {
        zip.start_file(name, options).unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    file
}

fn validate_chain(book: &Book, expected: &[&str]) -> Vec<TocEntry> {
    let mut output = Vec::new();
    let mut entries = &book.toc;
    for title in expected {
        assert_eq!(entries.len(), 1, "children of {title}");
        let entry = &entries[0];
        assert_eq!(entry.title, *title);
        output.push(entry.clone());
        entries = &entry.children;
    }
    assert!(entries.is_empty());
    output
}

#[test]
fn epub3_section_groups_and_auxiliary_parents_keep_five_nested_levels() {
    let nav = r#"<html xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><ol>
    <li><span>第一部</span><ol><li><a href="../Notes/補足%201.xhtml#注%2F一">補足入口</a><ol>
    <li><div><a href="../Text/../Text/main.xhtml#parent"><em>第一章</em></a></div><ol>
    <li><a href="../Text/second.xhtml?src=toc#深い%2F%23%3F">深い節</a><ol>
    <li><a href="../Text/second.xhtml#leaf">最深節</a></li></ol></li></ol></li></ol></li></ol></li>
    </ol></nav></body></html>"#;
    let file = publication(nav, false);
    let original = std::fs::read(file.path()).unwrap();
    let book = open(file.path()).unwrap();
    let chain = validate_chain(&book, &["第一部", "補足入口", "第一章", "深い節", "最深節"]);
    assert_eq!(chain[0].target, Some(TocTarget::Group));
    let Some(TocTarget::Document { href }) = &chain[1].target else {
        panic!("auxiliary TOC entry was discarded or clamped to the spine");
    };
    let target = book.load_link_target(0, href).unwrap().unwrap();
    assert_eq!(target.spine_index, None);
    assert_eq!(target.locator.chapter, chain[1].chapter);
    assert!(
        target
            .chapter
            .text_at_anchor(target.locator)
            .unwrap()
            .contains("補足の内容")
    );
    assert_eq!(chain[2].chapter, 0);
    assert_eq!(chain[2].fragment.as_deref(), Some("parent"));
    assert_eq!(chain[3].chapter, 1);
    assert_eq!(chain[3].fragment.as_deref(), Some("深い/#?"));
    assert!(chain[3].target.is_none());
    assert_eq!(book.chapters.len(), 2);
    assert_eq!(book.load_chapter(1).unwrap().anchors["leaf"].chapter, 1);
    assert_eq!(std::fs::read(file.path()).unwrap(), original);
}

#[test]
fn an_unresolvable_parent_link_never_discards_its_valid_descendants() {
    let nav = r#"<html><body><nav><ol><li><a href="../missing.xhtml">親の見出し</a><ol>
    <li><span>中間の見出し</span><ol><li><a href="../Text/main.xhtml#parent">第一章</a><ol>
    <li><a href="../Text/second.xhtml#leaf">末尾の節</a></li></ol></li></ol></li></ol></li></ol></nav></body></html>"#;
    let file = publication(nav, false);
    let book = open(file.path()).unwrap();
    let chain = validate_chain(&book, &["親の見出し", "中間の見出し", "第一章", "末尾の節"]);
    assert_eq!(chain[0].target, Some(TocTarget::Group));
    assert_eq!(chain[1].target, Some(TocTarget::Group));
    assert_eq!(chain[3].chapter, 1);
    assert_eq!(chain[3].fragment.as_deref(), Some("leaf"));
    assert_eq!(
        book.chapters[0].title, "第一章",
        "a group must not rename its first child's chapter"
    );
}

#[test]
fn epub2_ncx_missing_content_and_non_spine_parents_preserve_deep_navigation() {
    let ncx = r#"<ncx><navMap><navPoint><navLabel><text>第一部</text></navLabel>
    <navPoint><navLabel><text>補足入口</text></navLabel><content src="../Notes/補足%201.xhtml#注%2F一"/>
    <navPoint><navLabel><text>第一章</text></navLabel><content src="../Text/main.xhtml#parent"/>
    <navPoint><navLabel><text>壊れた親</text></navLabel><content src="../missing.xhtml"/>
    <navPoint><navLabel><text>最深節</text></navLabel><content src="../Text/second.xhtml#leaf"/>
    </navPoint></navPoint></navPoint></navPoint></navPoint></navMap></ncx>"#;
    let file = publication(ncx, true);
    let book = open(file.path()).unwrap();
    let chain = validate_chain(
        &book,
        &["第一部", "補足入口", "第一章", "壊れた親", "最深節"],
    );
    assert_eq!(chain[0].target, Some(TocTarget::Group));
    assert_eq!(chain[3].target, Some(TocTarget::Group));
    let Some(TocTarget::Document { href }) = &chain[1].target else {
        panic!("NCX supplementary destination is missing");
    };
    assert!(
        book.load_link_target(0, href)
            .unwrap()
            .unwrap()
            .spine_index
            .is_none()
    );
    assert_eq!(chain[4].chapter, 1);
    assert_eq!(chain[4].fragment.as_deref(), Some("leaf"));
}

#[test]
fn old_toc_json_keeps_legacy_destinations_and_new_targets_roundtrip() {
    let legacy: TocEntry =
        serde_json::from_str(r#"{"title":"Chapter","chapter":1,"fragment":"part","children":[]}"#)
            .unwrap();
    assert!(legacy.target.is_none());
    let group = TocEntry {
        title: "Group".into(),
        children: vec![legacy],
        target: Some(TocTarget::Group),
        ..TocEntry::default()
    };
    let disk = serde_json::to_string(&group).unwrap();
    assert_eq!(serde_json::from_str::<TocEntry>(&disk).unwrap(), group);
}
