use crate::fonts::{self, FontSystem};
use eframe::egui::{Pos2, Rect, Vec2, pos2, vec2};
use reader_core::{Atom, Block, BlockKind, Chapter, ImageAlign, ImageDisplay, ImageRef, Locator};
use reader_library::{ReadingMode, Settings};
use std::collections::HashMap;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone)]
pub struct Glyph {
    pub face: usize,
    pub id: u16,
    pub origin: Pos2,
    pub size: f32,
    pub rotated: bool,
    pub style: fonts::GlyphStyle,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub rect: Rect,
    pub locator: Locator,
    pub end: Locator,
    pub unit_id: u64,
    pub offset: usize,
    pub text: String,
}

#[derive(Clone)]
pub struct Picture {
    pub rect: Rect,
    pub resource: String,
    pub alt: String,
    pub inline_svg: Option<String>,
    pub pdf_page: Option<reader_core::PdfPageRef>,
    /// EPUB image pages are composed in the publisher's viewport before the
    /// whole canvas is scaled. PDF uses its existing native page path instead.
    pub fixed_canvas: Option<FixedCanvas>,
    /// Per-instance source semantics, independent of other uses of this resource.
    pub text_image: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct FixedCanvas {
    pub extent: Vec2,
    pub image_rect: Rect,
}

#[derive(Clone, Default)]
pub struct Page {
    pub glyphs: Vec<Glyph>,
    pub hits: Vec<Hit>,
    pub pictures: Vec<Picture>,
    /// Native table cell outlines and header fills (paint fills before text).
    pub rules: Vec<Rect>,
    pub header_fills: Vec<Rect>,
    pub anchor: Option<Locator>,
}

impl Page {
    fn content_bounds(&self, fonts: &mut FontSystem, body_size: f32) -> Rect {
        let mut bounds = Rect::NOTHING;
        for hit in &self.hits {
            bounds = bounds.union(hit.rect);
        }
        for picture in &self.pictures {
            bounds = bounds.union(picture.rect);
        }
        for rule in &self.rules {
            bounds = bounds.union(*rule);
        }
        // Ruby can extend outside its parent's source hit box. Measure only
        // these small glyphs; normal text cells already define the body grid.
        for glyph in self
            .glyphs
            .iter()
            .filter(|glyph| glyph.size < body_size * 0.7 && !glyph.rotated)
        {
            if let Some(outline) = fonts.outline_bounds(glyph.face, glyph.id) {
                bounds = bounds.union(Rect::from_min_max(
                    glyph.origin + outline.min.to_vec2() * glyph.size,
                    glyph.origin + outline.max.to_vec2() * glyph.size,
                ));
            }
        }
        bounds
    }

    fn balance_grid(
        &mut self,
        fonts: &mut FontSystem,
        extent: Vec2,
        margin: Vec2,
        settings: &Settings,
        vertical: bool,
    ) {
        let bounds = self.content_bounds(fonts, settings.font_size);
        if !bounds.is_finite() || bounds.is_negative() {
            return;
        }
        let available = extent - margin * 2.0;
        let character = settings.font_size * (1.0 + settings.letter_spacing).max(0.2);
        let line = settings.font_size * settings.line_spacing;
        let pitch = if vertical {
            vec2(line, character)
        } else {
            vec2(character, line)
        };
        let mut shift = Vec2::ZERO;
        // Do not vertically centre a short final paragraph or move a few
        // terminal vertical columns into the middle of an otherwise empty page.
        // Only distribute the unused grid remainder on an almost-filled axis.
        for axis in 0..2 {
            if bounds.size()[axis] >= available[axis] - pitch[axis] - 0.1
                && bounds.size()[axis] <= available[axis] + settings.font_size
            {
                shift[axis] = extent[axis] * 0.5 - bounds.center()[axis];
            }
        }
        if shift == Vec2::ZERO {
            return;
        }
        for glyph in &mut self.glyphs {
            glyph.origin += shift;
        }
        for hit in &mut self.hits {
            hit.rect = hit.rect.translate(shift);
        }
        for picture in &mut self.pictures {
            picture.rect = picture.rect.translate(shift);
        }
        for rule in &mut self.rules {
            *rule = rule.translate(shift);
        }
        for fill in &mut self.header_fills {
            *fill = fill.translate(shift);
        }
    }

    pub fn first_locator(&self) -> Option<Locator> {
        self.anchor.or_else(|| self.hits.first().map(|h| h.locator))
    }
    pub fn hit(&self, point: Pos2) -> Option<&Hit> {
        self.hits.iter().find(|h| h.rect.contains(point))
    }
    pub fn hit_for_locator(&self, locator: Locator) -> Option<&Hit> {
        self.hits
            .iter()
            .find(|hit| hit.locator <= locator && locator < hit.end)
    }
    /// A source can fall inside a ruby group or ligature rather than exactly
    /// on a glyph boundary. Image-only pages have a stable anchor of their own.
    pub fn contains_locator(&self, locator: Locator) -> bool {
        self.anchor == Some(locator) || self.hit_for_locator(locator).is_some()
    }

    pub(crate) fn append_at(&mut self, mut other: Page, offset: Vec2, scale: f32) {
        let transform = |point: Pos2| pos2(point.x * scale + offset.x, point.y * scale + offset.y);
        let rectangle = |rect: Rect| Rect::from_min_max(transform(rect.min), transform(rect.max));
        for glyph in &mut other.glyphs {
            glyph.origin = transform(glyph.origin);
            glyph.size *= scale;
        }
        for hit in &mut other.hits {
            hit.rect = rectangle(hit.rect);
        }
        for picture in &mut other.pictures {
            picture.rect = rectangle(picture.rect);
        }
        other.rules = other.rules.into_iter().map(rectangle).collect();
        other.header_fills = other.header_fills.into_iter().map(rectangle).collect();
        self.glyphs.extend(other.glyphs);
        self.hits.extend(other.hits);
        self.pictures.extend(other.pictures);
        self.rules.extend(other.rules);
        self.header_fills.extend(other.header_fills);
    }
}

#[derive(Default)]
pub struct BookLayout {
    pub pages: Vec<Page>,
}

pub fn unit_id(chapter: u32, atom_index: usize) -> u64 {
    (u64::from(chapter) << 32) | atom_index as u64
}

fn image_size(image: &ImageRef, em: f32, rem: f32, available: Vec2) -> Vec2 {
    let resolve = |length: reader_core::CssLength, reference| length.resolve(em, rem, reference);
    let width = image
        .width
        .map(|value| resolve(value, available.x))
        .filter(|value| *value >= 0.0);
    let height = image
        .height
        .map(|value| resolve(value, available.y))
        .filter(|value| *value >= 0.0);
    let intrinsic = image
        .intrinsic_size
        .map(|[width, height]| vec2(width, height));
    let ratio = image
        .aspect_ratio
        .filter(|ratio| ratio.is_finite() && *ratio > 0.0)
        .or_else(|| {
            intrinsic
                .filter(|size| size.x > 0.0 && size.y > 0.0)
                .map(|size| size.x / size.y)
        })
        .unwrap_or(if image.display == ImageDisplay::Inline {
            1.0
        } else {
            5.0 / 3.0
        });
    let mut size = match (width, height) {
        (Some(width), Some(height)) => vec2(width, height),
        (Some(width), None) => vec2(width, width / ratio),
        (None, Some(height)) => vec2(height * ratio, height),
        _ => intrinsic.unwrap_or_else(|| {
            if image.display == ImageDisplay::Inline {
                vec2(em, em)
            } else {
                vec2(em * 12.0, em * 12.0 / ratio)
            }
        }),
    };
    if size.x <= 0.0 || size.y <= 0.0 {
        return Vec2::ZERO;
    }
    let max_width = image
        .max_width
        .map(|value| resolve(value, available.x))
        .unwrap_or(available.x)
        .min(available.x);
    let max_height = image
        .max_height
        .map(|value| resolve(value, available.y))
        .unwrap_or(available.y)
        .min(available.y);
    let scale = (max_width / size.x)
        .min(max_height / size.y)
        .clamp(0.0, 1.0);
    size *= scale;
    size
}

struct FixedImage<'a> {
    image: &'a ImageRef,
    atom: Option<(usize, &'a Atom)>,
    canvas: FixedCanvas,
}

fn fixed_image(chapter: &Chapter) -> Result<FixedImage<'_>, &'static str> {
    let [width, height] = reader_core::page_document_geometry(chapter)
        .and_then(|geometry| geometry.viewport)
        .ok_or("固定レイアウトのページ寸法を取得できません。")?;
    if chapter.blocks.iter().any(|block| {
        block.table.is_some() || block.mathml.is_some() || matches!(block.kind, BlockKind::Rule)
    }) {
        return Err(
            "この固定レイアウトのHTML配置には未対応です。画像1枚またはSVGのページに対応しています。",
        );
    }
    let atoms = chapter.atoms();
    if atoms
        .iter()
        .any(|atom| atom.image.is_none() && !atom.text.trim().is_empty())
    {
        return Err(
            "文字と画像を配置した固定レイアウトHTMLには未対応です。原稿通りに表示できません。",
        );
    }
    let mut images = chapter
        .blocks
        .iter()
        .filter_map(|block| block.image.as_ref().map(|image| (image, None)))
        .chain(atoms.iter().enumerate().filter_map(|(index, atom)| {
            atom.image
                .as_deref()
                .map(|image| (image, Some((index, *atom))))
        }));
    let (image, atom) = images
        .next()
        .ok_or("この固定レイアウトには表示できる画像またはSVGがありません。")?;
    if images.next().is_some() {
        return Err(
            "複数の画像を配置した固定レイアウトHTMLには未対応です。原稿通りに表示できません。",
        );
    }
    let extent = vec2(width, height);
    let intrinsic = image
        .intrinsic_size
        .filter(|size| size.iter().all(|value| value.is_finite() && *value > 0.))
        .map(|size| vec2(size[0], size[1]));
    let resolve = |value: reader_core::CssLength, reference| value.resolve(16., 16., reference);
    let authored_width = image.width.map(|value| resolve(value, width));
    let authored_height = image.height.map(|value| resolve(value, height));
    let ratio = image
        .aspect_ratio
        .filter(|value| value.is_finite() && *value > 0.)
        .or_else(|| intrinsic.map(|size| size.x / size.y));
    let size = match (authored_width, authored_height) {
        (Some(width), Some(height)) => vec2(width, height),
        (Some(width), None) => vec2(
            width,
            width / ratio.ok_or("固定レイアウト画像の縦横比を取得できません。")?,
        ),
        (None, Some(height)) => vec2(
            height * ratio.ok_or("固定レイアウト画像の縦横比を取得できません。")?,
            height,
        ),
        (None, None) => intrinsic.ok_or("固定レイアウト画像の元サイズを取得できません。")?,
    };
    if !size.is_finite() || size.x <= 0. || size.y <= 0. {
        return Err("固定レイアウト画像のサイズが無効です。");
    }
    // Only explicit author max-* constraints apply. The viewport clips excess
    // content; it must not silently shrink an oversized image into the page.
    let max_width = image
        .max_width
        .map(|value| resolve(value, width))
        .unwrap_or(size.x);
    let max_height = image
        .max_height
        .map(|value| resolve(value, height))
        .unwrap_or(size.y);
    let scale = (max_width / size.x).min(max_height / size.y).clamp(0., 1.);
    Ok(FixedImage {
        image,
        atom,
        canvas: FixedCanvas {
            extent,
            image_rect: Rect::from_min_size(Pos2::ZERO, size * scale),
        },
    })
}

/// A fixed-layout EPUB must not silently fall back to reflowing arbitrary HTML.
/// The caller can display this diagnostic while preserving one spine item/page.
pub fn epub_fixed_layout_diagnostic(chapter: &Chapter) -> Option<String> {
    if chapter.fixed_page.is_some()
        || chapter.presentation.layout != Some(reader_core::RenditionLayout::PrePaginated)
    {
        return None;
    }
    fixed_image(chapter).err().map(str::to_owned)
}

fn epub_fixed_layout(chapter: &Chapter, extent: Vec2) -> BookLayout {
    let mut page = Page {
        anchor: chapter.start_locator(),
        ..Page::default()
    };
    if let Ok(fixed) = fixed_image(chapter) {
        let scale = (extent.x / fixed.canvas.extent.x)
            .min(extent.y / fixed.canvas.extent.y)
            .max(0.);
        let origin = (extent - fixed.canvas.extent * scale) * 0.5;
        page.pictures.push(Picture {
            rect: Rect::from_min_size(origin.to_pos2(), fixed.canvas.extent * scale),
            resource: fixed.image.resource.clone(),
            alt: fixed.image.alt.clone(),
            inline_svg: fixed.image.inline_svg.clone(),
            pdf_page: None,
            fixed_canvas: Some(fixed.canvas),
            text_image: false,
        });
        if let Some((index, atom)) = fixed.atom
            && let (Some(locator), Some(end)) =
                (atom.locator_at(0), atom.locator_at(atom.text.len()))
        {
            let rect = fixed
                .canvas
                .image_rect
                .intersect(Rect::from_min_size(Pos2::ZERO, fixed.canvas.extent));
            if rect.is_positive() {
                page.hits.push(Hit {
                    rect: Rect::from_min_max(
                        (origin + rect.min.to_vec2() * scale).to_pos2(),
                        (origin + rect.max.to_vec2() * scale).to_pos2(),
                    ),
                    locator,
                    end,
                    unit_id: unit_id(chapter.id, index),
                    offset: 0,
                    text: atom.text.clone(),
                });
            }
        }
    }
    BookLayout { pages: vec![page] }
}

/// Expand a glyph hit to a source-backed morphological word. Ruby is atomic.
pub fn expand_word(chapter: &Chapter, hit: &Hit) -> Hit {
    let Ok(Some(word)) = reader_core::word_at(chapter, hit.locator) else {
        return hit.clone();
    };
    Hit {
        locator: word.start,
        end: word.end,
        unit_id: unit_id(chapter.id, word.atom_index),
        offset: word.offset,
        text: word.text,
        ..hit.clone()
    }
}

struct Flow<'a, const EMIT: bool> {
    fonts: &'a mut FontSystem,
    settings: &'a Settings,
    extent: Vec2,
    margin: f32,
    margin_y: f32,
    heading_gap: f32,
    vertical: bool,
    x: f32,
    y: f32,
    page: Page,
    pages: Vec<Page>,
    starts: Vec<Option<Locator>>,
    counted: [usize; 4],
    first_hit: Option<Locator>,
    atom_index: usize,
    chapter: u32,
    in_cell: bool,
    line_ascent: f32,
    line_cross: f32,
    line_start: [usize; 3],
    break_hints: &'a HashMap<(u64, usize), BreakHint>,
}

#[derive(Clone, Copy)]
struct BreakHint {
    remaining: f32,
    before: bool,
}

struct AtomToken<'a> {
    text: &'a str,
    offset: usize,
    ruby: Option<&'a str>,
    tcy: bool,
}

fn atom_tokens(atom: &Atom, show_ruby: bool) -> impl Iterator<Item = AtomToken<'_>> {
    let ruby = show_ruby.then_some(atom.ruby.as_deref()).flatten();
    let grouped = ruby.is_some() || atom.style.tate_chu_yoko;
    let mut whole = grouped.then_some(AtomToken {
        text: &atom.text,
        offset: 0,
        ruby,
        tcy: atom.style.tate_chu_yoko,
    });
    let mut graphemes = atom.text.grapheme_indices(true).peekable();
    let ascii_word = |grapheme: &str| {
        grapheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_@./".contains(c))
    };
    std::iter::from_fn(move || {
        if grouped {
            return whole.take();
        }
        let (offset, grapheme) = graphemes.next()?;
        let mut end = offset + grapheme.len();
        if ascii_word(grapheme) {
            while let Some(&(next, grapheme)) = graphemes.peek() {
                if !ascii_word(grapheme) {
                    break;
                }
                end = next + grapheme.len();
                graphemes.next();
            }
        }
        Some(AtomToken {
            text: &atom.text[offset..end],
            offset,
            ruby: None,
            tcy: false,
        })
    })
}

fn cannot_start_line(character: char) -> bool {
    "、。，．・：；？！!?:;)]}）］｝〕〉》」』】〙〗〟’”｠»ぁぃぅぇぉっゃゅょゎゕゖァィゥェォッャュョヮヵヶㇰㇱㇲㇳㇴㇵㇶㇷㇸㇹㇺㇻㇼㇽㇾㇿ゛゜ヽヾゝゞ々〻ー〜～…‥".contains(character)
}

fn cannot_end_line(character: char) -> bool {
    "([{（［｛〔〈《「『【〘〖〝‘“｟«".contains(character)
}

fn closing_only_block(block: &Block) -> bool {
    matches!(block.kind, BlockKind::Paragraph | BlockKind::Quote)
        && block.image.is_none()
        && block.table.is_none()
        && block.mathml.is_none()
        && block.atoms.iter().all(|atom| atom.image.is_none())
        && block
            .atoms
            .iter()
            .flat_map(|atom| atom.text.chars())
            .any(cannot_start_line)
        && block
            .atoms
            .iter()
            .flat_map(|atom| atom.text.chars())
            .all(|c| c.is_whitespace() || cannot_start_line(c))
}

fn block_size(block: &Block, base: f32) -> f32 {
    block
        .font_size
        .map(|size| size.resolve(base, base, base))
        .unwrap_or(base)
        .clamp(1., 512.)
}

fn atom_size(atom: &Atom, fallback: f32, root: f32) -> f32 {
    atom.style
        .font_size
        .map(|size| size.resolve(root, root, root))
        .unwrap_or(fallback)
        .clamp(1., 512.)
}

fn collapsed_margin(a: f32, b: f32) -> f32 {
    if a >= 0. && b >= 0. {
        a.max(b)
    } else if a <= 0. && b <= 0. {
        a.min(b)
    } else {
        a + b
    }
}

/// Plan unbreakable groups using real shaped advances before painting. This
/// crosses inline spans (and stray closing-only paragraph wrappers) without
/// joining or changing their source atoms, ruby groups, or speech unit IDs.
fn prepare_break_hints(
    fonts: &mut FontSystem,
    blocks: &[Block],
    settings: &Settings,
    chapter: u32,
    start_atom: usize,
    vertical: bool,
) -> HashMap<(u64, usize), BreakHint> {
    #[derive(Clone, Copy)]
    struct Token<'a> {
        id: u64,
        offset: usize,
        first: char,
        last: char,
        text: &'a str,
        preferred: usize,
        size: f32,
        vertical: bool,
        tcy: bool,
    }
    fn flush_group(
        fonts: &mut FontSystem,
        settings: &Settings,
        group: &mut Vec<Token<'_>>,
        hints: &mut HashMap<(u64, usize), BreakHint>,
    ) {
        if group.len() > 1 {
            let mut remaining = 0.;
            for index in (0..group.len()).rev() {
                let token = &group[index];
                let face = fonts.face_for_text(token.preferred, token.text);
                let rotate = token.vertical
                    && !token.tcy
                    && matches!(
                        unicode_vo::char_orientation(token.first),
                        unicode_vo::Orientation::Rotated
                    );
                let glyphs = fonts.shape(face, token.text, token.vertical && !rotate && !token.tcy);
                let advance = if token.tcy {
                    token.size
                } else {
                    glyphs
                        .iter()
                        .map(|glyph| {
                            if token.vertical && !rotate {
                                glyph.advance.y
                            } else {
                                glyph.advance.x
                            }
                        })
                        .sum::<f32>()
                        * token.size
                } + settings.letter_spacing * token.size;
                remaining += if advance > 0. { advance } else { token.size };
                hints.insert(
                    (token.id, token.offset),
                    BreakHint {
                        remaining,
                        before: index == 0,
                    },
                );
            }
        }
        group.clear();
    }
    fn push_token<'a>(
        fonts: &mut FontSystem,
        settings: &Settings,
        group: &mut Vec<Token<'a>>,
        token: Token<'a>,
        hints: &mut HashMap<(u64, usize), BreakHint>,
    ) {
        if let Some(previous) = group.last()
            && !cannot_end_line(previous.last)
            && !cannot_start_line(token.first)
        {
            flush_group(fonts, settings, group, hints);
        }
        group.push(token);
    }
    fn atom_count(blocks: &[Block]) -> usize {
        blocks
            .iter()
            .map(|block| {
                block.atoms.len()
                    + block
                        .table
                        .as_ref()
                        .map(|table| {
                            table
                                .rows
                                .iter()
                                .flat_map(|row| &row.cells)
                                .map(|cell| atom_count(&cell.blocks))
                                .sum::<usize>()
                        })
                        .unwrap_or(0)
            })
            .sum()
    }
    // Only a pending unbreakable group needs storage. Ordinary characters are
    // discarded as soon as the next permitted break is known.
    let mut group = Vec::new();
    let mut hints = HashMap::new();
    let mut atom_index = start_atom;
    for block in blocks {
        if !closing_only_block(block) {
            flush_group(fonts, settings, &mut group, &mut hints);
        }
        if block.image.is_some()
            || block.table.is_some()
            || block.mathml.is_some()
            || matches!(block.kind, BlockKind::Rule)
        {
            atom_index += atom_count(std::slice::from_ref(block));
            flush_group(fonts, settings, &mut group, &mut hints);
            continue;
        }
        let vertical = vertical && !matches!(block.kind, BlockKind::Code);
        let base = block_size(block, settings.font_size);
        for atom in &block.atoms {
            let id = unit_id(chapter, atom_index);
            atom_index += 1;
            if atom.image.is_some() {
                flush_group(fonts, settings, &mut group, &mut hints);
                continue;
            }
            let preferred = if atom.style.code || matches!(block.kind, BlockKind::Code) {
                fonts::CODE_FACE
            } else {
                fonts.body_face(&settings.font_family)
            };
            let size = atom_size(atom, base, settings.font_size)
                * if atom.style.superscript || atom.style.subscript {
                    0.68
                } else {
                    1.
                };
            for token in atom_tokens(atom, settings.show_ruby) {
                if token.text.is_empty() {
                    continue;
                }
                if token.text.contains(['\n', '\t']) {
                    flush_group(fonts, settings, &mut group, &mut hints);
                    continue;
                }
                let first = token.text.chars().next().unwrap();
                let last = token.text.chars().last().unwrap();
                push_token(
                    fonts,
                    settings,
                    &mut group,
                    Token {
                        id,
                        offset: token.offset,
                        first,
                        last,
                        text: token.text,
                        preferred,
                        size,
                        vertical,
                        tcy: token.tcy,
                    },
                    &mut hints,
                );
            }
        }
    }
    flush_group(fonts, settings, &mut group, &mut hints);
    hints
}

impl<const EMIT: bool> Flow<'_, EMIT> {
    // Page output is optional, while every line-break and block-placement
    // decision runs through the same flow. Counting keeps only occupancy and
    // the first real source hit; it never constructs paint or selection data.
    fn output_sizes(&self) -> [usize; 4] {
        if EMIT {
            [
                self.page.glyphs.len(),
                self.page.hits.len(),
                self.page.pictures.len(),
                self.page.rules.len(),
            ]
        } else {
            self.counted
        }
    }

    fn page_count(&self) -> usize {
        if EMIT {
            self.pages.len()
        } else {
            self.starts.len()
        }
    }

    fn count_hit(&mut self, locator: Locator) {
        self.counted[1] += 1;
        self.first_hit.get_or_insert(locator);
    }

    fn hit(&mut self, locator: Locator, make: impl FnOnce() -> Hit) {
        if EMIT {
            self.page.hits.push(make());
        } else {
            self.count_hit(locator);
        }
    }

    fn token_source(
        &mut self,
        atom: &Atom,
        text: &str,
        offset: usize,
        whole: bool,
        glyphs: &[fonts::ShapedGlyph],
    ) {
        if whole {
            if let (Some(locator), Some(_)) = (
                atom.locator_at(offset),
                atom.locator_at(offset + text.len()),
            ) {
                self.count_hit(locator);
            }
            return;
        }
        let mut index = 0;
        while index < glyphs.len() {
            let cluster = glyphs[index].cluster;
            index += 1;
            while index < glyphs.len() && glyphs[index].cluster == cluster {
                index += 1;
            }
            let end = glyphs
                .get(index)
                .map(|glyph| glyph.cluster)
                .unwrap_or(text.len());
            if text.get(cluster..end).is_some()
                && let (Some(locator), Some(_)) = (
                    atom.locator_at(offset + cluster),
                    atom.locator_at(offset + end),
                )
            {
                // Subsequent hits cannot change this page's first source, and
                // occupancy only distinguishes an empty line from a nonempty one.
                self.count_hit(locator);
                break;
            }
        }
    }

    fn picture(&mut self, image: &ImageRef, rect: Rect) {
        if !EMIT {
            self.counted[2] += 1;
            return;
        }
        self.page.pictures.push(Picture {
            rect,
            resource: image.resource.clone(),
            alt: image.alt.clone(),
            inline_svg: image.inline_svg.clone(),
            pdf_page: image.pdf_page.clone(),
            fixed_canvas: None,
            text_image: image.display == ImageDisplay::Inline
                || (image.display == ImageDisplay::Block
                    && (image
                        .alt
                        .contains(['∫', '∑', '√', '∂', '∇', '∞', '=', '^', '\\'])
                        || image.title.as_deref().is_some_and(|title| {
                            title.contains("数式")
                                || title.to_ascii_lowercase().contains("equation")
                        }))),
        });
    }

    fn block_image(&mut self, image: &ImageRef, base: f32, _block_id: u32) {
        let size = image_size(
            image,
            base,
            self.settings.font_size,
            vec2(self.right() - self.margin, self.bottom() - self.margin_y),
        );
        if size.x <= 0.0 || size.y <= 0.0 {
            return;
        }
        if (!self.vertical && self.x > self.margin)
            || self.output_sizes()[0] != 0
            || self.output_sizes()[2] != 0
        {
            self.newline(base);
        }
        let rect = if self.vertical {
            if self.x + base - size.x < self.margin {
                self.fresh_page();
            }
            Rect::from_min_size(pos2(self.x + base - size.x, self.margin_y), size)
        } else {
            if self.y + size.y > self.bottom() && self.y > self.margin_y {
                self.fresh_page();
            }
            Rect::from_min_size(
                pos2(
                    self.margin + ((self.right() - self.margin) - size.x) * 0.5,
                    self.y,
                ),
                size,
            )
        };
        if self.output_sizes()[1] == 0 && self.page.anchor.is_none() {
            self.page.anchor = image.source.map(|source| source.start_locator());
        }
        self.picture(image, rect);
        if self.vertical {
            self.x = rect.left();
            self.y = self.margin_y;
        } else {
            self.x = self.margin;
            self.y = rect.bottom() + base * 0.5;
        }
        self.reset_line();
    }

    fn inline_image(&mut self, atom: &Atom, image: &ImageRef, base: f32, face: usize, id: u64) {
        let size = image_size(
            image,
            base,
            self.settings.font_size,
            vec2(self.right() - self.margin, self.bottom() - self.margin_y),
        );
        if size.x <= 0.0 || size.y <= 0.0 {
            return;
        }
        if image.display != ImageDisplay::Inline || (self.vertical && size.x > base * 2.0) {
            self.block_image(image, base, 0);
            self.newline(base);
            return;
        }
        let rect = if self.vertical {
            if self.y + size.y > self.bottom() {
                self.newline(base);
            }
            self.line_cross = self.line_cross.max(size.x);
            Rect::from_min_size(pos2(self.x + (base - size.x) * 0.5, self.y), size)
        } else {
            if self.x + size.x > self.right() {
                self.newline(base);
            }
            let mut offset = image
                .baseline_offset
                .map(|length| length.resolve(base, self.settings.font_size, base))
                .unwrap_or(0.0);
            if atom.style.superscript || image.vertical_align == ImageAlign::Super {
                offset += base * 0.35;
            } else if atom.style.subscript || image.vertical_align == ImageAlign::Sub {
                offset -= base * 0.2;
            }
            let ascent = match image.vertical_align {
                ImageAlign::Middle => size.y * 0.5 + base * 0.38,
                ImageAlign::Top => base * 0.88,
                ImageAlign::Bottom => size.y - base * 0.12,
                _ => size.y,
            } + offset;
            if self.y + ascent.max(self.line_ascent) + base * 0.27 > self.bottom()
                && self.y > self.margin_y
            {
                self.newline(base);
            }
            self.raise_baseline(ascent.max(base * 0.88));
            let top = match image.vertical_align {
                ImageAlign::Middle => self.y + self.line_ascent - base * 0.38 - size.y * 0.5,
                ImageAlign::Top => self.y,
                ImageAlign::Bottom => self.y + self.line_ascent + base * 0.12 - size.y,
                _ => self.y + self.line_ascent - size.y,
            } - offset;
            let rect = Rect::from_min_size(pos2(self.x, top), size);
            self.line_cross = self.line_cross.max(rect.bottom() - self.y);
            rect
        };
        self.picture(image, rect);
        if let (Some(locator), Some(end)) = (atom.locator_at(0), atom.locator_at(atom.text.len())) {
            self.hit(locator, || Hit {
                rect,
                locator,
                end,
                unit_id: id,
                offset: 0,
                text: atom.text.clone(),
            });
        }
        if EMIT
            && self.settings.show_ruby
            && let Some(ruby) = atom.ruby.as_ref().filter(|ruby| !ruby.is_empty())
        {
            let glyphs = self.fonts.shape(face, ruby, self.vertical);
            let advance: f32 = glyphs
                .iter()
                .map(|glyph| {
                    if self.vertical {
                        glyph.advance.y
                    } else {
                        glyph.advance.x
                    }
                })
                .sum();
            let extent = if self.vertical {
                rect.height()
            } else {
                rect.width()
            };
            let ruby_size = (base * self.settings.ruby_scale).min(extent / advance.max(0.01));
            let mut pen = Vec2::ZERO;
            for glyph in glyphs.iter() {
                let origin = if self.vertical {
                    pos2(
                        rect.right() + ruby_size * 0.52,
                        rect.top() + (extent - advance * ruby_size) * 0.5,
                    )
                } else {
                    pos2(
                        rect.left() + (extent - advance * ruby_size) * 0.5,
                        rect.top() - ruby_size * 0.12,
                    )
                };
                self.page.glyphs.push(Glyph {
                    face,
                    id: glyph.id,
                    origin: origin + pen + glyph.offset * ruby_size,
                    size: ruby_size,
                    rotated: false,
                    style: fonts::GlyphStyle::default(),
                });
                pen += glyph.advance * ruby_size;
            }
        }
        if self.vertical {
            self.y += size.y + self.settings.letter_spacing * base;
        } else {
            self.x += size.x + self.settings.letter_spacing * base;
        }
    }

    fn reset_line(&mut self) {
        self.line_ascent = 0.0;
        self.line_cross = 0.0;
        self.line_start = [
            self.output_sizes()[0],
            self.output_sizes()[1],
            self.output_sizes()[2],
        ];
    }
    fn raise_baseline(&mut self, ascent: f32) {
        if self.vertical || ascent <= self.line_ascent {
            return;
        }
        if !EMIT {
            self.line_ascent = ascent;
            return;
        }
        let shift = vec2(0.0, ascent - self.line_ascent);
        for glyph in &mut self.page.glyphs[self.line_start[0]..] {
            glyph.origin += shift;
        }
        for hit in &mut self.page.hits[self.line_start[1]..] {
            hit.rect = hit.rect.translate(shift);
        }
        for picture in &mut self.page.pictures[self.line_start[2]..] {
            picture.rect = picture.rect.translate(shift);
        }
        self.line_ascent = ascent;
    }
    fn bottom(&self) -> f32 {
        self.extent.y - self.margin_y
    }
    fn right(&self) -> f32 {
        self.extent.x - self.margin
    }
    fn fresh_page(&mut self) {
        if self.output_sizes()[0] != 0 || self.output_sizes()[2] != 0 || self.output_sizes()[3] != 0
        {
            if EMIT && !self.in_cell {
                self.page.balance_grid(
                    self.fonts,
                    self.extent,
                    vec2(self.margin, self.margin_y),
                    self.settings,
                    self.vertical,
                );
            }
            if EMIT {
                self.pages.push(std::mem::take(&mut self.page));
            } else {
                self.starts.push(self.page.anchor.or(self.first_hit));
                self.page.anchor = None;
                self.first_hit = None;
                self.counted = [0; 4];
            }
        }
        self.x = if self.vertical {
            self.right() - self.settings.font_size
        } else {
            self.margin
        };
        self.y = self.margin_y;
        self.heading_gap = 0.;
        self.reset_line();
    }
    fn newline(&mut self, size: f32) {
        if self.vertical {
            self.x -= (size * self.settings.line_spacing).max(self.line_cross);
            self.y = self.margin_y;
            if self.x < self.margin {
                self.fresh_page();
            }
        } else {
            self.x = self.margin;
            self.y += (size * self.settings.line_spacing)
                .max(self.line_cross)
                .max(self.line_ascent + size * 0.27);
            if self.y + size > self.bottom() {
                self.fresh_page();
            }
        }
        self.reset_line();
    }
    fn atom(&mut self, atom: &Atom, size: f32, face: usize) {
        let size = atom_size(atom, size, self.settings.font_size);
        let id = unit_id(self.chapter, self.atom_index);
        self.atom_index += 1;
        if let Some(image) = &atom.image {
            self.inline_image(atom, image, size, face, id);
            return;
        }
        if atom.text.is_empty() {
            return;
        }
        // Keep script glyphs attached to this line's baseline while letting
        // their real reduced advance determine following text positions.
        let size = if atom.style.superscript || atom.style.subscript {
            size * 0.68
        } else {
            size
        };
        for token in atom_tokens(atom, self.settings.show_ruby) {
            self.token(
                atom,
                token.text,
                token.offset,
                size,
                face,
                id,
                token.ruby,
                token.tcy,
            );
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn token(
        &mut self,
        atom: &Atom,
        text: &str,
        offset: usize,
        size: f32,
        face: usize,
        id: u64,
        ruby: Option<&str>,
        tcy: bool,
    ) {
        let base_size = if atom.style.superscript || atom.style.subscript {
            size / 0.68
        } else {
            size
        };
        if text == "\t" && !self.vertical {
            let stop = self
                .fonts
                .shape(fonts::CODE_FACE, " ", false)
                .iter()
                .map(|g| g.advance.x)
                .sum::<f32>()
                * size
                * 4.0;
            let advance = (stop - (self.x - self.margin) % stop).max(0.0);
            if self.x + advance > self.right() {
                self.newline(base_size);
            }
            let rect = Rect::from_min_size(pos2(self.x, self.y), vec2(advance, size * 1.15));
            if let (Some(locator), Some(end)) =
                (atom.locator_at(offset), atom.locator_at(offset + 1))
            {
                self.hit(locator, || Hit {
                    rect,
                    locator,
                    end,
                    unit_id: id,
                    offset,
                    text: "\t".into(),
                });
            }
            self.x += advance;
            return;
        }
        if text.contains('\n') {
            for (i, line) in text.split('\n').enumerate() {
                if i > 0 {
                    self.newline(base_size);
                }
                if !line.is_empty() {
                    self.token(atom, line, offset, size, face, id, None, false);
                }
            }
            return;
        }
        let face = self.fonts.face_for_text(face, text);
        let first = text.chars().next().unwrap_or(' ');
        let rotate = self.vertical
            && !tcy
            && matches!(
                unicode_vo::char_orientation(first),
                unicode_vo::Orientation::Rotated
            );
        let glyphs = self
            .fonts
            .shape(face, text, self.vertical && !rotate && !tcy);
        let mut advance = glyphs
            .iter()
            .map(|g| {
                if self.vertical && !rotate && !tcy {
                    g.advance.y
                } else {
                    g.advance.x
                }
            })
            .sum::<f32>()
            * size;
        if tcy {
            advance = size;
        }
        advance += self.settings.letter_spacing * size;
        if advance <= 0.0 {
            advance = size;
        }
        let available = if self.vertical {
            self.bottom() - self.y
        } else {
            self.right() - self.x
        };
        let hint = self.break_hints.get(&(id, offset));
        let need = hint.map_or(advance, |hint| hint.remaining);
        let can_break = hint.is_none_or(|hint| hint.before);
        let line_has_content = self.output_sizes()[0] > self.line_start[0]
            || self.output_sizes()[1] > self.line_start[1]
            || self.output_sizes()[2] > self.line_start[2];
        if need > available + 0.01 && can_break && line_has_content {
            self.newline(base_size);
        }
        // Keep arbitrarily long groups on the page instead of letting them escape its bounds.
        let max_advance = if self.vertical {
            self.bottom() - self.margin_y
        } else {
            self.right() - self.margin
        };
        if advance > max_advance && text.graphemes(true).count() > 1 && ruby.is_none() {
            for (i, c) in text.grapheme_indices(true) {
                self.token(atom, c, offset + i, size, face, id, None, false);
            }
            return;
        }
        if !self.vertical {
            self.raise_baseline(base_size * 0.88);
        }
        if !EMIT {
            let mut has_glyph = !glyphs.is_empty();
            // Ruby and emphasis never alter the flow advance. Only their
            // presence can make an otherwise empty output page nonempty.
            if !has_glyph && let Some(ruby) = ruby.filter(|text| !text.is_empty()) {
                has_glyph = !self.fonts.shape(face, ruby, self.vertical).is_empty();
            }
            if !has_glyph && atom.style.emphasis && text.chars().any(char::is_alphanumeric) {
                has_glyph = !self.fonts.shape(face, "・", self.vertical).is_empty();
            }
            self.counted[0] += usize::from(has_glyph);
            self.token_source(
                atom,
                text,
                offset,
                ruby.is_some() || tcy || glyphs.is_empty(),
                &glyphs,
            );
            self.counted[3] += usize::from(atom.style.strike);
            if self.vertical {
                self.y += advance;
            } else {
                self.x += advance;
            }
            return;
        }
        let start = pos2(self.x, self.y);
        let script_shift = if atom.style.superscript {
            if self.vertical {
                vec2(base_size * 0.32, 0.0)
            } else {
                Vec2::ZERO
            }
        } else if atom.style.subscript {
            if self.vertical {
                vec2(-base_size * 0.12, 0.0)
            } else {
                vec2(0.0, base_size * 0.38)
            }
        } else {
            Vec2::ZERO
        } + if self.vertical {
            Vec2::ZERO
        } else {
            vec2(0.0, self.line_ascent - base_size * 0.88)
        };
        let bounds = Rect::from_min_size(
            start + script_shift,
            if self.vertical {
                vec2(size, advance)
            } else {
                vec2(advance, size * 1.15)
            },
        );
        let mut pen = Vec2::ZERO;
        let scale_tcy = if tcy {
            (size / (glyphs.iter().map(|g| g.advance.x).sum::<f32>() * size)).min(1.0)
        } else {
            1.0
        };
        for glyph in glyphs.iter() {
            let glyph_size = size * scale_tcy;
            let origin = if self.vertical && !rotate && !tcy {
                pos2(self.x + size * 0.5, self.y) + pen + glyph.offset * size
            } else if rotate {
                pos2(self.x + size * 0.08, self.y + pen.x)
                    + vec2(-glyph.offset.y, glyph.offset.x) * size
            } else if tcy && self.vertical {
                let width = glyphs.iter().map(|g| g.advance.x).sum::<f32>() * glyph_size;
                pos2(
                    self.x + (size - width) * 0.5,
                    self.y + (size - glyph_size) * 0.5 + glyph_size * 0.88,
                ) + pen * scale_tcy
                    + glyph.offset * glyph_size
            } else {
                pos2(self.x, self.y + size * 0.88) + pen * scale_tcy + glyph.offset * glyph_size
            };
            self.page.glyphs.push(Glyph {
                face,
                id: glyph.id,
                origin: origin + script_shift,
                size: glyph_size,
                rotated: rotate,
                style: fonts::GlyphStyle {
                    bold: atom.style.bold,
                    italic: atom.style.italic,
                },
            });
            pen += glyph.advance * size;
        }
        if let Some(ruby) = ruby.filter(|r| !r.is_empty()) {
            let mut ruby_size = size * self.settings.ruby_scale;
            let ruby_glyphs = self.fonts.shape(face, ruby, self.vertical);
            let mut ruby_extent = ruby_glyphs
                .iter()
                .map(|g| {
                    if self.vertical {
                        g.advance.y
                    } else {
                        g.advance.x
                    }
                })
                .sum::<f32>()
                * ruby_size;
            if ruby_extent > advance {
                ruby_size *= advance / ruby_extent;
                ruby_extent = advance;
            }
            let mut ruby_pen = Vec2::ZERO;
            for glyph in ruby_glyphs.iter() {
                let origin = if self.vertical {
                    pos2(
                        self.x + size + ruby_size * 0.52,
                        self.y + (advance - ruby_extent).max(0.0) * 0.5,
                    ) + ruby_pen
                        + glyph.offset * ruby_size
                } else {
                    pos2(
                        self.x + (advance - ruby_extent).max(0.0) * 0.5,
                        self.y - ruby_size * 0.12,
                    ) + ruby_pen
                        + glyph.offset * ruby_size
                };
                self.page.glyphs.push(Glyph {
                    face,
                    id: glyph.id,
                    origin: origin + script_shift,
                    size: ruby_size,
                    rotated: false,
                    style: fonts::GlyphStyle::default(),
                });
                ruby_pen += glyph.advance * ruby_size;
            }
        }
        if ruby.is_some() || tcy || glyphs.is_empty() {
            if let (Some(locator), Some(end)) = (
                atom.locator_at(offset),
                atom.locator_at(offset + text.len()),
            ) {
                self.page.hits.push(Hit {
                    rect: bounds,
                    locator,
                    end,
                    unit_id: id,
                    offset,
                    text: text.to_owned(),
                });
            }
        } else {
            let mut clusters: Vec<(usize, f32)> = Vec::new();
            for glyph in glyphs.iter() {
                let width = if self.vertical && !rotate {
                    glyph.advance.y
                } else {
                    glyph.advance.x
                } * size;
                if let Some(last) = clusters.last_mut().filter(|last| last.0 == glyph.cluster) {
                    last.1 += width;
                } else {
                    clusters.push((glyph.cluster, width));
                }
            }
            let mut along = 0.0;
            for (index, &(cluster, width)) in clusters.iter().enumerate() {
                let end = clusters.get(index + 1).map(|c| c.0).unwrap_or(text.len());
                if let Some(cluster_text) = text.get(cluster..end)
                    && let (Some(locator), Some(end_locator)) = (
                        atom.locator_at(offset + cluster),
                        atom.locator_at(offset + end),
                    )
                {
                    let rect = if self.vertical {
                        Rect::from_min_size(
                            start + script_shift + vec2(0.0, along),
                            vec2(size, width.max(size * 0.12)),
                        )
                    } else {
                        Rect::from_min_size(
                            start + script_shift + vec2(along, 0.0),
                            vec2(width.max(size * 0.12), size * 1.15),
                        )
                    };
                    self.page.hits.push(Hit {
                        rect,
                        locator,
                        end: end_locator,
                        unit_id: id,
                        offset: offset + cluster,
                        text: cluster_text.to_owned(),
                    });
                }
                along += width;
            }
        }
        if atom.style.strike {
            self.page.rules.push(if self.vertical {
                Rect::from_min_size(
                    start + script_shift + vec2(size * 0.5, 0.0),
                    vec2(0.7, advance),
                )
            } else {
                Rect::from_min_size(
                    start + script_shift + vec2(0.0, size * 0.54),
                    vec2(advance, 0.7),
                )
            });
        }
        if atom.style.emphasis {
            let mark_size = size * 0.3;
            let mark = self.fonts.shape(face, "・", self.vertical);
            let count = text.graphemes(true).count().max(1);
            for (index, grapheme) in text.graphemes(true).enumerate() {
                if !grapheme.chars().any(char::is_alphanumeric) {
                    continue;
                }
                let along = advance * (index as f32 + 0.5) / count as f32;
                let ruby_clearance = if ruby.is_some() {
                    size * self.settings.ruby_scale
                } else {
                    0.0
                };
                for glyph in mark.iter() {
                    let origin = if self.vertical {
                        start
                            + script_shift
                            + vec2(
                                size + ruby_clearance + mark_size * 0.6,
                                along - mark_size * 0.5,
                            )
                    } else {
                        start
                            + script_shift
                            + vec2(along - mark_size * 0.5, -ruby_clearance - mark_size * 0.2)
                    };
                    self.page.glyphs.push(Glyph {
                        face,
                        id: glyph.id,
                        origin: origin + glyph.offset * mark_size,
                        size: mark_size,
                        rotated: false,
                        style: fonts::GlyphStyle::default(),
                    });
                }
            }
        }
        if self.vertical {
            self.y += advance;
        } else {
            self.x += advance;
        }
    }

    fn formula(&mut self, mathml: &str, block: &Block) -> bool {
        let Ok(root) = formulary::parse(mathml) else {
            return false;
        };
        let Ok(font) = formulary::MathFont::new(fonts::MATH, 0) else {
            return false;
        };
        let formula = formulary::layout(
            &root,
            &font,
            &formulary::LayoutOptions {
                font_size: self.settings.font_size,
            },
        );
        if formula.width <= 0.0 || !formula.width.is_finite() {
            return false;
        }
        let old_vertical = self.vertical;
        if old_vertical {
            self.fresh_page();
            self.vertical = false;
            self.x = self.margin;
        } else if self.x > self.margin {
            self.newline(self.settings.font_size);
        }
        let width = formula.width + 4.0;
        let height = formula.ascent + formula.descent + 4.0;
        let scale = ((self.right() - self.margin) / width).min(1.0);
        if self.y + height * scale > self.bottom() {
            self.fresh_page();
        }
        let rect = Rect::from_min_size(
            pos2(
                self.margin + ((self.right() - self.margin) - width * scale) * 0.5,
                self.y,
            ),
            vec2(width, height) * scale,
        );
        if EMIT {
            let palette = crate::theme::Palette::for_settings(self.settings);
            let color = format!(
                "fill=\"#{:02x}{:02x}{:02x}\"",
                palette.ink.r(),
                palette.ink.g(),
                palette.ink.b()
            );
            let svg = formulary::svg::to_svg(&formula, &font).replacen("fill=\"black\"", &color, 1);
            self.page.pictures.push(Picture {
                rect,
                resource: format!(
                    "math:{}:{}:{:.2}:{:?}",
                    self.chapter, block.id, self.settings.font_size, self.settings.theme
                ),
                alt: block.plain_text(),
                inline_svg: Some(svg),
                pdf_page: None,
                fixed_canvas: None,
                text_image: false,
            });
        } else {
            self.counted[2] += 1;
        }
        if let (Some(start), Some(end)) = (
            block.atoms.iter().find_map(|atom| atom.locator_at(0)),
            block
                .atoms
                .iter()
                .rev()
                .find_map(|atom| atom.locator_at(atom.text.len())),
        ) {
            let id = unit_id(self.chapter, self.atom_index);
            self.hit(start, || Hit {
                rect,
                locator: start,
                end,
                unit_id: id,
                offset: 0,
                text: block.plain_text(),
            });
        }
        self.atom_index += block.atoms.len();
        self.y = rect.bottom() + self.settings.font_size * 0.5;
        self.x = self.margin;
        if old_vertical {
            self.fresh_page();
            self.vertical = true;
            self.x = self.right() - self.settings.font_size;
        }
        true
    }

    fn table(&mut self, table: &reader_core::Table) {
        if table.rows.is_empty() {
            return;
        }
        let base = self.settings.font_size;
        let old_vertical = self.vertical;
        if old_vertical {
            self.fresh_page();
            self.vertical = false;
            self.x = self.margin;
        } else if self.x > self.margin || self.output_sizes()[0] != 0 {
            self.newline(base);
        }
        struct Cell {
            row: usize,
            col: usize,
            rowspan: usize,
            colspan: usize,
            header: bool,
            page: Page,
            height: f32,
        }
        let mut occupied: Vec<Vec<bool>> = vec![Vec::new(); table.rows.len()];
        let mut locations = Vec::new();
        let mut columns = 0;
        for (row, source_row) in table.rows.iter().enumerate() {
            let mut column = 0;
            for source in &source_row.cells {
                let colspan = source.colspan.max(1) as usize;
                let rowspan = source.rowspan.max(1) as usize;
                while occupied[row].get(column).copied().unwrap_or(false) {
                    column += 1;
                }
                while (column..column + colspan)
                    .any(|col| occupied[row].get(col).copied().unwrap_or(false))
                {
                    column += 1;
                }
                occupied.resize_with(occupied.len().max(row + rowspan), Vec::new);
                for occupancy in &mut occupied[row..row + rowspan] {
                    occupancy.resize(occupancy.len().max(column + colspan), false);
                    occupancy[column..column + colspan].fill(true);
                }
                locations.push((
                    row,
                    column,
                    rowspan,
                    colspan,
                    source_row.header || source.header,
                    source,
                ));
                columns = columns.max(column + colspan);
                column += colspan;
            }
        }
        if columns == 0 {
            return;
        }
        let available = self.right() - self.margin;
        let padding = (base * 0.35).max(4.0);
        let width = available.max(columns as f32 * (base * 3.0 + padding * 2.0));
        let column_width = width / columns as f32;
        let width_scale = available / width;
        let mut row_heights = vec![base * 1.6; occupied.len()];
        let mut cells = Vec::new();
        for (row, col, rowspan, colspan, header, source) in locations {
            let mut settings = self.settings.clone();
            settings.font_size = base * 0.86;
            settings.line_spacing = settings.line_spacing.max(1.4);
            settings.reading_mode = ReadingMode::Horizontal;
            if header {
                settings.font_family = "Noto Sans CJK JP".into();
            }
            let cell_hints = prepare_break_hints(
                self.fonts,
                &source.blocks,
                &settings,
                self.chapter,
                self.atom_index,
                false,
            );
            let mut sub = Flow::<true> {
                fonts: self.fonts,
                settings: &settings,
                extent: vec2(column_width * colspan as f32 - padding * 2.0, 1_000_000.0),
                margin: 0.0,
                margin_y: 0.0,
                heading_gap: 0.0,
                vertical: false,
                x: 0.0,
                y: 0.0,
                page: Page::default(),
                pages: Vec::new(),
                starts: Vec::new(),
                counted: [0; 4],
                first_hit: None,
                atom_index: self.atom_index,
                chapter: self.chapter,
                in_cell: true,
                line_ascent: 0.0,
                line_cross: 0.0,
                line_start: [0; 3],
                break_hints: &cell_hints,
            };
            for block in &source.blocks {
                sub.block(block);
            }
            let height = sub.y + settings.font_size * settings.line_spacing + padding * 2.0;
            self.atom_index = sub.atom_index;
            if rowspan == 1 {
                row_heights[row] = row_heights[row].max(height);
            }
            cells.push(Cell {
                row,
                col,
                rowspan,
                colspan,
                header,
                page: sub.page,
                height,
            });
        }
        // A spanning cell contributes only the extra height its row group needs.
        for cell in &cells {
            let available_height: f32 = row_heights[cell.row..cell.row + cell.rowspan].iter().sum();
            if cell.height > available_height {
                let extra = (cell.height - available_height) / cell.rowspan as f32;
                for height in &mut row_heights[cell.row..cell.row + cell.rowspan] {
                    *height += extra;
                }
            }
        }
        let mut row_offsets = vec![0.0; row_heights.len() + 1];
        for (index, height) in row_heights.iter().enumerate() {
            row_offsets[index + 1] = row_offsets[index] + height;
        }
        let header_rows = table.rows.iter().take_while(|row| row.header).count();
        let repeat_headers = header_rows > 0
            && !cells
                .iter()
                .any(|cell| cell.row < header_rows && cell.row + cell.rowspan > header_rows);
        let margin = self.margin;
        let draw_group = |flow: &mut Self, start: usize, end: usize, y: f32, scale: f32| {
            for cell in cells
                .iter()
                .filter(|cell| cell.row >= start && cell.row < end)
            {
                let rect = Rect::from_min_size(
                    pos2(
                        margin + cell.col as f32 * column_width * scale,
                        y + (row_offsets[cell.row] - row_offsets[start]) * scale,
                    ),
                    vec2(
                        column_width * cell.colspan as f32,
                        row_offsets[cell.row + cell.rowspan] - row_offsets[cell.row],
                    ) * scale,
                );
                if !EMIT {
                    flow.counted[0] += cell.page.glyphs.len();
                    flow.counted[2] += cell.page.pictures.len();
                    flow.counted[3] += 1 + cell.page.rules.len();
                    if let Some(hit) = cell.page.hits.first() {
                        flow.count_hit(hit.locator);
                    }
                    continue;
                }
                if cell.header {
                    flow.page.header_fills.push(rect);
                }
                flow.page.rules.push(rect);
                flow.page.append_at(
                    cell.page.clone(),
                    rect.min.to_vec2() + vec2(padding, padding) * scale,
                    scale,
                );
            }
        };
        let mut row = 0;
        while row < row_heights.len() {
            // Never split a rowspan across a page boundary.
            let mut end = row + 1;
            loop {
                let expanded = cells
                    .iter()
                    .filter(|cell| cell.row < end && cell.row + cell.rowspan > row)
                    .map(|cell| cell.row + cell.rowspan)
                    .max()
                    .unwrap_or(end)
                    .max(end);
                if expanded == end {
                    break;
                }
                end = expanded;
            }
            let natural_height = row_offsets[end] - row_offsets[row];
            let full_height = self.bottom() - self.margin_y;
            let scale = width_scale.min(full_height / natural_height.max(1.0));
            let height = natural_height * scale;
            let mut repeat = false;
            if self.y + height > self.bottom() {
                self.fresh_page();
                repeat = repeat_headers && row >= header_rows;
            }
            let header_height = if repeat {
                row_offsets[header_rows] * scale
            } else {
                0.0
            };
            let effective = if height + header_height > full_height {
                scale * full_height / (height + header_height)
            } else {
                scale
            };
            let header_height = if repeat {
                row_offsets[header_rows] * effective
            } else {
                0.0
            };
            let y = self.y;
            draw_group(self, row, end, y + header_height, effective);
            // Put repeated-header hits after body hits so page bookmarks resume
            // at the new row rather than repeatedly targeting the first header.
            if repeat {
                draw_group(self, 0, header_rows, y, effective);
            }
            self.y += (natural_height * effective) + header_height;
            self.x = self.margin;
            row = end;
        }
        self.y += base * 0.5;
        if old_vertical {
            self.fresh_page();
            self.vertical = true;
            self.x = self.right() - base;
        }
    }

    fn block(&mut self, block: &Block) {
        let base = self.settings.font_size;
        if let Some(mathml) = &block.mathml
            && self.formula(mathml, block)
        {
            return;
        }
        if let Some(image) = &block.image {
            self.block_image(image, base, block.id);
            self.atom_index += block.atoms.len();
            return;
        }
        if let Some(table) = &block.table {
            self.table(table);
            return;
        }
        if matches!(block.kind, BlockKind::Rule) {
            self.newline(base);
            return;
        }
        let size = block_size(block, base);
        let face = if matches!(block.kind, BlockKind::Code) {
            fonts::CODE_FACE
        } else {
            self.fonts.body_face(&self.settings.font_family)
        };
        let continue_closing = closing_only_block(block);
        if self.vertical && self.output_sizes()[0] == 0 && self.output_sizes()[2] == 0 {
            // A large authored/default heading starts inside the page's right
            // edge; its CSS size must not extend beyond a body-sized first cell.
            self.x = self.right() - size;
        }
        let reference = if self.vertical {
            self.bottom() - self.margin_y
        } else {
            self.right() - self.margin
        };
        let start_margin = block
            .margin_block_start
            .map(|length| length.resolve(size, base, reference))
            .unwrap_or(0.);
        if (self.output_sizes()[0] != 0 || self.output_sizes()[2] != 0) && !continue_closing {
            let heading_gap = collapsed_margin(std::mem::take(&mut self.heading_gap), start_margin);
            let old_page = self.page_count();
            self.newline(size);
            if old_page == self.page_count() && heading_gap != 0. {
                if self.vertical {
                    self.x -= heading_gap;
                    if self.x < self.margin {
                        self.fresh_page();
                    }
                } else {
                    self.y += heading_gap;
                    if self.y + size > self.bottom() {
                        self.fresh_page();
                    }
                }
            }
        } else if !continue_closing && start_margin != 0. {
            if self.vertical {
                self.x -= start_margin;
            } else {
                self.y += start_margin;
            }
        }
        if let Some(indent) = block.text_indent.filter(|_| !continue_closing) {
            let reference = if self.vertical {
                self.bottom() - self.margin_y
            } else {
                self.right() - self.margin
            };
            let indent = indent.resolve(size, self.settings.font_size, reference);
            let indent = if indent.is_finite() { indent } else { 0. };
            if self.vertical {
                self.y += indent;
            } else {
                self.x += indent;
            }
        }
        let old_vertical = self.vertical;
        if matches!(block.kind, BlockKind::Code) && self.vertical {
            self.fresh_page();
            self.vertical = false;
            self.x = self.margin;
        }
        if let Some(marker) = &block.list_marker {
            let marker_atom = Atom {
                text: format!("{marker} "),
                ..Atom::default()
            };
            // A marker is visual punctuation, not a fabricated source/speech unit.
            let old_index = self.atom_index;
            self.atom(&marker_atom, size, face);
            self.atom_index = old_index;
        }
        for atom in &block.atoms {
            self.atom(
                atom,
                size,
                if atom.style.code {
                    fonts::CODE_FACE
                } else {
                    face
                },
            );
        }
        if self.vertical != old_vertical {
            self.fresh_page();
            self.vertical = old_vertical;
            self.x = self.right() - base;
        }
        self.heading_gap = block
            .margin_block_end
            .map(|length| length.resolve(size, base, reference))
            .unwrap_or(0.);
    }
}

pub fn chapter_layout(
    fonts: &mut FontSystem,
    chapter: &Chapter,
    extent: Vec2,
    settings: &Settings,
) -> BookLayout {
    chapter_output::<true>(fonts, chapter, extent, settings).0
}

/// Exact page starts using the rendering flow, without producing paint or hit
/// arrays. No character ratio or separate approximate paginator is involved.
pub fn chapter_page_starts(
    fonts: &mut FontSystem,
    chapter: &Chapter,
    extent: Vec2,
    settings: &Settings,
) -> Vec<Option<Locator>> {
    chapter_output::<false>(fonts, chapter, extent, settings).1
}

fn chapter_output<const EMIT: bool>(
    fonts: &mut FontSystem,
    chapter: &Chapter,
    extent: Vec2,
    settings: &Settings,
) -> (BookLayout, Vec<Option<Locator>>) {
    if !EMIT && reader_core::page_document_geometry(chapter).is_some() {
        return (BookLayout::default(), vec![chapter.start_locator()]);
    }
    fonts.begin_layout(&settings.font_family);
    let vertical = match settings.reading_mode {
        ReadingMode::Vertical => true,
        ReadingMode::Horizontal => false,
        ReadingMode::Auto => chapter.writing_mode.is_vertical(),
    };
    let margin = settings
        .horizontal_margin()
        .min((extent.x * 0.18).max(12.0));
    let margin_y = settings.vertical_margin().min((extent.y * 0.18).max(12.0));
    if let Some(fixed) = &chapter.fixed_page {
        let scale = ((extent.x - margin * 2.0) / fixed.width)
            .min((extent.y - margin_y * 2.0) / fixed.height);
        let origin = pos2(
            (extent.x - fixed.width * scale) * 0.5,
            (extent.y - fixed.height * scale) * 0.5,
        );
        let mut page = Page::default();
        if fixed.chars.is_empty() {
            page.anchor = Some(Locator {
                chapter: chapter.id,
                node: u32::MAX,
                offset: 0,
            });
        }
        if let Some(image) = chapter.blocks.iter().find_map(|block| block.image.as_ref()) {
            page.pictures.push(Picture {
                rect: Rect::from_min_size(origin, vec2(fixed.width, fixed.height) * scale),
                resource: image.resource.clone(),
                alt: image.alt.clone(),
                inline_svg: image.inline_svg.clone(),
                pdf_page: image.pdf_page.clone(),
                fixed_canvas: None,
                text_image: false,
            });
        }
        for character in &fixed.chars {
            let [left, top, right, bottom] = character.rect;
            page.hits.push(Hit {
                rect: Rect::from_min_max(
                    origin + vec2(left, top) * scale,
                    origin + vec2(right, bottom) * scale,
                ),
                locator: character.source.start_locator(),
                end: character.source.end_locator(),
                unit_id: unit_id(chapter.id, 0),
                offset: character.source.start as usize,
                text: character.text.clone(),
            });
        }
        return (BookLayout { pages: vec![page] }, Vec::new());
    }
    if reader_core::page_document_geometry(chapter).is_some() {
        return (epub_fixed_layout(chapter, extent), Vec::new());
    }
    let break_hints =
        prepare_break_hints(fonts, &chapter.blocks, settings, chapter.id, 0, vertical);
    let mut flow = Flow::<EMIT> {
        fonts,
        settings,
        extent,
        margin,
        margin_y,
        heading_gap: 0.,
        vertical,
        x: 0.0,
        y: margin_y,
        page: Page::default(),
        pages: Vec::new(),
        starts: Vec::new(),
        counted: [0; 4],
        first_hit: None,
        atom_index: 0,
        chapter: chapter.id,
        in_cell: false,
        line_ascent: 0.0,
        line_cross: 0.0,
        line_start: [0; 3],
        break_hints: &break_hints,
    };
    flow.fresh_page();
    for block in &chapter.blocks {
        flow.block(block);
    }
    flow.fresh_page();
    if EMIT {
        if flow.pages.is_empty() {
            flow.pages.push(Page::default());
        }
    } else if flow.starts.is_empty() {
        flow.starts.push(None);
    }
    (BookLayout { pages: flow.pages }, flow.starts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_only_output_matches_every_rendered_page_start_for_rich_content() {
        let rows = (0..24).map(|row| format!(
            "<tr><td>{row}</td><td>日本語の文章と<ruby>漢字<rt>かんじ</rt></ruby>。</td><td>{row}.5</td></tr>"
        )).collect::<String>();
        let table = format!(
            "<table><thead><tr><th rowspan='2'>項目</th><th colspan='2'>説明</th></tr><tr><th>本文</th><th>数</th></tr></thead><tbody>{rows}</tbody></table>"
        );
        let fixtures = [
            "<h1>章の見出し</h1><h2>小見出し</h2><p style='text-indent:1em'>甲乙丙丁戊「<ruby>東京<rt>とうきょう</rt></ruby>」と<ruby>文章<rt>ぶんしょう</rt></ruby>。<span>」</span></p><p><span>』</span></p>".repeat(17),
            "<p><b>太字</b><i>斜体</i><sup>上</sup><sub>下</sub><s>訂正</s><em>強調</em>。<span style='text-combine-upright:all'>12</span>年。fi ffi café e&#x301;.</p><pre>let value = 3;\n\tprintln!(\"value\");</pre>".repeat(13),
            "<p>本文<img src='gaiji.png' alt='髙' style='width:1em;height:1em;vertical-align:super'/>の続き。</p><p><img src='illustration.png' width='240' height='180'/></p><p><ruby>図版<rt>ずはん</rt></ruby>の次を読む。</p>".repeat(11),
            format!("<p>表の前。</p>{table}<p>表の後。</p>"),
            "<p>数式の前。</p><math xmlns='http://www.w3.org/1998/Math/MathML' display='block'><mfrac><mrow><mi>x</mi><mo>+</mo><mn>1</mn></mrow><msqrt><mi>y</mi></msqrt></mfrac></math><p>数式の後。</p>".repeat(9),
            "<ul><li>最初の項目</li><li>次の項目</li></ul><hr/><p>　空白と禁則（「文章」）。</p>".repeat(19),
            "<p> </p><p>\t</p>".into(),
        ];
        for (fixture, body) in fixtures.iter().enumerate() {
            let chapter = reader_core::parse_html(
                &format!("<html><body>{body}</body></html>"),
                3,
                "exact.xhtml",
                "",
            )
            .unwrap();
            for mode in [ReadingMode::Horizontal, ReadingMode::Vertical] {
                for (extent, show_ruby) in [(vec2(381., 427.), true), (vec2(647., 503.), false)] {
                    let settings = Settings {
                        reading_mode: mode,
                        font_size: 23.,
                        margins: 21.,
                        margin_vertical: Some(27.),
                        margin_horizontal: Some(19.),
                        show_ruby,
                        letter_spacing: 0.03,
                        ..Settings::default()
                    };
                    let rendered =
                        chapter_layout(&mut FontSystem::new(), &chapter, extent, &settings);
                    let counted =
                        chapter_page_starts(&mut FontSystem::new(), &chapter, extent, &settings);
                    let expected: Vec<_> = rendered.pages.iter().map(Page::first_locator).collect();
                    assert_eq!(
                        counted, expected,
                        "fixture={fixture}, mode={mode:?}, extent={extent:?}, ruby={show_ruby}"
                    );
                    // Count mode never materializes output vectors, including
                    // textless, image-only and MathML pages.
                    let (output, starts) = chapter_output::<false>(
                        &mut FontSystem::new(),
                        &chapter,
                        extent,
                        &settings,
                    );
                    assert!(output.pages.is_empty());
                    assert_eq!(starts, expected);
                }
            }
        }
    }

    #[test]
    fn horizontal_and_vertical_margins_are_independent_in_both_writing_modes() {
        let chapter = reader_core::parse_html(
            "<html><body><p>読書の余白を確かめます。</p></body></html>",
            0,
            "margins.xhtml",
            "",
        )
        .unwrap();
        let extent = vec2(900., 700.);
        for mode in [ReadingMode::Horizontal, ReadingMode::Vertical] {
            let settings = Settings {
                reading_mode: mode,
                margin_horizontal: Some(82.),
                margin_vertical: Some(27.),
                ..Settings::default()
            };
            let mut fonts = FontSystem::new();
            let layout = chapter_layout(&mut fonts, &chapter, extent, &settings);
            let hit = &layout.pages[0].hits[0];
            assert!((hit.rect.top() - 27.).abs() < 0.1, "{mode:?}: {hit:?}");
            let edge = if mode == ReadingMode::Horizontal {
                hit.rect.left()
            } else {
                extent.x - hit.rect.right()
            };
            assert!((edge - 82.).abs() < 0.1, "{mode:?}: {hit:?}");
        }
    }

    #[test]
    fn headings_have_room_before_the_body_and_subheadings_are_larger_than_body_text() {
        let chapter = reader_core::parse_html(
            "<html><body><h2>小見出し</h2><p>ここから本文を読みます。</p></body></html>",
            0,
            "heading-space.xhtml",
            "",
        )
        .unwrap();
        let extent = vec2(900., 700.);
        for mode in [ReadingMode::Horizontal, ReadingMode::Vertical] {
            let settings = Settings {
                reading_mode: mode,
                ..Settings::default()
            };
            let layout = chapter_layout(&mut FontSystem::new(), &chapter, extent, &settings);
            let page = &layout.pages[0];
            let heading = page
                .hits
                .iter()
                .filter(|h| h.unit_id == unit_id(0, 0))
                .map(|h| h.rect)
                .reduce(Rect::union)
                .unwrap();
            let body = page
                .hits
                .iter()
                .filter(|h| h.unit_id == unit_id(0, 1))
                .map(|h| h.rect)
                .reduce(Rect::union)
                .unwrap();
            let gap = if mode == ReadingMode::Horizontal {
                body.top() - heading.bottom()
            } else {
                heading.left() - body.right()
            };
            assert!(gap >= settings.font_size * 0.5, "{mode:?}: {gap}");
            assert!(
                page.glyphs
                    .iter()
                    .any(|g| g.size >= settings.font_size * 1.24)
            );
        }
    }

    #[test]
    fn author_heading_css_can_replace_reader_size_weight_and_spacing_without_double_scaling() {
        let chapter = reader_core::parse_html(
            "<body><h1>甲乙</h1><p>丙丁</p></body>",
            0,
            "author-heading.xhtml",
            "h1 { font-size:100%; font-weight:normal; margin-block:0; }",
        )
        .unwrap();
        for mode in [ReadingMode::Horizontal, ReadingMode::Vertical] {
            let settings = Settings {
                reading_mode: mode,
                font_size: 24.,
                ..Settings::default()
            };
            let layout = chapter_layout(
                &mut FontSystem::new(),
                &chapter,
                vec2(900., 700.),
                &settings,
            );
            assert!(
                layout.pages[0]
                    .glyphs
                    .iter()
                    .all(|glyph| (glyph.size - 24.).abs() < 0.001 && !glyph.style.bold)
            );
        }
        let chapter = reader_core::parse_html(
            "<body><div class='chapter-title'>甲<span>乙</span></div><p>丙丁</p></body>",
            0,
            "class-heading.xhtml",
            ".chapter-title { font-size:150%; margin-block-end:2em; }",
        )
        .unwrap();
        let no_margin = reader_core::parse_html(
            "<body><div class='chapter-title'>甲<span>乙</span></div><p>丙丁</p></body>",
            0,
            "class-heading.xhtml",
            ".chapter-title { font-size:150%; margin-block-end:0; }",
        )
        .unwrap();
        for mode in [ReadingMode::Horizontal, ReadingMode::Vertical] {
            let settings = Settings {
                reading_mode: mode,
                font_size: 24.,
                ..Settings::default()
            };
            let layout = chapter_layout(
                &mut FontSystem::new(),
                &chapter,
                vec2(900., 700.),
                &settings,
            );
            let page = &layout.pages[0];
            assert!(
                page.glyphs
                    .iter()
                    .filter(|glyph| glyph.size > 30.)
                    .all(|glyph| (glyph.size - 36.).abs() < 0.001)
            );
            assert!(
                page.glyphs
                    .iter()
                    .any(|glyph| (glyph.size - 36.).abs() < 0.001)
            );
            let heading = page
                .hits
                .iter()
                .filter(|hit| matches!(hit.text.as_str(), "甲" | "乙"))
                .map(|hit| hit.rect)
                .reduce(Rect::union)
                .unwrap();
            let body = page
                .hits
                .iter()
                .filter(|hit| matches!(hit.text.as_str(), "丙" | "丁"))
                .map(|hit| hit.rect)
                .reduce(Rect::union)
                .unwrap();
            let gap = if mode == ReadingMode::Horizontal {
                body.top() - heading.bottom()
            } else {
                heading.left() - body.right()
            };
            let baseline = chapter_layout(
                &mut FontSystem::new(),
                &no_margin,
                vec2(900., 700.),
                &settings,
            );
            let bounds = |text: &[&str]| {
                baseline.pages[0]
                    .hits
                    .iter()
                    .filter(|hit| text.contains(&hit.text.as_str()))
                    .map(|hit| hit.rect)
                    .reduce(Rect::union)
                    .unwrap()
            };
            let before = bounds(&["甲", "乙"]);
            let after = bounds(&["丙", "丁"]);
            let baseline_gap = if mode == ReadingMode::Horizontal {
                after.top() - before.bottom()
            } else {
                before.left() - after.right()
            };
            assert!(
                (gap - baseline_gap - 72.).abs() < 0.01,
                "author 2em adds exactly 72px at the heading's 36px font ({mode:?}: gap={gap}, baseline={baseline_gap})"
            );
            if mode == ReadingMode::Vertical {
                assert!(heading.right() <= 900. - settings.horizontal_margin() + 0.1);
            }
        }
    }

    #[test]
    fn text_image_role_belongs_to_each_instance_and_never_to_a_fixed_page() {
        let chapter = reader_core::parse_html("<body><p>外<img src='shared.png' alt='字' style='width:1em;height:1em'/>字</p><img src='shared.png' alt='風景' style='width:300px;height:200px'/><img src='formula.png' alt='x = y' width='100' height='30'/></body>",0,"roles.xhtml","").unwrap();
        let layout = chapter_layout(
            &mut FontSystem::new(),
            &chapter,
            vec2(900., 700.),
            &Settings::default(),
        );
        let pictures = layout
            .pages
            .iter()
            .flat_map(|page| &page.pictures)
            .collect::<Vec<_>>();
        assert!(
            pictures
                .iter()
                .any(|p| p.resource == "shared.png" && p.text_image)
        );
        assert!(
            pictures
                .iter()
                .any(|p| p.resource == "shared.png" && !p.text_image)
        );
        assert!(
            pictures
                .iter()
                .any(|p| p.resource == "formula.png" && p.text_image)
        );
        let mut fixed = reader_core::parse_html("<html><head><meta name='viewport' content='width=600,height=900'/></head><body><img src='shared.png' alt='字' width='600' height='900'/></body></html>",0,"fixed.xhtml","").unwrap();
        fixed.presentation.layout = Some(reader_core::RenditionLayout::PrePaginated);
        let layout = chapter_layout(
            &mut FontSystem::new(),
            &fixed,
            vec2(900., 700.),
            &Settings::default(),
        );
        assert!(
            layout
                .pages
                .iter()
                .flat_map(|page| &page.pictures)
                .all(|p| !p.text_image)
        );
    }

    #[test]
    fn fixed_epub_canvas_preserves_author_boxes_sources_and_one_page_across_settings() {
        let mut chapter = reader_core::parse_html("<html><head><meta name='viewport' content='width=600,height=900'/></head><body><p><img src='plate.png' alt='図版' style='width:300px;height:450px'/></p></body></html>", 7, "plate.xhtml", "").unwrap();
        chapter.presentation.layout = Some(reader_core::RenditionLayout::PrePaginated);
        let atom = chapter.atoms()[0];
        let source = atom.locator_at(0).unwrap();
        let end = atom.locator_at(atom.text.len()).unwrap();
        assert!(epub_fixed_layout_diagnostic(&chapter).is_none());
        let extent = vec2(1200., 800.);
        let expected_page = Rect::from_min_size(pos2(1000. / 3., 0.), vec2(1600. / 3., 800.));
        let expected_hit = Rect::from_min_size(expected_page.min, expected_page.size() * 0.5);
        for (font_size, margins, mode) in [
            (16., 0., ReadingMode::Horizontal),
            (64., 120., ReadingMode::Vertical),
        ] {
            let settings = Settings {
                font_size,
                margins,
                reading_mode: mode,
                ..Settings::default()
            };
            let layout = chapter_layout(&mut FontSystem::new(), &chapter, extent, &settings);
            assert_eq!(layout.pages.len(), 1);
            let page = &layout.pages[0];
            assert!(page.glyphs.is_empty());
            assert_eq!(page.pictures.len(), 1);
            assert!((page.pictures[0].rect.min - expected_page.min).length() < 0.01);
            assert!((page.pictures[0].rect.size() - expected_page.size()).length() < 0.01);
            let canvas = page.pictures[0].fixed_canvas.unwrap();
            assert_eq!(canvas.extent, vec2(600., 900.));
            assert_eq!(canvas.image_rect.size(), vec2(300., 450.));
            let hit = page.hit_for_locator(source).unwrap();
            assert_eq!(hit.locator, source);
            assert_eq!(hit.end, end);
            assert_eq!(hit.unit_id, unit_id(7, 0));
            assert!((hit.rect.min - expected_hit.min).length() < 0.01);
            assert!((hit.rect.size() - expected_hit.size()).length() < 0.01);
        }
    }

    #[test]
    fn unsupported_fixed_html_is_one_page_with_an_explicit_diagnostic() {
        for body in [
            "<p>絶対位置で配置する本文</p><img src='one.png' width='600' height='900'/>",
            "<img src='one.png' width='600' height='900'/><img src='two.png' width='600' height='900'/>",
        ] {
            let mut chapter = reader_core::parse_html(&format!("<html><head><meta name='viewport' content='width=600,height=900'/></head><body>{body}</body></html>"), 0, "complex.xhtml", "").unwrap();
            chapter.presentation.layout = Some(reader_core::RenditionLayout::PrePaginated);
            assert!(
                epub_fixed_layout_diagnostic(&chapter)
                    .unwrap()
                    .contains("未対応")
            );
            let layout = chapter_layout(
                &mut FontSystem::new(),
                &chapter,
                vec2(300., 200.),
                &Settings::default(),
            );
            assert_eq!(
                layout.pages.len(),
                1,
                "unsupported FXL must never be silently reflowed"
            );
            assert!(layout.pages[0].pictures.is_empty());
            assert!(layout.pages[0].hits.is_empty());
        }
    }

    #[test]
    fn a_flow_image_is_not_promoted_by_a_full_page_hint_or_viewport_meta() {
        let mut chapter = reader_core::parse_html("<html><head><meta name='viewport' content='width=600,height=900'/></head><body><img src='gaiji.png' width='24' height='24' alt='字'/></body></html>", 0, "flow.xhtml", "").unwrap();
        chapter.blocks[0].image.as_mut().unwrap().display = ImageDisplay::FullPage;
        let layout = chapter_layout(
            &mut FontSystem::new(),
            &chapter,
            vec2(700., 500.),
            &Settings::default(),
        );
        assert_eq!(layout.pages.len(), 1);
        assert_eq!(layout.pages[0].pictures[0].rect.size(), vec2(24., 24.));
        assert!(layout.pages[0].pictures[0].fixed_canvas.is_none());
    }

    #[test]
    fn legacy_single_image_css_geometry_is_one_centered_page_in_native_output_and_count() {
        let chapter=reader_core::parse_html("<html><body><div><img src='plate.jpeg' class='plate' width='2480' height='3248' alt='Page'/></div></body></html>",0,"plate.xhtml",".plate{width:977px;height:1280px}").unwrap();
        let extent = vec2(1280., 760.);
        let settings = Settings::default();
        let layout = chapter_layout(&mut FontSystem::new(), &chapter, extent, &settings);
        assert_eq!(layout.pages.len(), 1);
        let picture = &layout.pages[0].pictures[0];
        assert!((picture.rect.center().x - extent.x / 2.).abs() < 0.01);
        assert!((picture.rect.center().y - extent.y / 2.).abs() < 0.01);
        assert_eq!(picture.fixed_canvas.unwrap().extent, vec2(977., 1280.));
        assert_eq!(
            chapter_page_starts(&mut FontSystem::new(), &chapter, extent, &settings),
            vec![chapter.start_locator()]
        );
    }

    #[test]
    fn paragraph_indent_comes_from_author_css_and_source_spaces_without_a_forced_extra_em() {
        for mode in [ReadingMode::Horizontal, ReadingMode::Vertical] {
            for (style, prefix, cells) in [
                ("", "", 0.),
                ("", "　", 1.),
                ("text-indent:1em", "", 1.),
                ("text-indent:1em", "　", 2.),
            ] {
                let chapter = reader_core::parse_html(
                    &format!("<html><body><p style='{style}'>{prefix}甲乙</p></body></html>"),
                    0,
                    "indent.xhtml",
                    "",
                )
                .unwrap();
                let original = chapter.plain_text();
                let settings = Settings {
                    font_size: 24.,
                    margins: 40.,
                    reading_mode: mode,
                    ..Settings::default()
                };
                let layout = chapter_layout(
                    &mut FontSystem::new(),
                    &chapter,
                    vec2(600., 500.),
                    &settings,
                );
                let hit = layout.pages[0]
                    .hits
                    .iter()
                    .find(|hit| hit.text == "甲")
                    .unwrap();
                let along = if mode == ReadingMode::Vertical {
                    hit.rect.top()
                } else {
                    hit.rect.left()
                };
                assert!(
                    (along - (40. + cells * 24.)).abs() < 0.01,
                    "{mode:?}: {style:?}, prefix={prefix:?} has {along}"
                );
                let atom = chapter
                    .atoms()
                    .into_iter()
                    .find(|atom| atom.text.contains('甲'))
                    .unwrap();
                assert_eq!(
                    hit.locator,
                    atom.locator_at(atom.text.find('甲').unwrap()).unwrap()
                );
                assert_eq!(chapter.plain_text(), original);
                assert!(original.starts_with(prefix));
            }
        }
    }

    #[test]
    fn japanese_line_break_groups_preserve_closing_marks_openers_small_kana_and_sources() {
        let fixtures = [
            "<p>甲乙丙丁戊己」次</p>",
            "<p>甲乙丙丁戊「次」後</p>",
            "<p>甲乙丙丁戊己」』）次</p>",
            "<p>甲乙丙丁戊己っゃー々次</p>",
            "<p>甲乙丙丁戊<span>己</span><span>」</span><span>』</span>次</p>",
            "<p>甲乙丙丁戊己</p><p><span>」</span></p><p>後の段落</p>",
            "<p>甲乙丙丁戊「<ruby>東京<rt>とうきょう</rt></ruby>」後</p>",
        ];
        for mode in [ReadingMode::Horizontal, ReadingMode::Vertical] {
            for body in fixtures {
                let chapter = reader_core::parse_html(
                    &format!("<html><body>{body}</body></html>"),
                    3,
                    "kinsoku.xhtml",
                    "",
                )
                .unwrap();
                let original = chapter.clone();
                let settings = Settings {
                    font_size: 20.,
                    margins: 10.,
                    line_spacing: 1.4,
                    reading_mode: mode,
                    ..Settings::default()
                };
                let extent = if mode == ReadingMode::Vertical {
                    vec2(180., 140.)
                } else {
                    vec2(140., 180.)
                };
                let layout = chapter_layout(&mut FontSystem::new(), &chapter, extent, &settings);
                for page in &layout.pages {
                    let mut lines: Vec<Vec<&Hit>> = Vec::new();
                    for hit in &page.hits {
                        let cross = |hit: &Hit| {
                            if mode == ReadingMode::Vertical {
                                hit.rect.left()
                            } else {
                                hit.rect.top()
                            }
                        };
                        if lines
                            .last()
                            .is_none_or(|line| (cross(line[0]) - cross(hit)).abs() > 5.)
                        {
                            lines.push(Vec::new());
                        }
                        lines.last_mut().unwrap().push(hit);
                        assert!(
                            hit.rect.left() >= 9.9
                                && hit.rect.top() >= 9.9
                                && hit.rect.right() <= extent.x - 9.9
                                && hit.rect.bottom() <= extent.y - 9.9,
                            "{mode:?}: text escapes the page in {body}: {:?}",
                            hit.rect
                        );
                    }
                    for line in lines {
                        let text = line.iter().map(|hit| hit.text.as_str()).collect::<String>();
                        assert!(
                            !cannot_start_line(text.trim_start().chars().next().unwrap()),
                            "{mode:?}: forbidden line start {text:?} in {body}"
                        );
                        assert!(
                            !cannot_end_line(text.trim_end().chars().last().unwrap()),
                            "{mode:?}: forbidden line end {text:?} in {body}"
                        );
                        assert!(
                            line.iter()
                                .any(|hit| hit.text.chars().any(|c| !cannot_start_line(c))),
                            "a closing quote must not form a line by itself"
                        );
                    }
                }
                for (index, atom) in chapter.atoms().iter().enumerate() {
                    let hits: Vec<_> = layout
                        .pages
                        .iter()
                        .flat_map(|page| &page.hits)
                        .filter(|hit| hit.unit_id == unit_id(3, index))
                        .collect();
                    assert_eq!(
                        hits.iter().map(|hit| hit.text.as_str()).collect::<String>(),
                        atom.text,
                        "source text must not be merged or dropped"
                    );
                    for hit in hits {
                        assert_eq!(hit.locator, atom.locator_at(hit.offset).unwrap());
                    }
                }
                assert_eq!(
                    chapter, original,
                    "line breaking must not mutate the original atoms or locators"
                );
            }
        }
    }
    #[test]
    fn filled_horizontal_and_vertical_text_grids_balance_all_four_margins() {
        let chapter = reader_core::parse_html(
            &format!(
                "<html><body><p>{}</p></body></html>",
                "<ruby>漢字<rt>かんじ</rt></ruby>を自然な位置で読みます。".repeat(180)
            ),
            0,
            "balanced.xhtml",
            "",
        )
        .unwrap();
        let extent = vec2(873.0, 647.0);
        for mode in [ReadingMode::Horizontal, ReadingMode::Vertical] {
            let settings = Settings {
                reading_mode: mode,
                margins: 47.0,
                ..Settings::default()
            };
            let mut fonts = FontSystem::new();
            let layout = chapter_layout(&mut fonts, &chapter, extent, &settings);
            assert!(layout.pages.len() > 2);
            for page in layout.pages.iter().take(layout.pages.len() - 1) {
                let bounds = page.content_bounds(&mut fonts, settings.font_size);
                assert!(
                    (bounds.left() - (extent.x - bounds.right())).abs() < 0.1,
                    "{mode:?}: horizontal margins differ: {bounds:?}"
                );
                assert!(
                    (bounds.top() - (extent.y - bounds.bottom())).abs() < 0.1,
                    "{mode:?}: vertical margins differ: {bounds:?}"
                );
                for hit in &page.hits {
                    assert!(page.hit(hit.rect.center()).is_some());
                    assert!(hit.locator.chapter == chapter.id);
                }
            }
        }
    }

    #[test]
    fn a_short_terminal_page_keeps_its_natural_reading_start() {
        let chapter = reader_core::parse_html(
            "<html><body><p>短い末尾。</p></body></html>",
            0,
            "short.xhtml",
            "",
        )
        .unwrap();
        let extent = vec2(873.0, 647.0);
        for mode in [ReadingMode::Horizontal, ReadingMode::Vertical] {
            let settings = Settings {
                reading_mode: mode,
                margins: 47.0,
                ..Settings::default()
            };
            let mut fonts = FontSystem::new();
            let layout = chapter_layout(&mut fonts, &chapter, extent, &settings);
            let first = layout.pages[0].hits.first().unwrap();
            if mode == ReadingMode::Vertical {
                assert!((extent.x - first.rect.right() - settings.margins).abs() < 0.1);
                assert!(first.rect.top() < 100.0);
            } else {
                assert!((first.rect.top() - settings.margins).abs() < 0.1);
                assert!(first.rect.left() < 100.0);
            }
        }
    }

    #[test]
    #[ignore = "manual timing probe: prints native layout and page clone costs"]
    fn long_chapter_layout_timing_probe() {
        for length in [20_000, 100_000] {
            let text="静かな部屋で本を読みながら、日本語の文章を自然に表示します。物語を進め、言葉を調べ、次のページへ移ります。".chars().cycle().take(length).collect::<String>();
            let parse = std::time::Instant::now();
            let chapter = reader_core::parse_html(
                &format!("<html><body><p>{text}</p></body></html>"),
                0,
                "long.xhtml",
                "",
            )
            .unwrap();
            let parse_ms = parse.elapsed().as_millis();
            for mode in [ReadingMode::Horizontal, ReadingMode::Vertical] {
                let mut fonts = FontSystem::new();
                let settings = Settings {
                    reading_mode: mode,
                    ..Settings::default()
                };
                let start = std::time::Instant::now();
                let layout = chapter_layout(&mut fonts, &chapter, vec2(1280.0, 760.0), &settings);
                let layout_ms = start.elapsed().as_millis();
                let start = std::time::Instant::now();
                for _ in 0..1000 {
                    std::hint::black_box(layout.pages[0].clone());
                }
                let clone_us = start.elapsed().as_micros() / 1000;
                eprintln!(
                    "native-layout chars={length} mode={mode:?} parse_ms={parse_ms} layout_ms={layout_ms} pages={} page_clone_us={clone_us}",
                    layout.pages.len()
                );
            }
        }
    }
    #[test]
    fn one_em_gaiji_and_its_ruby_stay_inside_the_text_flow_in_both_directions() {
        let chapter = reader_core::parse_html("<html><body><p>甲<img class='gaiji' src='g.png' alt='髙'/>乙<ruby><img class='gaiji' src='j.png' alt='𠮷'/><rt>よし</rt></ruby>丙</p></body></html>", 0, "gaiji.xhtml", ".gaiji{width:1em;height:1em;vertical-align:-.12em}").unwrap();
        for mode in [ReadingMode::Horizontal, ReadingMode::Vertical] {
            let settings = Settings {
                reading_mode: mode,
                margins: 20.0,
                ..Settings::default()
            };
            let layout = chapter_layout(
                &mut FontSystem::new(),
                &chapter,
                vec2(420.0, 350.0),
                &settings,
            );
            assert_eq!(
                layout.pages.len(),
                1,
                "an inline character must not create image pages"
            );
            let page = &layout.pages[0];
            assert_eq!(page.pictures.len(), 2);
            for picture in &page.pictures {
                assert!(
                    (picture.rect.width() - settings.font_size).abs() < 0.01
                        && (picture.rect.height() - settings.font_size).abs() < 0.01
                );
            }
            let before = page.hits.iter().find(|hit| hit.text == "甲").unwrap();
            let gaiji = page.hits.iter().find(|hit| hit.text == "髙").unwrap();
            let after = page.hits.iter().find(|hit| hit.text == "乙").unwrap();
            if mode == ReadingMode::Vertical {
                assert!(
                    gaiji.rect.top() >= before.rect.bottom() - 0.1
                        && after.rect.top() >= gaiji.rect.bottom() - 0.1
                );
            } else {
                assert!(
                    gaiji.rect.left() >= before.rect.right() - 0.1
                        && after.rect.left() >= gaiji.rect.right() - 0.1
                );
            }
            for (index, atom) in chapter.atoms().iter().enumerate() {
                assert!(page.hits.iter().any(|hit| hit.unit_id == unit_id(0, index)
                    && hit.locator == atom.locator_at(0).unwrap()));
            }
            assert!(
                page.glyphs
                    .iter()
                    .any(|glyph| glyph.size < settings.font_size * 0.6),
                "ruby above an image must still render"
            );
        }
    }

    #[test]
    fn block_image_percent_em_and_aspect_constraints_do_not_force_full_pages() {
        let chapter = reader_core::parse_html("<html><body><p>説明</p><img src='figure.png' style='display:block;width:50%;height:auto;aspect-ratio:2/1'/><p>続き</p></body></html>", 0, "figure.xhtml", "").unwrap();
        let settings = Settings {
            margins: 20.0,
            ..Settings::default()
        };
        let layout = chapter_layout(
            &mut FontSystem::new(),
            &chapter,
            vec2(600.0, 400.0),
            &settings,
        );
        assert_eq!(layout.pages.len(), 1);
        let picture = &layout.pages[0].pictures[0];
        assert!((picture.rect.width() - 280.0).abs() < 0.01);
        assert!((picture.rect.height() - 140.0).abs() < 0.01);
        assert!(layout.pages[0].hits.iter().any(|hit| hit.text == "続"));
        let image = ImageRef {
            display: ImageDisplay::Block,
            width: Some(reader_core::CssLength::Px(600.0)),
            max_height: Some(reader_core::CssLength::Em(3.0)),
            intrinsic_size: Some([1200.0, 600.0]),
            ..ImageRef::default()
        };
        assert_eq!(
            image_size(&image, 24.0, 24.0, vec2(560.0, 360.0)),
            vec2(144.0, 72.0)
        );
    }

    #[test]
    fn a_tall_inline_image_expands_its_line_instead_of_overlapping_the_next_one() {
        let chapter = reader_core::parse_html("<html><body><p>一<img src='g.png' alt='髙' style='width:2em;height:2em'/>二<br/>三</p></body></html>", 0, "tall.xhtml", "").unwrap();
        let settings = Settings {
            margins: 20.0,
            ..Settings::default()
        };
        let layout = chapter_layout(
            &mut FontSystem::new(),
            &chapter,
            vec2(500.0, 400.0),
            &settings,
        );
        let page = &layout.pages[0];
        let picture = &page.pictures[0];
        let next_line = page.hits.iter().find(|hit| hit.text == "三").unwrap();
        assert!(next_line.rect.top() >= picture.rect.bottom());
        let first = page.hits.iter().find(|hit| hit.text == "一").unwrap();
        let second = page.hits.iter().find(|hit| hit.text == "二").unwrap();
        assert!(
            (first.rect.top() - second.rect.top()).abs() < 0.01,
            "text on either side must share a baseline"
        );
    }
    #[test]
    fn changing_typography_preserves_source_and_pages_stay_inside_viewport() {
        let book = reader_core::Book::demo();
        let mut fonts = FontSystem::new();
        let mut settings = Settings {
            reading_mode: ReadingMode::Vertical,
            ..Settings::default()
        };
        let first = chapter_layout(&mut fonts, &book.chapters[0], vec2(700.0, 600.0), &settings);
        let locator = first
            .pages
            .iter()
            .flat_map(|p| &p.hits)
            .nth(20)
            .unwrap()
            .locator;
        settings.font_size = 30.0;
        let second = chapter_layout(&mut fonts, &book.chapters[0], vec2(700.0, 600.0), &settings);
        assert!(
            second
                .pages
                .iter()
                .flat_map(|p| &p.hits)
                .any(|hit| hit.locator == locator)
        );
        assert!(
            second
                .pages
                .iter()
                .flat_map(|p| &p.hits)
                .all(|hit| hit.rect.min.x >= 0.0 && hit.rect.max.y <= 602.0)
        );
    }

    #[test]
    fn table_grid_preserves_spans_and_source_units_across_pages() {
        let mut rows = String::from(
            "<tr><th rowspan='2'>分類</th><th colspan='2'>比較</th></tr><tr><th>名前</th><th>値</th></tr>",
        );
        for index in 0..18 {
            rows.push_str(&format!(
                "<tr><td>行{index}</td><td>日本語の説明</td><td>{index}</td></tr>"
            ));
        }
        let chapter = reader_core::parse_html(
            &format!(
                "<html><body><table><thead>{}</thead><tbody>{}</tbody></table></body></html>",
                &rows[..rows.find("<tr><td>").unwrap()],
                &rows[rows.find("<tr><td>").unwrap()..]
            ),
            0,
            "x.xhtml",
            "",
        )
        .unwrap();
        let settings = Settings {
            margins: 20.0,
            font_size: 18.0,
            ..Settings::default()
        };
        let layout = chapter_layout(
            &mut FontSystem::new(),
            &chapter,
            vec2(600.0, 350.0),
            &settings,
        );
        assert!(layout.pages.len() > 1);
        assert!(layout.pages[0].rules.len() >= 4);
        assert!(layout.pages[0].header_fills.len() >= 4);
        let all: Vec<_> = layout.pages.iter().flat_map(|page| &page.hits).collect();
        let sources = chapter.atoms();
        for (index, atom) in sources.iter().enumerate() {
            if atom.text.trim().is_empty() {
                continue;
            }
            assert!(
                all.iter().any(|hit| hit.unit_id == unit_id(0, index)),
                "lost table atom {}",
                atom.text
            );
        }
        assert!(
            layout
                .pages
                .iter()
                .flat_map(|page| &page.rules)
                .all(|rect| rect.left() >= 19.0 && rect.right() <= 581.0 && rect.bottom() <= 331.0)
        );
    }

    #[test]
    fn native_mathml_uses_real_fraction_geometry() {
        let chapter = reader_core::parse_html("<html><body><math xmlns='http://www.w3.org/1998/Math/MathML' display='block'><mfrac><mrow><mi>x</mi><mo>+</mo><mn>1</mn></mrow><msqrt><mi>y</mi></msqrt></mfrac></math></body></html>", 2, "math.xhtml", "").unwrap();
        let layout = chapter_layout(
            &mut FontSystem::new(),
            &chapter,
            vec2(600.0, 400.0),
            &Settings::default(),
        );
        let picture = &layout.pages[0].pictures[0];
        let svg = picture.inline_svg.as_ref().unwrap();
        assert!(svg.contains("<path"));
        assert!(svg.contains("<rect"), "fraction bar must be typeset");
        assert!(picture.rect.height() > Settings::default().font_size);
        assert!(!layout.pages[0].hits.is_empty());
    }

    #[test]
    fn pdf_hit_geometry_scales_with_the_native_picture() {
        let mut chapter =
            reader_core::parse_html("<html><body><p>x</p></body></html>", 4, "page", "").unwrap();
        chapter.fixed_page = Some(reader_core::FixedPage {
            width: 200.0,
            height: 100.0,
            chars: vec![reader_core::FixedChar {
                text: "x".into(),
                source: reader_core::SourceSpan {
                    chapter: 4,
                    node: 0,
                    start: 0,
                    end: 1,
                },
                rect: [20.0, 10.0, 40.0, 30.0],
            }],
        });
        chapter.blocks[0].image = Some(reader_core::ImageRef {
            resource: "pdf-page:0".into(),
            ..reader_core::ImageRef::default()
        });
        let settings = Settings {
            margins: 0.0,
            ..Settings::default()
        };
        let layout = chapter_layout(
            &mut FontSystem::new(),
            &chapter,
            vec2(400.0, 300.0),
            &settings,
        );
        assert_eq!(
            layout.pages[0].pictures[0].rect,
            Rect::from_min_max(pos2(0.0, 50.0), pos2(400.0, 250.0))
        );
        assert_eq!(
            layout.pages[0].hits[0].rect,
            Rect::from_min_max(pos2(40.0, 70.0), pos2(80.0, 110.0))
        );
        assert_eq!(layout.pages[0].hits[0].unit_id, unit_id(4, 0));
    }

    #[test]
    fn code_tabs_and_pages_keep_source_offsets() {
        let lines = (0..30)
            .map(|n| format!("\tlet value{n} = {n};\n"))
            .collect::<String>();
        let chapter = reader_core::parse_html(
            &format!("<html><body><pre>{lines}</pre></body></html>"),
            0,
            "code.xhtml",
            "",
        )
        .unwrap();
        let settings = Settings {
            font_size: 18.0,
            margins: 20.0,
            ..Settings::default()
        };
        let layout = chapter_layout(
            &mut FontSystem::new(),
            &chapter,
            vec2(400.0, 250.0),
            &settings,
        );
        assert!(layout.pages.len() > 1);
        assert!(
            layout
                .pages
                .iter()
                .flat_map(|page| &page.glyphs)
                .all(|glyph| glyph.face == fonts::CODE_FACE)
        );
        assert_eq!(
            layout
                .pages
                .iter()
                .flat_map(|page| &page.hits)
                .filter(|hit| hit.text == "\t")
                .count(),
            30
        );
        let last = layout
            .pages
            .iter()
            .flat_map(|page| &page.hits)
            .last()
            .unwrap();
        assert!(last.offset > lines.len() - 5);
    }

    #[test]
    fn vertical_japanese_advance_is_natural_and_font_choice_is_real() {
        let chapter = reader_core::parse_html(
            "<html><body style='writing-mode:vertical-rl'><p>一二三四五</p></body></html>",
            0,
            "ja.xhtml",
            "",
        )
        .unwrap();
        let settings = Settings {
            font_family: "Noto Sans CJK JP".into(),
            ..Settings::default()
        };
        let layout = chapter_layout(
            &mut FontSystem::new(),
            &chapter,
            vec2(600.0, 500.0),
            &settings,
        );
        let hits = &layout.pages[0].hits;
        assert!((hits[1].rect.top() - hits[0].rect.top() - settings.font_size).abs() < 0.1);
        assert!(
            layout.pages[0]
                .glyphs
                .iter()
                .all(|glyph| glyph.face == fonts::SANS_FACE)
        );
    }

    #[test]
    fn inline_styles_keep_sources_and_create_real_glyph_geometry() {
        let chapter = reader_core::parse_html("<html><body><p><b>太</b><i>斜</i>H<sub>2</sub>O x<sup>2</sup><s>消</s><span style='text-emphasis-style:dot'>強調</span></p></body></html>", 0, "styles.xhtml", "").unwrap();
        let settings = Settings::default();
        let layout = chapter_layout(
            &mut FontSystem::new(),
            &chapter,
            vec2(700.0, 400.0),
            &settings,
        );
        let page = &layout.pages[0];
        assert!(page.glyphs.iter().any(|glyph| glyph.style.bold));
        assert!(page.glyphs.iter().any(|glyph| glyph.style.italic));
        assert!(
            !page.rules.is_empty(),
            "strikethrough has native line geometry"
        );
        assert!(
            page.glyphs
                .iter()
                .any(|glyph| glyph.size < settings.font_size * 0.4),
            "emphasis marks are separate small glyphs"
        );
        let script_hits: Vec<_> = page.hits.iter().filter(|hit| hit.text == "2").collect();
        assert_eq!(script_hits.len(), 2);
        assert!(script_hits[0].rect.top() > script_hits[1].rect.top());
        for (index, atom) in chapter.atoms().iter().enumerate() {
            assert!(page.hits.iter().any(|hit| hit.unit_id == unit_id(0, index)
                && hit.locator == atom.locator_at(0).unwrap()));
        }
    }

    #[test]
    fn interior_ruby_search_and_image_anchors_restore_the_containing_page() {
        let text = format!(
            "<html><body>{}<p><ruby>東京<rt>とうきょう</rt></ruby>へ</p></body></html>",
            "<p>本文を読みます。</p>".repeat(30)
        );
        let chapter = reader_core::parse_html(&text, 0, "restore.xhtml", "").unwrap();
        let ruby = chapter
            .atoms()
            .into_iter()
            .find(|atom| atom.ruby.is_some())
            .unwrap();
        let inside = ruby.locator_at("東".len()).unwrap();
        let layout = chapter_layout(
            &mut FontSystem::new(),
            &chapter,
            vec2(300.0, 220.0),
            &Settings::default(),
        );
        let page = layout
            .pages
            .iter()
            .position(|page| page.contains_locator(inside))
            .unwrap();
        assert!(page > 0);
        assert_eq!(
            layout.pages[page].hit_for_locator(inside).unwrap().text,
            "東京"
        );
        let image = Page {
            anchor: Some(Locator {
                chapter: 3,
                node: u32::MAX,
                offset: 0,
            }),
            ..Page::default()
        };
        assert!(image.contains_locator(image.first_locator().unwrap()));
    }
}
