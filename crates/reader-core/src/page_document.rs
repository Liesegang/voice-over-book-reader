use crate::{Chapter, CssLength, ImageRef, RenditionLayout};
use std::sync::{Arc, OnceLock};

/// One complete source document/page, independent of the reader's spread size.
/// A missing viewport is valid for unloaded, explicitly fixed-layout metadata.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageDocumentGeometry {
    pub viewport: Option<[f32; 2]>,
    /// Legacy image-only HTML inferred from its authored/intrinsic geometry.
    pub inferred_image: bool,
}

/// Derived metadata shared by an EPUB stub, its parsed source and background
/// Book clones. Empty means unknown; Some(None) means a parsed flow document.
#[derive(Clone, Debug, Default)]
pub struct PageGeometryCache(Arc<OnceLock<Option<PageDocumentGeometry>>>);

impl PartialEq for PageGeometryCache {
    fn eq(&self, _other: &Self) -> bool {
        // A runtime cache is not part of the publication's logical contents.
        true
    }
}

impl PageGeometryCache {
    pub(crate) fn get(&self) -> Option<Option<PageDocumentGeometry>> {
        self.0.get().copied()
    }

    pub(crate) fn store(&self, geometry: Option<PageDocumentGeometry>) {
        let _ = self.0.set(geometry);
    }
}

fn valid_size(size: [f32; 2]) -> Option<[f32; 2]> {
    size.iter()
        .all(|value| value.is_finite() && *value > 0.)
        .then_some(size)
}

fn absolute_axis(value: Option<CssLength>) -> Option<Option<f32>> {
    match value {
        None => Some(None),
        Some(CssLength::Px(value)) if value.is_finite() && value >= 256. => Some(Some(value)),
        Some(CssLength::Percent(value)) if (99.0..=100.0).contains(&value) => Some(None),
        // Font-relative images and small explicit boxes are inline art/gaiji,
        // even when the source bitmap itself is a high-resolution scan.
        _ => None,
    }
}

fn image_viewport(image: &ImageRef, viewport: Option<[f32; 2]>) -> Option<[f32; 2]> {
    let width = absolute_axis(image.width)?;
    let height = absolute_axis(image.height)?;
    let intrinsic = image.intrinsic_size.and_then(valid_size);
    let ratio = image
        .aspect_ratio
        .filter(|value| value.is_finite() && *value > 0.)
        .or_else(|| intrinsic.map(|[w, h]| w / h));
    let size = match (width, height) {
        (Some(width), Some(height)) => [width, height],
        (Some(width), None) => [width, width / ratio?],
        (None, Some(height)) => [height * ratio?, height],
        (None, None) => intrinsic.or_else(|| {
            (matches!(image.width, Some(CssLength::Percent(w)) if w >= 99.)
                && matches!(image.height, Some(CssLength::Percent(h)) if h >= 99.))
            .then_some(viewport)
            .flatten()
        })?,
    };
    let [width, height] = valid_size(size)?;
    if width.min(height) < 256. || width.max(height) < 384. {
        return None;
    }
    for (constraint, extent) in [(image.max_width, width), (image.max_height, height)] {
        match constraint {
            None => {}
            Some(CssLength::Percent(value)) if (99.0..=100.0).contains(&value) => {}
            Some(CssLength::Px(value)) if value.is_finite() && value >= extent => {}
            // Constrained diagram boxes have reflow semantics. Do not invent
            // a page viewport from ambiguous max-* or font-relative sizing.
            _ => return None,
        }
    }
    Some([width, height])
}

/// Use publisher metadata first. In its absence, recognize only a standalone,
/// sufficiently large image/SVG with no prose, caption, table or MathML.
/// This reads parsed dimensions only and never decodes image pixels.
pub fn page_document_geometry(chapter: &Chapter) -> Option<PageDocumentGeometry> {
    let viewport = chapter.presentation.viewport.and_then(valid_size);
    if let Some(page) = &chapter.fixed_page {
        return Some(PageDocumentGeometry {
            viewport: valid_size([page.width, page.height]),
            inferred_image: false,
        });
    }
    if chapter.presentation.layout == Some(RenditionLayout::PrePaginated) {
        return Some(PageDocumentGeometry {
            viewport: viewport
                .or_else(|| standalone_image(chapter).and_then(|image| image_viewport(image, None)))
                .or_else(|| {
                    chapter
                        .geometry_cache
                        .get()
                        .flatten()
                        .and_then(|value| value.viewport)
                }),
            inferred_image: false,
        });
    }
    if chapter.presentation.layout == Some(RenditionLayout::Reflowable) {
        return None;
    }
    if !chapter.loaded {
        return chapter
            .geometry_cache
            .get()
            .flatten()
            .filter(|value| value.inferred_image);
    }
    Some(PageDocumentGeometry {
        viewport: Some(image_viewport(standalone_image(chapter)?, viewport)?),
        inferred_image: true,
    })
}

fn standalone_image(chapter: &Chapter) -> Option<&ImageRef> {
    let mut image = None;
    for block in &chapter.blocks {
        if block.table.is_some()
            || block.mathml.is_some()
            || matches!(block.kind, crate::BlockKind::Rule)
        {
            return None;
        }
        if let Some(current) = &block.image
            && image.replace(current).is_some()
        {
            return None;
        }
        for atom in &block.atoms {
            if let Some(current) = atom.image.as_deref() {
                if image.replace(current).is_some() {
                    return None;
                }
            } else if !atom.text.trim().is_empty() {
                return None;
            }
        }
    }
    image
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parsed(body: &str, css: &str) -> Chapter {
        crate::parse_html(
            &format!("<html><body>{body}</body></html>"),
            0,
            "page.xhtml",
            css,
        )
        .unwrap()
    }

    #[test]
    fn legacy_calibre_plate_uses_author_css_pixels_instead_of_bitmap_or_html_attributes() {
        let chapter = parsed(
            "<div><img src='plate.jpeg' class='plate' width='2480' height='3248' alt='Image description'/></div>",
            ".plate{width:977px;height:1280px;margin:0}",
        );
        assert_eq!(
            page_document_geometry(&chapter),
            Some(PageDocumentGeometry {
                viewport: Some([977., 1280.]),
                inferred_image: true
            })
        );
        assert_eq!(
            chapter.presentation.layout, None,
            "inference never rewrites publisher metadata"
        );
        let paragraph = parsed(
            "<p><img src='plate.jpeg' width='600' height='900' alt='Page description'/></p>",
            "",
        );
        assert!(
            page_document_geometry(&paragraph).is_some(),
            "an image alt is not a separate prose caption"
        );
    }

    #[test]
    fn small_gaiji_flow_constraints_and_unknown_sizes_do_not_become_pages() {
        for body in [
            "<img src='glyph.svg' width='24' height='24'/>",
            "<p><img src='glyph.svg' style='width:1em;height:1em'/></p>",
            "<img src='diagram.png' style='width:30%;height:100px'/>",
            "<img src='diagram.png' style='width:600px;height:900px;max-height:5rem'/>",
            "<img src='unknown.png'/>",
        ] {
            let mut chapter = parsed(body, "");
            for block in &mut chapter.blocks {
                if let Some(image) = &mut block.image {
                    image.intrinsic_size = Some([1466., 1920.]);
                }
                for atom in &mut block.atoms {
                    if let Some(image) = atom.image.as_mut() {
                        image.intrinsic_size = Some([1466., 1920.]);
                    }
                }
            }
            if body.contains("unknown") {
                chapter.blocks[0].image.as_mut().unwrap().intrinsic_size = None;
            }
            assert!(page_document_geometry(&chapter).is_none(), "{body}");
        }
    }

    #[test]
    fn captions_prose_tables_math_and_multiple_images_keep_document_flow() {
        for body in [
            "<p>Actual prose</p><img src='plate.png' width='600' height='900'/>",
            "<img src='plate.png' width='600' height='900'/><p>Caption</p>",
            "<img src='plate.png' width='600' height='900'/><table><tr><td>Cell</td></tr></table>",
            "<img src='plate.png' width='600' height='900'/><math><mi>x</mi></math>",
            "<img src='one.png' width='600' height='900'/><img src='two.png' width='600' height='900'/>",
        ] {
            assert!(
                page_document_geometry(&parsed(body, "")).is_none(),
                "{body}"
            );
        }
    }

    #[test]
    fn publisher_metadata_is_authoritative_even_without_loaded_content_or_dimensions() {
        let mut chapter = parsed("", "");
        chapter.loaded = false;
        chapter.presentation.layout = Some(RenditionLayout::PrePaginated);
        assert_eq!(
            page_document_geometry(&chapter),
            Some(PageDocumentGeometry {
                viewport: None,
                inferred_image: false
            })
        );
        chapter = parsed("<img src='plate.png' width='600' height='900'/>", "");
        chapter.presentation.layout = Some(RenditionLayout::Reflowable);
        assert!(page_document_geometry(&chapter).is_none());
    }

    #[test]
    fn intrinsic_dimensions_and_full_size_svg_are_valid_without_fixed_layout_metadata() {
        let mut chapter = parsed("<img src='plate.png'/>", "");
        chapter.blocks[0].image.as_mut().unwrap().intrinsic_size = Some([1200., 800.]);
        assert_eq!(
            page_document_geometry(&chapter).unwrap().viewport,
            Some([1200., 800.])
        );
        let svg = parsed(
            "<svg width='100%' height='100%' viewBox='0 0 1466 1920'><image href='cover.jpeg' width='1466' height='1920'/></svg>",
            "",
        );
        assert_eq!(
            page_document_geometry(&svg).unwrap().viewport,
            Some([1466., 1920.])
        );
    }
}
