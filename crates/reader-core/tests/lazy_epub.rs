use reader_core::{WritingMode, open};
use std::{
    io::{Seek, Write},
    sync::Arc,
    time::Instant,
};
use zip::write::SimpleFileOptions;

fn fixture(chapters: usize, reverse_entries: bool) -> tempfile::NamedTempFile {
    let mut file = tempfile::Builder::new().suffix(".epub").tempfile().unwrap();
    let mut zip = zip::ZipWriter::new(file.as_file_mut());
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut put = |name: &str, content: &[u8]| {
        zip.start_file(name, options).unwrap();
        zip.write_all(content).unwrap();
    };
    put(
        "META-INF/container.xml",
        b"<container><rootfiles><rootfile full-path='OPS/package.opf'/></rootfiles></container>",
    );
    let manifest = (0..chapters)
        .map(|index| {
            format!(
                "<item id='c{index}' href='c{index}.xhtml' media-type='application/xhtml+xml'/>"
            )
        })
        .collect::<String>();
    let spine = (0..chapters)
        .map(|index| format!("<itemref idref='c{index}'/>"))
        .collect::<String>();
    let package = format!(
        "<package><metadata><title>Lazy fixture</title></metadata><manifest>{manifest}<item id='nav' href='nav.xhtml' media-type='application/xhtml+xml' properties='nav'/></manifest><spine>{spine}</spine></package>"
    );
    put("OPS/package.opf", package.as_bytes());
    let nav = format!(
        "<html><body><nav><ol>{}</ol></nav></body></html>",
        (0..chapters)
            .map(|index| format!("<li><a href='c{index}.xhtml#part'>第{index}章</a></li>"))
            .collect::<String>()
    );
    put("OPS/nav.xhtml", nav.as_bytes());
    put("OPS/style.css", b"body{writing-mode:vertical-rl}");
    let indices: Vec<_> = if reverse_entries {
        (0..chapters).rev().collect()
    } else {
        (0..chapters).collect()
    };
    for index in indices {
        let unique = if index + 1 == chapters {
            "遅延対象語"
        } else {
            "通常の本文"
        };
        let html = format!(
            "<html><head><title>第{index}章</title><link rel='stylesheet' href='style.css'/></head><body><p id='part'><ruby>東京<rt>とうきょう</rt></ruby>{unique}</p>{}</body></html>",
            "<p>読みやすい日本語の本文です。</p>".repeat(40)
        );
        put(&format!("OPS/c{index}.xhtml"), html.as_bytes());
    }
    put("OPS/unused-font.bin", &vec![0x52; 8 * 1024 * 1024]);
    zip.finish().unwrap();
    file.as_file_mut().rewind().unwrap();
    file
}

#[test]
fn first_open_expands_only_metadata_and_first_chapter_then_loads_on_demand() {
    let file = fixture(120, false);
    let started = Instant::now();
    let mut book = open(file.path()).unwrap();
    let elapsed = started.elapsed();
    let first = book.chapters[0].clone();
    assert_eq!(book.chapters.len(), 120);
    assert!(book.chapters[0].loaded);
    assert!(
        book.chapters[1..]
            .iter()
            .all(|chapter| !chapter.loaded && chapter.blocks.is_empty())
    );
    assert_eq!(first.writing_mode, WritingMode::VerticalRl);
    assert!(book.resources.is_empty());
    let stats = book.archive_stats().unwrap();
    eprintln!(
        "lazy EPUB first open: {elapsed:?}, {} reads, {} bytes expanded, {} chapters parsed",
        stats.resource_reads, stats.expanded_bytes, stats.chapters_parsed
    );
    assert_eq!(stats.chapters_parsed, 1);
    assert!(
        stats.expanded_bytes < 128 * 1024,
        "the unused 8 MiB asset and other chapters must remain compressed"
    );
    let last = book.load_chapter(119).unwrap();
    assert!(last.loaded && last.plain_text().contains("遅延対象語"));
    assert!(
        !book.chapters[119].loaded,
        "a worker's load does not mutate the foreground Book"
    );
    book.ensure_chapter_loaded(119).unwrap();
    book.unload_chapters_except(&[119]);
    assert!(!book.chapters[0].loaded);
    assert!(book.chapters[0].blocks.is_empty());
    assert_eq!(
        book.load_chapter(0).unwrap(),
        first,
        "source IDs survive unload/reload"
    );
    let hits = book.search_all("遅延対象語").unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].chapter, 119);
    let stats = book.archive_stats().unwrap();
    assert!(stats.cached_chapters <= 3);
    assert!(stats.cached_resource_bytes <= 64 * 1024 * 1024);
    let asset = book.resource_bytes("OPS/unused-font.bin").unwrap().unwrap();
    assert_eq!(asset.len(), 8 * 1024 * 1024);
    assert!(Arc::ptr_eq(
        &asset,
        &book.resource_bytes("OPS/unused-font.bin").unwrap().unwrap()
    ));
    assert!(book.ensure_chapter_loaded(999).is_err());
}

#[test]
fn logical_identity_does_not_require_payload_reads_or_depend_on_zip_entry_order() {
    let first = fixture(3, false);
    let reordered = fixture(3, true);
    let a = open(first.path()).unwrap();
    let b = open(reordered.path()).unwrap();
    assert_eq!(a.id, b.id);
    assert_eq!(a.archive_stats().unwrap().chapters_parsed, 1);
    let target = a.resolve_link(0, "c2.xhtml#part").unwrap().unwrap();
    assert_eq!(target.chapter, 2);
    assert!(a.text_at_anchor(target).unwrap().contains("東京"));
    assert!(!a.chapters[2].loaded);
}
