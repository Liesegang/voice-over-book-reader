//! Read-only document parsing. Original text identities survive layout changes.
mod document;
mod epub;
mod formats;
mod image_meta;
mod import;
mod page_document;
mod pdf;
mod words;

pub use document::*;
pub use epub::{ArchiveLoadStats, EpubSource};
pub use import::{open, parse_html};
pub use page_document::{PageDocumentGeometry, PageGeometryCache, page_document_geometry};
pub use pdf::{PdfPageRef, pdfium_runtime_path, render_pdf_page};
pub use words::{WordSpan, word_at};

/// User-agent typography; author EPUB CSS has a separate, higher cascade origin.
pub const DEFAULT_READER_CSS: &str = include_str!("../../../assets/default-reader.css");
pub fn default_reader_css_revision() -> &'static str {
    static REVISION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    REVISION.get_or_init(|| {
        blake3::hash(DEFAULT_READER_CSS.as_bytes())
            .to_hex()
            .to_string()
    })
}
