use reader_core::{
    CssLength, ImageDisplay, PagePlacement, PagePresentation, PageProgression, RenditionLayout,
    RenditionSpread, open, parse_html,
};
use std::io::{Seek, Write};

fn fixture(
    metadata: &str,
    prefix: &str,
    direction: &str,
    documents: &[(&str, &str, &str, &str)],
) -> tempfile::NamedTempFile {
    let mut file = tempfile::Builder::new().suffix(".epub").tempfile().unwrap();
    let mut zip = zip::ZipWriter::new(file.as_file_mut());
    let options = zip::write::SimpleFileOptions::default();
    let mut put = |name: &str, bytes: &[u8]| {
        zip.start_file(name, options).unwrap();
        zip.write_all(bytes).unwrap();
    };
    put(
        "META-INF/container.xml",
        b"<container><rootfiles><rootfile full-path='OPS/package.opf'/></rootfiles></container>",
    );
    let manifest = documents
        .iter()
        .enumerate()
        .map(|(index, (href, media, _, _))| {
            format!("<item id='c{index}' href='{href}' media-type='{media}'/>")
        })
        .collect::<String>();
    let spine = documents
        .iter()
        .enumerate()
        .map(|(index, (_, _, properties, _))| {
            format!("<itemref idref='c{index}' properties='{properties}'/>")
        })
        .collect::<String>();
    let package = format!(
        "<package prefix='{prefix}'><metadata><title>Presentation fixture</title>{metadata}</metadata><manifest>{manifest}</manifest><spine page-progression-direction='{direction}'>{spine}</spine></package>"
    );
    put("OPS/package.opf", package.as_bytes());
    for (href, _, _, content) in documents {
        put(&format!("OPS/{href}"), content.as_bytes());
    }
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend_from_slice(&800u32.to_be_bytes());
    png.extend_from_slice(&1200u32.to_be_bytes());
    put("OPS/plate.png", &png);
    zip.finish().unwrap();
    file.as_file_mut().rewind().unwrap();
    file
}

#[test]
fn package_and_spine_overrides_survive_lazy_load_unload_and_svg_spine() {
    let file = fixture(
        "<meta property='rendition:layout'>pre-paginated</meta><meta property='rendition:spread'>landscape</meta>",
        "r: http://www.idpf.org/vocab/rendition/#",
        "rtl",
        &[
            (
                "cover.xhtml",
                "application/xhtml+xml",
                "rendition:page-spread-center",
                "<html><head><meta name='viewport' content='width=800, height=1200'/></head><body><img src='plate.png' style='width:100%;height:100%'/></body></html>",
            ),
            (
                "left.svg",
                "image/svg+xml",
                "page-spread-left",
                "<svg xmlns='http://www.w3.org/2000/svg' xmlns:xlink='http://www.w3.org/1999/xlink' viewBox='-20 -10 600 900' width='60' height='90'><title>左ページ</title><image id='plate' xlink:href='plate.png' x='-20' y='-10' width='600' height='900'/></svg>",
            ),
            (
                "right.xhtml",
                "application/xhtml+xml",
                "r:spread-both r:page-spread-right",
                "<html><head><meta name='viewport' content='initial-scale=1, width=1000, height=600'/></head><body><svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 400 300'><image href='plate.png' width='400' height='300'/></svg></body></html>",
            ),
            (
                "intro.xhtml",
                "application/xhtml+xml",
                "r:layout-reflowable rendition:spread-auto",
                "<html><head><meta name='viewport' content='width=device-width,height=device-height'/><meta name='viewport' content='width=999,height=999'/></head><body><p><ruby>日本語<rt>にほんご</rt></ruby>本文。</p></body></html>",
            ),
            (
                "single.xhtml",
                "application/xhtml+xml",
                "spread-none",
                "<html><head><meta name='viewport' content='height=700; width=500'/></head><body><img src='plate.png'/></body></html>",
            ),
            (
                "portrait.xhtml",
                "application/xhtml+xml",
                "rendition:spread-portrait rendition:page-spread-left",
                "<html><head><meta name='VIEWPORT' content='width=800,height=1200'/></head><body><img src='plate.png'/></body></html>",
            ),
        ],
    );
    let mut book = open(file.path()).unwrap();
    assert_eq!(book.page_progression, PageProgression::Rtl);
    assert_eq!(book.chapters.len(), 6);
    let cover = book.chapters[0].presentation;
    assert_eq!(
        cover,
        PagePresentation {
            layout: Some(RenditionLayout::PrePaginated),
            spread: Some(RenditionSpread::None),
            placement: PagePlacement::Center,
            viewport: Some([800.0, 1200.0])
        }
    );
    assert!(book.chapters[1..].iter().all(|chapter| !chapter.loaded));
    assert_eq!(
        book.chapters[1].presentation,
        PagePresentation {
            layout: Some(RenditionLayout::PrePaginated),
            spread: Some(RenditionSpread::Landscape),
            placement: PagePlacement::Left,
            viewport: None
        }
    );
    assert_eq!(
        book.chapters[2].presentation.spread,
        Some(RenditionSpread::Both)
    );
    assert_eq!(
        book.chapters[2].presentation.placement,
        PagePlacement::Right
    );
    assert_eq!(
        book.chapters[3].presentation.layout,
        Some(RenditionLayout::Reflowable)
    );
    assert_eq!(
        book.chapters[3].presentation.spread,
        Some(RenditionSpread::Auto)
    );
    assert_eq!(
        book.chapters[4].presentation.placement,
        PagePlacement::Center
    );
    assert_eq!(
        book.chapters[4].presentation.spread,
        Some(RenditionSpread::None)
    );
    assert_eq!(
        book.chapters[5].presentation.spread,
        Some(RenditionSpread::Portrait)
    );
    book.ensure_chapter_loaded(1).unwrap();
    let svg = book.chapters[1].clone();
    assert_eq!(svg.presentation.viewport, Some([600.0, 900.0]));
    assert_eq!(svg.blocks.len(), 1);
    let image = svg.blocks[0].image.as_ref().unwrap();
    assert_eq!(image.display, ImageDisplay::FullPage);
    assert_eq!(image.intrinsic_size, Some([600.0, 900.0]));
    let outer = image.inline_svg.as_deref().unwrap();
    assert!(outer.contains("viewBox='-20 -10 600 900'"));
    assert!(outer.contains("xlink:href='plate.png'"));
    assert_eq!(svg.anchors["plate"], svg.start_locator().unwrap());
    book.ensure_chapter_loaded(2).unwrap();
    assert_eq!(
        book.chapters[2].presentation.viewport,
        Some([1000.0, 600.0])
    );
    book.ensure_chapter_loaded(3).unwrap();
    assert_eq!(
        book.chapters[3].presentation.viewport, None,
        "only the first viewport meta tag applies"
    );
    assert_eq!(book.chapters[3].atoms()[0].spoken_text(), "にほんご");
    book.unload_chapters_except(&[3]);
    assert!(!book.chapters[1].loaded);
    assert_eq!(book.chapters[1].presentation, svg.presentation);
    assert_eq!(book.load_chapter(1).unwrap(), svg);
    assert_eq!(book.chapters[0].presentation, cover);
    let cover_image = book.load_chapter(0).unwrap().blocks[0]
        .image
        .clone()
        .unwrap();
    assert_eq!(cover_image.width, Some(CssLength::Percent(100.0)));
}

#[test]
fn invalid_or_refined_metadata_does_not_replace_global_defaults_and_old_json_still_loads() {
    let file = fixture(
        "<meta property='rendition:layout' refines='#c0'>pre-paginated</meta><meta property='rendition:layout'>unknown</meta><meta property='rendition:spread'>unknown</meta>",
        "",
        "ltr",
        &[(
            "body.xhtml",
            "application/xhtml+xml",
            "",
            "<html><body><p>本文</p></body></html>",
        )],
    );
    let book = open(file.path()).unwrap();
    assert_eq!(book.page_progression, PageProgression::Ltr);
    assert_eq!(book.chapters[0].presentation, PagePresentation::default());
    let mut json = serde_json::to_value(&book.chapters[0]).unwrap();
    json.as_object_mut().unwrap().remove("presentation");
    let restored: reader_core::Chapter = serde_json::from_value(json).unwrap();
    assert_eq!(restored.presentation, PagePresentation::default());
    for content in [
        "width=0,height=1200",
        "width=-1,height=1200",
        "width=NaN,height=1200",
        "width=800,height=inf",
        "width=device-width,height=900",
    ] {
        let chapter=parse_html(&format!("<html><head><meta name='viewport' content='{content}'/></head><body><p>本文</p></body></html>"),0,"invalid.xhtml","").unwrap();
        assert_eq!(chapter.presentation.viewport, None, "{content}");
    }
    for box_value in [
        "0 0 -1 200",
        "0 0 100 NaN",
        "invalid 0 100 200",
        "0 0 100 0",
    ] {
        let chapter=parse_html(&format!("<svg xmlns='http://www.w3.org/2000/svg' viewBox='{box_value}'><path d='M0 0'/></svg>"),0,"invalid.svg","").unwrap();
        assert_eq!(chapter.presentation.viewport, None);
    }
}

#[test]
fn author_text_indent_inherits_cascades_and_preserves_source_spaces() {
    let chapter=parse_html("<html><body style='text-indent:1em'><p>　元の空白</p><p class='wide'>二</p><p id='none' style='text-indent:0'>三</p><p style='text-indent:initial'>四</p><p style='text-indent:-2em'>五</p></body></html>",0,"indent.xhtml","p{text-indent:2em}.wide{text-indent:3em!important}#none{text-indent:5em}").unwrap();
    assert_eq!(
        chapter
            .blocks
            .iter()
            .map(|block| block.text_indent)
            .collect::<Vec<_>>(),
        [
            Some(CssLength::Em(2.0)),
            Some(CssLength::Em(3.0)),
            Some(CssLength::Px(0.0)),
            Some(CssLength::Px(0.0)),
            Some(CssLength::Em(-2.0))
        ]
    );
    assert!(chapter.blocks[0].atoms[0].text.starts_with('　'));
    let start = chapter.blocks[0].first_locator().unwrap();
    assert_eq!(start.offset, 0);
    let inherited=parse_html("<html><body style='text-indent:1.5em'><p>継承</p><p style='text-indent:inherit'>継承二</p></body></html>",0,"inherit.xhtml","").unwrap();
    assert!(
        inherited
            .blocks
            .iter()
            .all(|block| block.text_indent == Some(CssLength::Em(1.5)))
    );
}
