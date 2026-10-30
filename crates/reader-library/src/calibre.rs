use crate::{BookEntry, BookSource};
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug)]
pub struct CalibreLibrary {
    pub root: PathBuf,
    pub library_uuid: String,
    /// One available, supported source per book, in preferred format order.
    /// Scanning only probes file headers; full parsing happens when opened.
    pub entries: Vec<BookEntry>,
}

impl CalibreLibrary {
    /// A short read-only snapshot. Never takes calibredb's process-wide lock,
    /// creates a database, or writes PRAGMA journal_mode on the source.
    pub fn scan(root: impl AsRef<Path>) -> Result<Self> {
        let root =
            fs::canonicalize(root.as_ref()).context("Calibre library directory is unavailable")?;
        let db_path = root.join("metadata.db");
        let mut connection = Connection::open_with_flags(
            &db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("Read Calibre database {}", db_path.display()))?;
        connection.busy_timeout(Duration::from_millis(300))?;
        let transaction = connection.transaction()?;
        let library_uuid = transaction
            .query_row("SELECT uuid FROM library_id LIMIT 1", [], |row| {
                row.get::<_, String>(0)
            })
            .optional()?
            .unwrap_or_else(|| {
                blake3::hash(root.to_string_lossy().as_bytes())
                    .to_hex()
                    .to_string()
            });
        let authors = relation_map(
            &transaction,
            "SELECT l.book,a.name FROM books_authors_link l JOIN authors a ON a.id=l.author ORDER BY l.id",
        )?;
        let tags = relation_map(
            &transaction,
            "SELECT l.book,t.name FROM books_tags_link l JOIN tags t ON t.id=l.tag ORDER BY l.id",
        )?;
        let formats: HashMap<i64, Vec<Format>> = {
            let mut statement = transaction.prepare("SELECT book,name,format FROM data")?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            let mut formats = HashMap::<i64, Vec<Format>>::new();
            for row in rows {
                let (book, name, extension) = row?;
                let extension = extension.trim().to_ascii_lowercase();
                if let Some(priority) = format_priority(&extension)
                    && valid_filename(&name)
                {
                    formats.entry(book).or_default().push(Format {
                        name,
                        extension,
                        priority,
                    });
                }
            }
            for candidates in formats.values_mut() {
                candidates.sort_by(|a, b| {
                    a.priority
                        .cmp(&b.priority)
                        .then_with(|| a.extension.cmp(&b.extension))
                        .then_with(|| a.name.cmp(&b.name))
                });
            }
            formats
        };
        let entries = {
            let mut statement = transaction.prepare(
                "SELECT id,uuid,title,path,has_cover FROM books ORDER BY title COLLATE NOCASE",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, bool>(4)?,
                ))
            })?;
            let mut entries = Vec::new();
            for row in rows {
                let (id, uuid, title, relative_path, has_cover) = row?;
                let Some(formats) = formats.get(&id) else {
                    continue;
                };
                // Calibre paths are relative. Reject malformed DB paths rather than
                // interpreting them as arbitrary paths outside the chosen library.
                let relative_path = relative_path.replace('\\', "/");
                let relative = Path::new(&relative_path);
                anyhow::ensure!(
                    relative
                        .components()
                        .all(|c| matches!(c, Component::Normal(_))),
                    "Invalid relative path for Calibre book {id}"
                );
                let book_dir = root.join(relative);
                let Some(source) = preferred_source(&book_dir, formats) else {
                    // A rename, missing file, or damaged header should affect
                    // this candidate/book, not hide the rest of the library.
                    continue;
                };
                entries.push(BookEntry {
                    id: format!(
                        "calibre:{library_uuid}:{}",
                        uuid.as_deref().unwrap_or(&id.to_string())
                    ),
                    uuid,
                    calibre_id: Some(id),
                    title,
                    authors: authors.get(&id).cloned().unwrap_or_default(),
                    tags: tags.get(&id).cloned().unwrap_or_default(),
                    // BookEntry keeps this historical field name for every
                    // source format; the reader dispatches by its extension.
                    epub_path: source,
                    cover_path: has_cover
                        .then(|| book_dir.join("cover.jpg"))
                        .filter(|path| path.is_file()),
                    source: BookSource::Calibre,
                    added_at: 0,
                });
            }
            entries
        };
        transaction.commit()?;
        Ok(Self {
            root,
            library_uuid,
            entries,
        })
    }
}

struct Format {
    name: String,
    extension: String,
    priority: u8,
}

fn format_priority(extension: &str) -> Option<u8> {
    Some(match extension {
        "epub" => 0,
        "azw3" => 1,
        "mobi" | "azw" => 2,
        "fb2" => 3,
        "pdf" => 4,
        "cbz" => 5,
        "html" | "htm" | "xhtml" => 6,
        "md" | "markdown" => 7,
        "txt" | "text" => 8,
        _ => return None,
    })
}

fn valid_filename(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['/', '\\', ':'])
        && matches!(
            Path::new(name).components().next(),
            Some(Component::Normal(_))
        )
}

fn preferred_source(directory: &Path, formats: &[Format]) -> Option<PathBuf> {
    // Read each directory once, preserving the actual filename spelling on
    // case-sensitive and case-insensitive filesystems alike.
    let mut files: Vec<PathBuf> = fs::read_dir(directory)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    files.sort();
    for format in formats {
        let expected = format!("{}.{}", format.name, format.extension);
        for path in &files {
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case(&expected))
                && plausible_source(path, &format.extension)
            {
                return Some(path.clone());
            }
        }
    }
    None
}

fn plausible_source(path: &Path, extension: &str) -> bool {
    // Calibre scans can cover thousands of books on a network disk. Limit the
    // probe to 1 KiB: no ZIP expansion, PDFium initialization, conversion,
    // model download, or writes. The core reports deeper damage on open.
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() == 0 {
        return false;
    }
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let mut header = Vec::with_capacity(1024);
    if file.take(1024).read_to_end(&mut header).is_err() {
        return false;
    }
    match extension {
        "epub" | "cbz" => metadata.len() >= 30 && header.starts_with(b"PK\x03\x04"),
        "pdf" => header.windows(5).any(|window| window == b"%PDF-"),
        "mobi" | "azw" | "azw3" => {
            (metadata.len() >= 78
                && header
                    .get(60..68)
                    .is_some_and(|kind| kind == b"BOOKMOBI" || kind == b"TEXtREAd"))
                || header.starts_with(b"TPZ0")
        }
        _ => {
            if header.starts_with(b"\xff\xfe") || header.starts_with(b"\xfe\xff") {
                return true; // The core supports BOM-marked UTF-16.
            }
            if header.contains(&0) {
                return false;
            }
            let text = match std::str::from_utf8(&header) {
                Ok(text) => text,
                // The bounded read may stop inside the final UTF-8 character.
                Err(error) if error.error_len().is_none() => {
                    let Ok(text) = std::str::from_utf8(&header[..error.valid_up_to()]) else {
                        return false;
                    };
                    text
                }
                Err(_) => return false,
            };
            extension != "fb2"
                || text.contains("<FictionBook")
                || text
                    .trim_start_matches('\u{feff}')
                    .trim_start()
                    .starts_with("<?xml")
        }
    }
}

fn relation_map(connection: &Connection, query: &str) -> Result<HashMap<i64, Vec<String>>> {
    let mut result = HashMap::<i64, Vec<String>>::new();
    let mut statement = connection.prepare(query)?;
    for row in statement.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })? {
        let (book, value) = row?;
        result.entry(book).or_default().push(value);
    }
    Ok(result)
}

/// Reads Calibre's configured path without starting Calibre or invoking Python.
pub fn detect_calibre_library() -> Option<PathBuf> {
    let mut config_dirs = Vec::new();
    if let Some(path) = std::env::var_os("CALIBRE_CONFIG_DIRECTORY") {
        config_dirs.push(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("APPDATA") {
        config_dirs.push(PathBuf::from(path).join("calibre"));
    }
    if let Some(base) = directories::BaseDirs::new() {
        config_dirs.push(base.config_dir().join("calibre"));
        config_dirs.push(base.home_dir().join(".config/calibre"));
        config_dirs.push(base.home_dir().join("Library/Preferences/calibre"));
    }
    config_dirs.into_iter().find_map(|directory| {
        let raw = fs::read_to_string(directory.join("global.py.json")).ok()?;
        let config: serde_json::Value =
            serde_json::from_str(raw.trim_start_matches('\u{feff}')).ok()?;
        let path = PathBuf::from(config.get("library_path")?.as_str()?);
        path.join("metadata.db").is_file().then_some(path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        directory: tempfile::TempDir,
        connection: Connection,
    }
    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let connection = Connection::open(directory.path().join("metadata.db")).unwrap();
            connection.execute_batch("CREATE TABLE library_id(uuid TEXT); INSERT INTO library_id VALUES('fixture-library');
                CREATE TABLE books(id INTEGER PRIMARY KEY,uuid TEXT,title TEXT,path TEXT,has_cover INTEGER);
                CREATE TABLE data(book INTEGER,name TEXT,format TEXT);
                CREATE TABLE authors(id INTEGER PRIMARY KEY,name TEXT);
                CREATE TABLE books_authors_link(id INTEGER PRIMARY KEY,book INTEGER,author INTEGER);
                CREATE TABLE tags(id INTEGER PRIMARY KEY,name TEXT);
                CREATE TABLE books_tags_link(id INTEGER PRIMARY KEY,book INTEGER,tag INTEGER);").unwrap();
            Self {
                directory,
                connection,
            }
        }
        fn book(&self, id: i64) -> PathBuf {
            let relative = format!("Author/Book ({id})");
            self.connection
                .execute(
                    "INSERT INTO books VALUES(?1,?2,?3,?4,0)",
                    rusqlite::params![id, format!("uuid-{id}"), format!("Book {id}"), relative],
                )
                .unwrap();
            let directory = self.directory.path().join(relative);
            fs::create_dir_all(&directory).unwrap();
            directory
        }
        fn format(
            &self,
            id: i64,
            name: &str,
            format: &str,
            actual_filename: Option<&str>,
            content: &[u8],
        ) {
            self.connection
                .execute(
                    "INSERT INTO data VALUES(?1,?2,?3)",
                    rusqlite::params![id, name, format],
                )
                .unwrap();
            if let Some(filename) = actual_filename {
                fs::write(
                    self.directory
                        .path()
                        .join(format!("Author/Book ({id})"))
                        .join(filename),
                    content,
                )
                .unwrap();
            }
        }
        fn scan(&self) -> CalibreLibrary {
            CalibreLibrary::scan(self.directory.path()).unwrap()
        }
    }
    fn zip_header() -> Vec<u8> {
        let mut bytes = b"PK\x03\x04".to_vec();
        bytes.resize(64, 0);
        bytes
    }
    fn mobi_header() -> Vec<u8> {
        let mut bytes = vec![0; 78];
        bytes[60..68].copy_from_slice(b"BOOKMOBI");
        bytes
    }

    #[test]
    fn includes_pdf_and_azw3_only_books_with_actual_filename_case() {
        let fixture = Fixture::new();
        let pdf_dir = fixture.book(1);
        let kindle_dir = fixture.book(2);
        fixture.format(
            1,
            "Book - Author",
            "PDF",
            Some("Book - Author.PdF"),
            b"%PDF-1.7\nheader-only scan fixture",
        );
        fixture.format(
            2,
            "Book - Author",
            "azw3",
            Some("Book - Author.AzW3"),
            &mobi_header(),
        );
        let library = fixture.scan();
        assert_eq!(library.entries.len(), 2);
        assert_eq!(
            library.entries[0].epub_path,
            pdf_dir.join("Book - Author.PdF").canonicalize().unwrap()
        );
        assert_eq!(
            library.entries[1].epub_path,
            kindle_dir
                .join("Book - Author.AzW3")
                .canonicalize()
                .unwrap()
        );
        assert!(
            library
                .entries
                .iter()
                .all(|entry| entry.source == BookSource::Calibre)
        );
        assert_eq!(library.entries[0].id, "calibre:fixture-library:uuid-1");
    }

    #[test]
    fn chooses_one_preferred_format_independent_of_database_row_order() {
        let fixture = Fixture::new();
        let directory = fixture.book(1);
        fixture.format(1, "Book", "TXT", Some("Book.txt"), b"Plain text fallback");
        fixture.format(1, "Book", "PDF", Some("Book.pdf"), b"%PDF-1.7\nfixture");
        fixture.format(1, "Book", "AZW3", Some("Book.azw3"), &mobi_header());
        fixture.format(1, "Book", "EPUB", Some("Book.EPUB"), &zip_header());
        let library = fixture.scan();
        assert_eq!(library.entries.len(), 1);
        assert_eq!(
            library.entries[0].epub_path,
            directory.join("Book.EPUB").canonicalize().unwrap()
        );
        fs::remove_file(directory.join("Book.EPUB")).unwrap();
        assert_eq!(
            fixture.scan().entries[0].epub_path,
            directory.join("Book.azw3").canonicalize().unwrap()
        );
    }

    #[test]
    fn falls_back_past_missing_empty_directory_and_bad_header_candidates() {
        let fixture = Fixture::new();
        let directory = fixture.book(1);
        fixture.format(1, "Book", "EPUB", None, b"");
        fixture.format(
            1,
            "Book",
            "AZW3",
            Some("Book.azw3"),
            b"damaged Kindle header",
        );
        fixture.format(1, "Book", "MOBI", Some("Book.mobi"), b"");
        fixture.format(1, "Book", "PDF", None, b"");
        fs::create_dir(directory.join("Book.pdf")).unwrap();
        fixture.format(1, "Book", "CBZ", Some("Book.cbz"), &zip_header());
        fixture.format(
            1,
            "Book",
            "TXT",
            Some("Book.txt"),
            b"Native readable fallback",
        );
        assert_eq!(
            fixture.scan().entries[0].epub_path,
            directory.join("Book.cbz").canonicalize().unwrap()
        );
        fs::write(directory.join("Book.cbz"), b"broken comic").unwrap();
        let library = fixture.scan();
        assert_eq!(library.entries.len(), 1);
        assert_eq!(
            library.entries[0].epub_path,
            directory.join("Book.txt").canonicalize().unwrap()
        );
        assert!(
            reader_core::open(&library.entries[0].epub_path)
                .unwrap()
                .chapters[0]
                .plain_text()
                .contains("Native readable fallback")
        );
    }

    #[test]
    fn ignores_unsupported_and_malformed_format_records_without_writing_sources() {
        let fixture = Fixture::new();
        let directory = fixture.book(1);
        fixture.book(2);
        fixture.format(1, "../outside", "EPUB", None, b"");
        fixture.format(1, "Book", "DOCX", Some("Book.docx"), &zip_header());
        fixture.format(
            1,
            "Book",
            "TXT",
            Some("Book.txt"),
            "読み取り専用の本文。".as_bytes(),
        );
        fixture.format(2, "Missing", "PDF", None, b"");
        let database = fixture.directory.path().join("metadata.db");
        let original_database = fs::read(&database).unwrap();
        let original_text = fs::read(directory.join("Book.txt")).unwrap();
        let mut before_files: Vec<_> = fs::read_dir(fixture.directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        before_files.sort();
        let library = fixture.scan();
        assert_eq!(library.entries.len(), 1);
        assert_eq!(
            library.entries[0].epub_path,
            directory.join("Book.txt").canonicalize().unwrap()
        );
        assert_eq!(fs::read(database).unwrap(), original_database);
        assert_eq!(fs::read(directory.join("Book.txt")).unwrap(), original_text);
        let mut after_files: Vec<_> = fs::read_dir(fixture.directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        after_files.sort();
        assert_eq!(before_files, after_files);
    }
}
