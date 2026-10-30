//! Native image loading shared by both reading panes. SVG references resolve
//! exclusively inside the current book, and decoded/retained images are bounded.
use crate::{fonts, layout::Picture};
use eframe::egui::{self, TextureHandle, Vec2, vec2};
use reader_core::Book;
use std::{
    collections::{HashMap, VecDeque},
    hash::{Hash, Hasher},
    io::Cursor,
    sync::{Arc, Condvar, Mutex, OnceLock},
    time::{Duration, Instant},
};

const MAX_EDGE: f32 = 4096.0;
const MAX_DECODE_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Copy)]
pub struct CacheBudget {
    pub entries: usize,
    pub bytes: usize,
}

impl CacheBudget {
    pub const BOOK: Self = Self {
        // A page can contain hundreds of tiny gaiji without consuming the byte
        // budget. A small count cap would evict its own visible characters.
        entries: 1024,
        bytes: 128 * 1024 * 1024,
    };
}

struct DecodeJob {
    key: String,
    book: Book,
    chapter: String,
    picture: Picture,
    target: Vec2,
    context: egui::Context,
    context_id: usize,
    encoded_bytes: usize,
    ink: Option<egui::Color32>,
}
#[derive(Clone, Copy)]
struct DecodeStyle {
    context_id: usize,
    ink: Option<egui::Color32>,
}
struct Requested {
    at: Instant,
    frame: u64,
}
struct Decoded {
    key: String,
    result: Result<Arc<egui::ColorImage>, String>,
    bytes: usize,
}
#[derive(Default)]
struct DecodeState {
    queue: VecDeque<DecodeJob>,
    pending: HashMap<String, Requested>,
    ready: VecDeque<Decoded>,
    ready_bytes: usize,
    queued_bytes: usize,
}
struct Decoder {
    state: Mutex<DecodeState>,
    wake: Condvar,
}

fn decoder() -> &'static Arc<Decoder> {
    static DECODER: OnceLock<Arc<Decoder>> = OnceLock::new();
    DECODER.get_or_init(|| {
        let decoder = Arc::new(Decoder {
            state: Mutex::new(DecodeState::default()),
            wake: Condvar::new(),
        });
        for _ in 0..2 {
            let worker = Arc::clone(&decoder);
            std::thread::spawn(move || {
                loop {
                    let job = {
                        let mut state = worker
                            .state
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        while state.queue.is_empty() {
                            state = worker
                                .wake
                                .wait(state)
                                .unwrap_or_else(|error| error.into_inner());
                        }
                        let job = state.queue.pop_front().unwrap();
                        state.queued_bytes = state.queued_bytes.saturating_sub(job.encoded_bytes);
                        if !state.pending.get(&job.key).is_some_and(|requested| {
                            requested.at.elapsed() < Duration::from_millis(750)
                        }) {
                            state.pending.remove(&job.key);
                            continue;
                        }
                        job
                    };
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let mut image =
                            decode_picture(&job.book, &job.chapter, &job.picture, job.target)?;
                        if let Some(ink) = job.ink {
                            adapt_text_image(&mut image, ink);
                        }
                        Ok::<_, String>(image)
                    }))
                    .unwrap_or_else(|_| Err("画像のdecode処理に失敗しました".into()))
                    .map(Arc::new);
                    let bytes = result
                        .as_ref()
                        .map(|image| image.pixels.len() * 4)
                        .unwrap_or(0);
                    {
                        let mut state = worker
                            .state
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        state.pending.remove(&job.key);
                        while !state.ready.is_empty()
                            && (state.ready.len() >= 1024
                                || state.ready_bytes + bytes > 128 * 1024 * 1024)
                        {
                            if let Some(old) = state.ready.pop_front() {
                                state.ready_bytes -= old.bytes;
                            }
                        }
                        state.ready_bytes += bytes;
                        state.ready.push_back(Decoded {
                            key: job.key,
                            result,
                            bytes,
                        });
                    }
                    job.context.request_repaint();
                }
            });
        }
        decoder
    })
}

fn request_decode(
    ctx: &egui::Context,
    book: &Book,
    chapter: &str,
    picture: &Picture,
    target: Vec2,
    key: &str,
    style: DecodeStyle,
) -> Option<Result<Arc<egui::ColorImage>, String>> {
    let decoder = decoder();
    let frame = ctx.cumulative_frame_nr();
    {
        let mut state = decoder
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(index) = state.ready.iter().position(|entry| entry.key == key) {
            let ready = state.ready.remove(index).unwrap();
            state.ready_bytes -= ready.bytes;
            return Some(ready.result);
        }
        if let Some(requested) = state.pending.get_mut(key) {
            requested.at = Instant::now();
            requested.frame = frame;
            return None;
        }
        // Drop queued pages that this context stopped painting; do not churn
        // the first 64 jobs on a visible page containing many small gaiji.
        while state.pending.len() >= 64 {
            let old = state.queue.iter().position(|job| {
                state.pending.get(&job.key).is_some_and(|requested| {
                    (job.context_id == style.context_id && requested.frame < frame)
                        || requested.at.elapsed() > Duration::from_millis(750)
                })
            });
            let old = old?;
            let old = state.queue.remove(old).unwrap();
            state.queued_bytes = state.queued_bytes.saturating_sub(old.encoded_bytes);
            state.pending.remove(&old.key);
        }
        state.pending.insert(
            key.to_owned(),
            Requested {
                at: Instant::now(),
                frame,
            },
        );
    }
    let view = book.image_resource_view(&picture.resource, picture.inline_svg.as_deref(), chapter);
    let encoded_bytes = view.resources.values().map(Vec::len).sum();
    let mut state = decoder
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if !state.queue.is_empty() && state.queued_bytes + encoded_bytes > 128 * 1024 * 1024 {
        state.pending.remove(key);
        return None;
    }
    state.queued_bytes += encoded_bytes;
    state.queue.push_back(DecodeJob {
        key: key.to_owned(),
        book: view,
        chapter: chapter.to_owned(),
        picture: picture.clone(),
        target,
        context: ctx.clone(),
        context_id: style.context_id,
        encoded_bytes,
        ink: style.ink,
    });
    drop(state);
    decoder.wake.notify_one();
    None
}

fn texture_bytes(texture: &TextureHandle) -> usize {
    texture.size()[0]
        .saturating_mul(texture.size()[1])
        .saturating_mul(4)
}

/// Evict the oldest-created textures first. The existing HashMap API remains
/// usable by the app, while retaining a strict count and approximate RGBA budget.
pub fn enforce_cache_budget<K: Clone + Eq + Hash>(
    cache: &mut HashMap<K, Option<TextureHandle>>,
    budget: CacheBudget,
    protected: Option<&K>,
) {
    while cache.len() > budget.entries
        || cache.values().flatten().map(texture_bytes).sum::<usize>() > budget.bytes
    {
        let victim = cache
            .iter()
            .filter(|(key, _)| protected != Some(*key))
            .min_by_key(|(_, texture)| {
                texture
                    .as_ref()
                    .map(|texture| match texture.id() {
                        egui::TextureId::Managed(id) => id,
                        egui::TextureId::User(_) => u64::MAX,
                    })
                    .unwrap_or(0)
            })
            .map(|(key, _)| key.clone())
            .or_else(|| protected.cloned());
        let Some(victim) = victim else {
            break;
        };
        cache.remove(&victim);
    }
}

/// Queues decoding on two bounded workers. Pending images return None without
/// poisoning the cache; ready results request repaint and upload on the UI pass.
/// Failed images cache None and return their original error only once.
pub fn load_texture(
    ctx: &egui::Context,
    book: &Book,
    chapter_href: &str,
    picture: &Picture,
    cache: &mut HashMap<String, Option<TextureHandle>>,
) -> Result<Option<TextureHandle>, String> {
    load_texture_themed(ctx, book, chapter_href, picture, cache, None)
}

/// Textual inline images can use the current ink color. Ordinary illustrations,
/// fixed pages and PDF renders always retain their original colors.
pub fn load_texture_themed(
    ctx: &egui::Context,
    book: &Book,
    chapter_href: &str,
    picture: &Picture,
    cache: &mut HashMap<String, Option<TextureHandle>>,
    ink: Option<egui::Color32>,
) -> Result<Option<TextureHandle>, String> {
    let ink = ink.filter(|_| text_image_candidate(picture));
    let physical = picture.rect.size() * ctx.pixels_per_point();
    let bucket = |size: f32| {
        if size.is_finite() {
            ((size.clamp(32.0, MAX_EDGE) / 64.0).ceil() * 64.0).min(MAX_EDGE)
        } else {
            64.0
        }
    };
    let requested = vec2(bucket(physical.x), bucket(physical.y));
    let context_id = Arc::as_ptr(&ctx.tex_manager()) as usize;
    let mut variant = std::collections::hash_map::DefaultHasher::new();
    chapter_href.hash(&mut variant);
    picture.inline_svg.hash(&mut variant);
    ink.map(|color| color.to_array()).hash(&mut variant);
    if let Some(canvas) = picture.fixed_canvas {
        for value in [
            canvas.extent.x,
            canvas.extent.y,
            canvas.image_rect.left(),
            canvas.image_rect.top(),
            canvas.image_rect.right(),
            canvas.image_rect.bottom(),
        ] {
            value.to_bits().hash(&mut variant);
        }
    }
    let key = format!(
        "{context_id}:{}:{}:{}:{}:{}x{}",
        book.resource_epoch(),
        book.content_identity(),
        picture.resource,
        variant.finish(),
        requested.x as u32,
        requested.y as u32
    );
    if let Some(texture) = cache.get(&key) {
        return Ok(texture.clone());
    }
    let Some(result) = request_decode(
        ctx,
        book,
        chapter_href,
        picture,
        requested,
        &key,
        DecodeStyle { context_id, ink },
    ) else {
        return Ok(None);
    };
    let result = result.map(|image| ctx.load_texture(&key, image, egui::TextureOptions::LINEAR));
    match result {
        Ok(texture) => {
            cache.insert(key.clone(), Some(texture.clone()));
            enforce_cache_budget(cache, CacheBudget::BOOK, Some(&key));
            Ok(Some(texture))
        }
        Err(error) => {
            cache.insert(key.clone(), None);
            enforce_cache_budget(cache, CacheBudget::BOOK, Some(&key));
            Err(error)
        }
    }
}

/// Call for a few already-laid-out pictures just outside the viewport, after
/// visible pictures have been requested. The same byte/count budgets apply.
pub fn prefetch(
    ctx: &egui::Context,
    book: &Book,
    chapter: &str,
    pictures: &[Picture],
    cache: &mut HashMap<String, Option<TextureHandle>>,
    ink: Option<egui::Color32>,
) {
    for picture in pictures.iter().take(8) {
        let _ = load_texture_themed(ctx, book, chapter, picture, cache, ink);
    }
}

fn text_image_candidate(picture: &Picture) -> bool {
    picture.text_image && picture.fixed_canvas.is_none() && picture.pdf_page.is_none()
}

/// Convert a monochrome text tile to an antialiased alpha mask, once on the
/// decoder worker. Its paper becomes transparent, allowing both theme paper and
/// selection highlights to show through. Colored and continuous-tone images
/// fail the conservative gate and stay byte-for-byte unchanged.
fn adapt_text_image(image: &mut egui::ColorImage, ink: egui::Color32) -> bool {
    let [width, height] = image.size;
    if width == 0 || height == 0 || image.pixels.is_empty() {
        return false;
    }
    let mut visible = 0usize;
    let mut transparent = 0usize;
    let mut binary = 0usize;
    let mut dark = 0usize;
    let mut light = 0usize;
    let mut border_dark = 0usize;
    let mut border_light = 0usize;
    let mut border = 0usize;
    for (index, color) in image.pixels.iter().enumerate() {
        let [r, g, b, a] = color.to_srgba_unmultiplied();
        if a < 16 {
            transparent += 1;
            continue;
        }
        if r.max(g).max(b) - r.min(g).min(b) > 10 {
            return false;
        }
        visible += 1;
        let luma = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
        let is_dark = luma <= 40;
        let is_light = luma >= 215;
        dark += usize::from(is_dark);
        light += usize::from(is_light);
        binary += usize::from(is_dark || is_light);
        let x = index % width;
        let y = index / width;
        if x == 0 || x + 1 == width || y == 0 || y + 1 == height {
            border += 1;
            border_dark += usize::from(is_dark);
            border_light += usize::from(is_light);
        }
    }
    if visible == 0 || binary * 100 < visible * 65 {
        return false;
    }
    let light_paper = if transparent * 100 >= image.pixels.len() * 5 {
        if dark * 100 >= visible * 65 {
            true
        } else if light * 100 >= visible * 65 {
            false
        } else {
            return false;
        }
    } else if border > 0 && border_light * 100 >= border * 80 && dark > 0 {
        true
    } else if border > 0 && border_dark * 100 >= border * 80 && light > 0 {
        false
    } else {
        return false;
    };
    for color in &mut image.pixels {
        let [r, g, b, a] = color.to_srgba_unmultiplied();
        let luma = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
        let coverage = if light_paper { 255 - luma } else { luma };
        let alpha = ((coverage * u16::from(a) + 127) / 255) as u8;
        *color = egui::Color32::from_rgba_unmultiplied(ink.r(), ink.g(), ink.b(), alpha);
    }
    true
}

fn svg_fonts() -> Arc<resvg::usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    Arc::clone(FONTS.get_or_init(|| {
        let mut database = resvg::usvg::fontdb::Database::new();
        for bytes in [fonts::SANS, fonts::SERIF, fonts::MONO] {
            database.load_font_data(bytes.to_vec());
        }
        database.set_sans_serif_family("Noto Sans CJK JP");
        database.set_serif_family("Noto Serif CJK JP");
        database.set_monospace_family("JetBrains Mono");
        Arc::new(database)
    }))
}

fn canvas_pixmap(
    canvas: crate::layout::FixedCanvas,
    source: Vec2,
    target: Vec2,
) -> Result<(resvg::tiny_skia::Pixmap, resvg::tiny_skia::Transform), String> {
    if !canvas.extent.is_finite()
        || canvas.extent.x <= 0.
        || canvas.extent.y <= 0.
        || !canvas.image_rect.is_finite()
        || !canvas.image_rect.is_positive()
        || !source.is_finite()
        || source.x <= 0.
        || source.y <= 0.
    {
        return Err("固定レイアウト画像のページ寸法が無効です".into());
    }
    let scale = (target.x / canvas.extent.x).min(target.y / canvas.extent.y);
    let width = (canvas.extent.x * scale).ceil().clamp(1., MAX_EDGE) as u32;
    let height = (canvas.extent.y * scale).ceil().clamp(1., MAX_EDGE) as u32;
    let pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or("固定レイアウトの画像領域を確保できません")?;
    let transform = resvg::tiny_skia::Transform::from_scale(
        canvas.image_rect.width() * scale / source.x,
        canvas.image_rect.height() * scale / source.y,
    )
    .post_translate(
        canvas.image_rect.left() * scale,
        canvas.image_rect.top() * scale,
    );
    Ok((pixmap, transform))
}

pub fn decode_picture(
    book: &Book,
    chapter_href: &str,
    picture: &Picture,
    target: Vec2,
) -> Result<egui::ColorImage, String> {
    let target = vec2(target.x.clamp(1.0, MAX_EDGE), target.y.clamp(1.0, MAX_EDGE));
    let bytes = if let Some(page) = &picture.pdf_page {
        Arc::new(
            reader_core::render_pdf_page(page, target.x.ceil() as u32)
                .map_err(|error| error.to_string())?,
        )
    } else {
        book.resource_bytes(&picture.resource)
            .map_err(|error| error.to_string())?
            .unwrap_or_default()
    };
    let svg = picture
        .inline_svg
        .as_deref()
        .map(str::as_bytes)
        .or_else(|| {
            if picture.resource.to_ascii_lowercase().ends_with(".svg") || bytes.starts_with(b"<svg")
            {
                Some(bytes.as_slice())
            } else {
                None
            }
        });
    if let Some(svg) = svg {
        let base = if picture.inline_svg.is_some() {
            chapter_href
        } else {
            picture.resource.as_str()
        };
        let mut options = resvg::usvg::Options {
            fontdb: svg_fonts(),
            font_family: "Noto Sans CJK JP".into(),
            ..Default::default()
        };
        options.image_href_resolver.resolve_string = Box::new(|href, options| {
            let path = svg_resource_path(base, href)?;
            let data = book.resource_bytes(&path).ok()??;
            (resvg::usvg::ImageHrefResolver::default_data_resolver())("text/plain", data, options)
        });
        let tree = resvg::usvg::Tree::from_data(svg, &options)
            .map_err(|error| format!("SVGの描画: {error}"))?;
        let size = tree.size();
        if let Some(canvas) = picture.fixed_canvas {
            let (mut pixmap, transform) =
                canvas_pixmap(canvas, vec2(size.width(), size.height()), target)?;
            resvg::render(&tree, transform, &mut pixmap.as_mut());
            return Ok(egui::ColorImage::from_rgba_premultiplied(
                [pixmap.width() as usize, pixmap.height() as usize],
                pixmap.data(),
            ));
        }
        let scale = (target.x / size.width())
            .min(target.y / size.height())
            .max(f32::MIN_POSITIVE);
        let width = (size.width() * scale).ceil().clamp(1.0, MAX_EDGE) as u32;
        let height = (size.height() * scale).ceil().clamp(1.0, MAX_EDGE) as u32;
        let mut pixmap =
            resvg::tiny_skia::Pixmap::new(width, height).ok_or("SVGの画像領域を確保できません")?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        return Ok(egui::ColorImage::from_rgba_premultiplied(
            [width as usize, height as usize],
            pixmap.data(),
        ));
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes.as_slice()))
        .with_guessed_format()
        .map_err(|error| format!("画像の形式: {error}"))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(32_768);
    limits.max_image_height = Some(32_768);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|error| format!("画像の描画: {error}"))?;
    if let Some(canvas) = picture.fixed_canvas {
        let (width, height) = (image.width(), image.height());
        let (mut pixmap, transform) =
            canvas_pixmap(canvas, vec2(width as f32, height as f32), target)?;
        let mut rgba = image.into_rgba8().into_raw();
        // tiny-skia samples premultiplied pixels and clips directly to the
        // page canvas, avoiding an unbounded oversized intermediate bitmap.
        for pixel in rgba.chunks_exact_mut(4) {
            let alpha = u16::from(pixel[3]);
            for channel in &mut pixel[..3] {
                *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
            }
        }
        let source = resvg::tiny_skia::Pixmap::from_vec(
            rgba,
            resvg::tiny_skia::IntSize::from_wh(width, height).ok_or("画像のサイズが無効です")?,
        )
        .ok_or("画像のピクセル形式が無効です")?;
        pixmap.draw_pixmap(
            0,
            0,
            source.as_ref(),
            &resvg::tiny_skia::PixmapPaint {
                quality: resvg::tiny_skia::FilterQuality::Bicubic,
                ..Default::default()
            },
            transform,
            None,
        );
        return Ok(egui::ColorImage::from_rgba_premultiplied(
            [pixmap.width() as usize, pixmap.height() as usize],
            pixmap.data(),
        ));
    }
    let image = image
        .thumbnail(target.x.ceil() as u32, target.y.ceil() as u32)
        .to_rgba8();
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    ))
}

/// Archive-only URL resolution. No SVG image can request local files or URLs.
pub(crate) fn svg_resource_path(base: &str, href: &str) -> Option<String> {
    let raw = href.split(['#', '?']).next()?;
    let mut decoded = Vec::new();
    let bytes = raw.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let href = String::from_utf8(decoded).ok()?.replace('\\', "/");
    if href.contains(':') || href.starts_with("//") {
        return None;
    }
    let joined = if href.starts_with('/') {
        href.trim_start_matches('/').to_owned()
    } else {
        format!(
            "{}{}",
            base.rsplit_once('/')
                .map(|(directory, _)| format!("{directory}/"))
                .unwrap_or_default(),
            href
        )
    };
    let mut parts = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => (),
            ".." => {
                parts.pop()?;
            }
            part if !part.contains('\0') => parts.push(part),
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_picture(resource: &str, canvas: crate::layout::FixedCanvas) -> Picture {
        Picture {
            rect: egui::Rect::from_min_size(egui::Pos2::ZERO, canvas.extent),
            resource: resource.into(),
            alt: String::new(),
            inline_svg: None,
            pdf_page: None,
            fixed_canvas: Some(canvas),
            text_image: false,
        }
    }

    fn text_tile() -> egui::ColorImage {
        let mut pixels = vec![egui::Color32::WHITE; 49];
        for y in 2..5 {
            for x in 2..5 {
                pixels[y * 7 + x] = egui::Color32::BLACK;
            }
        }
        pixels[3 * 7 + 1] = egui::Color32::from_gray(128);
        egui::ColorImage::new([7, 7], pixels)
    }

    #[test]
    fn opaque_gaiji_paper_becomes_transparent_and_antialiasing_uses_theme_ink() {
        for ink in [
            egui::Color32::from_rgb(230, 234, 241),
            egui::Color32::from_rgb(37, 48, 60),
        ] {
            let mut tile = text_tile();
            assert!(adapt_text_image(&mut tile, ink));
            assert_eq!(tile.pixels[0], egui::Color32::TRANSPARENT);
            assert_eq!(tile.pixels[3 * 7 + 3], ink);
            let edge = tile.pixels[3 * 7 + 1].to_srgba_unmultiplied();
            assert_eq!(edge[3], 127);
            for channel in 0..3 {
                assert!(edge[channel].abs_diff(ink[channel]) <= 2);
            }
        }
    }

    #[test]
    fn transparent_black_and_white_text_masks_keep_their_original_coverage() {
        let ink = egui::Color32::from_rgb(208, 224, 248);
        for source in [0, 255] {
            let mut tile = egui::ColorImage::new([3, 3], vec![egui::Color32::TRANSPARENT; 9]);
            tile.pixels[4] = egui::Color32::from_rgba_unmultiplied(source, source, source, 96);
            assert!(adapt_text_image(&mut tile, ink));
            assert_eq!(tile.pixels[0], egui::Color32::TRANSPARENT);
            assert_eq!(tile.pixels[4].a(), 96);
            let pixel = tile.pixels[4].to_srgba_unmultiplied();
            for channel in 0..3 {
                assert!(pixel[channel].abs_diff(ink[channel]) <= 3);
            }
        }
    }

    #[test]
    fn colored_or_continuous_tone_images_remain_unchanged() {
        let mut colored = text_tile();
        colored.pixels[24] = egui::Color32::RED;
        let original = colored.clone();
        assert!(!adapt_text_image(&mut colored, egui::Color32::WHITE));
        assert_eq!(colored.pixels, original.pixels);
        let mut photo =
            egui::ColorImage::new([16, 16], (0..=255).map(egui::Color32::from_gray).collect());
        let original = photo.clone();
        assert!(!adapt_text_image(&mut photo, egui::Color32::WHITE));
        assert_eq!(photo.pixels, original.pixels);
    }

    #[test]
    fn only_semantically_inline_text_images_are_automatic_candidates() {
        let chapter = reader_core::parse_html(
            "<html><body><p>外<img src='glyph.png' alt='字' style='width:1em;height:1em'/>字</p><img src='photo.png' alt='風景'/></body></html>",
            0, "book.xhtml", "").unwrap();
        let layout = crate::layout::chapter_layout(
            &mut fonts::FontSystem::new(),
            &chapter,
            vec2(800., 700.),
            &reader_library::Settings::default(),
        );
        let pictures: Vec<_> = layout
            .pages
            .iter()
            .flat_map(|page| &page.pictures)
            .collect();
        assert!(text_image_candidate(
            pictures
                .iter()
                .find(|picture| picture.resource == "glyph.png")
                .unwrap()
        ));
        assert!(!text_image_candidate(
            pictures
                .iter()
                .find(|picture| picture.resource == "photo.png")
                .unwrap()
        ));
        let mut fixed = (**pictures
            .iter()
            .find(|picture| picture.resource == "glyph.png")
            .unwrap())
        .clone();
        fixed.fixed_canvas = Some(crate::layout::FixedCanvas {
            extent: vec2(800., 700.),
            image_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(24., 24.)),
        });
        assert!(!text_image_candidate(&fixed));
    }

    #[test]
    fn themed_decode_uploads_alpha_masks_and_separates_color_cache_entries() {
        let context = egui::Context::default();
        let mut book = Book::demo();
        let tile = text_tile();
        let pixels: Vec<u8> = tile
            .pixels
            .iter()
            .flat_map(|color| color.to_srgba_unmultiplied())
            .collect();
        let mut encoded = Cursor::new(Vec::new());
        image::RgbaImage::from_raw(7, 7, pixels)
            .unwrap()
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        book.resources
            .insert("glyph.png".into(), encoded.into_inner());
        let picture = Picture {
            rect: egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(7., 7.)),
            resource: "glyph.png".into(),
            alt: "字".into(),
            inline_svg: None,
            pdf_page: None,
            fixed_canvas: None,
            text_image: true,
        };
        let mut cache = HashMap::new();
        for ink in [egui::Color32::WHITE, egui::Color32::from_rgb(30, 70, 130)] {
            let started = Instant::now();
            let texture = loop {
                if let Some(texture) = load_texture_themed(
                    &context,
                    &book,
                    "book.xhtml",
                    &picture,
                    &mut cache,
                    Some(ink),
                )
                .unwrap()
                {
                    break texture;
                }
                assert!(started.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(5));
            };
            let mut delta = context.tex_manager().write().take_delta();
            let uploaded = delta.set.get(&texture.id()).unwrap().last().unwrap();
            let egui::ImageData::Color(image) = &uploaded.image;
            assert_eq!(image.pixels[0], egui::Color32::TRANSPARENT);
            assert_eq!(image[(image.size[0] / 2, image.size[1] / 2)], ink);
            delta.clear();
        }
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn fixed_svg_canvas_keeps_embedded_archive_images_and_author_box() {
        let mut book = Book::demo();
        let mut png = Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        book.resources
            .insert("OPS/Images/red.png".into(), png.into_inner());
        book.resources.insert("OPS/Pages/page.svg".into(), br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><image href="../Images/red.png" width="50" height="100"/><rect x="50" width="50" height="100" fill="blue"/></svg>"#.to_vec());
        let picture = fixed_picture(
            "OPS/Pages/page.svg",
            crate::layout::FixedCanvas {
                extent: vec2(200., 100.),
                image_rect: egui::Rect::from_min_size(egui::pos2(25., 10.), vec2(100., 80.)),
            },
        );
        let view = book.image_resource_view(&picture.resource, None, "OPS/Pages/page.xhtml");
        let image =
            decode_picture(&view, "OPS/Pages/page.xhtml", &picture, vec2(200., 100.)).unwrap();
        assert_eq!(image.size, [200, 100]);
        assert_eq!(image[(30, 50)], egui::Color32::RED);
        assert_eq!(image[(100, 50)], egui::Color32::BLUE);
        assert_eq!(
            image[(5, 50)].a(),
            0,
            "author whitespace must remain part of the page canvas"
        );
    }

    #[test]
    fn fixed_raster_is_clipped_in_author_coordinates_instead_of_shrunk_to_fit() {
        let mut source = image::RgbaImage::from_pixel(2, 1, image::Rgba([255, 0, 0, 255]));
        source.put_pixel(1, 0, image::Rgba([0, 0, 255, 255]));
        let mut png = Cursor::new(Vec::new());
        source.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let mut book = Book::demo();
        book.resources.insert("wide.png".into(), png.into_inner());
        let picture = fixed_picture(
            "wide.png",
            crate::layout::FixedCanvas {
                extent: vec2(100., 100.),
                image_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(200., 100.)),
            },
        );
        let image = decode_picture(&book, "page.xhtml", &picture, vec2(100., 100.)).unwrap();
        assert_eq!(image.size, [100, 100]);
        assert!(image[(25, 50)].r() > 240 && image[(25, 50)].b() < 10);
        assert!(
            image[(75, 50)].r() > image[(75, 50)].b(),
            "the right half of the oversized image is outside the viewport"
        );
    }

    #[test]
    fn fixed_svg_preserves_nonzero_viewbox_origin() {
        let mut picture = fixed_picture(
            "inline-svg:0:0",
            crate::layout::FixedCanvas {
                extent: vec2(600., 900.),
                image_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(600., 900.)),
            },
        );
        picture.inline_svg = Some("<svg xmlns='http://www.w3.org/2000/svg' viewBox='100 200 600 900'><rect x='100' y='200' width='600' height='900' fill='#00ff00'/></svg>".into());
        let image = decode_picture(&Book::demo(), "page.svg", &picture, vec2(300., 450.)).unwrap();
        assert_eq!(image.size, [300, 450]);
        assert_eq!(image[(5, 5)], egui::Color32::GREEN);
        assert_eq!(image[(295, 445)], egui::Color32::GREEN);
    }

    #[test]
    fn fixed_canvas_and_inline_svg_content_have_separate_texture_variants() {
        let context = egui::Context::default();
        let mut book = Book::demo();
        book.resources.insert("shared.svg".into(), b"<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'><rect width='100' height='100' fill='red'/></svg>".to_vec());
        let first = fixed_picture(
            "shared.svg",
            crate::layout::FixedCanvas {
                extent: vec2(100., 100.),
                image_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(100., 100.)),
            },
        );
        let mut second = first.clone();
        second.fixed_canvas.as_mut().unwrap().extent = vec2(200., 100.);
        let mut third = first.clone();
        third.inline_svg = Some("<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'><rect width='100' height='100' fill='blue'/></svg>".into());
        let mut cache = HashMap::new();
        let mut textures = Vec::new();
        for picture in [first, second, third] {
            let started = Instant::now();
            loop {
                if let Some(texture) =
                    load_texture(&context, &book, "page.xhtml", &picture, &mut cache).unwrap()
                {
                    textures.push(texture);
                    break;
                }
                assert!(started.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        assert_eq!(
            cache.len(),
            3,
            "shared resources with different page geometry/content must not reuse a stale bitmap"
        );
        assert_ne!(textures[0].id(), textures[1].id());
        assert_ne!(textures[0].id(), textures[2].id());
        assert_eq!(textures[0].size(), [128, 128]);
        assert_eq!(textures[1].size(), [128, 64]);
        let mut delta = context.tex_manager().write().take_delta();
        delta.clear();
    }

    #[test]
    fn cache_enforces_both_rgba_budget_and_failed_entry_count() {
        let context = egui::Context::default();
        let mut cache = HashMap::new();
        let budget = CacheBudget {
            entries: 3,
            bytes: 2 * 512 * 512 * 4,
        };
        for index in 0..8 {
            let texture = context.load_texture(
                format!("budget-{index}"),
                egui::ColorImage::new([512, 512], vec![egui::Color32::WHITE; 512 * 512]),
                egui::TextureOptions::LINEAR,
            );
            cache.insert(index, Some(texture));
            enforce_cache_budget(&mut cache, budget, Some(&index));
            assert!(cache.len() <= 3);
            assert!(cache.values().flatten().map(texture_bytes).sum::<usize>() <= budget.bytes);
            assert!(cache.contains_key(&index));
        }
        for index in 10..30 {
            cache.insert(index, None);
            enforce_cache_budget(&mut cache, budget, Some(&index));
            assert!(cache.len() <= 3);
        }
    }

    #[test]
    fn failed_picture_is_cached_without_retrying_or_repeating_errors() {
        let context = egui::Context::default();
        let book = Book::demo();
        let mut cache = HashMap::new();
        let picture = Picture {
            rect: egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(100.0, 100.0)),
            resource: "missing.png".into(),
            alt: "missing".into(),
            inline_svg: None,
            pdf_page: None,
            text_image: false,
            fixed_canvas: None,
        };
        assert!(
            load_texture(&context, &book, "book.xhtml", &picture, &mut cache)
                .unwrap()
                .is_none()
        );
        assert!(
            cache.is_empty(),
            "pending images must not be cached as failures"
        );
        let started = Instant::now();
        loop {
            if load_texture(&context, &book, "book.xhtml", &picture, &mut cache).is_err() {
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            load_texture(&context, &book, "book.xhtml", &picture, &mut cache)
                .unwrap()
                .is_none()
        );
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn a_page_with_many_small_gaiji_finishes_without_evicting_its_visible_images() {
        let context = egui::Context::default();
        let mut book = Book::demo();
        let mut cache = HashMap::new();
        let mut encoded = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2,
            2,
            image::Rgba([20, 30, 40, 255]),
        ))
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
        let pictures: Vec<_> = (0..96)
            .map(|index| {
                let resource = format!("gaiji-{index}.png");
                book.resources
                    .insert(resource.clone(), encoded.get_ref().clone());
                Picture {
                    rect: egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(24.0, 24.0)),
                    resource,
                    alt: "外字".into(),
                    inline_svg: None,
                    pdf_page: None,
                    text_image: false,
                    fixed_canvas: None,
                }
            })
            .collect();
        let started = Instant::now();
        loop {
            let mut ready = 0;
            for picture in &pictures {
                if load_texture(&context, &book, "book.xhtml", picture, &mut cache)
                    .unwrap()
                    .is_some()
                {
                    ready += 1;
                }
            }
            if ready == pictures.len() {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "bounded decoder must make progress on all visible images"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(cache.len(), 96);
        assert!(cache.values().all(Option::is_some));
        let mut delta = context.tex_manager().write().take_delta();
        delta.clear();
    }

    #[test]
    fn svg_cannot_load_an_existing_local_file_outside_the_book() {
        let directory = tempfile::tempdir().unwrap();
        let local_image = directory.path().join("private.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]))
            .save(&local_image)
            .unwrap();
        let picture = Picture {
            rect: egui::Rect::NOTHING,
            resource: "inline-svg:0:0".into(),
            alt: String::new(),
            inline_svg: Some(format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><image href="{}" width="32" height="32"/></svg>"#,
                local_image.display()
            )),
            pdf_page: None,
            text_image: false,
            fixed_canvas: None,
        };
        let image =
            decode_picture(&Book::demo(), "OPS/book.xhtml", &picture, vec2(64.0, 64.0)).unwrap();
        assert!(
            image.pixels.iter().all(|pixel| pixel.a() == 0),
            "only archive resources may supply SVG image pixels"
        );
    }

    #[test]
    fn raster_dimensions_are_limited_before_thumbnailing() {
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(32_769, 1))
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let mut book = Book::demo();
        book.resources.insert("wide.png".into(), bytes.into_inner());
        let picture = Picture {
            rect: egui::Rect::NOTHING,
            resource: "wide.png".into(),
            alt: String::new(),
            inline_svg: None,
            pdf_page: None,
            text_image: false,
            fixed_canvas: None,
        };
        assert!(decode_picture(&book, "book.xhtml", &picture, vec2(64.0, 64.0)).is_err());
    }
}
