use crate::*;
use anyhow::{Context, Result, bail};
use percent_encoding::percent_decode_str;
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{Read, Seek},
    path::Path,
};

const MAX_RESOURCE: u64 = 64 * 1024 * 1024;
const MAX_BOOK_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
struct Node {
    id: u32,
    tag: String,
    attrs: BTreeMap<String, String>,
    text: Option<String>,
    svg: Option<String>,
    children: Vec<Node>,
}

impl Node {
    fn attr(&self, name: &str) -> &str {
        self.attrs.get(name).map(String::as_str).unwrap_or("")
    }
    fn descendants<'a>(&'a self, tag: &'a str) -> Box<dyn Iterator<Item = &'a Node> + 'a> {
        Box::new(self.children.iter().flat_map(move |child| {
            std::iter::once(child)
                .filter(move |node| node.tag == tag)
                .chain(child.descendants(tag))
        }))
    }
    fn text_content(&self) -> String {
        if let Some(text) = &self.text {
            return text.clone();
        }
        self.children.iter().map(Node::text_content).collect()
    }
}

fn xml_tree(xml: &str) -> Result<Node> {
    let doc = roxmltree::Document::parse(xml)?;
    fn convert(node: roxmltree::Node<'_, '_>, next: &mut u32) -> Node {
        let id = *next;
        *next += 1;
        let mut attrs = BTreeMap::new();
        for attr in node.attributes() {
            attrs.insert(attr.name().to_owned(), attr.value().to_owned());
            if attr.namespace() == Some("http://www.idpf.org/2007/ops") {
                attrs.insert(format!("epub:{}", attr.name()), attr.value().to_owned());
            }
        }
        Node {
            id,
            tag: node.tag_name().name().to_ascii_lowercase(),
            attrs,
            text: node.is_text().then(|| node.text().unwrap_or("").to_owned()),
            svg: matches!(node.tag_name().name(), "svg" | "math")
                .then(|| node.document().input_text()[node.range()].to_owned()),
            children: node
                .children()
                .filter(|child| child.is_element() || child.is_text())
                .map(|child| convert(child, next))
                .collect(),
        }
    }
    Ok(convert(doc.root_element(), &mut 0))
}

fn html_tree(html: &str) -> Node {
    let document = scraper::Html::parse_document(html);
    // A local recursive closure would need a named ego_tree dependency. Walk with
    // scraper's ElementRef and preserve every text node through its public tree.
    fn convert_element(element: scraper::ElementRef<'_>, next: &mut u32) -> Node {
        let id = *next;
        *next += 1;
        let attrs = element
            .value()
            .attrs()
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect();
        let mut children = Vec::new();
        for child in element.children() {
            if let Some(child_element) = scraper::ElementRef::wrap(child) {
                children.push(convert_element(child_element, next));
            } else if let scraper::Node::Text(text) = child.value() {
                let child_id = *next;
                *next += 1;
                children.push(Node {
                    id: child_id,
                    text: Some(text.to_string()),
                    ..Node::default()
                });
            }
        }
        let svg = matches!(element.value().name(), "svg" | "math").then(|| element.html());
        Node {
            id,
            tag: element.value().name().to_owned(),
            attrs,
            text: None,
            svg,
            children,
        }
    }
    convert_element(document.root_element(), &mut 0)
}

/// Parse an XHTML/HTML chapter while retaining ruby and source-node identities.
pub fn parse_html(html: &str, chapter: u32, href: &str, stylesheets: &str) -> Result<Chapter> {
    let tree = xml_tree(html).unwrap_or_else(|_| html_tree(html));
    let inline_css = tree
        .descendants("style")
        .map(Node::text_content)
        .collect::<Vec<_>>()
        .join("\n");
    Ok(parse_tree(
        &tree,
        chapter,
        href,
        &format!("{stylesheets}\n{inline_css}"),
    ))
}

fn decode_text(bytes: &[u8]) -> Result<String> {
    if let Some((encoding, bom)) = encoding_rs::Encoding::for_bom(bytes) {
        let (text, _, errors) = encoding.decode(&bytes[bom..]);
        if errors {
            bail!("Invalid {} text", encoding.name());
        }
        return Ok(text.into_owned());
    }
    String::from_utf8(bytes.to_vec())
        .context("Text is not UTF-8. Save it as UTF-8 or UTF-16 with a byte-order mark.")
}

pub fn open(path: impl AsRef<Path>) -> Result<Book> {
    let path = path.as_ref();
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let metadata = fs::metadata(path).with_context(|| format!("Cannot open {}", path.display()))?;
    if metadata.len() > MAX_BOOK_BYTES {
        bail!("Book is larger than the 512 MiB import limit");
    }
    if extension == "epub" {
        return open_epub(fs::File::open(path)?, String::new());
    }
    let bytes = fs::read(path)?;
    let id = blake3::hash(&bytes).to_hex().to_string();
    if extension == "cbz" {
        return crate::formats::open_cbz(std::io::Cursor::new(bytes), path, id);
    }
    if extension == "fb2" {
        return crate::formats::open_fb2(&decode_text(&bytes)?, path, id);
    }
    if matches!(extension.as_str(), "mobi" | "azw3" | "azw") {
        return crate::formats::open_kindle(path, id);
    }
    if extension == "pdf" {
        return crate::pdf::open_pdf(path, id);
    }
    let text = decode_text(&bytes)?;
    let title = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("Untitled")
        .to_owned();
    let html = match extension.as_str() {
        "md" | "markdown" => {
            let mut output = String::new();
            let options = pulldown_cmark::Options::ENABLE_TABLES
                | pulldown_cmark::Options::ENABLE_FOOTNOTES
                | pulldown_cmark::Options::ENABLE_STRIKETHROUGH
                | pulldown_cmark::Options::ENABLE_TASKLISTS;
            pulldown_cmark::html::push_html(
                &mut output,
                pulldown_cmark::Parser::new_ext(&text, options),
            );
            format!(
                "<html><head><title>{}</title></head><body>{output}</body></html>",
                escape(&title)
            )
        }
        "html" | "htm" | "xhtml" => text,
        "txt" | "text" | "" => {
            let body = text
                .replace("\r\n", "\n")
                .split("\n\n")
                .map(|paragraph| format!("<p>{}</p>", escape(paragraph).replace('\n', "<br/>")))
                .collect::<String>();
            format!(
                "<html><head><title>{}</title></head><body>{body}</body></html>",
                escape(&title)
            )
        }
        _ => bail!(
            "This build opens EPUB, HTML, Markdown, and plain text. The .{extension} format is not available yet."
        ),
    };
    let mut resources = BTreeMap::new();
    let tree = html_tree(&html);
    let parent = path.parent().unwrap_or(Path::new("."));
    let mut css = String::new();
    for link in tree.descendants("link").filter(|node| {
        node.attr("rel")
            .split_whitespace()
            .any(|rel| rel == "stylesheet")
    }) {
        if let Ok(resource) = resolve_path("index.xhtml", link.attr("href"))
            && let Ok(metadata) = fs::metadata(parent.join(&resource))
            && metadata.len() <= MAX_RESOURCE
            && let Ok(bytes) = fs::read(parent.join(&resource))
        {
            if let Ok(text) = decode_text(&bytes) {
                css.push_str(&text);
                css.push('\n');
            }
            resources.insert(resource, bytes);
        }
    }
    let mut chapter = parse_html(&html, 0, "index.xhtml", &css)?;
    for image in tree.descendants("img") {
        if let Ok(resource) = resolve_path("index.xhtml", image.attr("src")) {
            let asset = parent.join(&resource);
            if let Ok(metadata) = fs::metadata(&asset)
                && metadata.len() <= MAX_RESOURCE
                && let Ok(bytes) = fs::read(asset)
            {
                resources.insert(resource, bytes);
            }
        }
    }
    hydrate_image_dimensions(&mut chapter, |resource| {
        resources
            .get(resource)
            .and_then(|bytes| crate::image_meta::dimensions(bytes))
    });
    let inferred_title = if chapter.title.is_empty() {
        title
    } else {
        chapter.title.clone()
    };
    Ok(Book {
        id,
        title: inferred_title,
        authors: Vec::new(),
        language: "und".into(),
        toc: vec![TocEntry {
            title: chapter.title.clone(),
            chapter: 0,
            ..TocEntry::default()
        }],
        chapters: vec![chapter],
        cover: None,
        resources,
        page_progression: PageProgression::Default,
        epub: None,
        resource_token: std::sync::Arc::new(()),
    })
}

pub(crate) fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Resolve an archive URL without permitting traversal above the archive root.
pub(crate) fn resolve_path(base: &str, reference: &str) -> Result<String> {
    let reference = reference
        .split('#')
        .next()
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("");
    if reference.contains(':') {
        bail!("External resource URL is not an archive path");
    }
    let reference = percent_decode_str(reference)
        .decode_utf8()?
        .replace('\\', "/");
    if reference.contains(':') || reference.starts_with("//") {
        bail!("External resource URL is not an archive path");
    }
    let joined = if reference.starts_with('/') {
        reference.trim_start_matches('/').to_owned()
    } else if reference.is_empty() {
        base.to_owned()
    } else {
        format!(
            "{}{}",
            base.rsplit_once('/')
                .map(|(dir, _)| format!("{dir}/"))
                .unwrap_or_default(),
            reference
        )
    };
    normalize_path(&joined)
}

pub(crate) fn normalize_path(path: &str) -> Result<String> {
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => (),
            ".." => {
                if parts.pop().is_none() {
                    bail!("Archive resource escapes its root");
                }
            }
            value => {
                if value.contains('\0') {
                    bail!("Invalid archive path");
                }
                parts.push(value);
            }
        }
    }
    Ok(parts.join("/"))
}

pub(crate) fn svg_image_paths(svg: &str, base: &str) -> Vec<String> {
    let Ok(tree) = xml_tree(svg) else {
        return Vec::new();
    };
    tree.descendants("image")
        .filter_map(|image| resolve_path(base, image.attr("href")).ok())
        .collect()
}

fn rendition_name<'a>(package: &Node, property: &'a str) -> Option<&'a str> {
    const URI: &str = "http://www.idpf.org/vocab/rendition/#";
    if let Some(name) = property
        .strip_prefix("rendition:")
        .or_else(|| property.strip_prefix(URI))
    {
        return Some(name);
    }
    let (prefix, name) = property.split_once(':')?;
    let bindings: Vec<_> = package.attr("prefix").split_whitespace().collect();
    bindings
        .chunks_exact(2)
        .any(|binding| binding[0].trim_end_matches(':') == prefix && binding[1] == URI)
        .then_some(name)
}

fn rendition_layout(value: &str) -> Option<RenditionLayout> {
    match value.trim() {
        "reflowable" => Some(RenditionLayout::Reflowable),
        "pre-paginated" => Some(RenditionLayout::PrePaginated),
        _ => None,
    }
}

fn rendition_spread(value: &str) -> Option<RenditionSpread> {
    match value.trim() {
        "auto" => Some(RenditionSpread::Auto),
        "none" => Some(RenditionSpread::None),
        "landscape" => Some(RenditionSpread::Landscape),
        "portrait" => Some(RenditionSpread::Portrait),
        "both" => Some(RenditionSpread::Both),
        _ => None,
    }
}

fn package_presentation(package: &Node) -> PagePresentation {
    let mut presentation = PagePresentation::default();
    if let Some(metadata) = package.descendants("metadata").next() {
        for meta in metadata
            .descendants("meta")
            .filter(|meta| meta.attr("refines").is_empty())
        {
            let text = meta.text_content();
            match rendition_name(package, meta.attr("property")) {
                Some("layout") if presentation.layout.is_none() => {
                    presentation.layout = rendition_layout(&text)
                }
                Some("spread") if presentation.spread.is_none() => {
                    presentation.spread = rendition_spread(&text)
                }
                Some("viewport") if presentation.viewport.is_none() => {
                    presentation.viewport = parse_viewport(&text)
                }
                _ => (),
            }
        }
    }
    presentation
}

fn spine_presentation(
    package: &Node,
    mut presentation: PagePresentation,
    properties: &str,
) -> PagePresentation {
    let mut center = false;
    for property in properties.split_whitespace() {
        let local = rendition_name(package, property).unwrap_or(property);
        if let Some(layout) = local.strip_prefix("layout-").and_then(rendition_layout) {
            presentation.layout = Some(layout);
        } else if let Some(spread) = local.strip_prefix("spread-").and_then(rendition_spread) {
            presentation.spread = Some(spread);
        }
        match local {
            "page-spread-left" => presentation.placement = PagePlacement::Left,
            "page-spread-right" => presentation.placement = PagePlacement::Right,
            "page-spread-center" => {
                presentation.placement = PagePlacement::Center;
                center = true;
            }
            "spread-none" => {
                presentation.placement = PagePlacement::Center;
                center = true;
            }
            _ => (),
        }
    }
    // Center is the defined alias of spread-none; it is a single full viewport
    // regardless of token ordering or inherited global spread preferences.
    if center {
        presentation.placement = PagePlacement::Center;
        presentation.spread = Some(RenditionSpread::None);
    }
    presentation
}

fn positive_dimensions(width: f32, height: f32) -> Option<[f32; 2]> {
    (width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0)
        .then_some([width, height])
}

fn parse_viewport(content: &str) -> Option<[f32; 2]> {
    let tokens = content
        .split(|ch: char| ch.is_ascii_whitespace() || matches!(ch, '=' | ',' | ';'))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let mut width = None;
    let mut height = None;
    for pair in tokens.chunks_exact(2) {
        match pair[0].to_ascii_lowercase().as_str() {
            "width" if width.is_none() => width = pair[1].parse::<f32>().ok(),
            "height" if height.is_none() => height = pair[1].parse::<f32>().ok(),
            _ => (),
        }
    }
    positive_dimensions(width?, height?)
}

fn content_viewport(tree: &Node) -> Option<[f32; 2]> {
    if tree.tag == "svg" {
        let view_box = if tree.attr("viewBox").is_empty() {
            tree.attr("viewbox")
        } else {
            tree.attr("viewBox")
        };
        if !view_box.is_empty() {
            let values = view_box
                .split(|ch: char| ch.is_ascii_whitespace() || ch == ',')
                .filter(|part| !part.is_empty())
                .map(str::parse::<f32>)
                .collect::<std::result::Result<Vec<_>, _>>()
                .ok()?;
            if values.len() != 4 || !values.iter().all(|value| value.is_finite()) {
                return None;
            }
            return positive_dimensions(values[2], values[3]);
        }
        // Tolerate older SVG publications lacking the required viewBox when
        // absolute width/height still provide an unambiguous intrinsic size.
        let (Some(CssLength::Px(width)), Some(CssLength::Px(height))) = (
            CssLength::parse(tree.attr("width")),
            CssLength::parse(tree.attr("height")),
        ) else {
            return None;
        };
        return positive_dimensions(width, height);
    }
    let head = tree.descendants("head").next()?;
    let first = head
        .descendants("meta")
        .find(|meta| meta.attr("name").eq_ignore_ascii_case("viewport"))?;
    parse_viewport(first.attr("content"))
}

pub(crate) fn open_epub<R: Read + Seek + Send + 'static>(reader: R, id: String) -> Result<Book> {
    let source = std::sync::Arc::new(crate::EpubSource::new(reader)?);
    let id = if id.is_empty() {
        source.identity().to_owned()
    } else {
        id
    };
    let get_text = |resource: &str| -> Result<String> {
        let bytes = source
            .resource(resource)?
            .with_context(|| format!("Missing EPUB resource: {resource}"))?;
        decode_text(bytes.as_slice())
    };
    let container = xml_tree(&get_text("META-INF/container.xml")?)
        .context("Invalid EPUB container metadata")?;
    let rootfile = container
        .descendants("rootfile")
        .next()
        .context("EPUB has no package document")?;
    let opf_path = resolve_path("", rootfile.attr("full-path"))?;
    let package = xml_tree(&get_text(&opf_path)?).context("Invalid EPUB package document")?;
    let presentation = package_presentation(&package);
    let page_progression = match package
        .descendants("spine")
        .next()
        .map(|node| node.attr("page-progression-direction"))
    {
        Some("ltr") => PageProgression::Ltr,
        Some("rtl") => PageProgression::Rtl,
        _ => PageProgression::Default,
    };
    let title = package
        .descendants("title")
        .next()
        .map(Node::text_content)
        .unwrap_or_else(|| "Untitled".into());
    let authors = package
        .descendants("creator")
        .map(Node::text_content)
        .collect();
    let language = package
        .descendants("language")
        .next()
        .map(Node::text_content)
        .unwrap_or_else(|| "und".into());
    let mut manifest = HashMap::new();
    for item in package.descendants("item") {
        manifest.insert(
            item.attr("id").to_owned(),
            (
                resolve_path(&opf_path, item.attr("href"))?,
                item.attr("media-type").to_owned(),
                item.attr("properties").to_owned(),
            ),
        );
    }
    let cover_id = package
        .descendants("meta")
        .find(|node| node.attr("name") == "cover")
        .map(|node| node.attr("content"));
    let cover = manifest
        .iter()
        .find(|(item_id, (_, _, properties))| {
            properties.split_whitespace().any(|p| p == "cover-image")
                || cover_id == Some(item_id.as_str())
        })
        .map(|(_, (path, _, _))| path.clone());
    let mut chapters = Vec::new();
    let mut auxiliary_presentation = HashMap::new();
    for itemref in package.descendants("itemref") {
        let (href, media, _) = manifest.get(itemref.attr("idref")).with_context(|| {
            format!(
                "EPUB spine references missing item {}",
                itemref.attr("idref")
            )
        })?;
        if !matches!(
            media.as_str(),
            "application/xhtml+xml" | "text/html" | "image/svg+xml"
        ) {
            continue;
        }
        if itemref.attr("linear") == "no" {
            auxiliary_presentation.insert(
                href.clone(),
                spine_presentation(&package, presentation, itemref.attr("properties")),
            );
            continue;
        }
        chapters.push(Chapter {
            id: chapters.len() as u32,
            title: format!("Chapter {}", chapters.len() + 1),
            href: href.clone(),
            writing_mode: WritingMode::Horizontal,
            blocks: Vec::new(),
            anchors: BTreeMap::new(),
            fixed_page: None,
            presentation: spine_presentation(&package, presentation, itemref.attr("properties")),
            loaded: false,
        });
    }
    if chapters.is_empty() {
        bail!("EPUB has no readable XHTML or SVG spine documents");
    }
    // Build the complete document identities before parsing navigation. A
    // non-linear parent can have perfectly valid linear descendants.
    let mut auxiliary: Vec<_> = manifest
        .values()
        .filter(|(href, media, _)| {
            matches!(
                media.as_str(),
                "application/xhtml+xml" | "text/html" | "image/svg+xml"
            ) && !chapters.iter().any(|chapter| chapter.href == *href)
        })
        .collect();
    auxiliary.sort_by(|left, right| left.0.cmp(&right.0));
    auxiliary.dedup_by(|left, right| left.0 == right.0);
    let navigation_documents: Vec<_> = chapters
        .iter()
        .map(|chapter| NavigationDocument {
            id: chapter.id,
            href: chapter.href.clone(),
            linear: true,
        })
        .chain(
            auxiliary
                .iter()
                .enumerate()
                .map(|(index, (href, _, _))| NavigationDocument {
                    id: (chapters.len() + index) as u32,
                    href: href.clone(),
                    linear: false,
                }),
        )
        .collect();
    let mut toc = Vec::new();
    if let Some((href, _, _)) = manifest
        .values()
        .find(|(_, _, properties)| properties.split_whitespace().any(|p| p == "nav"))
        && let Ok(content) = get_text(href)
    {
        let nav = xml_tree(&content).unwrap_or_else(|_| html_tree(&content));
        let target = nav
            .descendants("nav")
            .find(|node| {
                node.attr("epub:type")
                    .split_whitespace()
                    .any(|v| v == "toc")
                    || node.attr("type") == "toc"
            })
            .or_else(|| nav.descendants("nav").next());
        if let Some(target) = target {
            toc = nav_entries(target, href, &navigation_documents);
        }
    }
    if toc.is_empty()
        && let Some((href, _, _)) = manifest
            .values()
            .find(|(_, media, _)| media == "application/x-dtbncx+xml")
        && let Ok(content) = get_text(href)
        && let Ok(ncx) = xml_tree(&content)
    {
        toc = ncx_entries(&ncx, href, &navigation_documents);
    }
    if toc.is_empty() {
        toc = chapters
            .iter()
            .map(|chapter| TocEntry {
                title: chapter.title.clone(),
                chapter: chapter.id,
                ..TocEntry::default()
            })
            .collect();
    }
    fn toc_titles(entries: &[TocEntry], chapters: &mut [Chapter]) {
        for entry in entries {
            if !matches!(entry.target, Some(TocTarget::Group))
                && let Some(chapter) = chapters
                    .iter_mut()
                    .find(|chapter| chapter.id == entry.chapter)
                && chapter.title.starts_with("Chapter ")
            {
                chapter.title = entry.title.clone();
            }
            toc_titles(&entry.children, chapters);
        }
    }
    toc_titles(&toc, &mut chapters);
    let mut specs: Vec<_> = chapters
        .iter()
        .map(|chapter| crate::epub::ChapterSpec {
            id: chapter.id,
            href: chapter.href.clone(),
            title: chapter.title.clone(),
            presentation: chapter.presentation,
        })
        .collect();
    // Keep auxiliary notes/resources addressable without treating them as
    // ordinary chapter progression. Register metadata only; no content read.
    for (href, _, _) in auxiliary {
        specs.push(crate::epub::ChapterSpec {
            id: specs.len() as u32,
            href: href.clone(),
            title: "Supplementary document".into(),
            presentation: auxiliary_presentation
                .get(href)
                .copied()
                .unwrap_or(presentation),
        });
    }
    source.set_chapters(specs)?;
    // Keep the established borrowed resource API useful for library covers.
    // Everything else stays compressed until it is actually requested.
    let mut resources = BTreeMap::new();
    if let Some(cover) = &cover
        && let Ok(Some(bytes)) = source.resource(cover)
    {
        resources.insert(cover.clone(), (*bytes).clone());
    }
    let mut book = Book {
        id,
        title,
        authors,
        language,
        chapters,
        toc,
        cover,
        resources,
        page_progression,
        epub: Some(source),
        resource_token: std::sync::Arc::new(()),
    };
    book.ensure_chapter_loaded(0)?;
    Ok(book)
}

pub(crate) fn parse_epub_chapter(
    source: &crate::EpubSource,
    spec: &crate::epub::ChapterSpec,
) -> Result<Chapter> {
    let bytes = source
        .resource(&spec.href)?
        .with_context(|| format!("Missing EPUB chapter: {}", spec.href))?;
    let content = decode_text(bytes.as_slice())?;
    let tree = xml_tree(&content).unwrap_or_else(|_| html_tree(&content));
    let mut css = String::new();
    for link in tree.descendants("link").filter(|node| {
        node.attr("rel")
            .split_whitespace()
            .any(|rel| rel == "stylesheet")
    }) {
        if let Ok(path) = resolve_path(&spec.href, link.attr("href"))
            && let Ok(Some(bytes)) = source.resource(&path)
            && let Ok(stylesheet) = decode_text(bytes.as_slice())
        {
            css.push_str(&stylesheet);
            css.push('\n');
        }
    }
    for style in tree.descendants("style") {
        css.push_str(&style.text_content());
        css.push('\n');
    }
    let mut chapter = parse_tree(&tree, spec.id, &spec.href, &css);
    chapter.presentation = PagePresentation {
        viewport: chapter.presentation.viewport.or(spec.presentation.viewport),
        ..spec.presentation
    };
    hydrate_image_dimensions(&mut chapter, |resource| {
        source.image_dimensions(resource).ok().flatten()
    });
    if chapter.title.starts_with("Chapter ") {
        chapter.title = spec.title.clone();
    }
    Ok(chapter)
}

fn hydrate_image_dimensions(
    chapter: &mut Chapter,
    mut dimensions: impl FnMut(&str) -> Option<[f32; 2]>,
) {
    fn image(image: &mut ImageRef, dimensions: &mut impl FnMut(&str) -> Option<[f32; 2]>) {
        if image.intrinsic_size.is_none() {
            image.intrinsic_size = image
                .inline_svg
                .as_ref()
                .and_then(|svg| crate::image_meta::dimensions(svg.as_bytes()))
                .or_else(|| dimensions(&image.resource));
        }
    }
    fn blocks(nodes: &mut [Block], dimensions: &mut impl FnMut(&str) -> Option<[f32; 2]>) {
        for block in nodes {
            if let Some(reference) = &mut block.image {
                image(reference, dimensions);
            }
            for atom in &mut block.atoms {
                if let Some(reference) = &mut atom.image {
                    image(reference, dimensions);
                }
            }
            if let Some(table) = &mut block.table {
                for row in &mut table.rows {
                    for cell in &mut row.cells {
                        blocks(&mut cell.blocks, dimensions);
                    }
                }
            }
        }
    }
    blocks(&mut chapter.blocks, &mut dimensions);
}
struct NavigationDocument {
    id: u32,
    href: String,
    linear: bool,
}

fn link_target(
    reference: &str,
    base: &str,
    documents: &[NavigationDocument],
) -> Option<(u32, Option<String>, Option<TocTarget>)> {
    if reference.trim().is_empty() {
        return None;
    }
    let path = resolve_path(base, reference).ok()?;
    let document = documents.iter().find(|document| document.href == path)?;
    let fragment = reference
        .split_once('#')
        .map(|(_, fragment)| {
            percent_decode_str(fragment)
                .decode_utf8()
                .map(|value| value.into_owned())
        })
        .transpose()
        .ok()?;
    let target = if document.linear {
        None
    } else {
        let encoded = path
            .split('/')
            .map(|part| {
                percent_encoding::utf8_percent_encode(part, percent_encoding::NON_ALPHANUMERIC)
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("/");
        let suffix = fragment
            .as_ref()
            .map(|fragment| {
                format!(
                    "#{}",
                    percent_encoding::utf8_percent_encode(
                        fragment,
                        percent_encoding::NON_ALPHANUMERIC
                    )
                )
            })
            .unwrap_or_default();
        Some(TocTarget::Document {
            href: format!("/{encoded}{suffix}"),
        })
    };
    Some((document.id, fragment, target))
}

fn navigation_entry(
    title: String,
    reference: Option<&str>,
    base: &str,
    documents: &[NavigationDocument],
    children: Vec<TocEntry>,
) -> Vec<TocEntry> {
    if let Some((chapter, fragment, target)) =
        reference.and_then(|reference| link_target(reference, base, documents))
    {
        return vec![TocEntry {
            title,
            chapter,
            fragment,
            children,
            target,
        }];
    }
    if title.is_empty() {
        return children;
    }
    vec![TocEntry {
        title,
        chapter: children.first().map_or(0, |child| child.chapter),
        fragment: None,
        children,
        target: Some(TocTarget::Group),
    }]
}

fn nav_entries(node: &Node, base: &str, documents: &[NavigationDocument]) -> Vec<TocEntry> {
    fn label_anchor(node: &Node) -> Option<&Node> {
        for child in &node.children {
            if matches!(child.tag.as_str(), "ol" | "ul" | "li") {
                continue;
            }
            if child.tag == "a" {
                return Some(child);
            }
            if let Some(anchor) = label_anchor(child) {
                return Some(anchor);
            }
        }
        None
    }
    fn heading_text(node: &Node) -> String {
        if matches!(node.tag.as_str(), "ol" | "ul" | "li") {
            return String::new();
        }
        if let Some(text) = &node.text {
            return text.clone();
        }
        node.children.iter().map(heading_text).collect()
    }
    let mut entries = Vec::new();
    for child in &node.children {
        if child.tag == "li" {
            // Never condition parsing descendants on resolving the parent's
            // link. EPUB section headings commonly use a span, and a parent
            // may legitimately point to a non-spine introductory document.
            let children = nav_entries(child, base, documents);
            let anchor = label_anchor(child);
            let title = anchor
                .map(Node::text_content)
                .unwrap_or_else(|| child.children.iter().map(heading_text).collect())
                .trim()
                .to_owned();
            entries.extend(navigation_entry(
                title,
                anchor.map(|anchor| anchor.attr("href")),
                base,
                documents,
                children,
            ));
        } else {
            entries.extend(nav_entries(child, base, documents));
        }
    }
    entries
}

fn ncx_entries(node: &Node, base: &str, documents: &[NavigationDocument]) -> Vec<TocEntry> {
    let mut entries = Vec::new();
    for child in &node.children {
        if child.tag == "navpoint" {
            let children = ncx_entries(child, base, documents);
            let reference = child
                .children
                .iter()
                .find(|node| node.tag == "content")
                .map(|node| node.attr("src"));
            let title = child
                .children
                .iter()
                .find(|node| node.tag == "navlabel")
                .map(Node::text_content)
                .unwrap_or_default()
                .trim()
                .to_owned();
            entries.extend(navigation_entry(
                title, reference, base, documents, children,
            ));
        } else {
            entries.extend(ncx_entries(child, base, documents));
        }
    }
    entries
}

#[derive(Clone, Default)]
struct ContextStyle {
    inline: InlineStyle,
    link: Option<String>,
    link_is_note: bool,
    mode: WritingMode,
    pre: bool,
    quote: bool,
    list_depth: usize,
    image: ImageStyle,
    text_indent: Option<CssLength>,
    margin_block_start: Option<CssLength>,
    margin_block_end: Option<CssLength>,
}

#[derive(Clone, Default)]
struct ImageStyle {
    display: Option<ImageDisplay>,
    width: Option<CssLength>,
    height: Option<CssLength>,
    max_width: Option<CssLength>,
    max_height: Option<CssLength>,
    aspect_ratio: Option<f32>,
    align: ImageAlign,
    baseline_offset: Option<CssLength>,
    width_set: bool,
    height_set: bool,
}

#[derive(Clone, Default)]
struct CssRule {
    selector: String,
    declarations: String,
    user_agent: bool,
}

type CascadeRank = (u8, u32, usize, usize);
type CascadeDeclarations = HashMap<String, (String, CascadeRank)>;

struct ChapterParser<'a> {
    chapter: u32,
    href: &'a str,
    block_id: u32,
    rules: Vec<CssRule>,
}

fn css_rules(css: &str) -> Vec<CssRule> {
    // Deliberately bounded typography subset. Unsupported selectors are not
    // broadened into global rules; full book CSS layout is a later capability.
    let mut rules = Vec::new();
    let mut cleaned = String::new();
    let mut cursor = 0;
    let mut comment_quote = None;
    let mut comment_escape = false;
    while cursor < css.len() {
        if comment_quote.is_none() && css[cursor..].starts_with("/*") {
            if let Some(end) = css[cursor + 2..].find("*/") {
                cursor += end + 4;
            } else {
                break;
            }
        } else {
            let ch = css[cursor..].chars().next().unwrap();
            cleaned.push(ch);
            if comment_escape {
                comment_escape = false;
            } else if ch == '\\' {
                comment_escape = true;
            } else if let Some(quote) = comment_quote {
                if ch == quote {
                    comment_quote = None;
                }
            } else if matches!(ch, '\'' | '"') {
                comment_quote = Some(ch);
            }
            cursor += ch.len_utf8();
        }
    }
    let mut rest = cleaned.as_str();
    while let Some(open) = rest.find('{') {
        // Semicolon at-rules are not part of the following selector. A leading
        // @charset used to swallow the first writing-mode rule in real books.
        let trimmed = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
        if trimmed.starts_with('@')
            && let Some(end) = trimmed
                .find(';')
                .filter(|end| *end < trimmed.find('{').unwrap_or(usize::MAX))
        {
            rest = &trimmed[end + 1..];
            continue;
        }
        let mut depth = 1;
        let mut quote = None;
        let mut escaped = false;
        let mut close = None;
        for (offset, ch) in rest[open + 1..].char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if let Some(current) = quote {
                if ch == current {
                    quote = None;
                }
                continue;
            }
            match ch {
                '\'' | '"' => quote = Some(ch),
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + 1 + offset);
                        break;
                    }
                }
                _ => (),
            }
        }
        let Some(close) = close else { break };
        let declarations = &rest[open + 1..close];
        for selector in rest[..open].split(',') {
            let selector = selector.trim();
            if !selector.is_empty() && !selector.starts_with('@') {
                rules.push(CssRule {
                    selector: selector.to_owned(),
                    declarations: declarations.to_owned(),
                    user_agent: false,
                });
            }
        }
        rest = &rest[close + 1..];
    }
    rules
}

fn simple_matches(node: &Node, selector: &str) -> bool {
    if selector
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '>' | '+' | '~' | ':' | '[' | ']'))
    {
        return false;
    }
    if selector == "*" {
        return true;
    }
    let mut tag = selector;
    let mut suffix = "";
    if let Some(index) = selector.find(['.', '#']) {
        tag = &selector[..index];
        suffix = &selector[index..];
    }
    if !tag.is_empty() && tag != node.tag {
        return false;
    }
    while !suffix.is_empty() {
        let prefix = suffix.as_bytes()[0];
        let end = suffix[1..]
            .find(['.', '#'])
            .map(|i| i + 1)
            .unwrap_or(suffix.len());
        let name = &suffix[1..end];
        if prefix == b'.'
            && !node
                .attr("class")
                .split_whitespace()
                .any(|class| class == name)
        {
            return false;
        }
        if prefix == b'#' && node.attr("id") != name {
            return false;
        }
        suffix = &suffix[end..];
    }
    true
}

fn css_typography_length(value: &str) -> Option<CssLength> {
    let length = CssLength::parse(value)?;
    if matches!(length, CssLength::Px(_))
        && !value.ends_with("px")
        && value.parse::<f32>().ok() != Some(0.)
    {
        return None;
    }
    Some(length)
}

fn computed_font_size(value: &str, parent: Option<CssLength>) -> Option<CssLength> {
    let parent = parent.unwrap_or(CssLength::Em(1.));
    let length = match value {
        "initial" | "medium" => return Some(CssLength::Em(1.)),
        "xx-small" => return Some(CssLength::Em(0.6)),
        "x-small" => return Some(CssLength::Em(0.75)),
        "small" => return Some(CssLength::Em(0.89)),
        "large" => return Some(CssLength::Em(1.2)),
        "x-large" => return Some(CssLength::Em(1.5)),
        "xx-large" => return Some(CssLength::Em(2.)),
        "larger" => CssLength::Em(1.2),
        "smaller" => CssLength::Em(1. / 1.2),
        _ => css_typography_length(value)?,
    };
    let value = match length {
        CssLength::Px(value) => CssLength::Px(value),
        CssLength::Rem(value) => CssLength::Em(value),
        CssLength::Em(value) | CssLength::Percent(value) => {
            let factor = if matches!(length, CssLength::Percent(_)) {
                value / 100.
            } else {
                value
            };
            match parent {
                CssLength::Px(parent) => CssLength::Px(parent * factor),
                CssLength::Em(parent) | CssLength::Rem(parent) => CssLength::Em(parent * factor),
                CssLength::Percent(parent) => CssLength::Em(parent * factor / 100.),
            }
        }
    };
    (value.resolve(1., 1., 1.) >= 0.).then_some(value)
}

fn expand_margin_declaration(name: String, raw: &str) -> Vec<(String, String)> {
    if !matches!(name.as_str(), "margin" | "margin-block") {
        return vec![(name, raw.to_owned())];
    }
    let important = raw.to_ascii_lowercase().ends_with("!important");
    let raw = raw.to_ascii_lowercase();
    let value = raw.trim_end_matches("!important").trim();
    let mut values = value.split_whitespace().collect::<Vec<_>>();
    if values.is_empty()
        || values.iter().any(|part| {
            css_typography_length(part).is_none()
                && !matches!(*part, "inherit" | "initial" | "unset" | "auto")
        })
    {
        return Vec::new();
    }
    let names = if name == "margin-block" {
        if values.len() > 2 {
            return Vec::new();
        }
        if values.len() == 1 {
            values.push(values[0]);
        }
        ["margin-block-start", "margin-block-end"].as_slice()
    } else {
        if values.len() > 4 {
            return Vec::new();
        }
        values = match values.len() {
            1 => vec![values[0]; 4],
            2 => vec![values[0], values[1], values[0], values[1]],
            3 => vec![values[0], values[1], values[2], values[1]],
            _ => values,
        };
        ["margin-top", "margin-right", "margin-bottom", "margin-left"].as_slice()
    };
    names
        .iter()
        .zip(values)
        .map(|(name, value)| {
            (
                (*name).into(),
                format!("{value}{}", if important { " !important" } else { "" }),
            )
        })
        .collect()
}

fn apply_declarations(
    context: &mut ContextStyle,
    declarations: &str,
    inherited: &ContextStyle,
) -> bool {
    let mut hidden = false;
    for declaration in declarations.split(';') {
        let Some((name, value)) = declaration.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_ascii_lowercase();
        let value = value.trim_end_matches("!important").trim().to_owned();
        if value == "inherit" {
            match name.as_str() {
                "margin-block-start" => context.margin_block_start = inherited.margin_block_start,
                "margin-block-end" => context.margin_block_end = inherited.margin_block_end,
                _ => (),
            }
            continue;
        }
        if value == "unset" {
            continue;
        }
        match name.as_str() {
            "writing-mode" | "-epub-writing-mode" | "-webkit-writing-mode" => {
                if value == "vertical-rl" || value == "tb-rl" {
                    context.mode = WritingMode::VerticalRl;
                } else if value == "horizontal-tb" || value == "lr-tb" {
                    context.mode = WritingMode::Horizontal;
                }
            }
            "font-weight" => {
                context.inline.bold =
                    value == "bold" || value.parse::<u32>().is_ok_and(|v| v >= 600)
            }
            "font-size" => {
                if let Some(size) = computed_font_size(&value, inherited.inline.font_size) {
                    context.inline.font_size = Some(size);
                }
            }
            "margin-block-start" | "margin-block-end" => {
                let length = if matches!(value.as_str(), "initial" | "auto") {
                    Some(CssLength::Px(0.))
                } else {
                    CssLength::parse(&value)
                };
                if name == "margin-block-start" {
                    context.margin_block_start = length;
                } else {
                    context.margin_block_end = length;
                }
            }
            "font-style" => context.inline.italic = value == "italic" || value == "oblique",
            "text-decoration" => context.inline.strike = value.contains("line-through"),
            "text-emphasis"
            | "text-emphasis-style"
            | "-webkit-text-emphasis"
            | "-epub-text-emphasis-style" => {
                context.inline.emphasis = !value.split_whitespace().any(|part| part == "none")
            }
            "text-combine-upright" | "-webkit-text-combine" | "-epub-text-combine" => {
                context.inline.tate_chu_yoko = value != "none"
            }
            "vertical-align" if matches!(value.as_str(), "super" | "sub" | "baseline") => {
                context.inline.superscript = value == "super";
                context.inline.subscript = value == "sub";
            }
            "white-space" => {
                context.pre = matches!(value.as_str(), "pre" | "pre-wrap" | "break-spaces")
            }
            "text-indent" => {
                if value == "initial" {
                    context.text_indent = Some(CssLength::Px(0.0));
                } else if let Some(indent) = CssLength::parse(&value) {
                    context.text_indent = Some(indent);
                }
            }
            "width" => {
                context.image.width_set = true;
                context.image.width = CssLength::parse(&value);
            }
            "height" => {
                context.image.height_set = true;
                context.image.height = CssLength::parse(&value);
            }
            "max-width" => context.image.max_width = CssLength::parse(&value),
            "max-height" => context.image.max_height = CssLength::parse(&value),
            "aspect-ratio" => {
                let mut values = value
                    .split('/')
                    .map(str::trim)
                    .filter_map(|part| part.parse::<f32>().ok());
                let numerator = values.next().unwrap_or(0.0);
                let denominator = values.next().unwrap_or(1.0);
                if numerator > 0.0 && denominator > 0.0 {
                    context.image.aspect_ratio = Some(numerator / denominator);
                }
            }
            "display" if matches!(value.as_str(), "inline" | "inline-block") => {
                context.image.display = Some(ImageDisplay::Inline)
            }
            "display" if value == "block" => context.image.display = Some(ImageDisplay::Block),
            "display" if value == "none" => hidden = true,
            _ => (),
        }
    }
    hidden
}

fn parse_tree(tree: &Node, chapter: u32, href: &str, stylesheets: &str) -> Chapter {
    let mut rules = css_rules(crate::DEFAULT_READER_CSS);
    for rule in &mut rules {
        rule.user_agent = true;
    }
    rules.extend(css_rules(stylesheets));
    let mut parser = ChapterParser {
        chapter,
        href,
        block_id: 0,
        rules,
    };
    let mut context = ContextStyle::default();
    parser.style(tree, &mut context);
    let body = if tree.tag == "body" {
        tree
    } else {
        tree.descendants("body").next().unwrap_or(tree)
    };
    if !std::ptr::eq(tree, body) {
        parser.style(body, &mut context);
    }
    let mode = context.mode;
    let title = tree
        .descendants("title")
        .next()
        .map(Node::text_content)
        .or_else(|| tree.descendants("h1").next().map(Node::text_content))
        .unwrap_or_else(|| format!("Chapter {}", chapter + 1));
    let blocks = if tree.tag == "svg" {
        let mut block = parser.block(BlockKind::Image, Vec::new(), &context);
        block.image = parser
            .image_ref(tree, &context, ImageDisplay::FullPage)
            .map(|mut image| {
                image.display = ImageDisplay::FullPage;
                image.intrinsic_size = content_viewport(tree);
                image
            });
        vec![block]
    } else {
        parser.blocks(body, &context)
    };
    let mut anchors = BTreeMap::new();
    fn readable_offsets(blocks: &[Block], offsets: &mut BTreeMap<u32, u32>) {
        for block in blocks {
            for source in block
                .atoms
                .iter()
                .flat_map(|atom| &atom.sources)
                .chain(block.image.as_ref().and_then(|image| image.source).as_ref())
            {
                offsets
                    .entry(source.node)
                    .and_modify(|start| *start = (*start).min(source.start))
                    .or_insert(source.start);
            }
            if let Some(table) = &block.table {
                for row in &table.rows {
                    for cell in &row.cells {
                        readable_offsets(&cell.blocks, offsets);
                    }
                }
            }
        }
    }
    let mut offsets = BTreeMap::new();
    readable_offsets(&blocks, &mut offsets);
    fn flatten<'a>(node: &'a Node, all: &mut Vec<&'a Node>) {
        all.push(node);
        if !matches!(
            node.tag.as_str(),
            "head" | "script" | "style" | "rt" | "rp" | "rtc"
        ) {
            for child in &node.children {
                flatten(child, all);
            }
        }
    }
    let mut nodes = Vec::new();
    flatten(tree, &mut nodes);
    for (index, node) in nodes.iter().enumerate() {
        let anchor = if !node.attr("id").is_empty() {
            node.attr("id")
        } else if node.tag == "a" {
            node.attr("name")
        } else {
            ""
        };
        if !anchor.is_empty()
            && let Some(text) = nodes[index..]
                .iter()
                .find(|node| offsets.contains_key(&node.id))
        {
            anchors.insert(
                anchor.to_owned(),
                Locator {
                    chapter,
                    node: text.id,
                    offset: offsets[&text.id],
                },
            );
        }
    }
    if tree.tag == "svg" {
        // The native SVG renderer presents the outer SVG as one image. Preserve
        // fragment navigation at page precision without inventing text geometry.
        let page = Locator {
            chapter,
            node: tree.id,
            offset: 0,
        };
        for node in &nodes {
            if !node.attr("id").is_empty() {
                anchors.insert(node.attr("id").to_owned(), page);
            }
        }
    }
    Chapter {
        id: chapter,
        title,
        href: href.to_owned(),
        writing_mode: mode,
        blocks,
        anchors,
        fixed_page: None,
        presentation: PagePresentation {
            viewport: content_viewport(tree),
            ..PagePresentation::default()
        },
        loaded: true,
    }
}

impl ChapterParser<'_> {
    fn image_ref(
        &self,
        node: &Node,
        context: &ContextStyle,
        default: ImageDisplay,
    ) -> Option<ImageRef> {
        let svg = node.tag == "svg";
        let reference = if node.tag == "img" {
            node.attr("src")
        } else {
            node.attr("href")
        };
        let resource = if svg {
            format!("inline-svg:{}:{}", self.chapter, node.id)
        } else {
            resolve_path(self.href, reference).ok()?
        };
        let alt = if !node.attr("alt").is_empty() {
            node.attr("alt").to_owned()
        } else if svg {
            node.descendants("title")
                .next()
                .map(Node::text_content)
                .unwrap_or_else(|| node.attr("aria-label").to_owned())
        } else {
            node.attr("aria-label").to_owned()
        };
        let width = if context.image.width_set {
            context.image.width
        } else {
            CssLength::parse(node.attr("width"))
        };
        let height = if context.image.height_set {
            context.image.height
        } else {
            CssLength::parse(node.attr("height"))
        };
        let mut display = context.image.display.unwrap_or(default);
        if display == ImageDisplay::FullPage && (width.is_some() || height.is_some()) {
            display = ImageDisplay::Block;
        }
        let intrinsic = svg.then(|| content_viewport(node)).flatten();
        Some(ImageRef {
            resource,
            source: Some(SourceSpan {
                chapter: self.chapter,
                node: node.id,
                start: 0,
                end: if alt.is_empty() {
                    '\u{fffc}'.len_utf8() as u32
                } else {
                    alt.len() as u32
                },
            }),
            alt,
            title: (!node.attr("title").is_empty()).then(|| node.attr("title").to_owned()),
            inline_svg: svg.then(|| node.svg.clone()).flatten(),
            display,
            width,
            height,
            max_width: context.image.max_width,
            max_height: context.image.max_height,
            aspect_ratio: context.image.aspect_ratio,
            intrinsic_size: intrinsic,
            vertical_align: context.image.align,
            baseline_offset: context.image.baseline_offset,
            ..ImageRef::default()
        })
    }

    fn image_atom(&self, _node: &Node, context: &ContextStyle, image: ImageRef) -> Atom {
        Atom {
            text: if image.alt.is_empty() {
                "\u{fffc}".into()
            } else {
                image.alt.clone()
            },
            sources: image.source.into_iter().collect(),
            image: Some(Box::new(image)),
            style: context.inline.clone(),
            link: context.link.clone(),
            link_is_note: context.link_is_note,
            ..Atom::default()
        }
    }

    fn style(&self, node: &Node, context: &mut ContextStyle) -> bool {
        let inherited = context.clone();
        // Margins are not inherited; explicit `inherit` is handled separately.
        context.margin_block_start = None;
        context.margin_block_end = None;
        // Image box properties are not inherited from a parent div/span.
        context.image = ImageStyle::default();
        match node.tag.as_str() {
            "b" | "strong" => context.inline.bold = true,
            "i" | "em" => context.inline.italic = true,
            "code" | "kbd" | "samp" => context.inline.code = true,
            "s" | "strike" | "del" => context.inline.strike = true,
            "sup" => context.inline.superscript = true,
            "sub" => context.inline.subscript = true,
            "pre" => {
                context.pre = true;
                context.inline.code = true;
            }
            "blockquote" => context.quote = true,
            "a" => {
                context.link = Some(node.attr("href").to_owned());
                context.link_is_note = node
                    .attr("epub:type")
                    .split_whitespace()
                    .any(|value| value == "noteref")
                    || node
                        .attr("role")
                        .split_whitespace()
                        .any(|value| value == "doc-noteref")
                    || node
                        .attr("rel")
                        .split_whitespace()
                        .any(|value| value == "footnote");
            }
            _ => (),
        }
        // Apply the cascade for supported properties. Author declarations beat
        // semantic defaults; !important, specificity, then source order decide.
        let mut winners = CascadeDeclarations::new();
        let mut apply = |declarations: &str, specificity: u32, order: usize, user_agent: bool| {
            // Own the map data below: declarations belong to either a rule or
            // inline attributes, both alive for this call.
            for (declaration_order, declaration) in declarations.split(';').enumerate() {
                let Some((name, raw)) = declaration.split_once(':') else {
                    continue;
                };
                let name = name.trim().to_ascii_lowercase();
                let name = match name.as_str() {
                    "-epub-writing-mode" | "-webkit-writing-mode" => "writing-mode".to_owned(),
                    "-webkit-text-combine" | "-epub-text-combine" => {
                        "text-combine-upright".to_owned()
                    }
                    "text-emphasis" | "-webkit-text-emphasis" | "-epub-text-emphasis-style" => {
                        "text-emphasis-style".to_owned()
                    }
                    _ => name,
                };
                let raw = raw.trim();
                let important = raw.to_ascii_lowercase().ends_with("!important");
                let origin = match (user_agent, important) {
                    (true, false) => 0,
                    (false, false) => 1,
                    (false, true) => 2,
                    (true, true) => 3,
                };
                let rank = (origin, specificity, order, declaration_order);
                for (name, raw) in expand_margin_declaration(name, raw) {
                    let normalized = raw.trim().to_ascii_lowercase();
                    let normalized = normalized.trim_end_matches("!important").trim();
                    if name == "font-size"
                        && !matches!(normalized, "inherit" | "unset")
                        && computed_font_size(normalized, None).is_none()
                    {
                        continue;
                    }
                    if name.starts_with("margin-")
                        && !matches!(normalized, "inherit" | "unset" | "initial" | "auto")
                        && css_typography_length(normalized).is_none()
                    {
                        continue;
                    }
                    if winners
                        .get(&name)
                        .is_none_or(|(_, previous)| rank >= *previous)
                    {
                        winners.insert(name, (raw, rank));
                    }
                }
            }
        };
        for (order, rule) in self.rules.iter().enumerate() {
            if simple_matches(node, &rule.selector) {
                let specificity = rule.selector.matches('#').count() as u32 * 10_000
                    + rule.selector.matches('.').count() as u32 * 100
                    + u32::from(
                        rule.selector
                            .chars()
                            .next()
                            .is_some_and(char::is_alphabetic),
                    );
                apply(&rule.declarations, specificity, order, rule.user_agent);
            }
        }
        apply(node.attr("style"), 1_000_000, self.rules.len(), false);
        let vertical = winners
            .get("writing-mode")
            .and_then(|(value, _)| {
                let value = value.to_ascii_lowercase();
                match value.trim_end_matches("!important").trim() {
                    "vertical-rl" | "tb-rl" => Some(true),
                    "horizontal-tb" | "lr-tb" => Some(false),
                    _ => None,
                }
            })
            .unwrap_or(context.mode.is_vertical());
        for (physical, logical) in if vertical {
            [
                ("margin-right", "margin-block-start"),
                ("margin-left", "margin-block-end"),
            ]
        } else {
            [
                ("margin-top", "margin-block-start"),
                ("margin-bottom", "margin-block-end"),
            ]
        } {
            if let Some((value, rank)) = winners.get(physical).cloned()
                && winners
                    .get(logical)
                    .is_none_or(|(_, previous)| rank >= *previous)
            {
                winners.insert(logical.into(), (value, rank));
            }
        }
        let declarations = winners
            .into_iter()
            .map(|(name, (value, _))| format!("{name}:{value}"))
            .collect::<Vec<_>>()
            .join(";");
        let hidden = apply_declarations(context, &declarations, &inherited);
        if let Some((_, value)) = declarations
            .split(';')
            .filter_map(|value| value.split_once(':'))
            .find(|(name, _)| *name == "vertical-align")
        {
            context.image.align = match value.trim().trim_end_matches("!important").trim() {
                "middle" => ImageAlign::Middle,
                "top" | "text-top" => ImageAlign::Top,
                "bottom" | "text-bottom" => ImageAlign::Bottom,
                "sub" => ImageAlign::Sub,
                "super" => ImageAlign::Super,
                _ => ImageAlign::Baseline,
            };
            context.image.baseline_offset =
                CssLength::parse(value.trim().trim_end_matches("!important").trim());
        }
        hidden
            || node.attrs.contains_key("hidden")
            || matches!(
                node.tag.as_str(),
                "head" | "script" | "style" | "rt" | "rp" | "rtc"
            )
    }

    fn block(&mut self, kind: BlockKind, atoms: Vec<Atom>, context: &ContextStyle) -> Block {
        let id = self.block_id;
        self.block_id += 1;
        Block {
            id,
            kind,
            atoms,
            image: None,
            table: None,
            list_marker: None,
            writing_mode: context.mode,
            text_indent: context.text_indent,
            font_size: context.inline.font_size,
            margin_block_start: context.margin_block_start,
            margin_block_end: context.margin_block_end,
            mathml: None,
        }
    }

    fn flush(&mut self, pending: &mut Vec<Atom>, blocks: &mut Vec<Block>, context: &ContextStyle) {
        trim_atoms(pending, context.pre);
        if !pending.is_empty() {
            let kind = if context.pre {
                BlockKind::Code
            } else if context.quote {
                BlockKind::Quote
            } else {
                BlockKind::Paragraph
            };
            blocks.push(self.block(kind, std::mem::take(pending), context));
        }
    }

    fn blocks(&mut self, node: &Node, context: &ContextStyle) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut pending = Vec::new();
        for child in &node.children {
            let mut current = context.clone();
            if self.style(child, &mut current) {
                continue;
            }
            match child.tag.as_str() {
                "p" | "div" | "section" | "article" | "main" | "aside" | "figure"
                | "figcaption" | "header" | "footer" => {
                    self.flush(&mut pending, &mut blocks, context);
                    blocks.extend(self.blocks(child, &current));
                }
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    self.flush(&mut pending, &mut blocks, context);
                    let mut atoms = self.inline_children(child, &current);
                    trim_atoms(&mut atoms, false);
                    let level = child.tag.as_bytes()[1] - b'0';
                    blocks.push(self.block(BlockKind::Heading(level), atoms, &current));
                }
                "pre" | "blockquote" => {
                    self.flush(&mut pending, &mut blocks, context);
                    blocks.extend(self.blocks(child, &current));
                }
                "ol" | "ul" => {
                    self.flush(&mut pending, &mut blocks, context);
                    let start = child.attr("start").parse::<u32>().unwrap_or(1);
                    for (index, item) in child
                        .children
                        .iter()
                        .filter(|node| node.tag == "li")
                        .enumerate()
                    {
                        let number = start.saturating_add(index as u32);
                        let mut item_context = current.clone();
                        item_context.list_depth += 1;
                        self.style(item, &mut item_context);
                        let mut item_blocks = self.blocks(item, &item_context);
                        let marker = if child.tag == "ol" {
                            format!("{number}.")
                        } else {
                            "•".into()
                        };
                        if let Some(first) = item_blocks.first_mut() {
                            first.kind = BlockKind::ListItem;
                            first.list_marker =
                                Some(format!("{}{}", "  ".repeat(current.list_depth), marker));
                        }
                        blocks.extend(item_blocks);
                    }
                }
                "img" | "image" | "svg" => {
                    let inline_context = matches!(
                        node.tag.as_str(),
                        "p" | "span" | "a" | "li" | "td" | "th" | "em" | "strong" | "b" | "i"
                    ) || pending
                        .iter()
                        .any(|atom: &Atom| atom.image.is_some() || !atom.text.trim().is_empty())
                        || node.children.iter().any(|child| {
                            child
                                .text
                                .as_ref()
                                .is_some_and(|text| !text.trim().is_empty())
                        });
                    let default = if inline_context {
                        ImageDisplay::Inline
                    } else if node.tag == "body"
                        && node
                            .children
                            .iter()
                            .filter(|child| !child.tag.is_empty())
                            .count()
                            == 1
                    {
                        ImageDisplay::FullPage
                    } else {
                        ImageDisplay::Block
                    };
                    if let Some(image) = self.image_ref(child, &current, default) {
                        if image.display == ImageDisplay::Inline {
                            pending.push(self.image_atom(child, &current, image));
                        } else {
                            self.flush(&mut pending, &mut blocks, context);
                            let mut block = self.block(BlockKind::Image, Vec::new(), &current);
                            block.image = Some(image);
                            blocks.push(block);
                        }
                    }
                }
                "table" => {
                    self.flush(&mut pending, &mut blocks, context);
                    for caption in child.children.iter().filter(|node| node.tag == "caption") {
                        blocks.extend(self.blocks(caption, &current));
                    }
                    let mut rows = Vec::new();
                    let direct_rows = child
                        .children
                        .iter()
                        .flat_map(|node| {
                            if matches!(node.tag.as_str(), "thead" | "tbody" | "tfoot") {
                                node.children.iter().collect::<Vec<_>>()
                            } else {
                                vec![node]
                            }
                        })
                        .filter(|node| node.tag == "tr");
                    for row in direct_rows {
                        let mut cells = Vec::new();
                        for cell in row
                            .children
                            .iter()
                            .filter(|node| matches!(node.tag.as_str(), "td" | "th"))
                        {
                            let mut cell_context = current.clone();
                            self.style(cell, &mut cell_context);
                            cell_context.inline.bold |= cell.tag == "th";
                            cells.push(TableCell {
                                blocks: self.blocks(cell, &cell_context),
                                colspan: cell
                                    .attr("colspan")
                                    .parse::<u32>()
                                    .unwrap_or(1)
                                    .clamp(1, 1000),
                                rowspan: cell
                                    .attr("rowspan")
                                    .parse::<u32>()
                                    .unwrap_or(1)
                                    .clamp(1, 1000),
                                header: cell.tag == "th",
                            });
                        }
                        if !cells.is_empty() {
                            rows.push(TableRow {
                                header: cells.iter().all(|cell| cell.header),
                                cells,
                            });
                        }
                    }
                    let mut block = self.block(BlockKind::Table, Vec::new(), &current);
                    block.table = Some(Table { rows });
                    blocks.push(block);
                }
                "math" => {
                    self.flush(&mut pending, &mut blocks, context);
                    let mut block = self.block(
                        BlockKind::Paragraph,
                        self.inline_children(child, &current),
                        &current,
                    );
                    block.mathml = child.svg.clone();
                    blocks.push(block);
                }
                "hr" => {
                    self.flush(&mut pending, &mut blocks, context);
                    blocks.push(self.block(BlockKind::Rule, Vec::new(), &current));
                }
                _ => pending.extend(self.inline(child, &current)),
            }
        }
        self.flush(&mut pending, &mut blocks, context);
        blocks
    }

    fn inline_children(&self, node: &Node, context: &ContextStyle) -> Vec<Atom> {
        let mut atoms = Vec::new();
        for child in &node.children {
            let mut current = context.clone();
            if self.style(child, &mut current) {
                continue;
            }
            atoms.extend(self.inline(child, &current));
        }
        atoms
    }

    fn inline(&self, node: &Node, context: &ContextStyle) -> Vec<Atom> {
        if matches!(node.tag.as_str(), "img" | "image" | "svg") {
            return self
                .image_ref(node, context, ImageDisplay::Inline)
                .map(|image| self.image_atom(node, context, image))
                .into_iter()
                .collect();
        }
        if let Some(text) = &node.text {
            return self.text_atoms(node.id, text, context);
        }
        if node.tag == "br" {
            return vec![Atom {
                text: "\n".into(),
                ..Atom::default()
            }];
        }
        if node.tag == "ruby" {
            let reading_nodes: Vec<_> = node
                .children
                .iter()
                .filter(|child| child.tag == "rt")
                .collect();
            let base_nodes: Vec<_> = node
                .children
                .iter()
                .filter(|child| child.tag == "rb")
                .collect();
            // Explicit rb/rt pairs retain mono-ruby boundaries for pinpoint speech
            // and hit testing. One rt spanning several base nodes remains a group.
            if base_nodes.len() > 1 && base_nodes.len() == reading_nodes.len() {
                return base_nodes
                    .into_iter()
                    .zip(reading_nodes)
                    .filter_map(|(base, reading)| {
                        self.ruby_atom(
                            self.inline_children(base, context),
                            reading.text_content(),
                            context,
                        )
                    })
                    .collect();
            }
            if reading_nodes.len() > 1 {
                let mut output = Vec::new();
                let mut pending = Vec::new();
                for child in &node.children {
                    if child.tag == "rt" {
                        if let Some(atom) = self.ruby_atom(
                            std::mem::take(&mut pending),
                            child.text_content(),
                            context,
                        ) {
                            output.push(atom);
                        }
                    } else if !matches!(child.tag.as_str(), "rp" | "rtc") {
                        let mut current = context.clone();
                        if !self.style(child, &mut current) {
                            pending.extend(self.inline(child, &current));
                        }
                    }
                }
                output.extend(pending);
                return output;
            }
            let bases = self.inline_children(node, context);
            let ruby = node
                .descendants("rt")
                .map(Node::text_content)
                .collect::<String>();
            return self.ruby_atom(bases, ruby, context).into_iter().collect();
        }
        self.inline_children(node, context)
    }

    fn ruby_atom(
        &self,
        mut bases: Vec<Atom>,
        ruby: String,
        context: &ContextStyle,
    ) -> Option<Atom> {
        trim_atoms(&mut bases, false);
        let text = bases
            .iter()
            .map(|atom| atom.text.as_str())
            .collect::<String>();
        if text.is_empty() {
            return None;
        }
        let style = bases
            .first()
            .map(|base| base.style.clone())
            .unwrap_or_else(|| context.inline.clone());
        let image = if bases.len() == 1 {
            bases[0].image.clone()
        } else {
            None
        };
        Some(Atom {
            text,
            ruby: (!ruby.trim().is_empty()).then(|| ruby.trim().to_owned()),
            sources: bases.into_iter().flat_map(|atom| atom.sources).collect(),
            style,
            link: context.link.clone(),
            link_is_note: context.link_is_note,
            image,
        })
    }

    fn text_atoms(&self, node: u32, text: &str, context: &ContextStyle) -> Vec<Atom> {
        let make = |text: &str, start: usize| Atom {
            text: text.to_owned(),
            ruby: None,
            sources: vec![SourceSpan {
                chapter: self.chapter,
                node,
                start: start as u32,
                end: (start + text.len()) as u32,
            }],
            style: context.inline.clone(),
            link: context.link.clone(),
            link_is_note: context.link_is_note,
            image: None,
        };
        if context.pre {
            return if text.is_empty() {
                Vec::new()
            } else {
                vec![make(text, 0)]
            };
        }
        let mut atoms = Vec::new();
        let mut run_start = None;
        let mut was_space = false;
        for (offset, ch) in text.char_indices() {
            let collapsible = matches!(ch, ' ' | '\n' | '\r' | '\t' | '\u{000c}');
            if collapsible {
                if let Some(start) = run_start.take() {
                    atoms.push(make(&text[start..offset], start));
                }
                if !was_space {
                    atoms.push(make(" ", offset));
                }
            } else if run_start.is_none() {
                run_start = Some(offset);
            }
            was_space = collapsible;
        }
        if let Some(start) = run_start {
            atoms.push(make(&text[start..], start));
        }
        atoms
    }
}

fn trim_atoms(atoms: &mut Vec<Atom>, preserve: bool) {
    if preserve {
        return;
    }
    let mut output: Vec<Atom> = Vec::with_capacity(atoms.len());
    for atom in atoms.drain(..) {
        if atom.text == " "
            && (output.is_empty() || output.last().is_some_and(|a| a.text.ends_with([' ', '\n'])))
        {
            continue;
        }
        output.push(atom);
    }
    while output.last().is_some_and(|a| a.text == " ") {
        output.pop();
    }
    *atoms = output;
}

pub(crate) fn demo_book() -> Book {
    const NOVEL: &str = r#"<html lang="ja"><head><title>頁をひらく</title></head><body style="writing-mode:vertical-rl">
    <h1>頁をひらく</h1><p>朝の<ruby>光<rt>ひかり</rt></ruby>が、窓辺の本をそっと照らしていた。</p>
    <p><ruby>栞<rt>しおり</rt></ruby>を挟んだ頁をひらく。活字の向こうに、まだ知らない景色がある。</p>
    <p>「声にして読めば、ことばは少し違って聞こえる。」</p>
    <p>文字の大きさや余白を整えて、自分の読みやすい一冊に。縦書きと横書きを切り替えてみてください。</p>
    <p><ruby>東京<rt>とうきょう</rt></ruby>から届いた手紙には、<span style="text-combine-upright:all">12</span>月の約束が記されていた。</p>
    <p>読み上げは、準備できた音声エンジンから始まります。文を選んで、そこから声をたどることもできます。</p>
    </body></html>"#;
    const TECH: &str = r#"<html lang="ja"><head><title>読むための道具</title></head><body>
    <h1>読むための道具</h1><p>本は、物語だけではありません。<strong>技術書</strong>のコードや表も、構造を保って読み進めます。</p>
    <h2>短いRustの例</h2><pre><code>fn main() {
    let message = "こんにちは、読書の時間です";
    println!("{message}");
}</code></pre>
    <blockquote><p>読みやすさは、必要な情報へ迷わず戻れることから始まります。</p></blockquote>
    <ul><li>目次から章へ移動する</li><li>本文を選択し、辞書で調べる</li><li>読んだ場所に栞を残す</li></ul>
    <h2>表示の設定</h2><table><thead><tr><th>項目</th><th>使い方</th></tr></thead><tbody>
    <tr><td>フォント</td><td>本文の書体と文字サイズを変更</td></tr>
    <tr><td>余白</td><td>画面に合わせて読み幅を調整</td></tr>
    <tr><td>読み上げ</td><td>利用できる音声で再生</td></tr></tbody></table>
    <p>EPUB、テキスト、HTML、Markdownを開けます。未実装の形式は対応済みとは表示しません。</p>
    </body></html>"#;
    let chapters = vec![
        parse_html(NOVEL, 0, "demo/novel.xhtml", "").expect("valid demo"),
        parse_html(TECH, 1, "demo/tech.xhtml", "").expect("valid demo"),
    ];
    Book {
        id: "reader-demo-v1".into(),
        title: "頁をひらく".into(),
        authors: vec!["EPUB Reader".into()],
        language: "ja".into(),
        toc: chapters
            .iter()
            .map(|chapter| TocEntry {
                title: chapter.title.clone(),
                chapter: chapter.id,
                ..TocEntry::default()
            })
            .collect(),
        chapters,
        cover: None,
        resources: BTreeMap::new(),
        page_progression: PageProgression::Rtl,
        epub: None,
        resource_token: std::sync::Arc::new(()),
    }
}

#[cfg(test)]
mod charset_regression {
    #[test]
    fn charset_before_the_first_rule_does_not_erase_vertical_writing_mode() {
        for prefix in [
            "@charset \"UTF-8\";",
            "\u{feff}@charset \"UTF-8\"; @namespace epub url(http://www.idpf.org/2007/ops);",
        ] {
            let css = format!(
                "{prefix} .class-0 {{ -webkit-writing-mode: vertical-rl; writing-mode: vertical-rl; }}"
            );
            let chapter = super::parse_html(
                "<html><body class='class-0'><p>縦書きの本文。</p></body></html>",
                0,
                "chapter.xhtml",
                &css,
            )
            .unwrap();
            assert_eq!(chapter.writing_mode, crate::WritingMode::VerticalRl);
        }
    }
}
