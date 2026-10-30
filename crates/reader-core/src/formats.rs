use crate::import::{escape, normalize_path, open_epub};
use crate::*;
use anyhow::{Context, Result, bail};
use base64::Engine;
use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap},
    fs,
    io::{Read, Seek},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

pub(crate) fn image_block(resource: String, alt: String) -> Block {
    Block {
        id: 0,
        kind: BlockKind::Image,
        atoms: Vec::new(),
        image: Some(ImageRef {
            resource,
            alt,
            ..ImageRef::default()
        }),
        table: None,
        list_marker: None,
        writing_mode: WritingMode::Horizontal,
        text_indent: None,
        font_size: None,
        margin_block_start: None,
        margin_block_end: None,
        mathml: None,
    }
}

pub(crate) fn open_cbz<R: Read + Seek>(reader: R, path: &Path, id: String) -> Result<Book> {
    let mut archive = zip::ZipArchive::new(reader).context("Invalid comic ZIP archive")?;
    let mut resources = BTreeMap::new();
    let mut pages = Vec::new();
    let mut comic_info = String::new();
    let mut total = 0;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.is_dir() {
            continue;
        }
        if entry.size() > 64 * 1024 * 1024 {
            bail!("Comic page exceeds the 64 MiB limit");
        }
        total += entry.size();
        if total > 512 * 1024 * 1024 {
            bail!("Expanded comic exceeds the 512 MiB limit");
        }
        let name = normalize_path(&entry.name().replace('\\', "/"))?;
        let extension = Path::new(&name)
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if name.to_ascii_lowercase().ends_with("comicinfo.xml") {
            entry.read_to_string(&mut comic_info)?;
        } else if matches!(
            extension.as_str(),
            "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff" | "avif"
        ) {
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut bytes)?;
            pages.push(name.clone());
            resources.insert(name, bytes);
        }
    }
    if pages.is_empty() {
        bail!("Comic archive contains no supported page images");
    }
    pages.sort_by(|a, b| natural_cmp(a, b));
    let info = roxmltree::Document::parse(&comic_info).ok();
    let field = |name: &str| {
        info.as_ref()
            .and_then(|doc| {
                doc.descendants()
                    .find(|node| node.tag_name().name() == name)
            })
            .and_then(|node| node.text())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let title = field("Title").unwrap_or_else(|| {
        path.file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });
    let authors = field("Writer").into_iter().collect();
    let page_progression =
        if field("Manga").is_some_and(|value| value.eq_ignore_ascii_case("YesAndRightToLeft")) {
            PageProgression::Rtl
        } else {
            PageProgression::Ltr
        };
    let chapters: Vec<_> = pages
        .iter()
        .enumerate()
        .map(|(index, resource)| Chapter {
            presentation: crate::PagePresentation {
                layout: Some(crate::RenditionLayout::PrePaginated),
                viewport: resources
                    .get(resource)
                    .and_then(|bytes| crate::image_meta::dimensions(bytes)),
                ..crate::PagePresentation::default()
            },
            id: index as u32,
            title: format!("{} — {}", index + 1, resource),
            href: resource.clone(),
            writing_mode: WritingMode::Horizontal,
            blocks: vec![image_block(resource.clone(), format!("Page {}", index + 1))],
            anchors: BTreeMap::new(),
            fixed_page: None,
            loaded: true,
        })
        .collect();
    let toc = chapters
        .iter()
        .map(|chapter| TocEntry {
            title: chapter.title.clone(),
            chapter: chapter.id,
            ..TocEntry::default()
        })
        .collect();
    Ok(Book {
        id,
        title,
        authors,
        language: "und".into(),
        chapters,
        toc,
        cover: pages.first().cloned(),
        resources,
        page_progression,
        epub: None,
        resource_token: std::sync::Arc::new(()),
    })
}

fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut a = a.chars().peekable();
    let mut b = b.chars().peekable();
    loop {
        match (a.peek(), b.peek()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(ac), Some(bc)) if ac.is_ascii_digit() && bc.is_ascii_digit() => {
                let mut an = String::new();
                let mut bn = String::new();
                while a.peek().is_some_and(char::is_ascii_digit) {
                    an.push(a.next().unwrap());
                }
                while b.peek().is_some_and(char::is_ascii_digit) {
                    bn.push(b.next().unwrap());
                }
                let ax = an.trim_start_matches('0');
                let bx = bn.trim_start_matches('0');
                let order = ax
                    .len()
                    .cmp(&bx.len())
                    .then_with(|| ax.cmp(bx))
                    .then_with(|| an.len().cmp(&bn.len()));
                if order != Ordering::Equal {
                    return order;
                }
            }
            _ => {
                let order = a
                    .next()
                    .unwrap()
                    .to_ascii_lowercase()
                    .cmp(&b.next().unwrap().to_ascii_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

pub(crate) fn open_fb2(text: &str, path: &Path, id: String) -> Result<Book> {
    let document = roxmltree::Document::parse(text).context("Invalid FictionBook XML")?;
    let find = |name: &str| {
        document
            .descendants()
            .find(|node| node.tag_name().name() == name)
    };
    let node_text = |node: roxmltree::Node<'_, '_>| {
        node.descendants()
            .filter(|&child| child.is_text())
            .map(|child| child.text().unwrap_or(""))
            .collect::<String>()
    };
    let title = find("book-title").map(&node_text).unwrap_or_else(|| {
        path.file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });
    let language = find("lang").map(&node_text).unwrap_or_else(|| "und".into());
    let authors = document
        .descendants()
        .filter(|node| node.tag_name().name() == "author")
        .map(|node| {
            node.children()
                .filter(|child| {
                    matches!(
                        child.tag_name().name(),
                        "first-name" | "middle-name" | "last-name"
                    )
                })
                .map(&node_text)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|author| !author.is_empty())
        .collect();
    let mut resources = BTreeMap::new();
    let mut resource_ids = HashMap::new();
    for binary in document
        .descendants()
        .filter(|node| node.tag_name().name() == "binary")
    {
        let Some(key) = binary.attribute("id") else {
            continue;
        };
        let base64 = binary
            .text()
            .unwrap_or("")
            .split_whitespace()
            .collect::<String>();
        if base64.len() > 90 * 1024 * 1024 {
            bail!("FictionBook embedded resource exceeds the 64 MiB limit");
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&base64)
            .context("Invalid FictionBook base64 resource")?;
        let resource = format!("fb2-res/{}", blake3::hash(key.as_bytes()).to_hex());
        resource_ids.insert(key.to_owned(), resource.clone());
        resources.insert(resource, bytes);
    }
    fn render(
        node: roxmltree::Node<'_, '_>,
        resources: &HashMap<String, String>,
        level: u8,
    ) -> String {
        if node.is_text() {
            return escape(node.text().unwrap_or(""));
        }
        if !node.is_element() {
            return String::new();
        }
        let name = node.tag_name().name();
        let children = || {
            node.children()
                .map(|child| render(child, resources, level + u8::from(name == "section")))
                .collect::<String>()
        };
        if name == "image" {
            let target = node
                .attributes()
                .find(|attr| attr.name() == "href")
                .map(|attr| attr.value().trim_start_matches('#'))
                .unwrap_or("");
            return resources
                .get(target)
                .map(|path| {
                    format!(
                        "<img src=\"/{}\" alt=\"{}\"/>",
                        escape(path),
                        escape(node.attribute("alt").unwrap_or(""))
                    )
                })
                .unwrap_or_default();
        }
        if name == "empty-line" {
            return "<p><br/></p>".into();
        }
        let tag = match name {
            "title" => format!("h{}", level.clamp(1, 6)),
            "subtitle" => "h3".into(),
            "emphasis" => "em".into(),
            "strikethrough" => "s".into(),
            "cite" | "epigraph" => "blockquote".into(),
            "section" | "poem" | "stanza" => "section".into(),
            "v" => "p".into(),
            "p" | "strong" | "sub" | "sup" | "code" | "table" | "tr" | "td" | "th" | "a"
            | "ruby" | "rb" | "rt" | "rp" => name.into(),
            _ => return children(),
        };
        let mut attrs = String::new();
        for attr in node.attributes() {
            if matches!(attr.name(), "id" | "colspan" | "rowspan" | "href") {
                attrs.push_str(&format!(" {}=\"{}\"", attr.name(), escape(attr.value())));
            }
        }
        format!("<{tag}{attrs}>{}</{tag}>", children())
    }
    let mut chapters = Vec::new();
    for body in document
        .root_element()
        .children()
        .filter(|node| node.tag_name().name() == "body")
    {
        let sections: Vec<_> = body
            .children()
            .filter(|node| node.tag_name().name() == "section")
            .collect();
        let parts = if sections.is_empty() {
            vec![body]
        } else {
            sections
        };
        for section in parts {
            let html = format!(
                "<html><body>{}</body></html>",
                render(section, &resource_ids, 0)
            );
            let index = chapters.len() as u32;
            chapters.push(parse_html(
                &html,
                index,
                &format!("fb2/chapter-{index}.xhtml"),
                "",
            )?);
        }
    }
    if chapters.is_empty() {
        bail!("FictionBook contains no readable body");
    }
    let cover = find("coverpage")
        .and_then(|node| {
            node.descendants()
                .find(|child| child.tag_name().name() == "image")
        })
        .and_then(|node| node.attributes().find(|attr| attr.name() == "href"))
        .and_then(|attr| resource_ids.get(attr.value().trim_start_matches('#')))
        .cloned();
    let toc = chapters
        .iter()
        .map(|chapter| TocEntry {
            title: chapter.title.clone(),
            chapter: chapter.id,
            ..TocEntry::default()
        })
        .collect();
    Ok(Book {
        id,
        title,
        authors,
        language,
        chapters,
        toc,
        cover,
        resources,
        page_progression: PageProgression::Default,
        epub: None,
        resource_token: std::sync::Arc::new(()),
    })
}

pub(crate) fn app_data_dir() -> Result<PathBuf> {
    Ok(
        directories::ProjectDirs::from("", "Liesegang", "EpubReader")
            .context("Cannot determine application data directory")?
            .data_local_dir()
            .to_path_buf(),
    )
}

fn ebook_convert() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("EBOOK_CONVERT_PATH")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    {
        return Some(path);
    }
    let binary = if cfg!(windows) {
        "ebook-convert.exe"
    } else {
        "ebook-convert"
    };
    if let Some(paths) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&paths) {
            let candidate = directory.join(binary);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let mut candidates = vec![
        PathBuf::from("/Applications/calibre.app/Contents/MacOS/ebook-convert"),
        PathBuf::from("/usr/bin/ebook-convert"),
        PathBuf::from("/opt/calibre/ebook-convert"),
    ];
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        candidates.push(PathBuf::from(program_files).join("Calibre2/ebook-convert.exe"));
    }
    if let Some(program_files) = std::env::var_os("ProgramFiles(x86)") {
        candidates.push(PathBuf::from(program_files).join("Calibre2/ebook-convert.exe"));
    }
    candidates.into_iter().find(|path| path.is_file())
}

pub(crate) fn open_kindle(path: &Path, id: String) -> Result<Book> {
    let cache = app_data_dir()?.join("converted").join("calibre-v1");
    fs::create_dir_all(&cache)?;
    let converted = cache.join(format!("{id}.epub"));
    if converted.is_file() {
        return open_epub(fs::File::open(&converted)?, id);
    }
    let executable = ebook_convert().context(
        "MOBI/AZW3 requires Calibre's ebook-convert. Install Calibre or set EBOOK_CONVERT_PATH.",
    )?;
    let temp = tempfile::Builder::new()
        .prefix("conversion-")
        .tempdir_in(&cache)?;
    let output_path = temp.path().join("book.epub");
    let log_path = temp.path().join("conversion.log");
    let log = fs::File::create(&log_path)?;
    let mut command = Command::new(executable);
    command
        .arg(path.canonicalize()?)
        .arg(&output_path)
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // no console window
    }
    let mut child = command.spawn().context("Start ebook-convert")?;
    let deadline = Instant::now() + Duration::from_secs(180);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Calibre conversion timed out after 3 minutes");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    if !status.success() {
        let log = fs::read_to_string(&log_path).unwrap_or_default();
        let tail = log
            .chars()
            .rev()
            .take(2000)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
        bail!("Calibre could not convert this book. It may be DRM-protected or damaged.\n{tail}");
    }
    // Parse first, then atomically publish the immutable conversion result.
    let book = open_epub(fs::File::open(&output_path)?, id.clone())?;
    match fs::rename(&output_path, &converted) {
        Ok(()) => (),
        Err(error) if converted.is_file() => {
            let _ = error;
        }
        Err(error) => return Err(error).context("Store converted EPUB in the application cache"),
    }
    Ok(book)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn natural_page_order_handles_long_numbers() {
        let mut pages = vec![
            "page10.png",
            "page2.png",
            "page000000000000000000000100.png",
            "page1.png",
        ];
        pages.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            pages,
            [
                "page1.png",
                "page2.png",
                "page10.png",
                "page000000000000000000000100.png"
            ]
        );
    }
}
