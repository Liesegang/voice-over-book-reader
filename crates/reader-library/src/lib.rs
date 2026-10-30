//! Application-owned persistence and read-only Calibre / MDict sources.
mod calibre;
mod dictionary;
pub mod services;
mod settings;
mod store;

pub use calibre::{CalibreLibrary, detect_calibre_library};
pub use dictionary::{
    DictionaryCatalog, DictionaryHit, DictionaryInfo, DictionaryStatus, DictionarySuggestion,
    RegistrationReport,
};
pub use services::{
    OpdsClient, OpdsConfig, OpdsLink, OpdsPage, OpdsPublication, WebImportResult, import_web_page,
    parse_opds,
};
pub use settings::{ReadingMode, Settings, SpreadMode, Theme, TtsSettings};
pub use store::{
    Annotation, BookEntry, BookSource, Bookmark, LibraryStore, ReadingPosition, unix_millis,
};
