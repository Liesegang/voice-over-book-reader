use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

/// UTF-8 byte offset in a decoded source node. Inline images use their decoded
/// alt value (or the object replacement character) at the image's DOM node.
/// This is never a page index.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Locator {
    pub chapter: u32,
    pub node: u32,
    pub offset: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub chapter: u32,
    pub node: u32,
    pub start: u32,
    pub end: u32,
}

impl SourceSpan {
    pub fn start_locator(self) -> Locator {
        Locator {
            chapter: self.chapter,
            node: self.node,
            offset: self.start,
        }
    }

    pub fn end_locator(self) -> Locator {
        Locator {
            chapter: self.chapter,
            node: self.node,
            offset: self.end,
        }
    }

    pub fn len(self) -> usize {
        self.end.saturating_sub(self.start) as usize
    }
    pub fn is_empty(self) -> bool {
        self.start >= self.end
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WritingMode {
    #[default]
    Horizontal,
    VerticalRl,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PageProgression {
    #[default]
    Default,
    Ltr,
    Rtl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenditionLayout {
    Reflowable,
    PrePaginated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenditionSpread {
    Auto,
    None,
    Landscape,
    Portrait,
    Both,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PagePlacement {
    #[default]
    Auto,
    Left,
    Right,
    Center,
}

/// Effective package metadata after spine-local overrides. Missing metadata
/// stays None, preserving the distinction between publisher intent and reader
/// preferences. A numeric viewport is one page, never the combined spread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PagePresentation {
    pub layout: Option<RenditionLayout>,
    pub spread: Option<RenditionSpread>,
    pub placement: PagePlacement,
    pub viewport: Option<[f32; 2]>,
}

impl WritingMode {
    pub fn is_vertical(self) -> bool {
        self == Self::VerticalRl
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct InlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub superscript: bool,
    pub subscript: bool,
    pub emphasis: bool,
    pub tate_chu_yoko: bool,
    /// Computed against the reader root: Em is relative to its body font, Px is
    /// absolute. Parent em/% values are resolved at import, not multiplied again.
    #[serde(default)]
    pub font_size: Option<CssLength>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Atom {
    pub text: String,
    pub ruby: Option<String>,
    /// The spans concatenate to `text`; collapsed whitespace retains its original span.
    pub sources: Vec<SourceSpan>,
    pub style: InlineStyle,
    pub link: Option<String>,
    /// EPUB noteref / ARIA doc-noteref semantics, independent of visual superscript.
    #[serde(default)]
    pub link_is_note: bool,
    #[serde(default)]
    pub image: Option<Box<ImageRef>>,
}

impl Atom {
    pub fn spoken_text(&self) -> &str {
        if let Some(image) = &self.image {
            return self.ruby.as_deref().unwrap_or(&image.alt);
        }
        self.ruby.as_deref().unwrap_or(&self.text)
    }

    /// Maps a displayed UTF-8 byte offset back to source. Caller should use char boundaries.
    pub fn locator_at(&self, byte_offset: usize) -> Option<Locator> {
        let mut remaining = byte_offset.min(self.text.len());
        for (index, source) in self.sources.iter().enumerate() {
            if remaining < source.len() || index + 1 == self.sources.len() {
                return Some(Locator {
                    chapter: source.chapter,
                    node: source.node,
                    offset: source.start + remaining.min(source.len()) as u32,
                });
            }
            remaining -= source.len();
        }
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockKind {
    Paragraph,
    Heading(u8),
    Code,
    Quote,
    ListItem,
    Image,
    Table,
    Rule,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ImageRef {
    pub resource: String,
    pub alt: String,
    pub title: Option<String>,
    pub inline_svg: Option<String>,
    pub pdf_page: Option<crate::PdfPageRef>,
    #[serde(default)]
    pub display: ImageDisplay,
    #[serde(default)]
    pub width: Option<CssLength>,
    #[serde(default)]
    pub height: Option<CssLength>,
    #[serde(default)]
    pub max_width: Option<CssLength>,
    #[serde(default)]
    pub max_height: Option<CssLength>,
    #[serde(default)]
    pub intrinsic_size: Option<[f32; 2]>,
    #[serde(default)]
    pub aspect_ratio: Option<f32>,
    #[serde(default)]
    pub vertical_align: ImageAlign,
    #[serde(default)]
    pub baseline_offset: Option<CssLength>,
    #[serde(default)]
    pub source: Option<SourceSpan>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageDisplay {
    Inline,
    Block,
    #[default]
    FullPage,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageAlign {
    #[default]
    Baseline,
    Middle,
    Top,
    Bottom,
    Sub,
    Super,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum CssLength {
    Px(f32),
    Em(f32),
    Rem(f32),
    Percent(f32),
}

impl CssLength {
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim().to_ascii_lowercase();
        let (number, unit) = if let Some(number) = value.strip_suffix("rem") {
            (number, "rem")
        } else if let Some(number) = value.strip_suffix("em") {
            (number, "em")
        } else if let Some(number) = value.strip_suffix("px") {
            (number, "px")
        } else if let Some(number) = value.strip_suffix('%') {
            (number, "%")
        } else {
            (value.as_str(), "px")
        };
        let number = number.trim().parse::<f32>().ok()?;
        if !number.is_finite() {
            return None;
        }
        Some(match unit {
            "rem" => Self::Rem(number),
            "em" => Self::Em(number),
            "%" => Self::Percent(number),
            _ => Self::Px(number),
        })
    }

    pub fn resolve(self, em: f32, rem: f32, reference: f32) -> f32 {
        match self {
            Self::Px(value) => value,
            Self::Em(value) => value * em,
            Self::Rem(value) => value * rem,
            Self::Percent(value) => value * reference / 100.0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub rows: Vec<TableRow>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TableRow {
    pub cells: Vec<TableCell>,
    pub header: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TableCell {
    pub blocks: Vec<Block>,
    pub colspan: u32,
    pub rowspan: u32,
    pub header: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub id: u32,
    pub kind: BlockKind,
    pub atoms: Vec<Atom>,
    pub image: Option<ImageRef>,
    pub table: Option<Table>,
    pub list_marker: Option<String>,
    pub writing_mode: WritingMode,
    #[serde(default)]
    pub text_indent: Option<CssLength>,
    #[serde(default)]
    pub font_size: Option<CssLength>,
    #[serde(default)]
    pub margin_block_start: Option<CssLength>,
    #[serde(default)]
    pub margin_block_end: Option<CssLength>,
    /// Original MathML, to be typeset as a formula rather than flattened text.
    pub mathml: Option<String>,
}

impl Block {
    pub fn plain_text(&self) -> String {
        if let Some(table) = &self.table {
            return table
                .rows
                .iter()
                .map(|row| {
                    row.cells
                        .iter()
                        .map(|cell| {
                            cell.blocks
                                .iter()
                                .map(Block::plain_text)
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .collect::<Vec<_>>()
                        .join("\t")
                })
                .collect::<Vec<_>>()
                .join("\n");
        }
        if let Some(image) = &self.image
            && self.atoms.is_empty()
        {
            return image.alt.clone();
        }
        self.atoms.iter().map(|atom| atom.text.as_str()).collect()
    }

    pub fn spoken_text(&self) -> String {
        self.atoms.iter().map(Atom::spoken_text).collect()
    }

    pub fn first_locator(&self) -> Option<Locator> {
        self.atoms
            .iter()
            .find_map(|atom| atom.locator_at(0))
            .or_else(|| {
                self.table
                    .as_ref()?
                    .rows
                    .iter()
                    .flat_map(|row| &row.cells)
                    .flat_map(|cell| &cell.blocks)
                    .find_map(Block::first_locator)
            })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    pub id: u32,
    pub title: String,
    pub href: String,
    pub writing_mode: WritingMode,
    pub blocks: Vec<Block>,
    pub anchors: BTreeMap<String, Locator>,
    pub fixed_page: Option<FixedPage>,
    #[serde(default)]
    pub presentation: PagePresentation,
    #[serde(default = "loaded_by_default")]
    pub loaded: bool,
}

fn loaded_by_default() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FixedChar {
    pub text: String,
    pub source: SourceSpan,
    /// Top-left origin coordinates in PDF points, [left, top, right, bottom].
    pub rect: [f32; 4],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FixedPage {
    pub width: f32,
    pub height: f32,
    pub chars: Vec<FixedChar>,
}

impl Chapter {
    pub fn text_at_anchor(&self, locator: Locator) -> Option<String> {
        fn find(blocks: &[Block], locator: Locator) -> Option<String> {
            for block in blocks {
                if let Some(table) = &block.table {
                    for row in &table.rows {
                        for cell in &row.cells {
                            if let Some(text) = find(&cell.blocks, locator) {
                                return Some(text);
                            }
                        }
                    }
                }
                if block
                    .atoms
                    .iter()
                    .flat_map(|atom| &atom.sources)
                    .any(|source| {
                        source.chapter == locator.chapter
                            && source.node == locator.node
                            && source.start <= locator.offset
                            && locator.offset < source.end
                    })
                {
                    let text = block.plain_text().trim().to_owned();
                    if !text.is_empty() {
                        return Some(text);
                    }
                }
            }
            None
        }
        find(&self.blocks, locator)
    }
    /// First native reading target, including pages that contain only an image.
    pub fn start_locator(&self) -> Option<Locator> {
        if let Some(page) = &self.fixed_page {
            return Some(
                page.chars
                    .first()
                    .map(|character| character.source.start_locator())
                    .unwrap_or(Locator {
                        chapter: self.id,
                        node: u32::MAX,
                        offset: 0,
                    }),
            );
        }
        self.blocks.iter().find_map(|block| {
            if block.image.is_some() {
                if let Some(source) = block.image.as_ref().and_then(|image| image.source) {
                    return Some(source.start_locator());
                }
                Some(Locator {
                    chapter: self.id,
                    node: u32::MAX - block.id,
                    offset: 0,
                })
            } else {
                block.first_locator()
            }
        })
    }

    pub fn plain_text(&self) -> String {
        self.blocks
            .iter()
            .map(Block::plain_text)
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Reading atoms in source order, including table cells. Images are not invented text.
    pub fn atoms(&self) -> Vec<&Atom> {
        fn collect<'a>(blocks: &'a [Block], atoms: &mut Vec<&'a Atom>) {
            for block in blocks {
                atoms.extend(&block.atoms);
                if let Some(table) = &block.table {
                    for row in &table.rows {
                        for cell in &row.cells {
                            collect(&cell.blocks, atoms);
                        }
                    }
                }
            }
        }
        let mut atoms = Vec::new();
        collect(&self.blocks, &mut atoms);
        atoms
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TocEntry {
    pub title: String,
    pub chapter: u32,
    pub fragment: Option<String>,
    pub children: Vec<TocEntry>,
    /// None retains the established linear chapter/fragment destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<TocTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TocTarget {
    /// An authored section label without a readable linear destination.
    Group,
    /// An archive-root URL, including any escaped fragment, for an auxiliary
    /// document. Resolve with Book::load_link_target; never clamp to a chapter.
    Document { href: String },
}

#[derive(Clone, Debug)]
pub struct Book {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub language: String,
    pub chapters: Vec<Chapter>,
    pub toc: Vec<TocEntry>,
    pub cover: Option<String>,
    pub resources: BTreeMap<String, Vec<u8>>,
    pub page_progression: PageProgression,
    #[doc(hidden)]
    pub epub: Option<Arc<crate::EpubSource>>,
    #[doc(hidden)]
    pub resource_token: Arc<()>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub chapter: u32,
    pub block: u32,
    pub start: Locator,
    pub end: Locator,
    pub snippet: String,
}

/// A resolved local link, ready for a foreground navigation or note popup.
/// Auxiliary/non-linear documents have stable locators but are not appended to
/// the book's ordinary reading order, progress, search, or TTS chapter list.
#[derive(Clone, Debug)]
pub struct LinkedDocument {
    pub chapter: Chapter,
    pub locator: Locator,
    pub spine_index: Option<usize>,
}

impl Book {
    pub fn author(&self) -> String {
        self.authors.join("、")
    }
    pub fn resource(&self, path: &str) -> Option<&[u8]> {
        self.resources.get(path).map(Vec::as_slice)
    }

    pub fn resource_bytes(&self, path: &str) -> anyhow::Result<Option<Arc<Vec<u8>>>> {
        if let Some(bytes) = self.resources.get(path) {
            return Ok(Some(Arc::new(bytes.clone())));
        }
        if let Some(source) = &self.epub {
            return source.resource(path);
        }
        Ok(None)
    }

    pub fn load_chapter(&self, index: usize) -> anyhow::Result<Chapter> {
        let chapter = self
            .chapters
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("Chapter index is out of range"))?;
        if chapter.loaded {
            return Ok(chapter.clone());
        }
        self.epub
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No chapter loader is attached"))?
            .load_chapter(index)
    }

    pub fn ensure_chapter_loaded(&mut self, index: usize) -> anyhow::Result<()> {
        if !self
            .chapters
            .get(index)
            .is_some_and(|chapter| chapter.loaded)
        {
            let loaded = self.load_chapter(index)?;
            self.chapters[index] = loaded;
        }
        Ok(())
    }

    pub fn unload_chapters_except(&mut self, keep: &[usize]) {
        if self.epub.is_none() {
            return;
        }
        for (index, chapter) in self.chapters.iter_mut().enumerate() {
            if !keep.contains(&index) {
                chapter.blocks.clear();
                chapter.anchors.clear();
                chapter.loaded = false;
            }
        }
    }

    pub fn archive_stats(&self) -> Option<crate::ArchiveLoadStats> {
        self.epub.as_ref().map(|source| source.stats())
    }

    pub fn content_identity(&self) -> &str {
        self.epub
            .as_ref()
            .map(|source| source.identity())
            .unwrap_or(&self.id)
    }

    pub fn resource_epoch(&self) -> usize {
        Arc::as_ptr(&self.resource_token) as usize
    }

    /// A lightweight resource-only view for background image jobs. EPUBs share
    /// the archive Arc; in-memory imports copy only the requested image graph.
    pub fn image_resource_view(
        &self,
        resource: &str,
        inline_svg: Option<&str>,
        chapter_href: &str,
    ) -> Self {
        let mut resources = BTreeMap::new();
        if self.epub.is_none() {
            let mut pending = vec![resource.to_owned()];
            if let Some(svg) = inline_svg {
                pending.extend(crate::import::svg_image_paths(svg, chapter_href));
            }
            while let Some(path) = pending.pop() {
                if resources.contains_key(&path) {
                    continue;
                }
                if let Some(bytes) = self.resources.get(&path) {
                    if (path.ends_with(".svg")
                        || bytes[..bytes.len().min(4096)]
                            .windows(4)
                            .any(|window| window == b"<svg"))
                        && let Ok(svg) = std::str::from_utf8(bytes)
                        && svg.contains("<svg")
                    {
                        pending.extend(crate::import::svg_image_paths(svg, &path));
                    }
                    resources.insert(path, bytes.clone());
                }
            }
        }
        Self {
            id: self.id.clone(),
            title: String::new(),
            authors: Vec::new(),
            language: String::new(),
            chapters: Vec::new(),
            toc: Vec::new(),
            cover: None,
            resources,
            page_progression: self.page_progression,
            epub: self.epub.clone(),
            resource_token: Arc::clone(&self.resource_token),
        }
    }

    /// Resolves only links within this book. External URLs and protocols are
    /// returned as None and are never opened or fetched. Invalid archive paths
    /// are errors, including attempts to traverse above the book's root.
    pub fn resolve_link(&self, chapter: u32, link: &str) -> anyhow::Result<Option<Locator>> {
        Ok(self
            .load_link_target(chapter, link)?
            .map(|target| target.locator))
    }

    /// Worker-safe loading for an internal link, including non-spine notes.
    /// This performs ZIP/XML work only for the target, without mutating Book.
    pub fn load_link_target(
        &self,
        chapter: u32,
        link: &str,
    ) -> anyhow::Result<Option<LinkedDocument>> {
        let source_href = self
            .chapters
            .iter()
            .find(|candidate| candidate.id == chapter)
            .map(|source| source.href.as_str())
            .or_else(|| {
                self.epub
                    .as_ref()?
                    .document_spec(chapter)
                    .map(|spec| spec.href.as_str())
            });
        let Some(source_href) = source_href else {
            return Ok(None);
        };
        let link = link.trim();
        let path_part = link.split(['#', '?']).next().unwrap_or("");
        let decoded = percent_encoding::percent_decode_str(path_part).decode_utf8()?;
        if decoded.contains(':') || decoded.starts_with("//") || decoded.starts_with("\\\\") {
            return Ok(None);
        }
        let target_path = crate::import::resolve_path(source_href, link)?;
        let spine_index = self
            .chapters
            .iter()
            .position(|candidate| candidate.href == target_path);
        let target = if let Some(index) = spine_index {
            self.load_chapter(index)?
        } else if let Some(source) = &self.epub {
            let Some(spec) = source.document_spec_by_href(&target_path) else {
                return Ok(None);
            };
            source.load_chapter(spec.id as usize)?
        } else {
            return Ok(None);
        };
        let mut locator = target.start_locator();
        if let Some((_, fragment)) = link.split_once('#')
            && !fragment.is_empty()
        {
            let fragment = percent_encoding::percent_decode_str(fragment).decode_utf8()?;
            locator = target.anchors.get(fragment.as_ref()).copied();
            if locator.is_none() && fragment.eq_ignore_ascii_case("top") {
                locator = target.start_locator();
            }
        }
        Ok(locator.map(|locator| LinkedDocument {
            chapter: target,
            locator,
            spine_index,
        }))
    }

    /// Plain text of the paragraph or table cell block containing an anchor.
    /// Useful for a native footnote popover; this does not execute HTML.
    pub fn text_at_anchor(&self, locator: Locator) -> Option<String> {
        let chapter = if let Some(index) = self
            .chapters
            .iter()
            .position(|chapter| chapter.id == locator.chapter)
        {
            self.load_chapter(index).ok()?
        } else {
            let source = self.epub.as_ref()?;
            source.document_spec(locator.chapter)?;
            source.load_chapter(locator.chapter as usize).ok()?
        };
        chapter.text_at_anchor(locator)
    }

    pub fn search(&self, query: &str) -> Vec<SearchHit> {
        self.search_all(query).unwrap_or_default()
    }

    /// Complete-book search for a worker thread. Chapters load one at a time;
    /// a failed chapter is reported rather than silently producing partial hits.
    pub fn search_all(&self, query: &str) -> anyhow::Result<Vec<SearchHit>> {
        if query.is_empty() {
            return Ok(Vec::new());
        }
        fn search_blocks(blocks: &[Block], query: &str, hits: &mut Vec<SearchHit>) {
            for block in blocks {
                let text = block
                    .atoms
                    .iter()
                    .map(|a| a.text.as_str())
                    .collect::<String>();
                for (start, matched) in text.match_indices(query) {
                    let locate = |mut offset: usize| {
                        for (i, atom) in block.atoms.iter().enumerate() {
                            if offset < atom.text.len() || i + 1 == block.atoms.len() {
                                return atom.locator_at(offset);
                            }
                            offset -= atom.text.len();
                        }
                        None
                    };
                    if let (Some(begin), Some(end)) = (locate(start), locate(start + matched.len()))
                    {
                        let snippet = text.chars().take(120).collect();
                        hits.push(SearchHit {
                            chapter: begin.chapter,
                            block: block.id,
                            start: begin,
                            end,
                            snippet,
                        });
                    }
                }
                if let Some(table) = &block.table {
                    for row in &table.rows {
                        for cell in &row.cells {
                            search_blocks(&cell.blocks, query, hits);
                        }
                    }
                }
            }
        }
        let mut hits = Vec::new();
        for (index, chapter) in self.chapters.iter().enumerate() {
            let chapter = if chapter.loaded {
                std::borrow::Cow::Borrowed(chapter)
            } else {
                std::borrow::Cow::Owned(self.load_chapter(index)?)
            };
            search_blocks(&chapter.blocks, query, &mut hits);
        }
        Ok(hits)
    }

    pub fn demo() -> Self {
        crate::import::demo_book()
    }
}
