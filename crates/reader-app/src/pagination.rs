//! Exact page weights for the active typography and viewport; no chapter ratios.
use crate::{fonts::FontSystem, layout, reading_surface};
use anyhow::{Result, ensure};
use eframe::egui::Vec2;
use reader_core::{Book, Locator};
use reader_library::Settings;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// EPUB fixed-layout spine items are exactly one source page, independently of
/// viewport size, image decoding or whether two items are shown in a spread.
/// An unloaded item may have no source anchor yet; its count is still exact.
pub fn known_chromium_page_starts(chapter: &reader_core::Chapter) -> Option<Vec<Option<Locator>>> {
    reader_core::page_document_geometry(chapter).map(|_| vec![chapter.start_locator()])
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PageIndex {
    pub counts: Vec<usize>,
    /// Prefix offsets, including a final `total` sentinel.
    pub offsets: Vec<usize>,
    pub total: usize,
    /// Real first source locator of each rendered page. Empty only for legacy
    /// counts-only indexes. `None` is an explicitly source-less page.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub starts: Vec<Vec<Option<Locator>>>,
    #[serde(skip)]
    ordered: Vec<bool>,
}

impl<'de> Deserialize<'de> for PageIndex {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Stored {
            counts: Vec<usize>,
            #[serde(default)]
            starts: Vec<Vec<Option<Locator>>>,
        }
        let stored = Stored::deserialize(deserializer)?;
        // Derived prefix sums are rebuilt, never trusted from a disk cache.
        Self::new_with_starts(stored.counts, stored.starts).map_err(serde::de::Error::custom)
    }
}

impl PageIndex {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Count-only constructor for callers without source anchors"
        )
    )]
    pub fn new(counts: Vec<usize>) -> Result<Self> {
        Self::new_with_starts(counts, Vec::new())
    }

    pub fn new_with_starts(counts: Vec<usize>, starts: Vec<Vec<Option<Locator>>>) -> Result<Self> {
        ensure!(!counts.is_empty(), "ページ数を計算する章がありません");
        ensure!(
            starts.is_empty() || starts.len() == counts.len(),
            "ページ位置の章数が一致しません"
        );
        for (chapter, pages) in starts.iter().enumerate() {
            ensure!(
                pages.len() == counts[chapter],
                "章のページ数と原文位置の数が一致しません"
            );
            ensure!(
                pages
                    .iter()
                    .flatten()
                    .all(|location| location.chapter as usize == chapter),
                "原文位置が別の章を指しています"
            );
        }
        let mut offsets = Vec::with_capacity(counts.len() + 1);
        offsets.push(0usize);
        for count in &counts {
            ensure!(*count > 0, "章のページ数は1以上である必要があります");
            offsets.push(
                offsets
                    .last()
                    .unwrap()
                    .checked_add(*count)
                    .ok_or_else(|| anyhow::anyhow!("本のページ数が上限を超えています"))?,
            );
        }
        let total = *offsets.last().unwrap();
        let ordered = starts
            .iter()
            .map(|pages| {
                let mut previous = None;
                pages.iter().all(|start| {
                    let Some(start) = start else {
                        return false;
                    };
                    let ordered = previous.is_none_or(|previous| previous < *start);
                    previous = Some(*start);
                    ordered
                })
            })
            .collect();
        Ok(Self {
            counts,
            offsets,
            total,
            starts,
            ordered,
        })
    }

    /// Resolve a retained source position against this layout's actual page
    /// starts. This never estimates a character ratio or a chapter weight.
    pub fn locator_fraction(&self, locator: Locator) -> Option<f32> {
        let chapter = locator.chapter as usize;
        let count = *self.counts.get(chapter)?;
        if count == 1 {
            return Some(self.fraction(chapter, 0));
        }
        let starts = self.starts.get(chapter)?;
        if starts.len() != count {
            return None;
        }
        if self.ordered.get(chapter).copied().unwrap_or(false) {
            let page = starts
                .partition_point(|start| start.is_some_and(|start| start <= locator))
                .saturating_sub(1);
            return Some(self.fraction(chapter, page));
        }
        // Exact matches also agree with native restore's first-containing-page
        // rule when a source (e.g. a table heading) occurs more than once.
        if let Some(page) = starts.iter().position(|start| *start == Some(locator)) {
            return Some(self.fraction(chapter, page));
        }
        None
    }

    pub fn fraction(&self, chapter: usize, page: usize) -> f32 {
        if self.total <= 1 || self.counts.is_empty() {
            return 0.;
        }
        let chapter = chapter.min(self.counts.len() - 1);
        let global = self.offsets[chapter] + page.min(self.counts[chapter] - 1);
        global as f32 / (self.total - 1) as f32
    }

    pub fn locate(&self, fraction: f32) -> (usize, usize) {
        if self.counts.is_empty() || self.total == 0 {
            return (0, 0);
        }
        let fraction = if fraction.is_nan() {
            0.
        } else {
            fraction.clamp(0., 1.)
        };
        let page = (fraction as f64 * self.total.saturating_sub(1) as f64).round() as usize;
        let page = page.min(self.total - 1);
        let chapter = self
            .offsets
            .partition_point(|offset| *offset <= page)
            .saturating_sub(1)
            .min(self.counts.len() - 1);
        (chapter, page - self.offsets[chapter])
    }
}

pub fn compute(
    book: &Book,
    extent: Vec2,
    settings: &Settings,
    epoch: &AtomicU64,
    generation: u64,
) -> Result<PageIndex> {
    // Leave headroom for painting and speech synthesis. Each worker owns one
    // font/shaping cache and at most one parsed chapter; the archive remains
    // shared and only requested ZIP members are expanded.
    let workers = std::thread::available_parallelism()
        .map(|n| (n.get() / 2).clamp(1, 3))
        .unwrap_or(1);
    compute_with_workers(book, extent, settings, epoch, generation, workers)
}

fn compute_with_workers(
    book: &Book,
    extent: Vec2,
    settings: &Settings,
    epoch: &AtomicU64,
    generation: u64,
    workers: usize,
) -> Result<PageIndex> {
    compute_output::<false>(book, extent, settings, epoch, generation, workers)
}

fn compute_output<const EMIT: bool>(
    book: &Book,
    extent: Vec2,
    settings: &Settings,
    epoch: &AtomicU64,
    generation: u64,
    workers: usize,
) -> Result<PageIndex> {
    let check = || -> Result<()> {
        ensure!(
            epoch.load(Ordering::Acquire) == generation,
            "ページ数の計算は取り消されました"
        );
        Ok(())
    };
    check()?;
    ensure!(
        extent.x.is_finite() && extent.y.is_finite() && extent.x > 0. && extent.y > 0.,
        "ページ領域が不正です"
    );
    ensure!(
        !book.chapters.is_empty(),
        "ページ数を計算する章がありません"
    );
    if book
        .chapters
        .iter()
        .all(|chapter| reader_core::page_document_geometry(chapter).is_some())
    {
        // Fixed-layout metadata already declares one source page per chapter.
        // Counting needs no font cache, image-resource clone or text layout.
        return PageIndex::new_with_starts(
            vec![1; book.chapters.len()],
            book.chapters
                .iter()
                .map(|chapter| vec![chapter.start_locator()])
                .collect(),
        );
    }
    let next = AtomicUsize::new(0);
    let worker = || -> Result<Vec<(usize, Vec<Option<Locator>>)>> {
        #[cfg(test)]
        let mut phases = [std::time::Duration::ZERO; 4];
        #[cfg(test)]
        let started = std::time::Instant::now();
        let mut working = book.clone();
        working.unload_chapters_except(&[]);
        #[cfg(test)]
        {
            phases[0] += started.elapsed();
        }
        #[cfg(test)]
        let started = std::time::Instant::now();
        let mut fonts = FontSystem::new();
        #[cfg(test)]
        {
            phases[1] += started.elapsed();
        }
        let mut results = Vec::new();
        loop {
            check()?;
            let index = next.fetch_add(1, Ordering::Relaxed);
            let Some(metadata) = working.chapters.get(index) else {
                break;
            };
            if reader_core::page_document_geometry(metadata).is_some() {
                results.push((index, vec![metadata.start_locator()]));
                continue;
            }
            #[cfg(test)]
            let started = std::time::Instant::now();
            working.ensure_chapter_loaded(index)?;
            #[cfg(test)]
            {
                phases[2] += started.elapsed();
            }
            #[cfg(test)]
            let started = std::time::Instant::now();
            if reader_core::page_document_geometry(&working.chapters[index]).is_some() {
                results.push((index, vec![working.chapters[index].start_locator()]));
                working.unload_chapters_except(&[]);
                continue;
            }
            let content = reading_surface::content_rect(
                eframe::egui::Rect::from_min_size(eframe::egui::Pos2::ZERO, extent),
                &working.chapters[index],
                settings,
            );
            let effective =
                reading_surface::effective_layout_settings(settings, &working.chapters[index]);
            let plan = reading_surface::chapter_spread(&working, index, content.size(), settings);
            let starts = if EMIT {
                let page = layout::chapter_layout(
                    &mut fonts,
                    &working.chapters[index],
                    plan.extent(content.size()),
                    &effective,
                );
                if page.pages.is_empty() {
                    vec![None]
                } else {
                    page.pages.iter().map(|page| page.first_locator()).collect()
                }
            } else {
                layout::chapter_page_starts(
                    &mut fonts,
                    &working.chapters[index],
                    plan.extent(content.size()),
                    &effective,
                )
            };
            #[cfg(test)]
            {
                phases[3] += started.elapsed();
            }
            check()?;
            results.push((index, starts));
            working.unload_chapters_except(&[]);
        }
        #[cfg(test)]
        if std::env::var_os("VOBR_PAGINATION_PHASES").is_some() {
            eprintln!(
                "pagination_worker: emit={EMIT} chapters={} clone_us={} fonts_us={} load_us={} layout_us={}",
                results.len(),
                phases[0].as_micros(),
                phases[1].as_micros(),
                phases[2].as_micros(),
                phases[3].as_micros()
            );
        }
        Ok(results)
    };
    let workers = workers.clamp(1, 3).min(book.chapters.len());
    let chunks = if workers == 1 {
        vec![worker()?]
    } else {
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..workers).map(|_| scope.spawn(worker)).collect();
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .map_err(|_| anyhow::anyhow!("ページ数の計算に失敗しました"))?
                })
                .collect::<Result<Vec<_>>>()
        })?
    };
    let mut starts = vec![Vec::new(); book.chapters.len()];
    for (chapter, pages) in chunks.into_iter().flatten() {
        starts[chapter] = pages;
    }
    check()?;
    PageIndex::new_with_starts(starts.iter().map(Vec::len).collect(), starts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::vec2;
    use reader_core::{PagePresentation, RenditionLayout, parse_html};
    use reader_library::{ReadingMode, SpreadMode};

    #[test]
    fn chromium_fixed_metadata_counts_without_loading_artwork_or_source_nodes() {
        let mut chapter = Book::demo().chapters[0].clone();
        chapter.loaded = false;
        chapter.blocks.clear();
        chapter.anchors.clear();
        chapter.presentation.layout = Some(RenditionLayout::PrePaginated);
        assert_eq!(known_chromium_page_starts(&chapter), Some(vec![None]));

        let index = PageIndex::new_with_starts(vec![1; 40], vec![vec![None]; 40]).unwrap();
        let location = Locator {
            chapter: 19,
            node: 999,
            offset: 12,
        };
        assert_eq!(index.total, 40);
        assert_eq!(
            index.locate(index.locator_fraction(location).unwrap()),
            (19, 0)
        );
    }

    #[test]
    fn a_legacy_image_book_has_one_source_page_per_spine_item_without_pixel_decoding() {
        let mut book = Book::demo();
        book.chapters=(0..46).map(|id|parse_html("<html><body><div><img src='plate.jpeg' class='plate' width='2480' height='3248' alt='Image alt'/></div></body></html>",id,&format!("{id}.xhtml"),".plate{width:977px;height:1280px}").unwrap()).collect();
        for chapter in &book.chapters {
            assert_eq!(
                known_chromium_page_starts(chapter),
                Some(vec![chapter.start_locator()])
            );
        }
        let index = compute(
            &book,
            vec2(1280., 760.),
            &Settings {
                page_gap: Some(120.),
                ..Settings::default()
            },
            &AtomicU64::new(0),
            0,
        )
        .unwrap();
        assert_eq!(index.total, 46);
        assert_eq!(index.counts, vec![1; 46]);
    }

    #[test]
    fn chromium_fast_count_requires_explicit_fixed_layout_and_keeps_known_anchor() {
        let mut chapter = parse_html(
            "<html><body><img src='not-loaded.png' style='width:100%;height:100%'/></body></html>",
            0,
            "artwork.xhtml",
            "",
        )
        .unwrap();
        assert_eq!(known_chromium_page_starts(&chapter), None);
        chapter.presentation.layout = Some(RenditionLayout::Reflowable);
        assert_eq!(known_chromium_page_starts(&chapter), None);
        chapter.presentation.layout = Some(RenditionLayout::PrePaginated);
        let anchor = chapter.start_locator();
        assert!(anchor.is_some());
        assert_eq!(known_chromium_page_starts(&chapter), Some(vec![anchor]));
    }

    #[test]
    #[ignore = "Manual pagination benchmark; VOBR_PAGINATION_BOOK selects a local, read-only EPUB"]
    fn whole_book_timing_probe() {
        let path = std::env::var_os("VOBR_PAGINATION_BOOK")
            .expect("Set VOBR_PAGINATION_BOOK to a local EPUB");
        let started = std::time::Instant::now();
        let book = reader_core::open(path).unwrap();
        let opened = started.elapsed();
        let settings = Settings::default();
        let before = std::time::Instant::now();
        let emit = std::env::var("VOBR_PAGINATION_OUTPUT").is_ok_and(|mode| mode == "full");
        let workers = std::env::var("VOBR_PAGINATION_WORKERS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(3);
        let result = if emit {
            compute_output::<true>(
                &book,
                vec2(1280., 760.),
                &settings,
                &AtomicU64::new(0),
                0,
                workers,
            )
        } else {
            compute_output::<false>(
                &book,
                vec2(1280., 760.),
                &settings,
                &AtomicU64::new(0),
                0,
                workers,
            )
        }
        .unwrap();
        eprintln!(
            "pagination: open_ms={} layout_ms={} chapters={} pages={}",
            opened.as_millis(),
            before.elapsed().as_millis(),
            book.chapters.len(),
            result.total
        );
        assert!(result.total >= book.chapters.len());
        if let Some(path) = std::env::var_os("VOBR_PAGINATION_INDEX_OUTPUT") {
            std::fs::write(path, serde_json::to_vec(&result).unwrap()).unwrap();
        }
    }

    #[test]
    fn parallel_pages_and_source_starts_match_serial_layout_in_both_directions() {
        let mut book = Book::demo();
        let text = "<h2>本文と図版</h2><p>文章と<ruby>漢字<rt>かんじ</rt></ruby>、閉じ括弧「文章」。<b>太字</b>を読みます。</p>";
        book.chapters = (0..7)
            .map(|id| {
                let body = text.repeat((id as usize + 1) * 17);
                parse_html(
                    &format!("<html><body>{body}</body></html>"),
                    id,
                    &format!("chapter-{id}.xhtml"),
                    "",
                )
                .unwrap()
            })
            .collect();
        for mode in [ReadingMode::Horizontal, ReadingMode::Vertical] {
            let settings = Settings {
                reading_mode: mode,
                font_size: 27.,
                margin_vertical: Some(38.),
                margin_horizontal: Some(57.),
                ..Settings::default()
            };
            let epoch = AtomicU64::new(1);
            let serial =
                compute_with_workers(&book, vec2(590., 650.), &settings, &epoch, 1, 1).unwrap();
            let parallel =
                compute_with_workers(&book, vec2(590., 650.), &settings, &epoch, 1, 3).unwrap();
            assert_eq!(serial, parallel);
            let rendered =
                compute_output::<true>(&book, vec2(590., 650.), &settings, &epoch, 1, 1).unwrap();
            assert_eq!(serial, rendered);
            assert!(
                compute_with_workers(&book, vec2(590., 650.), &settings, &epoch, 0, 3).is_err()
            );
        }
    }

    #[test]
    fn one_image_page_has_the_same_weight_as_one_text_page() {
        let index = PageIndex::new(vec![41, 1, 20]).unwrap();
        assert_eq!(index.total, 62);
        assert_eq!(index.offsets, [0, 41, 42, 62]);
        let before = index.fraction(0, 40);
        let image = index.fraction(1, 0);
        let after = index.fraction(2, 0);
        assert!(((image - before) - (after - image)).abs() < 0.000001);
        assert!((image - 41. / 61.).abs() < 0.000001);
        for (chapter, count) in index.counts.iter().enumerate() {
            for page in 0..*count {
                assert_eq!(index.locate(index.fraction(chapter, page)), (chapter, page));
            }
        }
        assert_eq!(index.locate(0.), (0, 0));
        assert_eq!(index.locate(1.), (2, 19));
    }

    #[test]
    fn invalid_counts_and_nonfinite_slider_values_are_handled() {
        assert!(PageIndex::new(vec![]).is_err());
        assert!(PageIndex::new(vec![1, 0]).is_err());
        assert!(PageIndex::new(vec![usize::MAX, 1]).is_err());
        let index = PageIndex::new(vec![1]).unwrap();
        assert_eq!(index.fraction(usize::MAX, usize::MAX), 0.);
        assert_eq!(index.locate(f32::NAN), (0, 0));
        let index = PageIndex::new(vec![2, 3]).unwrap();
        assert_eq!(index.locate(f32::NEG_INFINITY), (0, 0));
        assert_eq!(index.locate(f32::INFINITY), (1, 2));
    }

    #[test]
    fn typography_and_viewport_change_actual_page_weights() {
        let mut book = Book::demo();
        let content = format!(
            "<html><body><p>{}</p></body></html>",
            "日本語の文章を読みます。正しい組版とページ数を確認します。".repeat(180)
        );
        book.chapters = vec![parse_html(&content, 0, "chapter.xhtml", "").unwrap()];
        let settings = Settings {
            font_size: 20.,
            margins: 20.,
            spread: SpreadMode::Single,
            reading_mode: ReadingMode::Horizontal,
            ..Settings::default()
        };
        let epoch = AtomicU64::new(7);
        let normal = compute(&book, vec2(640., 700.), &settings, &epoch, 7).unwrap();
        let large = compute(
            &book,
            vec2(640., 700.),
            &Settings {
                font_size: 32.,
                ..settings.clone()
            },
            &epoch,
            7,
        )
        .unwrap();
        let narrow = compute(&book, vec2(360., 700.), &settings, &epoch, 7).unwrap();
        assert!(large.total > normal.total);
        assert!(narrow.total > normal.total);
        epoch.store(8, Ordering::Release);
        assert!(compute(&book, vec2(640., 700.), &settings, &epoch, 7).is_err());
    }

    #[test]
    fn fixed_metadata_does_not_require_an_available_chapter_loader() {
        let mut book = Book::demo();
        book.chapters.truncate(1);
        book.chapters[0].blocks.clear();
        book.chapters[0].loaded = false;
        book.chapters[0].presentation = PagePresentation {
            layout: Some(RenditionLayout::PrePaginated),
            ..PagePresentation::default()
        };
        assert!(book.load_chapter(0).is_err());
        let result = compute(
            &book,
            vec2(640., 700.),
            &Settings::default(),
            &AtomicU64::new(0),
            0,
        )
        .unwrap();
        assert_eq!(result.counts, [1]);
        assert_eq!(result.starts, vec![vec![None]]);
        assert_eq!(
            result.locator_fraction(Locator {
                chapter: 0,
                node: 99,
                offset: 300
            }),
            Some(0.)
        );
    }

    #[test]
    fn source_offsets_choose_real_page_boundaries_without_character_ratios() {
        let at = |chapter, node, offset| {
            Some(Locator {
                chapter,
                node,
                offset,
            })
        };
        let index = PageIndex::new_with_starts(
            vec![3, 1, 2],
            vec![
                vec![at(0, 7, 4), at(0, 7, 120), at(0, 12, 0)],
                vec![None],
                vec![at(2, 3, 0), at(2, 8, 30)],
            ],
        )
        .unwrap();
        for (locator, expected) in [
            (
                Locator {
                    chapter: 0,
                    node: 7,
                    offset: 0,
                },
                (0, 0),
            ),
            (
                Locator {
                    chapter: 0,
                    node: 7,
                    offset: 119,
                },
                (0, 0),
            ),
            (
                Locator {
                    chapter: 0,
                    node: 7,
                    offset: 120,
                },
                (0, 1),
            ),
            (
                Locator {
                    chapter: 0,
                    node: 10,
                    offset: 999,
                },
                (0, 1),
            ),
            (
                Locator {
                    chapter: 0,
                    node: 12,
                    offset: 1,
                },
                (0, 2),
            ),
            (
                Locator {
                    chapter: 1,
                    node: 999,
                    offset: 999,
                },
                (1, 0),
            ),
            (
                Locator {
                    chapter: 2,
                    node: 8,
                    offset: 31,
                },
                (2, 1),
            ),
        ] {
            assert_eq!(
                index.locate(index.locator_fraction(locator).unwrap()),
                expected
            );
        }
        assert!(
            index
                .locator_fraction(Locator {
                    chapter: 3,
                    node: 0,
                    offset: 0
                })
                .is_none()
        );
    }

    #[test]
    fn missing_or_overlapping_source_starts_do_not_invent_a_dot_position() {
        let legacy = PageIndex::new(vec![3, 1]).unwrap();
        assert!(
            legacy
                .locator_fraction(Locator {
                    chapter: 0,
                    node: 10,
                    offset: 40
                })
                .is_none()
        );
        assert_eq!(
            legacy.locator_fraction(Locator {
                chapter: 1,
                node: 10,
                offset: 40
            }),
            Some(1.)
        );
        let start = Locator {
            chapter: 0,
            node: 5,
            offset: 0,
        };
        let overlap =
            PageIndex::new_with_starts(vec![2], vec![vec![Some(start), Some(start)]]).unwrap();
        assert_eq!(overlap.locator_fraction(start), Some(0.));
        assert!(
            overlap
                .locator_fraction(Locator { offset: 1, ..start })
                .is_none()
        );
        let unknown = PageIndex::new_with_starts(vec![2], vec![vec![Some(start), None]]).unwrap();
        assert!(
            unknown
                .locator_fraction(Locator { offset: 1, ..start })
                .is_none()
        );
    }

    #[test]
    fn serialized_source_index_validates_counts_and_rebuilds_derived_offsets() {
        let json = serde_json::json!({"counts":[2,1],"offsets":[999],"total":1,"starts":[[{"chapter":0,"node":5,"offset":0},{"chapter":0,"node":5,"offset":80}],[null]]});
        let index: PageIndex = serde_json::from_value(json).unwrap();
        assert_eq!(index.offsets, [0, 2, 3]);
        assert_eq!(index.total, 3);
        let encoded = serde_json::to_vec(&index).unwrap();
        assert_eq!(
            serde_json::from_slice::<PageIndex>(&encoded).unwrap(),
            index
        );
        let legacy: PageIndex = serde_json::from_str(r#"{"counts":[2,1]}"#).unwrap();
        assert!(legacy.starts.is_empty());
        for bad in [
            serde_json::json!({"counts":[2],"starts":[[null]]}),
            serde_json::json!({"counts":[2,1],"starts":[[null,null]]}),
            serde_json::json!({"counts":[1],"starts":[[{"chapter":3,"node":0,"offset":0}]]}),
            serde_json::json!({"counts":[0],"starts":[[]]}),
        ] {
            assert!(serde_json::from_value::<PageIndex>(bad).is_err());
        }
    }

    #[test]
    fn source_history_follows_actual_font_viewport_and_writing_mode_reflow() {
        let mut book = Book::demo();
        book.chapters=vec![parse_html(&format!("<html><body>{}</body></html>","<p>本の本文を読みます。<ruby>東京<rt>とうきょう</rt></ruby>の街を歩き、次の文章へ進みます。</p>".repeat(100)),0,"history.xhtml","").unwrap()];
        let base = Settings {
            font_size: 20.,
            margins: 20.,
            spread: SpreadMode::Single,
            reading_mode: ReadingMode::Horizontal,
            ..Settings::default()
        };
        let original = layout::chapter_layout(
            &mut FontSystem::new(),
            &book.chapters[0],
            vec2(580., 620.),
            &base,
        );
        let locator = original.pages[original.pages.len() / 2].hits[3].locator;
        for (size, font_size, reading_mode) in [
            (vec2(580., 620.), 20., ReadingMode::Horizontal),
            (vec2(480., 600.), 28., ReadingMode::Vertical),
            (vec2(400., 450.), 34., ReadingMode::Horizontal),
        ] {
            let settings = Settings {
                font_size,
                reading_mode,
                ..base.clone()
            };
            let rendered =
                layout::chapter_layout(&mut FontSystem::new(), &book.chapters[0], size, &settings);
            let expected = rendered
                .pages
                .iter()
                .position(|page| page.contains_locator(locator))
                .unwrap();
            let index = compute(&book, size, &settings, &AtomicU64::new(0), 0).unwrap();
            assert_eq!(
                index.locate(index.locator_fraction(locator).unwrap()),
                (0, expected)
            );
            for (page, start) in index.starts[0].iter().enumerate() {
                if let Some(start) = start {
                    assert_eq!(
                        index.locate(index.locator_fraction(*start).unwrap()),
                        (0, page)
                    );
                }
            }
        }
    }
}
