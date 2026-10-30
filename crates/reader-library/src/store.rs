use crate::Settings;
use anyhow::{Context, Result};
use reader_core::Locator;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BookSource {
    Imported,
    Calibre,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BookEntry {
    pub id: String,
    pub uuid: Option<String>,
    pub calibre_id: Option<i64>,
    pub title: String,
    pub authors: Vec<String>,
    pub tags: Vec<String>,
    pub epub_path: PathBuf,
    pub cover_path: Option<PathBuf>,
    pub source: BookSource,
    pub added_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadingPosition {
    pub book_id: String,
    pub locator: Locator,
    pub progress: f32,
    pub updated_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bookmark {
    pub id: String,
    pub book_id: String,
    pub locator: Locator,
    pub label: String,
    pub created_at: u64,
}

impl Bookmark {
    pub fn new(book_id: impl Into<String>, locator: Locator, label: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            book_id: book_id.into(),
            locator,
            label: label.into(),
            created_at: unix_millis(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Annotation {
    pub id: String,
    pub book_id: String,
    pub start: Locator,
    pub end: Locator,
    pub quote: String,
    pub note: String,
    pub color: [u8; 3],
    pub created_at: u64,
}

pub struct LibraryStore {
    connection: Connection,
    data_dir: PathBuf,
    settings: Settings,
    books: Vec<BookEntry>,
}

impl LibraryStore {
    pub fn open_default() -> Result<Self> {
        let dirs = directories::ProjectDirs::from("", "Liesegang", "EpubReader")
            .context("Cannot determine application data directory")?;
        Self::open(dirs.data_local_dir())
    }

    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();
        fs::create_dir_all(&data_dir).context("Create application data directory")?;
        let connection = Connection::open(data_dir.join("reader.sqlite3"))?;
        connection.busy_timeout(std::time::Duration::from_secs(2))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, json TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS books (id TEXT PRIMARY KEY, json TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS positions (book_id TEXT PRIMARY KEY, json TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS bookmarks (id TEXT PRIMARY KEY, book_id TEXT NOT NULL, json TEXT NOT NULL);
             CREATE INDEX IF NOT EXISTS bookmarks_book ON bookmarks(book_id);
             CREATE TABLE IF NOT EXISTS annotations (id TEXT PRIMARY KEY, book_id TEXT NOT NULL, json TEXT NOT NULL);
             CREATE INDEX IF NOT EXISTS annotations_book ON annotations(book_id);"
        )?;
        let settings_json: Option<String> = connection
            .query_row("SELECT json FROM settings WHERE key='app'", [], |row| {
                row.get(0)
            })
            .optional()?;
        // Invalid stored settings are reported instead of silently overwriting the user's data.
        let mut settings = match settings_json {
            Some(json) => {
                let mut value: serde_json::Value =
                    serde_json::from_str(&json).context("Read saved application settings")?;
                let cleaned = remove_obsolete_settings(&mut value)
                    .then(|| serde_json::to_string(&value))
                    .transpose()?;
                let settings =
                    serde_json::from_value(value).context("Read saved application settings")?;
                if let Some(cleaned) = cleaned {
                    connection.execute("UPDATE settings SET json=?1 WHERE key='app'", [cleaned])?;
                }
                settings
            }
            None => Settings::default(),
        };
        if settings.calibre_path.is_none() {
            settings.calibre_path = crate::detect_calibre_library();
        }
        let books = {
            let mut statement = connection.prepare("SELECT json FROM books ORDER BY rowid DESC")?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .map(|row| Ok(serde_json::from_str(&row?)?))
                .collect::<Result<Vec<BookEntry>>>()?
        };
        Ok(Self {
            connection,
            data_dir,
            settings,
            books,
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
    pub fn settings(&self) -> &Settings {
        &self.settings
    }
    pub fn books(&self) -> &[BookEntry] {
        &self.books
    }

    pub fn save_settings(&mut self, settings: &Settings) -> Result<()> {
        settings.validate()?;
        self.connection.execute("INSERT INTO settings(key,json) VALUES('app',?1) ON CONFLICT(key) DO UPDATE SET json=excluded.json", [serde_json::to_string(settings)?])?;
        self.settings = settings.clone();
        Ok(())
    }

    /// Parse and register a supported source in place. Source files are read-only.
    pub fn import_book(&mut self, path: impl AsRef<Path>) -> Result<BookEntry> {
        let document = reader_core::open(path.as_ref())?;
        self.register_document(path, &document)
    }

    /// Reuse an already-opened document so the GUI does not need to parse twice.
    pub fn register_document(
        &mut self,
        path: impl AsRef<Path>,
        document: &reader_core::Book,
    ) -> Result<BookEntry> {
        let path = fs::canonicalize(path.as_ref()).context("Open imported EPUB")?;
        let id = document.id.clone();
        let previous = self.books.iter().find(|book| book.id == id).cloned();
        let cover_path = if let Some((resource_name, bytes)) = document
            .cover
            .as_deref()
            .and_then(|name| document.resource(name).map(|bytes| (name, bytes)))
        {
            let covers = self.data_dir.join("covers");
            fs::create_dir_all(&covers)?;
            let ext = Path::new(resource_name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("img");
            let cover = covers.join(format!("{id}.{ext}"));
            if !cover.exists() {
                fs::write(&cover, bytes)?;
            }
            Some(cover)
        } else {
            None
        };
        let entry = BookEntry {
            id,
            uuid: None,
            calibre_id: None,
            title: document.title.clone(),
            authors: document.authors.clone(),
            tags: previous
                .as_ref()
                .map(|entry| entry.tags.clone())
                .unwrap_or_default(),
            epub_path: path,
            cover_path,
            source: BookSource::Imported,
            added_at: previous
                .map(|entry| entry.added_at)
                .unwrap_or_else(unix_millis),
        };
        self.save_book(&entry)?;
        Ok(entry)
    }

    pub fn save_book(&mut self, book: &BookEntry) -> Result<()> {
        self.connection.execute("INSERT INTO books(id,json) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET json=excluded.json", params![book.id, serde_json::to_string(book)?])?;
        if let Some(existing) = self.books.iter_mut().find(|entry| entry.id == book.id) {
            *existing = book.clone();
        } else {
            self.books.insert(0, book.clone());
        }
        Ok(())
    }

    /// Removes an application library entry only. Source files are untouched.
    pub fn remove_book(&mut self, book_id: &str) -> Result<()> {
        self.connection
            .execute("DELETE FROM books WHERE id=?1", [book_id])?;
        self.books.retain(|book| book.id != book_id);
        Ok(())
    }

    pub fn save_position(&self, position: &ReadingPosition) -> Result<()> {
        anyhow::ensure!(
            position.progress.is_finite() && (0.0..=1.0).contains(&position.progress),
            "Reading progress must be 0–1"
        );
        self.connection.execute("INSERT INTO positions(book_id,json) VALUES(?1,?2) ON CONFLICT(book_id) DO UPDATE SET json=excluded.json", params![position.book_id, serde_json::to_string(position)?])?;
        Ok(())
    }

    pub fn position(&self, book_id: &str) -> Result<Option<ReadingPosition>> {
        let raw: Option<String> = self
            .connection
            .query_row(
                "SELECT json FROM positions WHERE book_id=?1",
                [book_id],
                |row| row.get(0),
            )
            .optional()?;
        raw.map(|json| serde_json::from_str(&json).map_err(Into::into))
            .transpose()
    }

    pub fn save_bookmark(&self, bookmark: &Bookmark) -> Result<()> {
        self.connection.execute("INSERT INTO bookmarks(id,book_id,json) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET book_id=excluded.book_id,json=excluded.json", params![bookmark.id, bookmark.book_id, serde_json::to_string(bookmark)?])?;
        Ok(())
    }
    pub fn bookmarks(&self, book_id: &str) -> Result<Vec<Bookmark>> {
        self.records_for_book("bookmarks", book_id)
    }
    pub fn delete_bookmark(&self, id: &str) -> Result<()> {
        self.connection
            .execute("DELETE FROM bookmarks WHERE id=?1", [id])?;
        Ok(())
    }

    pub fn save_annotation(&self, annotation: &Annotation) -> Result<()> {
        self.connection.execute("INSERT INTO annotations(id,book_id,json) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET book_id=excluded.book_id,json=excluded.json", params![annotation.id, annotation.book_id, serde_json::to_string(annotation)?])?;
        Ok(())
    }
    pub fn annotations(&self, book_id: &str) -> Result<Vec<Annotation>> {
        self.records_for_book("annotations", book_id)
    }
    pub fn delete_annotation(&self, id: &str) -> Result<()> {
        self.connection
            .execute("DELETE FROM annotations WHERE id=?1", [id])?;
        Ok(())
    }

    fn records_for_book<T: serde::de::DeserializeOwned>(
        &self,
        table: &str,
        book_id: &str,
    ) -> Result<Vec<T>> {
        // table is a private, fixed call-site constant, never user input.
        let mut statement = self.connection.prepare(&format!(
            "SELECT json FROM {table} WHERE book_id=?1 ORDER BY rowid"
        ))?;
        statement
            .query_map([book_id], |row| row.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str(&row?)?))
            .collect()
    }
}

/// Remove retired feature data without rewriting unrelated or newer settings.
fn remove_obsolete_settings(value: &mut serde_json::Value) -> bool {
    let Some(fields) = value.as_object_mut() else {
        return false;
    };
    let mut changed = fields.remove("translation").is_some();
    changed |= fields.remove("translation_target").is_some();
    if let Some(bindings) = fields
        .get_mut("key_bindings")
        .and_then(serde_json::Value::as_object_mut)
    {
        changed |= bindings.remove("TranslateSelection").is_some();
        changed |= bindings.remove("TranslateChapter").is_some();
    }
    changed
}

#[cfg(test)]
mod settings_migration_tests {
    use super::*;

    #[test]
    fn retired_service_data_is_purged_without_resetting_library_or_other_settings() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let exported = directory.path().join("translations");
        fs::create_dir(&exported)?;
        let text = exported.join("saved.txt");
        fs::write(&text, "Previously saved text")?;
        let connection = Connection::open(directory.path().join("reader.sqlite3"))?;
        connection.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, json TEXT NOT NULL);
             CREATE TABLE books (id TEXT PRIMARY KEY, json TEXT NOT NULL);",
        )?;
        let old = serde_json::json!({
            "translation": {"provider":"DeepLFree","api_key":"fixture-retired-key"},
            "translation_target":"JA",
            "theme":"OneDark", "font_size":31.5, "calibre_path":"fixture-calibre",
            "key_bindings": {"TranslateSelection":"Alt+T", "TranslateChapter":"Alt+K",
                             "NextPage":"Alt+N", "PinDictionary":"Alt+D"},
            "tts":{"provider_id":"windows_sapi","voice":"fixture-voice","speed":3.25},
            "opds":{"url":"https://catalog.example.invalid/feed","password":"fixture-kept-password"},
            "custom_shelves":{"kept shelf":["book-1"]},
            "future_setting":{"preserved":true}
        });
        connection.execute("INSERT INTO settings VALUES('app',?1)", [old.to_string()])?;
        let entry = BookEntry {
            id: "book-1".into(),
            uuid: None,
            calibre_id: None,
            title: "Kept book".into(),
            authors: vec!["Fixture".into()],
            tags: vec![],
            epub_path: directory.path().join("source.epub"),
            cover_path: None,
            source: BookSource::Imported,
            added_at: 7,
        };
        connection.execute(
            "INSERT INTO books VALUES(?1,?2)",
            params![entry.id, serde_json::to_string(&entry)?],
        )?;
        drop(connection);

        let store = LibraryStore::open(directory.path())?;
        assert_eq!(store.settings().font_size, 31.5);
        assert_eq!(store.settings().theme, crate::Theme::OneDark);
        assert_eq!(store.settings().tts.voice.as_deref(), Some("fixture-voice"));
        assert_eq!(store.settings().tts.speed, 3.25);
        assert_eq!(store.settings().key_bindings["NextPage"], "Alt+N");
        assert_eq!(store.books()[0].id, entry.id);
        assert_eq!(fs::read_to_string(text)?, "Previously saved text");
        let raw: String =
            store
                .connection
                .query_row("SELECT json FROM settings WHERE key='app'", [], |row| {
                    row.get(0)
                })?;
        assert!(!raw.contains("fixture-retired-key"));
        let cleaned: serde_json::Value = serde_json::from_str(&raw)?;
        assert!(cleaned.get("translation").is_none());
        assert!(cleaned.get("translation_target").is_none());
        assert!(cleaned["key_bindings"].get("TranslateSelection").is_none());
        assert!(cleaned["key_bindings"].get("TranslateChapter").is_none());
        assert_eq!(cleaned["key_bindings"]["PinDictionary"], "Alt+D");
        for field in ["opds", "tts", "custom_shelves", "future_setting"] {
            assert_eq!(cleaned[field], old[field]);
        }
        drop(store);
        let reopened = LibraryStore::open(directory.path())?;
        let repeated: String = reopened.connection.query_row(
            "SELECT json FROM settings WHERE key='app'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(repeated, raw);
        Ok(())
    }

    #[test]
    fn invalid_saved_settings_are_not_overwritten_by_migration() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let connection = Connection::open(directory.path().join("reader.sqlite3"))?;
        connection
            .execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, json TEXT NOT NULL);")?;
        let invalid = r#"{"font_size":"invalid","translation":{"api_key":"fixture-retired-key"}}"#;
        connection.execute("INSERT INTO settings VALUES('app',?1)", [invalid])?;
        assert!(LibraryStore::open(directory.path()).is_err());
        let preserved: String =
            connection.query_row("SELECT json FROM settings WHERE key='app'", [], |row| {
                row.get(0)
            })?;
        assert_eq!(preserved, invalid);
        Ok(())
    }
}
