use eframe::egui::{self, TextureHandle};
use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender},
    },
};
type ResultImage = (PathBuf, Option<egui::ColorImage>);
pub struct CoverCache {
    images: HashMap<PathBuf, (Option<TextureHandle>, u64)>,
    pending: HashSet<PathBuf>,
    jobs: SyncSender<PathBuf>,
    results: Receiver<ResultImage>,
    clock: u64,
}
impl CoverCache {
    pub fn new(data: &Path) -> Self {
        let cache = data.join("cover-thumbnails-v2");
        let (jobs, requests) = mpsc::sync_channel::<PathBuf>(24);
        let requests = Arc::new(Mutex::new(requests));
        let (results_tx, results) = mpsc::channel();
        for _ in 0..2 {
            let requests = Arc::clone(&requests);
            let tx = results_tx.clone();
            let cache = cache.clone();
            std::thread::spawn(move || {
                loop {
                    let request = {
                        let Ok(rx) = requests.lock() else {
                            return;
                        };
                        rx.recv()
                    };
                    let Ok(path) = request else {
                        return;
                    };
                    let image = thumbnail(&path, &cache).ok().map(|rgba| {
                        egui::ColorImage::from_rgba_unmultiplied(
                            [rgba.width() as usize, rgba.height() as usize],
                            rgba.as_raw(),
                        )
                    });
                    if tx.send((path, image)).is_err() {
                        return;
                    }
                }
            });
        }
        Self {
            images: HashMap::new(),
            pending: HashSet::new(),
            jobs,
            results,
            clock: 0,
        }
    }
    pub fn get(&mut self, ctx: &egui::Context, path: &Path) -> Option<TextureHandle> {
        while let Ok((key, image)) = self.results.try_recv() {
            self.pending.remove(&key);
            let texture = image.map(|image| {
                ctx.load_texture(key.to_string_lossy(), image, egui::TextureOptions::LINEAR)
            });
            self.images.insert(key, (texture, self.clock));
        }
        self.clock = self.clock.wrapping_add(1);
        while self.images.len() > 64 {
            let Some(oldest) = self
                .images
                .iter()
                .filter(|(key, _)| key.as_path() != path)
                .min_by_key(|(_, (_, used))| used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.images.remove(&oldest);
        }
        if !self.pending.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(24));
        }
        if let Some((texture, used)) = self.images.get_mut(path) {
            *used = self.clock;
            return texture.clone();
        }
        if !self.pending.contains(path) && self.jobs.try_send(path.to_owned()).is_ok() {
            self.pending.insert(path.to_owned());
        }
        if !self.pending.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(24));
        }
        None
    }
}
fn thumbnail(path: &Path, cache: &Path) -> anyhow::Result<image::RgbaImage> {
    let metadata = std::fs::metadata(path)?;
    let mut key = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut key);
    metadata.len().hash(&mut key);
    metadata.modified().ok().hash(&mut key);
    let target = cache.join(format!("{:016x}.png", key.finish()));
    if let Ok(image) = image::open(&target) {
        return Ok(image.to_rgba8());
    }
    let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode()?.thumbnail(280, 380).to_rgba8();
    if std::fs::create_dir_all(cache).is_ok() {
        let _ = image.save(&target);
    }
    Ok(image)
}
