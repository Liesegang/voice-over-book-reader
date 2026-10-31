use crate::{Chapter, import::normalize_path};
use anyhow::{Context, Result, anyhow, bail};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    fmt,
    io::{Read, Seek},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

const MAX_RESOURCE: u64 = 64 * 1024 * 1024;
const RESOURCE_CACHE_BYTES: usize = 64 * 1024 * 1024;
trait ArchiveReader: Read + Seek + Send {}
impl<T: Read + Seek + Send> ArchiveReader for T {}

#[derive(Clone, Debug)]
pub(crate) struct ChapterSpec {
    pub id: u32,
    pub href: String,
    pub title: String,
    pub presentation: crate::PagePresentation,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ArchiveLoadStats {
    pub entries: usize,
    pub resource_reads: u64,
    pub expanded_bytes: u64,
    pub chapters_parsed: u64,
    pub cached_resource_bytes: usize,
    pub cached_chapters: usize,
}

#[derive(Default)]
struct ResourceCache {
    entries: VecDeque<(String, Arc<Vec<u8>>)>,
    bytes: usize,
}

/// Shared read-only EPUB source. A clone of Book shares its archive and bounded
/// caches instead of copying all compressed/expanded assets or parsing chapters.
pub struct EpubSource {
    zip: Mutex<zip::ZipArchive<Box<dyn ArchiveReader>>>,
    index: HashMap<String, usize>,
    identity: String,
    resources: Mutex<ResourceCache>,
    specs: OnceLock<Vec<ChapterSpec>>,
    media_types: OnceLock<BTreeMap<String, String>>,
    chapters: Mutex<VecDeque<(usize, Arc<Chapter>)>>,
    reads: AtomicU64,
    expanded: AtomicU64,
    parsed: AtomicU64,
    dimensions: Mutex<HashMap<String, Option<[f32; 2]>>>,
}

impl fmt::Debug for EpubSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EpubSource")
            .field("stats", &self.stats())
            .finish()
    }
}

impl EpubSource {
    pub(crate) fn new<R: Read + Seek + Send + 'static>(reader: R) -> Result<Self> {
        let mut zip = zip::ZipArchive::new(Box::new(reader) as Box<dyn ArchiveReader>)
            .context("Invalid EPUB ZIP archive")?;
        let mut index = HashMap::new();
        let mut fingerprint = Vec::new();
        for position in 0..zip.len() {
            let entry = zip.by_index_raw(position)?;
            if entry.is_dir() {
                continue;
            }
            let name = normalize_path(&entry.name().replace('\\', "/"))?;
            if index.insert(name.clone(), position).is_some() {
                bail!("Duplicate normalized EPUB resource: {name}");
            }
            fingerprint.push((name, entry.crc32(), entry.size()));
        }
        fingerprint.sort_by(|left, right| left.0.cmp(&right.0));
        let mut hash = blake3::Hasher::new();
        hash.update(b"epub-logical-content-v1\0");
        for (name, crc, size) in fingerprint {
            hash.update(&(name.len() as u64).to_le_bytes());
            hash.update(name.as_bytes());
            hash.update(&crc.to_le_bytes());
            hash.update(&size.to_le_bytes());
        }
        Ok(Self {
            zip: Mutex::new(zip),
            index,
            identity: hash.finalize().to_hex().to_string(),
            resources: Mutex::new(ResourceCache::default()),
            specs: OnceLock::new(),
            media_types: OnceLock::new(),
            chapters: Mutex::new(VecDeque::new()),
            reads: AtomicU64::new(0),
            expanded: AtomicU64::new(0),
            parsed: AtomicU64::new(0),
            dimensions: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn identity(&self) -> &str {
        &self.identity
    }
    pub(crate) fn set_chapters(&self, specs: Vec<ChapterSpec>) -> Result<()> {
        self.specs
            .set(specs)
            .map_err(|_| anyhow!("EPUB chapter metadata was already initialized"))
    }

    /// Declared manifest media types, keyed by decoded archive-relative paths.
    /// These remain available without opening or expanding the listed resources.
    pub fn media_types(&self) -> Option<&BTreeMap<String, String>> {
        self.media_types.get()
    }

    pub(crate) fn set_media_types(&self, media_types: BTreeMap<String, String>) -> Result<()> {
        self.media_types
            .set(media_types)
            .map_err(|_| anyhow!("EPUB resource media types were already initialized"))
    }

    pub(crate) fn document_spec(&self, id: u32) -> Option<&ChapterSpec> {
        self.specs
            .get()?
            .get(id as usize)
            .filter(|spec| spec.id == id)
    }

    pub(crate) fn document_spec_by_href(&self, href: &str) -> Option<&ChapterSpec> {
        self.specs.get()?.iter().find(|spec| spec.href == href)
    }

    pub fn resource(&self, path: &str) -> Result<Option<Arc<Vec<u8>>>> {
        let path = normalize_path(path)?;
        let Some(&position) = self.index.get(&path) else {
            return Ok(None);
        };
        {
            let mut cache = self
                .resources
                .lock()
                .map_err(|_| anyhow!("EPUB resource cache lock poisoned"))?;
            if let Some(index) = cache.entries.iter().position(|(name, _)| *name == path) {
                let entry = cache.entries.remove(index).unwrap();
                let bytes = Arc::clone(&entry.1);
                cache.entries.push_back(entry);
                return Ok(Some(bytes));
            }
        }
        let bytes = {
            let mut zip = self
                .zip
                .lock()
                .map_err(|_| anyhow!("EPUB archive lock poisoned"))?;
            let entry = zip
                .by_index(position)
                .with_context(|| format!("Read EPUB resource {path}"))?;
            if entry.size() > MAX_RESOURCE {
                bail!("EPUB resource is larger than 64 MiB: {path}");
            }
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            entry
                .take(MAX_RESOURCE + 1)
                .read_to_end(&mut bytes)
                .with_context(|| format!("Expand EPUB resource {path}"))?;
            if bytes.len() > MAX_RESOURCE as usize {
                bail!("EPUB resource exceeded its expansion limit: {path}");
            }
            Arc::new(bytes)
        };
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.expanded
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        let mut cache = self
            .resources
            .lock()
            .map_err(|_| anyhow!("EPUB resource cache lock poisoned"))?;
        while !cache.entries.is_empty()
            && (cache.entries.len() >= 128 || cache.bytes + bytes.len() > RESOURCE_CACHE_BYTES)
        {
            if let Some((_, removed)) = cache.entries.pop_front() {
                cache.bytes -= removed.len();
            }
        }
        cache.bytes += bytes.len();
        cache.entries.push_back((path, Arc::clone(&bytes)));
        Ok(Some(bytes))
    }

    pub fn load_chapter(&self, index: usize) -> Result<Chapter> {
        {
            let mut cache = self
                .chapters
                .lock()
                .map_err(|_| anyhow!("EPUB chapter cache lock poisoned"))?;
            if let Some(position) = cache.iter().position(|(id, _)| *id == index) {
                let entry = cache.remove(position).unwrap();
                let chapter = (*entry.1).clone();
                cache.push_back(entry);
                return Ok(chapter);
            }
        }
        let spec = self
            .specs
            .get()
            .and_then(|specs| specs.get(index))
            .context("EPUB chapter index is out of range")?;
        let chapter = crate::import::parse_epub_chapter(self, spec)?;
        self.parsed.fetch_add(1, Ordering::Relaxed);
        let mut cache = self
            .chapters
            .lock()
            .map_err(|_| anyhow!("EPUB chapter cache lock poisoned"))?;
        if cache.len() >= 3 {
            cache.pop_front();
        }
        cache.push_back((index, Arc::new(chapter.clone())));
        Ok(chapter)
    }

    pub(crate) fn image_dimensions(&self, path: &str) -> Result<Option<[f32; 2]>> {
        if let Some(dimensions) = self
            .dimensions
            .lock()
            .map_err(|_| anyhow!("Image metadata cache lock poisoned"))?
            .get(path)
            .copied()
        {
            return Ok(dimensions);
        }
        let Some(&position) = self.index.get(path) else {
            return Ok(None);
        };
        let cached = self
            .resources
            .lock()
            .map_err(|_| anyhow!("EPUB resource cache lock poisoned"))?
            .entries
            .iter()
            .find(|(name, _)| name == path)
            .map(|(_, bytes)| Arc::clone(bytes));
        let dimensions = if let Some(bytes) = cached {
            crate::image_meta::dimensions(&bytes)
        } else {
            let mut zip = self
                .zip
                .lock()
                .map_err(|_| anyhow!("EPUB archive lock poisoned"))?;
            let mut entry = zip.by_index(position)?;
            let mut prefix = Vec::new();
            (&mut entry).take(4096).read_to_end(&mut prefix)?;
            let mut dimensions = crate::image_meta::dimensions(&prefix);
            if dimensions.is_none() && prefix.starts_with(&[0xff, 0xd8]) {
                (&mut entry).take(65536 - 4096).read_to_end(&mut prefix)?;
                dimensions = crate::image_meta::dimensions(&prefix);
            }
            self.reads.fetch_add(1, Ordering::Relaxed);
            self.expanded
                .fetch_add(prefix.len() as u64, Ordering::Relaxed);
            dimensions
        };
        let mut cache = self
            .dimensions
            .lock()
            .map_err(|_| anyhow!("Image metadata cache lock poisoned"))?;
        if cache.len() >= 512 {
            cache.clear();
        }
        cache.insert(path.to_owned(), dimensions);
        Ok(dimensions)
    }

    pub fn stats(&self) -> ArchiveLoadStats {
        ArchiveLoadStats {
            entries: self.index.len(),
            resource_reads: self.reads.load(Ordering::Relaxed),
            expanded_bytes: self.expanded.load(Ordering::Relaxed),
            chapters_parsed: self.parsed.load(Ordering::Relaxed),
            cached_resource_bytes: self.resources.lock().map(|cache| cache.bytes).unwrap_or(0),
            cached_chapters: self.chapters.lock().map(|cache| cache.len()).unwrap_or(0),
        }
    }
}
