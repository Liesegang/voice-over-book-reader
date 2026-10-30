use anyhow::{Context, Result};
use mdictlib::{
    ChecksumPolicy, KEY_INDEX_REVISION, KeyIndex, KeyIndexOptions, KeyOrdinal, MddFile, MdxFile,
    OpenOptions,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DictionaryStatus {
    Registered,
    Ready,
    Unavailable(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DictionaryInfo {
    pub id: String,
    pub path: PathBuf,
    pub title: String,
    pub mdd_paths: Vec<PathBuf>,
    pub enabled: bool,
    pub entry_count: u64,
    pub format_version: String,
    pub status: DictionaryStatus,
}

#[derive(Clone, Debug)]
pub struct DictionaryHit {
    pub dictionary_id: String,
    pub dictionary_title: String,
    pub headword: String,
    pub html: String,
    pub ordinal: u64,
    pub redirected_from: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DictionarySuggestion {
    pub dictionary_id: String,
    pub headword: String,
    pub ordinal: u64,
}

#[derive(Clone, Debug, Default)]
pub struct RegistrationReport {
    pub registered: Vec<DictionaryInfo>,
    pub errors: Vec<String>,
}

struct OpenDictionary {
    mdx: MdxFile,
    index: KeyIndex,
    used_at: u64,
    source_stamp: String,
}

/// File-backed dictionaries. Registration reads headers only; index construction
/// and queries can be moved to a worker thread. MDD archives are never unpacked.
pub struct DictionaryCatalog {
    cache_dir: PathBuf,
    dictionaries: Vec<DictionaryInfo>,
    opened: HashMap<String, OpenDictionary>,
    counter: u64,
    errors: Vec<String>,
}

impl DictionaryCatalog {
    pub fn open(cache_dir: impl AsRef<Path>) -> Result<Self> {
        let cache_dir = cache_dir.as_ref().to_path_buf();
        fs::create_dir_all(&cache_dir)?;
        let path = cache_dir.join("dictionaries.json");
        let mut dictionaries: Vec<DictionaryInfo> = if path.is_file() {
            serde_json::from_str(&fs::read_to_string(path)?)
                .context("Read registered dictionaries")?
        } else {
            Vec::new()
        };
        for info in &mut dictionaries {
            info.title = display_dictionary_title(&info.title, &info.path);
        }
        Ok(Self {
            cache_dir,
            dictionaries,
            opened: HashMap::new(),
            counter: 0,
            errors: Vec::new(),
        })
    }

    pub fn dictionaries(&self) -> &[DictionaryInfo] {
        &self.dictionaries
    }
    pub fn last_errors(&self) -> &[String] {
        &self.errors
    }

    /// Add one MDX or discover all MDX files below a directory. Existing files
    /// retain their enabled state and priority. Invalid files get visible diagnostics.
    pub fn register(&mut self, path: impl AsRef<Path>) -> Result<RegistrationReport> {
        let path = fs::canonicalize(path.as_ref()).context("Dictionary path is unavailable")?;
        let mut report = RegistrationReport::default();
        let mut paths = if path.is_dir() {
            let mut found = Vec::new();
            for entry in walkdir::WalkDir::new(&path).follow_links(false) {
                match entry {
                    Ok(entry)
                        if entry.file_type().is_file() && is_extension(entry.path(), "mdx") =>
                    {
                        found.push(entry.into_path())
                    }
                    Ok(_) => {}
                    Err(error) => report.errors.push(error.to_string()),
                }
            }
            found
        } else {
            anyhow::ensure!(
                is_extension(&path, "mdx"),
                "Choose an MDX file or a dictionary folder"
            );
            vec![path]
        };
        paths.sort();
        for path in paths {
            match self.register_one(&path) {
                Ok(info) => {
                    if let DictionaryStatus::Unavailable(error) = &info.status {
                        report
                            .errors
                            .push(format!("{}: {error}", info.path.display()));
                    }
                    report.registered.push(info);
                }
                Err(error) => report.errors.push(format!("{}: {error:#}", path.display())),
            }
        }
        self.persist()?;
        Ok(report)
    }

    fn register_one(&mut self, path: &Path) -> Result<DictionaryInfo> {
        let path = fs::canonicalize(path)?;
        let id = blake3::hash(path.to_string_lossy().as_bytes())
            .to_hex()
            .to_string();
        let fallback_title = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let (title, entry_count, format_version, status) =
            match MdxFile::open_with_options(&path, &parser_options()) {
                Ok(mdx) => (
                    display_dictionary_title(mdx.header().title().unwrap_or_default(), &path),
                    mdx.len(),
                    mdx.header().generated_by_engine_version().to_owned(),
                    DictionaryStatus::Registered,
                ),
                Err(error) => (
                    fallback_title,
                    0,
                    "unavailable".into(),
                    DictionaryStatus::Unavailable(error.to_string()),
                ),
            };
        let existing = self.dictionaries.iter().position(|info| info.id == id);
        let enabled = existing
            .map(|index| self.dictionaries[index].enabled)
            .unwrap_or(true);
        let info = DictionaryInfo {
            id: id.clone(),
            path: path.clone(),
            title,
            mdd_paths: companion_mdds(&path)?,
            enabled,
            entry_count,
            format_version,
            status,
        };
        self.opened.remove(&id);
        if let Some(index) = existing {
            self.dictionaries[index] = info.clone();
        } else {
            self.dictionaries.push(info.clone());
        }
        Ok(info)
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<()> {
        self.dictionaries
            .iter_mut()
            .find(|info| info.id == id)
            .context("Unknown dictionary")?
            .enabled = enabled;
        self.persist()
    }

    pub fn move_to(&mut self, id: &str, position: usize) -> Result<()> {
        let index = self
            .dictionaries
            .iter()
            .position(|info| info.id == id)
            .context("Unknown dictionary")?;
        let entry = self.dictionaries.remove(index);
        self.dictionaries
            .insert(position.min(self.dictionaries.len()), entry);
        self.persist()
    }

    pub fn remove(&mut self, id: &str) -> Result<()> {
        self.dictionaries.retain(|info| info.id != id);
        self.opened.remove(id);
        self.persist()
    }

    /// Builds an on-disk, source-bound MDX key index with cancellation. Call on
    /// a worker thread, never in an immediate-mode GUI frame.
    pub fn prepare(&mut self, id: &str, mut cancelled: impl FnMut() -> bool) -> Result<()> {
        self.counter = self.counter.wrapping_add(1);
        let info = self
            .dictionaries
            .iter()
            .find(|info| info.id == id)
            .context("Unknown dictionary")?
            .clone();
        let source_stamp = file_stamp(&info.path)?;
        if let Some(opened) = self.opened.get_mut(id)
            && opened.source_stamp == source_stamp
        {
            opened.used_at = self.counter;
            return Ok(());
        }
        self.opened.remove(id);
        let mdx = MdxFile::open_with_options(&info.path, &parser_options())
            .with_context(|| format!("Open dictionary {}", info.title))?;
        let index_path = self
            .cache_dir
            .join(format!("{id}-{KEY_INDEX_REVISION}.aaidx"));
        let options = KeyIndexOptions::new()
            .with_scratch_directory(&self.cache_dir)
            .with_checksum_policy(ChecksumPolicy::Verify);
        let identity = mdx.key_index_source_identity()?;
        let index = match mdx.open_key_index(&index_path, &identity, &options) {
            Ok(index) => index,
            Err(_) => {
                let pending = self
                    .cache_dir
                    .join(format!("{id}.{}.pending", uuid::Uuid::new_v4()));
                let result = mdx.build_key_index_to_path(&pending, &options, &mut cancelled);
                if let Err(error) = result {
                    let _ = fs::remove_file(&pending);
                    return Err(error.into());
                }
                // Both paths are app-owned caches in the same directory. A stale
                // cache is disposable; the original MDX is never modified.
                if index_path.exists() {
                    fs::remove_file(&index_path)?;
                }
                fs::rename(&pending, &index_path)?;
                mdx.open_key_index(&index_path, &identity, &options)?
            }
        };
        if self.opened.len() >= 8
            && let Some(oldest) = self
                .opened
                .iter()
                .min_by_key(|(_, entry)| entry.used_at)
                .map(|(id, _)| id.clone())
        {
            self.opened.remove(&oldest);
        }
        self.opened.insert(
            id.to_owned(),
            OpenDictionary {
                mdx,
                index,
                used_at: self.counter,
                source_stamp,
            },
        );
        if let Some(info) = self.dictionaries.iter_mut().find(|info| info.id == id) {
            info.status = DictionaryStatus::Ready;
        }
        Ok(())
    }

    pub fn lookup(&mut self, query: &str, limit: usize) -> Result<Vec<DictionaryHit>> {
        self.lookup_cancellable(query, limit, || false)
    }

    pub fn lookup_cancellable(
        &mut self,
        query: &str,
        limit: usize,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Vec<DictionaryHit>> {
        self.errors.clear();
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let ids: Vec<_> = self
            .dictionaries
            .iter()
            .filter(|info| info.enabled)
            .map(|info| info.id.clone())
            .collect();
        let mut hits = Vec::new();
        for id in ids {
            anyhow::ensure!(!cancelled(), "Dictionary lookup cancelled");
            match self.lookup_in_cancellable(
                &id,
                query,
                limit.saturating_sub(hits.len()),
                &mut cancelled,
            ) {
                Ok(rows) => hits.extend(rows),
                Err(error) => self.errors.push(format!("{id}: {error:#}")),
            }
            if hits.len() >= limit {
                break;
            }
        }
        Ok(hits)
    }

    pub fn lookup_in(&mut self, id: &str, query: &str, limit: usize) -> Result<Vec<DictionaryHit>> {
        self.lookup_in_cancellable(id, query, limit, || false)
    }

    pub fn lookup_in_cancellable(
        &mut self,
        id: &str,
        query: &str,
        limit: usize,
        cancelled: impl FnMut() -> bool,
    ) -> Result<Vec<DictionaryHit>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.prepare(id, cancelled)?;
        let info = self
            .dictionaries
            .iter()
            .find(|info| info.id == id)
            .context("Unknown dictionary")?;
        let opened = self.opened.get(id).context("Dictionary not ready")?;
        let mut result = Vec::new();
        let Some(page) = opened
            .mdx
            .locate_page_with_key_index(&opened.index, query, 0, limit)?
        else {
            return Ok(result);
        };
        for ordinal in page.iter() {
            if let Some((headword, html, resolved_ordinal, redirected)) =
                resolve_entry(opened, ordinal, &mut HashSet::new(), 0)?
            {
                result.push(DictionaryHit {
                    dictionary_id: id.into(),
                    dictionary_title: info.title.clone(),
                    headword,
                    html,
                    ordinal: resolved_ordinal,
                    redirected_from: redirected.then(|| query.into()),
                });
            }
        }
        Ok(result)
    }

    pub fn prefix(&mut self, query: &str, limit: usize) -> Result<Vec<DictionarySuggestion>> {
        self.errors.clear();
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let ids: Vec<_> = self
            .dictionaries
            .iter()
            .filter(|info| info.enabled)
            .map(|info| info.id.clone())
            .collect();
        let mut results = Vec::new();
        for id in ids {
            if let Err(error) = self.prepare(&id, || false) {
                self.errors.push(error.to_string());
                continue;
            }
            let opened = &self.opened[&id];
            match opened.mdx.prefix_keys_with_index(
                &opened.index,
                query,
                limit.saturating_sub(results.len()),
            ) {
                Ok(keys) => results.extend(keys.into_iter().map(|key| DictionarySuggestion {
                    dictionary_id: id.clone(),
                    headword: key.key().into(),
                    ordinal: key.ordinal().get(),
                })),
                Err(error) => self.errors.push(error.to_string()),
            }
            if results.len() >= limit {
                break;
            }
        }
        Ok(results)
    }

    /// Resolve one resource from companion MDDs, then the dictionary folder.
    /// Reads only requested record blocks; never expands an archive to disk.
    pub fn resource(&self, id: &str, key: &str) -> Result<Option<Vec<u8>>> {
        let info = self
            .dictionaries
            .iter()
            .find(|info| info.id == id)
            .context("Unknown dictionary")?;
        let relative = normalize_resource_key(key)?;
        let mdd_key = format!("\\{}", relative.replace('/', "\\"));
        for path in &info.mdd_paths {
            let mdd = MddFile::open_with_options(path, &parser_options())
                .with_context(|| format!("Open resource container {}", path.display()))?;
            // MDD's built-in locator is in-memory. A private SQLite key sidecar
            // avoids retaining every resource key for a multi-gigabyte archive.
            if let Some(ordinal) = self.resource_ordinal(path, &mdd, &mdd_key)? {
                let Some(span) = mdd.span_at(KeyOrdinal::new(ordinal))? else {
                    continue;
                };
                anyhow::ensure!(
                    span.len() <= 64 * 1024 * 1024,
                    "Dictionary resource exceeds 64 MiB"
                );
                let mut bytes = Vec::with_capacity(span.len() as usize);
                span.copy_to(&mut bytes)?;
                return Ok(Some(bytes));
            }
        }
        let directory = info
            .path
            .parent()
            .context("Dictionary has no parent folder")?;
        let path = directory.join(&relative);
        if path.is_file() {
            let canonical = fs::canonicalize(&path)?;
            anyhow::ensure!(
                canonical.starts_with(directory),
                "Dictionary resource is outside its directory"
            );
            anyhow::ensure!(
                fs::metadata(&canonical)?.len() <= 64 * 1024 * 1024,
                "Dictionary resource exceeds 64 MiB"
            );
            return Ok(Some(fs::read(canonical)?));
        }
        Ok(None)
    }

    fn resource_ordinal(&self, source: &Path, mdd: &MddFile, key: &str) -> Result<Option<u64>> {
        let stamp = file_stamp(source)?;
        let source_id = blake3::hash(source.to_string_lossy().as_bytes())
            .to_hex()
            .to_string();
        let index_path = self.cache_dir.join(format!("mdd-{source_id}-v1.sqlite3"));
        let cached = Connection::open_with_flags(&index_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()
            .filter(|connection| {
                connection
                    .query_row(
                        "SELECT value FROM metadata WHERE name='source_stamp'",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .ok()
                    .as_deref()
                    == Some(&stamp)
            });
        let connection = if let Some(connection) = cached {
            connection
        } else {
            let pending = tempfile::NamedTempFile::new_in(&self.cache_dir)?;
            let mut connection = Connection::open(pending.path())?;
            connection.execute_batch("CREATE TABLE metadata(name TEXT PRIMARY KEY,value TEXT NOT NULL); CREATE TABLE resources(ordinal INTEGER PRIMARY KEY,raw_key TEXT NOT NULL,folded_key TEXT NOT NULL); CREATE INDEX resource_raw ON resources(raw_key); CREATE INDEX resource_folded ON resources(folded_key);")?;
            {
                let transaction = connection.transaction()?;
                {
                    let mut insert = transaction.prepare(
                        "INSERT INTO resources(ordinal,raw_key,folded_key) VALUES(?1,?2,?3)",
                    )?;
                    for key in mdd.keys() {
                        let key = key?;
                        insert.execute(params![
                            i64::try_from(key.ordinal().get())?,
                            key.key(),
                            key.key().to_lowercase()
                        ])?;
                    }
                }
                anyhow::ensure!(file_stamp(source)? == stamp, "MDD changed while indexing");
                transaction.execute(
                    "INSERT INTO metadata(name,value) VALUES('source_stamp',?1)",
                    [&stamp],
                )?;
                transaction.commit()?;
            }
            drop(connection);
            pending.persist(&index_path).map_err(|error| error.error)?;
            Connection::open_with_flags(&index_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?
        };
        let without_prefix = key.trim_start_matches('\\');
        let exact: Option<i64> = connection
            .query_row(
                "SELECT ordinal FROM resources WHERE raw_key IN (?1,?2) ORDER BY ordinal LIMIT 1",
                params![key, without_prefix],
                |row| row.get(0),
            )
            .optional()?;
        if exact.is_some() || mdd.header().key_case_sensitive() {
            return exact
                .map(|value| u64::try_from(value).map_err(Into::into))
                .transpose();
        }
        let folded: Option<i64> = connection.query_row("SELECT ordinal FROM resources WHERE folded_key IN (?1,?2) ORDER BY ordinal LIMIT 1", params![key.to_lowercase(),without_prefix.to_lowercase()], |row| row.get(0)).optional()?;
        folded
            .map(|value| u64::try_from(value).map_err(Into::into))
            .transpose()
    }

    fn persist(&self) -> Result<()> {
        let mut pending = tempfile::NamedTempFile::new_in(&self.cache_dir)?;
        serde_json::to_writer_pretty(&mut pending, &self.dictionaries)?;
        pending.flush()?;
        pending.as_file().sync_all()?;
        pending
            .persist(self.cache_dir.join("dictionaries.json"))
            .map_err(|e| e.error)?;
        Ok(())
    }
}

fn parser_options() -> OpenOptions {
    OpenOptions::new().with_checksum_policy(ChecksumPolicy::Verify)
}

fn file_stamp(path: &Path) -> Result<String> {
    let metadata = fs::metadata(path)?;
    let modified = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(format!("{}:{modified}", metadata.len()))
}

fn resolve_entry(
    opened: &OpenDictionary,
    ordinal: KeyOrdinal,
    visited: &mut HashSet<u64>,
    depth: usize,
) -> Result<Option<(String, String, u64, bool)>> {
    anyhow::ensure!(
        depth <= 16 && visited.insert(ordinal.get()),
        "Dictionary redirect cycle or depth limit"
    );
    let Some(entry) = opened.mdx.entry_at(ordinal)? else {
        return Ok(None);
    };
    if let Some(target) = entry
        .text()
        .trim_start_matches('\u{feff}')
        .trim()
        .strip_prefix("@@@LINK=")
    {
        let target = target.trim_matches(|ch: char| ch.is_whitespace() || ch == '\0');
        let Some(matches) = opened
            .mdx
            .locate_page_with_key_index(&opened.index, target, 0, 1)?
        else {
            return Ok(None);
        };
        let Some(next) = matches.iter().next() else {
            return Ok(None);
        };
        return Ok(resolve_entry(opened, next, visited, depth + 1)?
            .map(|(key, html, ordinal, _)| (key, html, ordinal, true)));
    }
    let html = expand_stylesheet(entry.text(), opened.mdx.header().attribute("StyleSheet"));
    Ok(Some((entry.key().into(), html, ordinal.get(), depth > 0)))
}

fn expand_stylesheet(text: &str, stylesheet: Option<&str>) -> String {
    let Some(stylesheet) = stylesheet else {
        return text.into();
    };
    let lines: Vec<_> = stylesheet
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .collect();
    let mut styles = HashMap::new();
    for group in lines.chunks_exact(3) {
        styles.insert(group[0].trim(), (group[1], group[2]));
    }
    if styles.is_empty() {
        return text.into();
    }
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    let mut close = "";
    while let Some(start) = text[cursor..].find('`').map(|offset| cursor + offset) {
        output.push_str(&text[cursor..start]);
        let Some(end) = text[start + 1..].find('`').map(|offset| start + 1 + offset) else {
            cursor = start;
            break;
        };
        let token = &text[start + 1..end];
        if let Some((begin, end_tag)) = styles.get(token) {
            output.push_str(close);
            output.push_str(begin);
            close = end_tag;
        } else {
            output.push_str(&text[start..=end]);
        }
        cursor = end + 1;
    }
    output.push_str(&text[cursor..]);
    output.push_str(close);
    output
}

fn normalize_resource_key(key: &str) -> Result<String> {
    let key = key.strip_prefix("sound://").unwrap_or(key);
    let decoded = percent_encoding::percent_decode_str(key)
        .decode_utf8()
        .context("Invalid dictionary resource name")?;
    let normalized = decoded.replace('\\', "/");
    let relative = normalized.trim_start_matches('/');
    anyhow::ensure!(
        !relative.is_empty() && !relative.contains('\0') && !relative.contains(':'),
        "Invalid dictionary resource path"
    );
    anyhow::ensure!(
        Path::new(relative)
            .components()
            .all(|c| matches!(c, Component::Normal(_))),
        "Invalid dictionary resource path"
    );
    Ok(relative.into())
}

fn display_dictionary_title(title: &str, path: &Path) -> String {
    let compact = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let lowercase = compact.to_ascii_lowercase();
    if compact.is_empty()
        || compact.contains(['<', '>'])
        || lowercase.contains("&lt;")
        || lowercase.contains("&gt;")
        || lowercase.contains("no html code allowed")
        || matches!(
            lowercase.as_str(),
            "title" | "untitled" | "dictionary title"
        )
    {
        path.file_stem()
            .filter(|name| !name.is_empty())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "辞書".into())
    } else {
        compact
    }
}

fn companion_mdds(mdx: &Path) -> Result<Vec<PathBuf>> {
    let stem = mdx.file_stem().unwrap_or_default().to_string_lossy();
    let mut result = Vec::<(u64, PathBuf)>::new();
    for entry in fs::read_dir(mdx.parent().context("MDX parent directory")?)? {
        let path = entry?.path();
        if !is_extension(&path, "mdd") {
            continue;
        }
        let name = path.file_stem().unwrap_or_default().to_string_lossy();
        if name.eq_ignore_ascii_case(&stem) {
            result.push((0, path));
        } else if let Some((base, number)) = name.rsplit_once('.')
            && base.eq_ignore_ascii_case(&stem)
            && let Ok(number) = number.parse::<u64>()
        {
            result.push((number.saturating_add(1), path));
        }
    }
    result.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    Ok(result.into_iter().map(|(_, path)| path).collect())
}

fn is_extension(path: &Path, expected: &str) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(expected))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stylesheet_switches_close_previous_style() {
        assert_eq!(
            expand_stylesheet("`1`first`2`second", Some("1\n<b>\n</b>\n2\n<i>\n</i>")),
            "<b>first</b><i>second</i>"
        );
        assert_eq!(
            expand_stylesheet("text `oops", Some("1\n<b>\n</b>")),
            "text `oops"
        );
    }
    #[test]
    fn resource_paths_preserve_unicode_and_reject_traversal() {
        assert_eq!(
            normalize_resource_key("sound://%E9%9F%B3%E5%A3%B0/a.mp3").unwrap(),
            "音声/a.mp3"
        );
        assert!(normalize_resource_key("../outside.txt").is_err());
        assert!(normalize_resource_key("C:/outside.txt").is_err());
    }
}
