//! PDF pages stay native images with real extracted text geometry; they are not
//! converted to HTML or eagerly rasterized at import.
use crate::*;
use anyhow::{Context, Result, bail};
use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PdfPageRef {
    pub path: PathBuf,
    pub page: u32,
    pub width: f32,
    pub height: f32,
}

const PDFIUM_RELEASE: &str = "8125";
static PDFIUM: OnceLock<Pdfium> = OnceLock::new();
static PDFIUM_INIT: Mutex<()> = Mutex::new(());

fn runtime_asset() -> Result<(&'static str, &'static str, &'static str)> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Ok((
            "win-x64",
            "pdfium.dll",
            "9a964261760cff52570aabf038ab346d3a5034dd8b30d0d5fe8861235d356b8b",
        )),
        ("macos", "x86_64") => Ok((
            "mac-x64",
            "libpdfium.dylib",
            "8a5193535b80c7215d8932f6647ae5097465b6dba2811602c21e120c2af112b0",
        )),
        ("macos", "aarch64") => Ok((
            "mac-arm64",
            "libpdfium.dylib",
            "4939bd935ac83ddc09a3a4f038bc43a19cea1660316a3667f26cb07a070e139d",
        )),
        ("linux", "x86_64") => Ok((
            "linux-x64",
            "libpdfium.so",
            "e1c3ce6e270cb5e0c20816b379ad9f2c1309972697c505f508de92bf5e7d6659",
        )),
        ("linux", "aarch64") => Ok((
            "linux-arm64",
            "libpdfium.so",
            "8b8818f2ec57d1b213d952ca6d7f0ffeca9d0110487d0e20ad83ba918bc92efa",
        )),
        _ => bail!(
            "A managed PDFium runtime is not available for this platform. Set PDFIUM_LIBRARY_PATH to a compatible native library."
        ),
    }
}

/// Finds an override/bundled runtime or installs the pinned, checksum-verified
/// runtime in the application data directory. Called only when PDF is requested.
pub fn pdfium_runtime_path() -> Result<PathBuf> {
    let (platform, filename, checksum) = runtime_asset()?;
    if let Some(override_path) = std::env::var_os("PDFIUM_LIBRARY_PATH") {
        let path = PathBuf::from(override_path);
        let path = if path.is_dir() {
            path.join(filename)
        } else {
            path
        };
        if !path.is_file() {
            bail!("PDFIUM_LIBRARY_PATH does not point to an existing PDFium library");
        }
        return Ok(path);
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        let bundled = directory
            .join("runtimes/pdfium")
            .join(platform)
            .join(filename);
        if bundled.is_file() {
            return Ok(bundled);
        }
    }
    let directory = crate::formats::app_data_dir()?
        .join("runtimes/pdfium")
        .join(format!("chromium-{PDFIUM_RELEASE}"))
        .join(platform);
    let library = directory.join(filename);
    if library.is_file() {
        return Ok(library);
    }
    fs::create_dir_all(&directory)?;
    let url = format!(
        "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/{PDFIUM_RELEASE}/pdfium-{platform}.tgz"
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .user_agent("EpubReader/0.1 native PDF runtime")
        .build()?;
    let response = client
        .get(&url)
        .send()
        .context("Download the native PDF engine (first PDF open only)")?
        .error_for_status()?;
    let mut archive_bytes = Vec::new();
    response
        .take(150 * 1024 * 1024 + 1)
        .read_to_end(&mut archive_bytes)?;
    if archive_bytes.len() > 150 * 1024 * 1024 {
        bail!("PDF engine download exceeded its size limit");
    }
    let digest = format!("{:x}", Sha256::digest(&archive_bytes));
    if digest != checksum {
        bail!("Downloaded PDF engine checksum does not match the pinned release");
    }
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(Cursor::new(archive_bytes)));
    let mut found = false;
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let name = entry.path()?.into_owned();
        if name.components().any(|component| {
            !matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        }) {
            bail!("Invalid path inside PDF engine package");
        }
        let basename = name.file_name().unwrap_or_default().to_string_lossy();
        if basename == filename {
            if entry.size() > 200 * 1024 * 1024 {
                bail!("PDF engine binary exceeds its size limit");
            }
            let mut temporary = tempfile::NamedTempFile::new_in(&directory)?;
            std::io::copy(&mut entry, &mut temporary)?;
            temporary.flush()?;
            temporary.persist(&library).map_err(|error| error.error)?;
            found = true;
        } else if basename.to_ascii_lowercase().contains("license") || name.starts_with("licenses")
        {
            let destination = directory.join("notices").join(&name);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut output = fs::File::create(destination)?;
            std::io::copy(&mut entry, &mut output)?;
        }
    }
    if !found {
        bail!("Downloaded PDF engine package contained no native library");
    }
    Ok(library)
}

fn pdfium() -> Result<&'static Pdfium> {
    if let Some(pdfium) = PDFIUM.get() {
        return Ok(pdfium);
    }
    let _guard = PDFIUM_INIT
        .lock()
        .map_err(|_| anyhow::anyhow!("PDF initialization lock was poisoned"))?;
    if let Some(pdfium) = PDFIUM.get() {
        return Ok(pdfium);
    }
    let path = pdfium_runtime_path()?;
    let bindings = Pdfium::bind_to_library(&path).context("Load the native PDF engine")?;
    let mut instance = Pdfium::new(bindings);
    let _ = instance.use_platform_default_font_provider();
    let _ = PDFIUM.set(instance);
    PDFIUM.get().context("Initialize the native PDF engine")
}

pub(crate) fn open_pdf(path: &Path, id: String) -> Result<Book> {
    let pdfium = pdfium()?;
    let document = pdfium
        .load_pdf_from_file(path, None)
        .context("Open PDF (password-protected PDFs require an unlocked copy)")?;
    let title = document
        .metadata()
        .get(PdfDocumentMetadataTagType::Title)
        .map(|tag| tag.value().to_owned())
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    let authors = document
        .metadata()
        .get(PdfDocumentMetadataTagType::Author)
        .map(|tag| tag.value().to_owned())
        .into_iter()
        .collect();
    let absolute_path = path.canonicalize()?;
    let mut chapters = Vec::new();
    for (index, page) in document.pages().iter().enumerate() {
        let width = page.width().value;
        let height = page.height().value;
        // Ask PDFium for the same page/device transform used for rendering; this
        // includes crop boxes and intrinsic quarter-turn page rotation.
        let geometry_config = PdfRenderConfig::new()
            .set_target_width((width * 10.0).round() as i32)
            .set_target_height((height * 10.0).round() as i32);
        let mut text = String::new();
        let mut chars = Vec::new();
        if let Ok(page_text) = page.text() {
            for character in page_text.chars().iter() {
                let Some(unicode) = character.unicode_char().filter(|ch| *ch != '\0') else {
                    continue;
                };
                let start = text.len() as u32;
                text.push(unicode);
                let source = SourceSpan {
                    chapter: index as u32,
                    node: 0,
                    start,
                    end: text.len() as u32,
                };
                if let Ok(bounds) = character.tight_bounds() {
                    let rect = match (
                        page.points_to_pixels(bounds.left(), bounds.top(), &geometry_config),
                        page.points_to_pixels(bounds.right(), bounds.bottom(), &geometry_config),
                    ) {
                        (Ok((x1, y1)), Ok((x2, y2))) => [
                            x1.min(x2) as f32 / 10.0,
                            y1.min(y2) as f32 / 10.0,
                            x1.max(x2) as f32 / 10.0,
                            y1.max(y2) as f32 / 10.0,
                        ],
                        _ => top_left_rect(
                            bounds.left().value,
                            bounds.bottom().value,
                            bounds.right().value,
                            bounds.top().value,
                            height,
                        ),
                    };
                    if rect.iter().all(|value| value.is_finite())
                        && rect[2] > rect[0]
                        && rect[3] > rect[1]
                    {
                        chars.push(FixedChar {
                            text: unicode.to_string(),
                            source,
                            rect,
                        });
                    }
                }
            }
        }
        let mut block =
            crate::formats::image_block(format!("pdf-page:{index}"), format!("Page {}", index + 1));
        block.image.as_mut().unwrap().pdf_page = Some(PdfPageRef {
            path: absolute_path.clone(),
            page: index as u32,
            width,
            height,
        });
        if !text.is_empty() {
            let end = text.len() as u32;
            block.atoms.push(Atom {
                text,
                sources: vec![SourceSpan {
                    chapter: index as u32,
                    node: 0,
                    start: 0,
                    end,
                }],
                ..Atom::default()
            });
        }
        chapters.push(Chapter {
            presentation: crate::PagePresentation::default(),
            id: index as u32,
            title: format!("Page {}", index + 1),
            href: format!("pdf-page:{index}"),
            writing_mode: WritingMode::Horizontal,
            blocks: vec![block],
            anchors: BTreeMap::new(),
            fixed_page: Some(FixedPage {
                width,
                height,
                chars,
            }),
            loaded: true,
        });
    }
    if chapters.is_empty() {
        bail!("PDF contains no pages");
    }
    let mut toc: Vec<_> = document
        .bookmarks()
        .iter()
        .filter_map(|bookmark| {
            let page = bookmark.destination()?.page_index().ok()? as u32;
            Some(TocEntry {
                title: bookmark.title().unwrap_or_default(),
                chapter: page,
                ..TocEntry::default()
            })
        })
        .collect();
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
    Ok(Book {
        id,
        title,
        authors,
        language: "und".into(),
        chapters,
        toc,
        cover: None,
        resources: BTreeMap::new(),
        page_progression: PageProgression::Ltr,
        epub: None,
        resource_token: std::sync::Arc::new(()),
    })
}

fn top_left_rect(left: f32, bottom: f32, right: f32, top: f32, page_height: f32) -> [f32; 4] {
    [left, page_height - top, right, page_height - bottom]
}

/// Rasterizes only the requested page. Call on a worker thread, then cache the
/// PNG/texture. Image coordinates correspond to `Chapter::fixed_page` geometry.
pub fn render_pdf_page(reference: &PdfPageRef, target_width: u32) -> Result<Vec<u8>> {
    let document = pdfium()?.load_pdf_from_file(&reference.path, None)?;
    let page_index =
        i32::try_from(reference.page).context("PDF page index exceeds the engine limit")?;
    let page = document.pages().get(page_index)?;
    let config = PdfRenderConfig::new()
        .set_target_width(target_width.clamp(64, 4096) as i32)
        .set_maximum_height(8192);
    let bitmap = page.render_with_config(&config)?;
    let image = bitmap.as_image()?;
    let mut output = Cursor::new(Vec::new());
    image.write_to(&mut output, image::ImageFormat::Png)?;
    Ok(output.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pdf_geometry_changes_origin_without_character_estimation() {
        assert_eq!(
            top_left_rect(12.5, 30.0, 20.0, 42.0, 100.0),
            [12.5, 58.0, 20.0, 70.0]
        );
    }
}
