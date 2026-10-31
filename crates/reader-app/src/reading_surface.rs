#[cfg(test)]
use crate::layout::BookLayout;
use crate::layout::Page;
use eframe::egui::{Rect, Vec2, vec2};
use reader_core::{Book, PageProgression};
use reader_library::{ReadingMode, Settings, SpreadMode};

/// Metadata and the always-visible rail occupy the configured vertical margin,
/// rather than taking an additional band away from every text page.
pub const READER_METADATA_HEIGHT: f32 = 28.;
pub fn content_rect(client: Rect, chapter: &reader_core::Chapter, settings: &Settings) -> Rect {
    if !is_image_chapter(chapter) {
        return client;
    }
    let horizontal = settings
        .horizontal_margin()
        .min((client.width() * 0.18).max(0.));
    let vertical = settings
        .vertical_margin()
        .max(READER_METADATA_HEIGHT)
        .min((client.height() * 0.45).max(0.));
    client.shrink2(vec2(horizontal, vertical))
}
pub fn effective_layout_settings<'a>(
    settings: &'a Settings,
    chapter: &reader_core::Chapter,
) -> std::borrow::Cow<'a, Settings> {
    if is_image_chapter(chapter) {
        let mut result = settings.clone();
        result.margins = 0.;
        result.margin_horizontal = Some(0.);
        result.margin_vertical = Some(0.);
        std::borrow::Cow::Owned(result)
    } else if settings.vertical_margin() < READER_METADATA_HEIGHT {
        let mut result = settings.clone();
        result.margin_vertical = Some(READER_METADATA_HEIGHT);
        std::borrow::Cow::Owned(result)
    } else {
        std::borrow::Cow::Borrowed(settings)
    }
}

pub fn next_to_left(book: &Book, chapter: usize, settings: &Settings) -> bool {
    match settings.reading_mode {
        ReadingMode::Horizontal => false,
        ReadingMode::Vertical => true,
        ReadingMode::Auto => match book.page_progression {
            PageProgression::Rtl => true,
            PageProgression::Ltr => false,
            PageProgression::Default => book
                .chapters
                .get(chapter)
                .is_some_and(|c| c.writing_mode.is_vertical()),
        },
    }
}

pub fn page_count(extent: Vec2, mode: SpreadMode) -> usize {
    match mode {
        SpreadMode::Single => 1,
        SpreadMode::Double => 2,
        SpreadMode::Auto => {
            if extent.x >= 960. && extent.x / extent.y.max(1.) >= 1.25 {
                2
            } else {
                1
            }
        }
    }
}

pub fn is_image_chapter(chapter: &reader_core::Chapter) -> bool {
    // Image-only flow can contain several local pages. It still uses image
    // spread/spacing preferences, but is not an atomic cross-chapter page.
    reader_core::page_document_geometry(chapter).is_some()
        || (!chapter.blocks.is_empty()
            && chapter.blocks.iter().any(|block| block.image.is_some())
            && chapter.blocks.iter().all(|block| {
                block.image.as_ref().is_some_and(|image| {
                    image.display == reader_core::ImageDisplay::FullPage
                        || (matches!(image.width, Some(reader_core::CssLength::Percent(w)) if w >= 100.)
                            && matches!(image.height, Some(reader_core::CssLength::Percent(h)) if h >= 100.))
                }) || (block.image.is_none()
                    && block.table.is_none()
                    && block.mathml.is_none()
                    && !matches!(block.kind, reader_core::BlockKind::Rule)
                    && block.atoms.iter().all(|atom| atom.text.trim().is_empty()))
            }))
}

pub fn page_count_for_chapter(
    extent: Vec2,
    mode: SpreadMode,
    text_spread: bool,
    chapter: &reader_core::Chapter,
) -> usize {
    use reader_core::{PagePlacement as P, RenditionSpread as S};
    if chapter.presentation.placement == P::Center
        || chapter.presentation.spread == Some(S::None)
        || mode == SpreadMode::Single
    {
        return 1;
    }
    let authored = match chapter.presentation.spread {
        Some(S::Both) => Some(true),
        Some(S::Landscape) => Some(extent.x > extent.y),
        Some(S::Portrait) => Some(extent.y > extent.x),
        Some(S::None) => Some(false),
        _ => None,
    };
    if mode == SpreadMode::Auto
        && let Some(enabled) = authored
    {
        return if enabled { 2 } else { 1 };
    }
    if !text_spread && !is_image_chapter(chapter) && authored.is_none() {
        return 1;
    }
    // A wide, already-composed plate stays intact unless the publisher explicitly
    // requests two slots. It must not be treated as two independent source pages.
    if authored.is_none() && chapter.presentation.placement == P::Auto && is_image_chapter(chapter)
    {
        let wide = reader_core::page_document_geometry(chapter)
            .and_then(|geometry| geometry.viewport)
            .is_some_and(|[w, h]| w / h.max(1.) >= 1.25);
        if wide {
            return 1;
        }
    }
    page_count(extent, mode)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChapterSpread {
    pub first: usize,
    pub second: Option<usize>,
    pub columns: usize,
    pub first_slot: usize,
    pub gap: f32,
}
impl Default for ChapterSpread {
    fn default() -> Self {
        Self {
            first: 0,
            second: None,
            columns: 1,
            first_slot: 0,
            gap: 0.,
        }
    }
}
impl ChapterSpread {
    pub fn consumed(self) -> usize {
        1 + usize::from(self.second.is_some())
    }
    pub fn extent(self, viewport: Vec2) -> Vec2 {
        vec2((viewport.x - self.gap) / self.columns as f32, viewport.y)
    }
}

pub fn chapter_spread(
    book: &Book,
    index: usize,
    extent: Vec2,
    settings: &Settings,
) -> ChapterSpread {
    use reader_core::PagePlacement as P;
    let Some(chapter) = book.chapters.get(index) else {
        return ChapterSpread::default();
    };
    if index == 0
        && book.cover.is_some()
        && reader_core::page_document_geometry(chapter).is_some()
        && chapter.presentation.placement == P::Auto
    {
        return ChapterSpread {
            first: index,
            ..ChapterSpread::default()
        };
    }
    let columns = page_count_for_chapter(extent, settings.spread, settings.text_spread, chapter);
    let rtl = next_to_left(book, index, settings);
    let start_slot = usize::from(rtl);
    let first_slot = if columns == 1 {
        0
    } else {
        match chapter.presentation.placement {
            P::Left => 0,
            P::Right => 1,
            _ => start_slot,
        }
    };
    let atomic = reader_core::page_document_geometry(chapter).is_some();
    let gap = if columns == 2 {
        settings
            .spread_gap(is_image_chapter(chapter))
            .min(extent.x * 0.25)
    } else {
        0.
    };
    let second = if columns == 2 && atomic && first_slot == start_slot {
        book.chapters
            .get(index + 1)
            .filter(|next| {
                reader_core::page_document_geometry(next).is_some()
                    && page_count_for_chapter(extent, settings.spread, settings.text_spread, next)
                        == 2
                    && matches!(next.presentation.placement, P::Auto | P::Left | P::Right)
                    && match next.presentation.placement {
                        P::Left => start_slot != 0,
                        P::Right => start_slot != 1,
                        _ => true,
                    }
                    && (chapter.presentation.layout
                        == Some(reader_core::RenditionLayout::PrePaginated))
                        == (next.presentation.layout
                            == Some(reader_core::RenditionLayout::PrePaginated))
            })
            .map(|_| index + 1)
    } else {
        None
    };
    if atomic && second.is_none() && chapter.presentation.placement == P::Auto {
        return ChapterSpread {
            first: index,
            second: None,
            columns: 1,
            first_slot: 0,
            gap: 0.,
        };
    }
    ChapterSpread {
        first: index,
        second,
        columns,
        first_slot,
        gap,
    }
}

/// Derive the grouping from source declarations, never from navigation history.
pub fn chapter_spread_start(book: &Book, index: usize, extent: Vec2, settings: &Settings) -> usize {
    let mut cursor = 0;
    while cursor < index {
        if matches!(
            reader_core::page_geometry_snapshot(&book.chapters[cursor]),
            reader_core::PageGeometrySnapshot::Unknown
        ) {
            // Unparsed legacy chapters must not be assumed to be flow when
            // normalizing a retained source position on a cold cache miss.
            return index;
        }
        let plan = chapter_spread(book, cursor, extent, settings);
        if plan.second == Some(index) {
            return cursor;
        }
        cursor += plan.consumed();
    }
    index
}
pub fn previous_chapter_spread(
    book: &Book,
    index: usize,
    extent: Vec2,
    settings: &Settings,
) -> usize {
    let mut cursor = 0;
    let mut previous = 0;
    while cursor < index {
        previous = cursor;
        cursor += chapter_spread(book, cursor, extent, settings).consumed();
    }
    previous
}

#[derive(Clone, Copy, Debug)]
pub struct PageWindow {
    pub start: usize,
    pub end: usize,
    pub first_slot: usize,
}
pub fn page_window(
    chapter: &reader_core::Chapter,
    index: usize,
    total: usize,
    columns: usize,
    rtl: bool,
) -> PageWindow {
    use reader_core::PagePlacement as P;
    if columns == 1 {
        return PageWindow {
            start: index.min(total.saturating_sub(1)),
            end: (index + 1).min(total),
            first_slot: 0,
        };
    }
    let start_slot = usize::from(rtl);
    let declared = match chapter.presentation.placement {
        P::Left => 0,
        P::Right => 1,
        _ => start_slot,
    };
    let leading = usize::from(declared != start_slot);
    let start = if index < leading {
        0
    } else {
        leading + (index - leading) / 2 * 2
    };
    let first_slot = if start == 0 { declared } else { start_slot };
    let count = if start == 0 && leading == 1 { 1 } else { 2 };
    PageWindow {
        start,
        end: (start + count).min(total),
        first_slot,
    }
}

pub fn compose(
    first: Page,
    second: Option<Page>,
    extent: Vec2,
    columns: usize,
    first_slot: usize,
    gap: f32,
) -> Page {
    let width = (extent.x - gap) / columns as f32;
    let mut result = Page::default();
    for (page, slot) in
        std::iter::once((first, first_slot)).chain(second.map(|p| (p, 1 - first_slot)))
    {
        if result.anchor.is_none() {
            result.anchor = page.first_locator();
        }
        let image_offset =
            if columns == 2 && gap == 0. && page.glyphs.is_empty() && page.pictures.len() == 1 {
                let picture = &page.pictures[0];
                if picture.fixed_canvas.is_some() || picture.pdf_page.is_some() {
                    if slot == 0 {
                        width - picture.rect.right()
                    } else {
                        -picture.rect.left()
                    }
                } else {
                    0.
                }
            } else {
                0.
            };
        let offset = vec2(slot as f32 * (width + gap) + image_offset, 0.);
        result.glyphs.extend(page.glyphs.into_iter().map(|mut g| {
            g.origin += offset;
            g
        }));
        result.hits.extend(page.hits.into_iter().map(|mut h| {
            h.rect = h.rect.translate(offset);
            h
        }));
        result
            .pictures
            .extend(page.pictures.into_iter().map(|mut p| {
                p.rect = p.rect.translate(offset);
                p
            }));
        result
            .rules
            .extend(page.rules.into_iter().map(|r| r.translate(offset)));
        result
            .header_fills
            .extend(page.header_fills.into_iter().map(|r| r.translate(offset)));
    }
    result
}

#[cfg(test)]
pub fn pair(first: Page, second: Page, extent: Vec2, rtl: bool) -> Page {
    spread(
        &BookLayout {
            pages: vec![first, second],
        },
        0,
        2,
        extent,
        rtl,
    )
}

#[cfg(test)]
pub fn page_extent(extent: Vec2, count: usize) -> Vec2 {
    vec2(
        (extent.x - if count == 2 { 24. } else { 0. }) / count as f32,
        extent.y,
    )
}

/// Compose two source-backed pages without changing their logical source order.
#[cfg(test)]
pub fn spread(layout: &BookLayout, first: usize, count: usize, extent: Vec2, rtl: bool) -> Page {
    let mut result = Page::default();
    let width = page_extent(extent, count).x;
    for logical in 0..count {
        let Some(mut page) = layout.pages.get(first + logical).cloned() else {
            continue;
        };
        if result.anchor.is_none() {
            result.anchor = page.first_locator();
        }
        let slot = if rtl && count == 2 {
            1 - logical
        } else {
            logical
        };
        let offset = vec2(slot as f32 * (width + 24.), 0.);
        for glyph in &mut page.glyphs {
            glyph.origin += offset;
        }
        for hit in &mut page.hits {
            hit.rect = hit.rect.translate(offset);
        }
        for picture in &mut page.pictures {
            picture.rect = picture.rect.translate(offset);
        }
        result.glyphs.extend(page.glyphs);
        result.hits.extend(page.hits);
        result.pictures.extend(page.pictures);
        result
            .rules
            .extend(page.rules.into_iter().map(|r| r.translate(offset)));
        result
            .header_fills
            .extend(page.header_fills.into_iter().map(|r| r.translate(offset)));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn landscape_spreads_and_book_direction_are_independent_of_ui() {
        assert_eq!(page_count(vec2(1280., 760.), SpreadMode::Auto), 2);
        assert_eq!(page_count(vec2(760., 1280.), SpreadMode::Auto), 1);
        let book = Book::demo();
        let mut settings = Settings::default();
        assert!(next_to_left(&book, 0, &settings));
        settings.reading_mode = ReadingMode::Horizontal;
        assert!(!next_to_left(&book, 0, &settings));
        settings.reading_mode = ReadingMode::Vertical;
        assert!(next_to_left(&book, 1, &settings));
    }

    #[test]
    fn image_spreads_are_automatic_but_text_spreads_require_an_explicit_choice() {
        let text = reader_core::parse_html(
            "<html><body><p>文章を続けて読みます。</p></body></html>",
            0,
            "text.xhtml",
            "",
        )
        .unwrap();
        let image = reader_core::parse_html(
            "<html><body><img src='plate.png' width='600' height='900' alt='挿絵'/></body></html>",
            1,
            "image.xhtml",
            "",
        )
        .unwrap();
        let mixed = reader_core::parse_html("<html><body><p>外字<img src='gaiji.png' style='width:1em;height:1em'/>も本文の一部です。</p></body></html>", 2, "mixed.xhtml", "").unwrap();
        let wide = vec2(1280., 760.);
        let portrait = vec2(760., 1280.);
        assert!(!is_image_chapter(&text));
        assert!(
            !is_image_chapter(&mixed),
            "an inline gaiji must not make prose an image chapter"
        );
        assert!(is_image_chapter(&image));
        assert_eq!(
            page_count_for_chapter(wide, SpreadMode::Auto, false, &image),
            2
        );
        assert_eq!(
            page_count_for_chapter(portrait, SpreadMode::Auto, false, &image),
            1
        );
        assert_eq!(
            page_count_for_chapter(wide, SpreadMode::Single, false, &image),
            1
        );
        assert_eq!(
            page_count_for_chapter(portrait, SpreadMode::Double, false, &image),
            2
        );
        for chapter in [&text, &mixed] {
            assert_eq!(
                page_count_for_chapter(wide, SpreadMode::Auto, false, chapter),
                1
            );
            assert_eq!(
                page_count_for_chapter(wide, SpreadMode::Double, false, chapter),
                1
            );
            assert_eq!(
                page_count_for_chapter(wide, SpreadMode::Auto, true, chapter),
                2
            );
            assert_eq!(
                page_count_for_chapter(portrait, SpreadMode::Auto, true, chapter),
                1
            );
            assert_eq!(
                page_count_for_chapter(wide, SpreadMode::Single, true, chapter),
                1
            );
        }
    }

    #[test]
    fn paired_images_follow_physical_direction_and_preserve_source_highlights() {
        use crate::layout::{Hit, Picture};
        use eframe::egui::{Rect, pos2};
        use reader_core::Locator;
        let page = |chapter| {
            let start = Locator {
                chapter,
                node: 3,
                offset: 0,
            };
            let rect = Rect::from_min_size(pos2(12., 18.), vec2(200., 300.));
            Page {
                anchor: Some(start),
                hits: vec![Hit {
                    rect,
                    locator: start,
                    end: Locator { offset: 3, ..start },
                    unit_id: u64::from(chapter) << 32,
                    offset: 0,
                    text: "図".into(),
                }],
                pictures: vec![Picture {
                    rect,
                    resource: format!("plate-{chapter}.png"),
                    alt: "挿絵".into(),
                    inline_svg: None,
                    pdf_page: None,
                    fixed_canvas: None,
                    text_image: false,
                }],
                rules: vec![rect],
                header_fills: vec![rect],
                ..Page::default()
            }
        };
        let extent = vec2(1280., 760.);
        let first = page(4);
        let second = page(5);
        for rtl in [false, true] {
            let paired = pair(first.clone(), second.clone(), extent, rtl);
            assert_eq!(paired.first_locator(), first.first_locator());
            assert_eq!(paired.pictures.len(), 2);
            assert_eq!(
                paired
                    .hits
                    .iter()
                    .map(|hit| hit.locator.chapter)
                    .collect::<Vec<_>>(),
                [4, 5]
            );
            for (logical, original) in [&first, &second].iter().enumerate() {
                let source = original.first_locator().unwrap();
                let hit = paired.hit_for_locator(source).unwrap();
                assert_eq!(hit.locator, source);
                assert_eq!(paired.pictures[logical].rect, hit.rect);
                assert_eq!(paired.rules[logical], hit.rect);
                assert_eq!(paired.header_fills[logical], hit.rect);
                assert_eq!(hit.rect.center().x > extent.x * 0.5, (logical == 0) == rtl);
            }
        }
        let last = pair(first.clone(), Page::default(), extent, true);
        assert_eq!(
            last.pictures.len(),
            1,
            "an odd last page must not repeat a previous image"
        );
        assert_eq!(last.hits.len(), 1);
        assert_eq!(last.first_locator(), first.first_locator());
    }
}

#[cfg(test)]
mod publisher_tests {
    use super::*;
    use reader_core::{
        PagePlacement as P, PagePresentation, RenditionLayout as L, RenditionSpread as S,
    };
    fn page(id: u32, placement: P, spread: Option<S>) -> reader_core::Chapter {
        let mut c=reader_core::parse_html("<html><head><meta name='viewport' content='width=600,height=900'/></head><body><img src='page.svg'/></body></html>",id,&format!("{id}.xhtml"),"").unwrap();
        c.presentation = PagePresentation {
            layout: Some(L::PrePaginated),
            spread,
            placement,
            viewport: Some([600., 900.]),
        };
        c
    }
    #[test]
    fn publisher_none_and_center_override_automatic_image_pairing() {
        let wide = vec2(1280., 760.);
        for c in [
            page(0, P::Auto, Some(S::None)),
            page(0, P::Center, Some(S::Both)),
        ] {
            assert_eq!(page_count_for_chapter(wide, SpreadMode::Auto, false, &c), 1);
            assert_eq!(
                page_count_for_chapter(wide, SpreadMode::Double, false, &c),
                1
            );
        }
        assert_eq!(
            page_count_for_chapter(
                vec2(820., 600.),
                SpreadMode::Auto,
                false,
                &page(0, P::Auto, Some(S::Landscape))
            ),
            2
        );
        assert_eq!(
            page_count_for_chapter(
                vec2(600., 900.),
                SpreadMode::Auto,
                false,
                &page(0, P::Auto, Some(S::Landscape))
            ),
            1
        );
        assert_eq!(
            page_count_for_chapter(
                vec2(600., 900.),
                SpreadMode::Auto,
                false,
                &page(0, P::Auto, Some(S::Both))
            ),
            2
        );
    }
    #[test]
    fn source_slots_preserve_cover_and_incompatible_pages_without_navigation_history() {
        let mut book = Book::demo();
        let settings = Settings::default();
        let size = vec2(1280., 760.);
        book.page_progression = PageProgression::Rtl;
        book.chapters = vec![
            page(0, P::Center, None),
            page(1, P::Right, None),
            page(2, P::Left, None),
            page(3, P::Right, None),
            page(4, P::Right, None),
            page(5, P::Left, None),
        ];
        let cover = chapter_spread(&book, 0, size, &settings);
        assert_eq!((cover.columns, cover.second, cover.gap), (1, None, 0.));
        let pair = chapter_spread(&book, 1, size, &settings);
        assert_eq!((pair.first_slot, pair.second, pair.gap), (1, Some(2), 0.));
        let blank = chapter_spread(&book, 3, size, &settings);
        assert_eq!((blank.first_slot, blank.second), (1, None));
        assert_eq!(chapter_spread_start(&book, 2, size, &settings), 1);
        assert_eq!(previous_chapter_spread(&book, 3, size, &settings), 1);
        assert_eq!(previous_chapter_spread(&book, 4, size, &settings), 3);
        assert_eq!(chapter_spread(&book, 4, size, &settings).second, Some(5));
    }
    #[test]
    fn a_right_first_ltr_page_leaves_a_blank_then_starts_a_pair() {
        let mut book = Book::demo();
        book.page_progression = PageProgression::Ltr;
        book.chapters = vec![
            page(0, P::Right, None),
            page(1, P::Left, None),
            page(2, P::Right, None),
        ];
        let size = vec2(1280., 760.);
        let settings = Settings::default();
        assert_eq!(chapter_spread(&book, 0, size, &settings).second, None);
        assert_eq!(chapter_spread(&book, 0, size, &settings).first_slot, 1);
        assert_eq!(chapter_spread(&book, 1, size, &settings).second, Some(2));
        let c = page(0, P::Right, None);
        assert_eq!(
            (
                page_window(&c, 0, 5, 2, false).start,
                page_window(&c, 0, 5, 2, false).end
            ),
            (0, 1)
        );
        assert_eq!(
            (
                page_window(&c, 2, 5, 2, false).start,
                page_window(&c, 2, 5, 2, false).end
            ),
            (1, 3)
        );
    }
    #[test]
    fn embedded_sized_images_are_never_promoted_to_full_pages() {
        for html in [
            "<p><img src='gaiji.svg' width='24' height='24'/></p>",
            "<img src='diagram.svg' style='display:block;width:30%;height:100px'/>",
        ] {
            let chapter = reader_core::parse_html(html, 0, "inline.xhtml", "").unwrap();
            assert!(!is_image_chapter(&chapter));
            assert_eq!(
                page_count_for_chapter(vec2(1280., 760.), SpreadMode::Auto, false, &chapter),
                1
            );
        }
    }

    #[test]
    fn legacy_image_pairs_touch_and_an_unpaired_source_page_is_centered_at_full_width() {
        let image = |id| {
            reader_core::parse_html("<html><body><img src='plate.jpeg' style='width:977px;height:1280px'/></body></html>",id,&format!("{id}.xhtml"),"").unwrap()
        };
        let mut book = Book::demo();
        book.chapters = vec![image(0), image(1), image(2)];
        book.page_progression = PageProgression::Ltr;
        let settings = Settings {
            page_gap: Some(120.),
            ..Settings::default()
        };
        let viewport = vec2(1280., 760.);
        let paired = chapter_spread(&book, 0, viewport, &settings);
        assert_eq!(
            (paired.columns, paired.second, paired.gap),
            (2, Some(1), 0.)
        );
        assert_eq!(paired.extent(viewport), vec2(640., 760.));
        let last = chapter_spread(&book, 2, viewport, &settings);
        assert_eq!(
            (last.columns, last.second, last.first_slot, last.gap),
            (1, None, 0, 0.)
        );
        assert_eq!(last.extent(viewport), viewport);
        book.cover = Some("plate.jpeg".into());
        let cover = chapter_spread(&book, 0, viewport, &settings);
        assert_eq!((cover.columns, cover.second), (1, None));
    }

    #[test]
    fn background_geometry_survives_unloading_and_keeps_previous_spread_source_order() {
        use std::io::Write as _;
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("legacy-image-cache.epub");
        let mut archive = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        let manifest = (0..19)
            .map(|id| {
                format!("<item id='p{id}' href='{id}.xhtml' media-type='application/xhtml+xml'/>")
            })
            .collect::<String>();
        let spine = (0..19)
            .map(|id| format!("<itemref idref='p{id}'/>"))
            .collect::<String>();
        let mut files = vec![
            ("mimetype".to_owned(), "application/epub+zip".to_owned()),
            ("META-INF/container.xml".to_owned(), "<container xmlns='urn:oasis:names:tc:opendocument:xmlns:container'><rootfiles><rootfile full-path='package.opf'/></rootfiles></container>".to_owned()),
            ("package.opf".to_owned(), format!("<package xmlns='http://www.idpf.org/2007/opf' version='2.0' unique-identifier='id'><metadata xmlns:dc='http://purl.org/dc/elements/1.1/'><dc:identifier id='id'>geometry-cache-fixture</dc:identifier><dc:title>Image pages</dc:title></metadata><manifest>{manifest}</manifest><spine>{spine}</spine></package>")),
        ];
        for id in 0..19 {
            files.push((format!("{id}.xhtml"), "<html xmlns='http://www.w3.org/1999/xhtml'><body><img src='plate.jpeg' style='width:977px;height:1280px'/></body></html>".to_owned()));
        }
        for (name, contents) in files {
            archive.start_file(name, options).unwrap();
            archive.write_all(contents.as_bytes()).unwrap();
        }
        archive.finish().unwrap();
        let mut book = reader_core::open(&path).unwrap();
        book.cover = Some("plate.jpeg".into());
        assert!(
            !is_image_chapter(&book.chapters[1]),
            "an unparsed stub is unknown"
        );
        let publisher = book
            .chapters
            .iter()
            .map(|chapter| chapter.presentation)
            .collect::<Vec<_>>();

        // The global counter parses its own Book clone. Its derived metadata
        // must reach original stubs even when those chapters were never opened.
        let background = book.clone();
        for index in 1..19 {
            assert!(is_image_chapter(&background.load_chapter(index).unwrap()));
        }
        assert!(!book.chapters[1].loaded);
        assert!(is_image_chapter(&book.chapters[1]));
        for index in [2, 3, 4] {
            book.ensure_chapter_loaded(index).unwrap();
        }
        book.unload_chapters_except(&[2, 3, 4]);
        assert!(book.chapters[1].blocks.is_empty());
        assert!(is_image_chapter(&book.chapters[1]));
        assert_eq!(
            book.chapters
                .iter()
                .map(|chapter| chapter.presentation)
                .collect::<Vec<_>>(),
            publisher
        );
        let viewport = vec2(1280., 760.);
        let settings = Settings::default();
        assert_eq!(previous_chapter_spread(&book, 3, viewport, &settings), 1);
        assert_eq!(chapter_spread_start(&book, 4, viewport, &settings), 3);
        assert_eq!(
            chapter_spread(&book, 3, viewport, &settings).second,
            Some(4)
        );

        let cached =
            crate::pagination::PageIndex::new_with_starts(vec![1; 19], vec![vec![None]; 19])
                .unwrap()
                .capture_geometry(&book);
        let bytes = serde_json::to_vec(&cached).unwrap();
        let cached: crate::pagination::PageIndex = serde_json::from_slice(&bytes).unwrap();
        let mut cold = reader_core::open(&path).unwrap();
        cold.cover = Some("plate.jpeg".into());
        for index in 12..18 {
            cold.ensure_chapter_loaded(index).unwrap();
        }
        cold.unload_chapters_except(&(12..18).collect::<Vec<_>>());
        let parsed_before = cold.archive_stats().unwrap().chapters_parsed;
        assert_eq!(
            chapter_spread_start(&cold, 13, viewport, &settings),
            13,
            "a partial-classification cache miss preserves the requested source chapter"
        );
        assert!(!is_image_chapter(&cold.chapters[1]));
        let legacy = crate::pagination::PageIndex::new(vec![1; 19]).unwrap();
        assert!(
            legacy.restore_geometry(&cold).is_err(),
            "counts alone cannot establish spread parity"
        );
        let mut unknown = cached.clone();
        unknown.geometry[1] = reader_core::PageGeometrySnapshot::Unknown;
        assert!(unknown.restore_geometry(&cold).is_err());
        assert!(!is_image_chapter(&cold.chapters[1]));
        cached.restore_geometry(&cold).unwrap();
        assert!(!cold.chapters[1].loaded);
        assert!(is_image_chapter(&cold.chapters[1]));
        assert_eq!(previous_chapter_spread(&cold, 13, viewport, &settings), 11);
        assert_eq!(chapter_spread_start(&cold, 13, viewport, &settings), 13);
        assert_eq!(chapter_spread_start(&cold, 14, viewport, &settings), 13);
        assert_eq!(
            cold.archive_stats().unwrap().chapters_parsed,
            parsed_before,
            "restoring a complete pagination cache never reparses the book"
        );

        let serialized = serde_json::to_value(&book.chapters[1]).unwrap();
        assert!(serialized.get("geometry_cache").is_none());
        let reopened: reader_core::Chapter = serde_json::from_value(serialized).unwrap();
        assert_eq!(
            reopened, book.chapters[1],
            "runtime caches do not affect source equality"
        );
        assert!(
            !is_image_chapter(&reopened),
            "a serialized stub recomputes on reopen"
        );
    }

    #[test]
    fn several_image_flow_pages_keep_local_spreads_without_cross_chapter_pairing() {
        let mut book = Book::demo();
        book.chapters = vec![
            reader_core::parse_html("<html><body><img src='one.svg' style='display:block;width:100%;height:100%'/><img src='two.svg' style='display:block;width:100%;height:100%'/></body></html>",0,"flow.xhtml","").unwrap(),
            reader_core::parse_html("<html><body><img src='next.svg' width='600' height='900'/></body></html>",1,"next.xhtml","").unwrap(),
        ];
        assert!(is_image_chapter(&book.chapters[0]));
        assert!(reader_core::page_document_geometry(&book.chapters[0]).is_none());
        let viewport = vec2(1280., 760.);
        let settings = Settings::default();
        assert_eq!(
            page_count_for_chapter(viewport, settings.spread, false, &book.chapters[0]),
            2
        );
        let plan = chapter_spread(&book, 0, viewport, &settings);
        assert_eq!((plan.columns, plan.second, plan.consumed()), (2, None, 1));
        assert_eq!(plan.gap, 0.);
    }

    #[test]
    fn a_landscape_source_image_is_one_plate_even_when_two_page_display_is_requested() {
        let chapter = reader_core::parse_html(
            "<html><body><img src='spread.jpeg' width='1600' height='900'/></body></html>",
            0,
            "spread.xhtml",
            "",
        )
        .unwrap();
        for spread in [SpreadMode::Auto, SpreadMode::Double] {
            assert_eq!(
                page_count_for_chapter(vec2(1280., 760.), spread, false, &chapter),
                1
            );
        }
    }

    #[test]
    fn native_fixed_images_align_to_the_join_without_adding_letterbox_gutter() {
        use crate::layout::{FixedCanvas, Picture};
        use eframe::egui::pos2;
        let page = || Page {
            pictures: vec![Picture {
                rect: Rect::from_min_size(pos2(80., 0.), vec2(480., 760.)),
                resource: "plate.png".into(),
                alt: String::new(),
                inline_svg: None,
                pdf_page: None,
                fixed_canvas: Some(FixedCanvas {
                    extent: vec2(600., 950.),
                    image_rect: Rect::from_min_size(eframe::egui::Pos2::ZERO, vec2(600., 950.)),
                }),
                text_image: false,
            }],
            ..Page::default()
        };
        let joined = compose(page(), Some(page()), vec2(1280., 760.), 2, 0, 0.);
        assert_eq!(joined.pictures[0].rect.right(), 640.);
        assert_eq!(joined.pictures[1].rect.left(), 640.);
    }
}
