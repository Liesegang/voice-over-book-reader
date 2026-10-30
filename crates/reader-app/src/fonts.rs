use eframe::egui::{self, Color32, ColorImage, Pos2, Rect, TextureHandle, TextureId, Vec2};
use std::{collections::HashMap, sync::Arc};
use swash::scale::{Render, ScaleContext, Source};

pub const SERIF: &[u8] = include_bytes!("../../../assets/fonts/NotoSerifCJKjp-Regular.otf");
pub const SANS: &[u8] = include_bytes!("../../../assets/fonts/NotoSansCJKjp-Regular.otf");
pub const MONO: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");
pub const MATH: &[u8] = include_bytes!("../../../assets/fonts/STIXTwoMath-Regular.otf");
pub const SERIF_FACE: usize = 0;
pub const SANS_FACE: usize = 1;
pub const CODE_FACE: usize = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GlyphStyle {
    pub bold: bool,
    pub italic: bool,
}

pub fn install_ui(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert("noto-jp".into(), egui::FontData::from_static(SANS).into());
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "noto-jp".into());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push("noto-jp".into());
    ctx.set_fonts(fonts);
}

struct Face {
    data: harfrust::font::Blob,
    font: harfrust::Font,
    key: swash::CacheKey,
    index: usize,
    weight: u16,
    fallback: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct ShapedGlyph {
    pub id: u16,
    pub cluster: usize,
    pub offset: Vec2,
    pub advance: Vec2,
}

#[derive(Clone, Copy)]
struct CachedGlyph {
    texture: TextureId,
    uv: Rect,
    size: Vec2,
    bearing: Vec2,
    color: bool,
}

impl CachedGlyph {
    /// Swash already rasterizes coverage at the physical display resolution.
    /// Keep each atlas texel on exactly one screen pixel so linear filtering
    /// does not soften that coverage a second time. Rotate integer pixel
    /// offsets around the snapped baseline before converting back to points.
    fn quad(self, origin: Pos2, dpi: f32, rotated: bool) -> [Pos2; 4] {
        let baseline = Vec2::new((origin.x * dpi).round(), (origin.y * dpi).round());
        let bearing = Vec2::new(
            (self.bearing.x * dpi).round(),
            (self.bearing.y * dpi).round(),
        );
        let size = Vec2::new((self.size.x * dpi).round(), (self.size.y * dpi).round());
        [
            Vec2::ZERO,
            Vec2::new(size.x, 0.),
            size,
            Vec2::new(0., size.y),
        ]
        .map(|corner| {
            let offset = bearing + corner;
            let offset = if rotated {
                Vec2::new(-offset.y, offset.x)
            } else {
                offset
            };
            let pixel = baseline + offset;
            Pos2::new(pixel.x / dpi, pixel.y / dpi)
        })
    }
}

struct Atlas {
    texture: TextureHandle,
    x: usize,
    y: usize,
    row: usize,
}

/// Expanding the coverage mask preserves every existing stroke. CFF outline
/// emboldening can reverse/collapse hinted contours in glyphs such as Noto
/// Serif CJK's 設 and 項 at common reading sizes.
fn embolden_mask(image: &mut swash::scale::image::Image, strength: f32) {
    if strength <= 0.0 || image.data.is_empty() {
        return;
    }
    let padding = strength.ceil() as usize;
    let width = image.placement.width as usize;
    let height = image.placement.height as usize;
    let expanded_width = width + padding * 2;
    let expanded_height = height + padding * 2;
    let kernel: Vec<_> = (-(padding as isize)..=padding as isize)
        .map(|offset| {
            (
                offset,
                (strength + 1.0 - offset.unsigned_abs() as f32).clamp(0.0, 1.0),
            )
        })
        .collect();
    let original = std::mem::take(&mut image.data);
    let mut horizontal = vec![0_u8; expanded_width * height];
    for y in 0..height {
        for x in 0..expanded_width {
            let origin = x as isize - padding as isize;
            horizontal[y * expanded_width + x] = kernel
                .iter()
                .filter(|&&(offset, _)| {
                    let source = origin + offset;
                    source >= 0 && source < width as isize
                })
                .map(|&(offset, weight)| {
                    (f32::from(original[y * width + (origin + offset) as usize]) * weight).round()
                        as u8
                })
                .max()
                .unwrap_or(0);
        }
    }
    image.data = vec![0_u8; expanded_width * expanded_height];
    for y in 0..expanded_height {
        let origin = y as isize - padding as isize;
        for x in 0..expanded_width {
            image.data[y * expanded_width + x] = kernel
                .iter()
                .filter(|&&(offset, _)| {
                    let source = origin + offset;
                    source >= 0 && source < height as isize
                })
                .map(|&(offset, weight)| {
                    (f32::from(horizontal[(origin + offset) as usize * expanded_width + x])
                        * weight)
                        .round() as u8
                })
                .max()
                .unwrap_or(0);
        }
    }
    image.placement.left -= padding as i32;
    image.placement.top += padding as i32;
    image.placement.width = expanded_width as u32;
    image.placement.height = expanded_height as u32;
}

type ShapedRuns = HashMap<(usize, bool), HashMap<String, Arc<[ShapedGlyph]>>>;

pub struct FontSystem {
    faces: Vec<Face>,
    scale: ScaleContext,
    glyphs: HashMap<(usize, u16, u32, GlyphStyle), Option<CachedGlyph>>,
    shapes: ShapedRuns,
    shape_count: usize,
    outline_bounds: HashMap<(usize, u16), Option<Rect>>,
    atlases: Vec<Atlas>,
    pixels_per_point: f32,
    system_fonts: Option<fontdb::Database>,
    families: Vec<String>,
    family_faces: HashMap<String, usize>,
    system_faces: HashMap<fontdb::ID, usize>,
    coverage: HashMap<(usize, char), bool>,
    active_family: String,
    fallback_faces: HashMap<(usize, String), usize>,
    draw_glyphs: Vec<(usize, CachedGlyph)>,
}

impl FontSystem {
    pub fn new() -> Self {
        let faces = [SERIF, SANS, MONO]
            .into_iter()
            .enumerate()
            .map(|(index, data)| Face {
                data: harfrust::font::Blob::from(data),
                font: harfrust::Font::new(data, 0).expect("bundled font"),
                key: swash::CacheKey::new(),
                index: 0,
                weight: 400,
                fallback: if index == SERIF_FACE {
                    SERIF_FACE
                } else {
                    SANS_FACE
                },
            })
            .collect();
        Self {
            faces,
            scale: ScaleContext::new(),
            glyphs: HashMap::new(),
            shapes: HashMap::new(),
            shape_count: 0,
            outline_bounds: HashMap::new(),
            atlases: Vec::new(),
            pixels_per_point: 1.0,
            system_fonts: None,
            families: vec![
                "Noto Serif CJK JP".into(),
                "Noto Sans CJK JP".into(),
                "JetBrains Mono".into(),
            ],
            family_faces: HashMap::new(),
            system_faces: HashMap::new(),
            coverage: HashMap::new(),
            active_family: String::new(),
            fallback_faces: HashMap::new(),
            draw_glyphs: Vec::new(),
        }
    }

    /// Enumerated once on first use; subsequent UI frames only borrow a slice.
    pub fn families(&mut self) -> &[String] {
        if self.system_fonts.is_none() {
            let mut database = fontdb::Database::new();
            database.load_system_fonts();
            for face in database.faces() {
                for (name, _) in &face.families {
                    self.families.push(name.clone());
                }
            }
            self.families.sort_by_cached_key(|name| name.to_lowercase());
            self.families.dedup();
            self.system_fonts = Some(database);
        }
        &self.families
    }

    /// A typography change invalidates old dynamic face indices only between
    /// layouts. Header/table fallback choices never discard a face mid-layout.
    pub fn begin_layout(&mut self, family: &str) {
        if self.active_family != family {
            self.active_family = family.to_owned();
            self.faces.truncate(3);
            self.family_faces.clear();
            self.system_faces.clear();
            self.coverage.clear();
            self.fallback_faces.clear();
            self.glyphs.clear();
            self.shapes.clear();
            self.shape_count = 0;
            self.outline_bounds.clear();
            self.atlases.clear();
        }
    }

    fn load_system_face(&mut self, id: fontdb::ID) -> Option<usize> {
        if let Some(&face) = self.system_faces.get(&id) {
            return Some(face);
        }
        let database = self.system_fonts.as_ref()?;
        let info = database.face(id)?;
        let weight = info.weight.0;
        let sans = info.monospaced
            || info.families.iter().any(|(family, _)| {
                let family = family.to_ascii_lowercase();
                ["sans", "gothic", "arial", "segoe"]
                    .iter()
                    .any(|part| family.contains(part))
                    || family.contains("ゴシック")
            });
        let (bytes, index) = database.with_face_data(id, |bytes, index| {
            let shared: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(bytes.to_vec());
            (harfrust::font::Blob::from(shared), index)
        })?;
        let font = harfrust::Font::new(bytes.clone(), index)?;
        let face = self.faces.len();
        self.faces.push(Face {
            data: bytes,
            font,
            key: swash::CacheKey::new(),
            index: index as usize,
            weight,
            fallback: if sans { SANS_FACE } else { SERIF_FACE },
        });
        self.system_faces.insert(id, face);
        Some(face)
    }

    pub fn body_face(&mut self, family: &str) -> usize {
        match family {
            "Noto Serif CJK JP" | "serif" => return SERIF_FACE,
            "Noto Sans CJK JP" | "sans-serif" => return SANS_FACE,
            "JetBrains Mono" | "monospace" => return CODE_FACE,
            _ => (),
        }
        if let Some(&face) = self.family_faces.get(family) {
            return face;
        }
        self.families();
        let actual = self
            .families
            .iter()
            .find(|candidate| candidate.eq_ignore_ascii_case(family))
            .map(String::as_str)
            .unwrap_or(family);
        let id = self.system_fonts.as_ref().and_then(|database| {
            database.query(&fontdb::Query {
                families: &[fontdb::Family::Name(actual)],
                ..fontdb::Query::default()
            })
        });
        let face = id
            .and_then(|id| self.load_system_face(id))
            .unwrap_or(SERIF_FACE);
        self.family_faces.insert(family.to_owned(), face);
        face
    }

    fn covers(&mut self, face: usize, text: &str) -> bool {
        text.chars()
            .filter(|character| {
                !character.is_control()
                    && !matches!(
                        *character,
                        '\u{200c}' | '\u{200d}' | '\u{fe0e}' | '\u{fe0f}'
                    )
            })
            .all(|character| {
                *self.coverage.entry((face, character)).or_insert_with(|| {
                    let data = &self.faces[face];
                    swash::FontRef::from_index(data.data.as_ref(), data.index)
                        .is_some_and(|font| font.charmap().map(character) != 0)
                })
            })
    }

    pub fn face_for_text(&mut self, preferred: usize, text: &str) -> usize {
        if self.covers(preferred, text) {
            return preferred;
        }
        let key = (preferred, text.to_owned());
        if let Some(&face) = self.fallback_faces.get(&key) {
            return face;
        }
        if self.fallback_faces.len() >= 10_000 {
            self.fallback_faces.clear();
        }
        let fallback = self.faces[preferred].fallback;
        for face in [fallback, SERIF_FACE, SANS_FACE] {
            if self.covers(face, text) {
                self.fallback_faces.insert(key, face);
                return face;
            }
        }
        self.families();
        let database = self.system_fonts.as_ref().unwrap();
        let mut candidates: Vec<_> = [
            "Segoe UI Emoji",
            "Apple Color Emoji",
            "Noto Color Emoji",
            "Segoe UI Symbol",
        ]
        .iter()
        .filter_map(|family| {
            database.query(&fontdb::Query {
                families: &[fontdb::Family::Name(family)],
                ..fontdb::Query::default()
            })
        })
        .collect();
        candidates.extend(database.faces().map(|face| face.id));
        for id in candidates {
            let supported = self
                .system_fonts
                .as_ref()
                .unwrap()
                .with_face_data(id, |bytes, index| {
                    swash::FontRef::from_index(bytes, index as usize).is_some_and(|font| {
                        text.chars()
                            .filter(|character| {
                                !character.is_control()
                                    && !matches!(
                                        *character,
                                        '\u{200c}' | '\u{200d}' | '\u{fe0e}' | '\u{fe0f}'
                                    )
                            })
                            .all(|character| font.charmap().map(character) != 0)
                    })
                })
                .unwrap_or(false);
            if supported && let Some(face) = self.load_system_face(id) {
                self.fallback_faces.insert(key, face);
                return face;
            }
        }
        if self.fallback_faces.len() >= 10_000 {
            self.fallback_faces.clear();
        }
        self.fallback_faces.insert(key, fallback);
        fallback
    }

    /// Cached outline bounds in em units, without creating or uploading a raster.
    pub fn outline_bounds(&mut self, face: usize, id: u16) -> Option<Rect> {
        let key = (face, id);
        if let Some(bounds) = self.outline_bounds.get(&key) {
            return *bounds;
        }
        let face = self.faces.get(face)?;
        let font = harfrust::ShaperFont::new(&face.font);
        let scale = 1.0 / face.font.units_per_em() as f32;
        let bounds = font
            .glyph_extents(harfrust::GlyphId::new(u32::from(id)))
            .map(|bounds| {
                Rect::from_two_pos(
                    Pos2::new(bounds.x_bearing as f32, -bounds.y_bearing as f32) * scale,
                    Pos2::new(
                        (bounds.x_bearing + bounds.width) as f32,
                        -(bounds.y_bearing + bounds.height) as f32,
                    ) * scale,
                )
            });
        self.outline_bounds.insert(key, bounds);
        bounds
    }

    /// Shaping results are immutable and in em units. A warm lookup borrows the
    /// text key and shares its run, without allocating or copying glyph arrays.
    pub fn shape(&mut self, face: usize, text: &str, vertical: bool) -> Arc<[ShapedGlyph]> {
        let key = (face, vertical);
        if let Some(glyphs) = self.shapes.get(&key).and_then(|runs| runs.get(text)) {
            return Arc::clone(glyphs);
        }
        let font = harfrust::ShaperFont::new(&self.faces[face].font);
        let mut buffer = harfrust::Buffer::new();
        buffer.push_str(text);
        buffer.set_direction(if vertical {
            harfrust::Direction::TopToBottom
        } else {
            harfrust::Direction::LeftToRight
        });
        buffer.set_language("ja".parse().ok());
        buffer.guess_segment_properties();
        let mut output = Vec::new();
        if harfrust::shape(&font, &mut buffer, harfrust::ShapeOptions::default()).is_ok() {
            let scale = 1.0 / self.faces[face].font.units_per_em() as f32;
            for (info, pos) in buffer.glyph_infos().iter().zip(buffer.glyph_positions()) {
                output.push(ShapedGlyph {
                    id: info.glyph_id as u16,
                    cluster: info.cluster as usize,
                    offset: Vec2::new(pos.x_offset as f32 * scale, -pos.y_offset as f32 * scale),
                    advance: Vec2::new(pos.x_advance as f32 * scale, -pos.y_advance as f32 * scale),
                });
            }
        }
        if self.shape_count > 30_000 {
            self.shapes.clear();
            self.shape_count = 0;
        }
        let output: Arc<[ShapedGlyph]> = output.into();
        self.shapes
            .entry(key)
            .or_default()
            .insert(text.to_owned(), Arc::clone(&output));
        self.shape_count += 1;
        output
    }

    fn raster(
        &mut self,
        ctx: &egui::Context,
        face: usize,
        id: u16,
        size: f32,
        style: GlyphStyle,
    ) -> Option<CachedGlyph> {
        let dpi = ctx.pixels_per_point();
        self.raster_at_scale(ctx, face, id, size, style, dpi)
    }

    fn raster_at_scale(
        &mut self,
        ctx: &egui::Context,
        face: usize,
        id: u16,
        size: f32,
        style: GlyphStyle,
        dpi: f32,
    ) -> Option<CachedGlyph> {
        // Cached dimensions/bearings are in points at this exact scale. Even a
        // small UI zoom must not reuse metrics converted at the previous DPI.
        if dpi.to_bits() != self.pixels_per_point.to_bits() {
            self.glyphs.clear();
            self.atlases.clear();
            self.pixels_per_point = dpi;
        }
        let pixels = (size * dpi * 8.0).round() as u32;
        let key = (face, id, pixels, style);
        if let Some(value) = self.glyphs.get(&key) {
            return *value;
        }
        let data = self.faces[face].data.as_ref();
        let mut font = swash::FontRef::from_index(data, self.faces[face].index)?;
        font.key = self.faces[face].key;
        let mut scaler = self
            .scale
            .builder(font)
            .size(pixels as f32 / 8.0)
            .hint(true)
            .build();
        let mut render = Render::new(&[
            Source::ColorOutline(0),
            Source::ColorBitmap(swash::scale::StrikeWith::BestFit),
            Source::Outline,
            Source::Bitmap(swash::scale::StrikeWith::BestFit),
        ]);
        render.format(swash::zeno::Format::Alpha);
        if style.italic {
            // Shear the outline before rasterization so its measured bitmap
            // includes the overhang; applying a texture transform would clip it.
            render.transform(Some(swash::zeno::Transform::new(
                1.0, 0.0, 0.2, 1.0, 0.0, 0.0,
            )));
        }
        let raster = render.render(&mut scaler, id);
        let Some(mut raster) = raster else {
            self.glyphs.insert(key, None);
            return None;
        };
        if style.bold
            && self.faces[face].weight < 600
            && raster.content == swash::scale::image::Content::Mask
        {
            embolden_mask(&mut raster, size * dpi * 0.025);
        }
        let w = raster.placement.width as usize;
        let h = raster.placement.height as usize;
        if w == 0 || h == 0 {
            self.glyphs.insert(key, None);
            return None;
        }
        const EXTENT: usize = 2048;
        if self
            .atlases
            .last()
            .is_none_or(|a| a.y + h + a.row + 2 >= EXTENT)
        {
            let texture = ctx.load_texture(
                format!("book-glyphs-{}", self.atlases.len()),
                ColorImage::new(
                    [EXTENT, EXTENT],
                    vec![Color32::TRANSPARENT; EXTENT * EXTENT],
                ),
                egui::TextureOptions::LINEAR,
            );
            self.atlases.push(Atlas {
                texture,
                x: 1,
                y: 1,
                row: 0,
            });
        }
        let atlas = self.atlases.last_mut()?;
        if atlas.x + w + 1 >= EXTENT {
            atlas.x = 1;
            atlas.y += atlas.row + 1;
            atlas.row = 0;
        }
        let color = raster.content == swash::scale::image::Content::Color;
        let image = if color {
            ColorImage::from_rgba_unmultiplied([w, h], &raster.data)
        } else {
            ColorImage::new(
                [w, h],
                raster
                    .data
                    .iter()
                    .take(w * h)
                    .map(|a| Color32::from_white_alpha(*a))
                    .collect(),
            )
        };
        atlas
            .texture
            .set_partial([atlas.x, atlas.y], image, egui::TextureOptions::LINEAR);
        let result = CachedGlyph {
            texture: atlas.texture.id(),
            uv: Rect::from_min_max(
                Pos2::new(
                    atlas.x as f32 / EXTENT as f32,
                    atlas.y as f32 / EXTENT as f32,
                ),
                Pos2::new(
                    (atlas.x + w) as f32 / EXTENT as f32,
                    (atlas.y + h) as f32 / EXTENT as f32,
                ),
            ),
            size: Vec2::new(w as f32 / dpi, h as f32 / dpi),
            bearing: Vec2::new(
                raster.placement.left as f32 / dpi,
                -raster.placement.top as f32 / dpi,
            ),
            color,
        };
        atlas.x += w + 1;
        atlas.row = atlas.row.max(h);
        self.glyphs.insert(key, Some(result));
        Some(result)
    }

    /// Keep source paint order while combining adjacent glyphs on the same
    /// atlas. In particular, ruby, synthetic styles and rotated glyphs must not
    /// be globally sorted by texture. The reusable scratch list also avoids
    /// allocating per-glyph meshes on each idle/read-aloud repaint.
    pub fn draw_batch(
        &mut self,
        painter: &egui::Painter,
        glyphs: &[crate::layout::Glyph],
        offset: Vec2,
        color: Color32,
    ) {
        let dpi = painter.ctx().pixels_per_point();
        let mut cached = std::mem::take(&mut self.draw_glyphs);
        cached.clear();
        for (index, glyph) in glyphs.iter().enumerate() {
            if let Some(raster) = self.raster_at_scale(
                painter.ctx(),
                glyph.face,
                glyph.id,
                glyph.size,
                glyph.style,
                dpi,
            ) {
                cached.push((index, raster));
            }
        }
        let mut start = 0;
        while start < cached.len() {
            let texture = cached[start].1.texture;
            let mut end = start + 1;
            // Fits 16-bit backends as well as egui's u32 indices. Limit each
            // run before multiplying vertex/index counts or casting indices.
            while end < cached.len() && end - start < 16_384 && cached[end].1.texture == texture {
                end += 1;
            }
            let mut mesh = egui::Mesh::with_texture(texture);
            mesh.vertices.reserve_exact((end - start) * 4);
            mesh.indices.reserve_exact((end - start) * 6);
            for &(index, raster) in &cached[start..end] {
                let glyph = &glyphs[index];
                let quad = raster.quad(glyph.origin + offset, dpi, glyph.rotated);
                let tint = if raster.color { Color32::WHITE } else { color };
                if !glyph.rotated {
                    // Same vertex and triangle order as Painter::image.
                    mesh.add_rect_with_uv(Rect::from_min_max(quad[0], quad[2]), raster.uv, tint);
                } else {
                    let base = mesh.vertices.len() as u32;
                    for (pos, uv) in quad.into_iter().zip([
                        raster.uv.left_top(),
                        raster.uv.right_top(),
                        raster.uv.right_bottom(),
                        raster.uv.left_bottom(),
                    ]) {
                        mesh.vertices.push(egui::epaint::Vertex {
                            pos,
                            uv,
                            color: tint,
                        });
                    }
                    mesh.indices.extend_from_slice(&[
                        base,
                        base + 1,
                        base + 2,
                        base,
                        base + 2,
                        base + 3,
                    ]);
                }
            }
            painter.add(mesh);
            start = end;
        }
        self.draw_glyphs = cached;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_styled(
        &mut self,
        painter: &egui::Painter,
        face: usize,
        id: u16,
        origin: Pos2,
        size: f32,
        color: Color32,
        rotated: bool,
        style: GlyphStyle,
    ) {
        let Some(glyph) = self.raster(painter.ctx(), face, id, size, style) else {
            return;
        };
        let quad = glyph.quad(origin, painter.ctx().pixels_per_point(), rotated);
        let color = if glyph.color { Color32::WHITE } else { color };
        if !rotated {
            painter.image(
                glyph.texture,
                Rect::from_min_max(quad[0], quad[2]),
                glyph.uv,
                color,
            );
            return;
        }
        let mut mesh = egui::Mesh::with_texture(glyph.texture);
        for (pos, uv) in quad.into_iter().zip([
            glyph.uv.left_top(),
            glyph.uv.right_top(),
            glyph.uv.right_bottom(),
            glyph.uv.left_bottom(),
        ]) {
            mesh.vertices.push(egui::epaint::Vertex { pos, uv, color });
        }
        mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
        painter.add(mesh);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    type PaintVertexSnapshot = (TextureId, [u32; 2], [u32; 2], [u8; 4]);

    #[test]
    fn shaped_runs_share_warm_results_and_remain_valid_across_font_changes() {
        let mut fonts = FontSystem::new();
        fonts.begin_layout("Noto Serif CJK JP");
        let first = fonts.shape(SERIF_FACE, "本。", true);
        let owned = String::from("本。");
        let repeated = fonts.shape(SERIF_FACE, &owned, true);
        assert!(Arc::ptr_eq(&first, &repeated));
        let horizontal = fonts.shape(SERIF_FACE, &owned, false);
        assert_ne!(first[1].id, horizontal[1].id);
        fonts.begin_layout("Noto Serif CJK JP");
        assert!(Arc::ptr_eq(&first, &fonts.shape(SERIF_FACE, &owned, true)));
        fonts.begin_layout("Noto Sans CJK JP");
        let refreshed = fonts.shape(SERIF_FACE, &owned, true);
        assert!(!Arc::ptr_eq(&first, &refreshed));
        for (old, new) in first.iter().zip(refreshed.iter()) {
            assert_eq!(old.id, new.id);
            assert_eq!(old.cluster, new.cluster);
            assert_eq!(old.offset, new.offset);
            assert_eq!(old.advance, new.advance);
        }
    }

    #[test]
    fn batched_glyph_triangles_match_individual_paints_across_atlases_and_dpi() {
        fn input(dpi: f32) -> egui::RawInput {
            let mut input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800., 600.))),
                ..Default::default()
            };
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .native_pixels_per_point = Some(dpi);
            input
        }
        fn triangles(output: &egui::FullOutput) -> Vec<PaintVertexSnapshot> {
            output
                .shapes
                .iter()
                .flat_map(|shape| {
                    let egui::Shape::Mesh(mesh) = &shape.shape else {
                        return Vec::new();
                    };
                    mesh.indices
                        .iter()
                        .map(|index| {
                            let vertex = mesh.vertices[*index as usize];
                            (
                                mesh.texture_id,
                                [vertex.pos.x.to_bits(), vertex.pos.y.to_bits()],
                                [vertex.uv.x.to_bits(), vertex.uv.y.to_bits()],
                                vertex.color.to_array(),
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .collect()
        }
        let tint = Color32::from_rgba_unmultiplied(91, 134, 213, 187);
        let offset = Vec2::new(10.17, 18.29);
        for dpi in [1.0, 1.25, 2.0] {
            let ctx = egui::Context::default();
            let mut fonts = FontSystem::new();
            let first = fonts.shape(SERIF_FACE, "日", false)[0].id;
            let second = fonts.shape(SERIF_FACE, "月", false)[0].id;
            let space = fonts.shape(SERIF_FACE, " ", false)[0].id;
            let glyphs: Vec<_> = [
                first, first, second, second, first, second, space, second, second,
            ]
            .into_iter()
            .enumerate()
            .map(|(index, id)| crate::layout::Glyph {
                face: SERIF_FACE,
                id,
                origin: Pos2::new(120.27 + index as f32 * 27.3, 90.43),
                size: 28.,
                rotated: index >= 7,
                style: GlyphStyle {
                    bold: index == 8,
                    italic: index == 8,
                },
            })
            .collect();
            let individual = ctx.run_ui(input(dpi), |ui| {
                let original = fonts
                    .raster(&ctx, SERIF_FACE, first, 28., GlyphStyle::default())
                    .unwrap();
                // Fill the existing atlas's remaining space so the next real
                // raster uses a different texture without allocating thousands
                // of unrelated large glyph masks for this batching regression.
                fonts.atlases.last_mut().unwrap().y = 2048;
                let next = fonts
                    .raster(&ctx, SERIF_FACE, second, 28., GlyphStyle::default())
                    .unwrap();
                assert_ne!(original.texture, next.texture);
                for glyph in &glyphs {
                    fonts.draw_styled(
                        ui.painter(),
                        glyph.face,
                        glyph.id,
                        glyph.origin + offset,
                        glyph.size,
                        tint,
                        glyph.rotated,
                        glyph.style,
                    );
                }
            });
            let batched = ctx.run_ui(input(dpi), |ui| {
                fonts.draw_batch(ui.painter(), &glyphs, offset, tint);
            });
            assert_eq!(triangles(&individual), triangles(&batched));
            let meshes = |output: &egui::FullOutput| {
                output
                    .shapes
                    .iter()
                    .filter(|shape| matches!(&shape.shape, egui::Shape::Mesh(_)))
                    .count()
            };
            assert!(meshes(&batched) < meshes(&individual));
        }
    }

    #[test]
    fn batch_splits_before_exceeding_16_bit_indices() {
        let ctx = egui::Context::default();
        let mut fonts = FontSystem::new();
        let id = fonts.shape(SERIF_FACE, "本", false)[0].id;
        let glyph = crate::layout::Glyph {
            face: SERIF_FACE,
            id,
            origin: Pos2::new(50., 60.),
            size: 24.,
            rotated: false,
            style: GlyphStyle::default(),
        };
        let glyphs = vec![glyph; 16_385];
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            fonts.draw_batch(ui.painter(), &glyphs, Vec2::ZERO, Color32::WHITE);
        });
        let meshes: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let egui::Shape::Mesh(mesh) = &shape.shape {
                    Some(mesh)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(meshes.len(), 2);
        assert_eq!(
            meshes.iter().map(|mesh| mesh.vertices.len()).sum::<usize>(),
            16_385 * 4
        );
        assert!(
            meshes
                .iter()
                .all(|mesh| mesh.indices.iter().all(|index| *index <= 65_535))
        );
    }

    #[test]
    fn installed_family_uses_its_real_bytes_and_falls_back_for_japanese() {
        let mut database = fontdb::Database::new();
        database.load_font_data(MONO.to_vec());
        let mut info = database.faces().next().unwrap().clone();
        database.remove_face(info.id);
        info.families[0].0 = "Deterministic installed font".into();
        database.push_face_info(info);
        let mut fonts = FontSystem::new();
        fonts.system_fonts = Some(database);
        fonts.families.push("Deterministic installed font".into());
        let pointer = fonts.families().as_ptr();
        assert_eq!(fonts.families().as_ptr(), pointer);
        fonts.begin_layout("Deterministic installed font");
        let selected = fonts.body_face("Deterministic installed font");
        assert!(selected >= 3);
        assert_eq!(fonts.faces[selected].data.as_ref(), MONO);
        assert_eq!(fonts.body_face("Deterministic installed font"), selected);
        let selected_width: f32 = fonts
            .shape(selected, "iii", false)
            .iter()
            .map(|glyph| glyph.advance.x)
            .sum();
        let serif_width: f32 = fonts
            .shape(SERIF_FACE, "iii", false)
            .iter()
            .map(|glyph| glyph.advance.x)
            .sum();
        assert!(
            (selected_width - serif_width).abs() > 0.5,
            "actual selected font metrics must affect layout"
        );
        assert_eq!(fonts.face_for_text(selected, "ASCII"), selected);
        let japanese = fonts.face_for_text(selected, "日本語");
        assert_eq!(japanese, SANS_FACE);
        assert!(
            fonts
                .shape(japanese, "日本語", true)
                .iter()
                .all(|glyph| glyph.id != 0)
        );
        // A header's bundled font must not invalidate earlier custom-font glyphs.
        assert_eq!(fonts.body_face("Noto Sans CJK JP"), SANS_FACE);
        assert_eq!(fonts.faces[selected].data.as_ref(), MONO);
    }
    #[test]
    fn vertical_shaping_has_one_em_advance_and_vertical_punctuation() {
        let mut fonts = FontSystem::new();
        let glyphs = fonts.shape(0, "本。", true);
        assert_eq!(glyphs.len(), 2);
        assert!((glyphs[0].advance.y - 1.0).abs() < 0.01);
        assert_ne!(glyphs[1].id, fonts.shape(0, "。", false)[0].id);
    }

    #[test]
    fn outline_styles_have_separate_measured_rasters() {
        let mut fonts = FontSystem::new();
        let ctx = egui::Context::default();
        let id = fonts.shape(SERIF_FACE, "M", false)[0].id;
        let normal = fonts
            .raster(&ctx, SERIF_FACE, id, 48.0, GlyphStyle::default())
            .unwrap();
        let bold = fonts
            .raster(
                &ctx,
                SERIF_FACE,
                id,
                48.0,
                GlyphStyle {
                    bold: true,
                    italic: false,
                },
            )
            .unwrap();
        let italic = fonts
            .raster(
                &ctx,
                SERIF_FACE,
                id,
                48.0,
                GlyphStyle {
                    bold: false,
                    italic: true,
                },
            )
            .unwrap();
        assert!(bold.size.x >= normal.size.x);
        assert!(
            italic.size.x > normal.size.x,
            "slanted outline needs measured overhang"
        );
        assert_ne!(normal.uv, bold.uv);
        assert_ne!(normal.uv, italic.uv);
    }

    #[test]
    fn bold_kanji_rasters_preserve_normal_stroke_coverage() {
        let mut fonts = FontSystem::new();
        let ctx = egui::Context::default();
        let mut missing = Vec::new();
        for text in ["設", "定", "短", "項", "目", "読"] {
            let id = fonts.shape(SERIF_FACE, text, false)[0].id;
            for size in [24.0, 27.6, 32.0] {
                let mut coverage = Vec::new();
                for bold in [false, true] {
                    let raster = fonts
                        .raster(
                            &ctx,
                            SERIF_FACE,
                            id,
                            size,
                            GlyphStyle {
                                bold,
                                italic: false,
                            },
                        )
                        .unwrap();
                    let mut delta = ctx.tex_manager().write().take_delta();
                    let sum: u64 = match &delta
                        .set
                        .get(&raster.texture)
                        .unwrap()
                        .last()
                        .unwrap()
                        .image
                    {
                        egui::ImageData::Color(image) => {
                            image.pixels.iter().map(|pixel| u64::from(pixel.a())).sum()
                        }
                    };
                    delta.clear();
                    coverage.push(sum);
                }
                if coverage[1] < coverage[0] {
                    missing.push((text, size, coverage));
                }
            }
        }
        assert!(
            missing.is_empty(),
            "bold must not erase glyph strokes: {missing:?}"
        );
    }

    #[test]
    fn book_glyph_meshes_keep_one_physical_pixel_per_texel_at_every_dpi() {
        let mut fonts = FontSystem::new();
        let ctx = egui::Context::default();
        let tint = Color32::from_rgb(41, 45, 44);
        for dpi in [1.0, 1.25, 1.2501, 1.5, 2.0, 2.5] {
            let mut input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(600., 320.))),
                ..Default::default()
            };
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .native_pixels_per_point = Some(dpi);
            let mut count = 0;
            let mut output = ctx.run_ui(input, |ui| {
                for (face, text) in [(SERIF_FACE, "頁"), (SANS_FACE, "M")] {
                    let id = fonts.shape(face, text, false)[0].id;
                    for size in [24.0, 32.0, 47.25] {
                        for style in [
                            GlyphStyle::default(),
                            GlyphStyle {
                                bold: true,
                                italic: false,
                            },
                            GlyphStyle {
                                bold: false,
                                italic: true,
                            },
                            GlyphStyle {
                                bold: true,
                                italic: true,
                            },
                        ] {
                            for rotated in [false, true] {
                                fonts.draw_styled(
                                    ui.painter(),
                                    face,
                                    id,
                                    Pos2::new(121.37, 163.61),
                                    size,
                                    tint,
                                    rotated,
                                    style,
                                );
                                count += 1;
                            }
                        }
                    }
                }
            });
            assert_eq!(
                fonts.pixels_per_point, dpi,
                "even small zoom changes must refresh point metrics"
            );
            let meshes: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Mesh(mesh) => Some(mesh),
                    _ => None,
                })
                .collect();
            assert_eq!(meshes.len(), count);
            for mesh in meshes {
                assert_eq!(mesh.vertices.len(), 4);
                for vertex in &mesh.vertices {
                    for coordinate in [vertex.pos.x * dpi, vertex.pos.y * dpi] {
                        assert!(
                            (coordinate - coordinate.round()).abs() < 0.0001,
                            "glyph edge is off the physical grid at {dpi}x: {coordinate}"
                        );
                    }
                    assert_eq!(vertex.color, tint, "mask tint must survive snapping");
                }
                for (index, a) in mesh.vertices.iter().enumerate() {
                    for b in mesh.vertices.iter().skip(index + 1) {
                        let pixels = (a.pos - b.pos).length() * dpi;
                        let texels = (a.uv - b.uv).length() * 2048.;
                        assert!(
                            (pixels - texels).abs() < 0.0001,
                            "glyph bitmap was rescaled at {dpi}x: {pixels} pixels / {texels} texels"
                        );
                    }
                }
            }
            output.textures_delta.clear();
        }
    }

    #[test]
    fn physical_grid_avoids_a_second_softening_of_title_strokes() {
        let mut fonts = FontSystem::new();
        let ctx = egui::Context::default();
        let id = fonts.shape(SERIF_FACE, "頁", false)[0].id;
        let raster = fonts
            .raster(
                &ctx,
                SERIF_FACE,
                id,
                32.,
                GlyphStyle {
                    bold: true,
                    italic: false,
                },
            )
            .unwrap();
        let mut delta = ctx.tex_manager().write().take_delta();
        let egui::ImageData::Color(image) = &delta.set[&raster.texture].last().unwrap().image;
        let width = image.size[0] + 6;
        let height = image.size[1] + 6;
        // Model the atlas's existing linear sampler at a half-pixel baseline
        // and at the integer-pixel baseline used by draw_styled. Swash coverage
        // is identical in both; only the unnecessary second interpolation differs.
        let sample = |offset: f32| -> Vec<u8> {
            (0..height)
                .flat_map(|y| {
                    (0..width).map(move |x| {
                        let px = x as f32 - 3. - offset;
                        let py = y as f32 - 3. - offset;
                        let left = px.floor() as isize;
                        let top = py.floor() as isize;
                        let fx = px - px.floor();
                        let fy = py - py.floor();
                        let mut alpha = 0.;
                        for (dy, wy) in [(0, 1. - fy), (1, fy)] {
                            for (dx, wx) in [(0, 1. - fx), (1, fx)] {
                                let sx = left + dx;
                                let sy = top + dy;
                                if sx >= 0
                                    && sy >= 0
                                    && sx < image.size[0] as isize
                                    && sy < image.size[1] as isize
                                {
                                    alpha += f32::from(
                                        image.pixels[sy as usize * image.size[0] + sx as usize].a(),
                                    ) * wx
                                        * wy;
                                }
                            }
                        }
                        alpha.round().clamp(0., 255.) as u8
                    })
                })
                .collect()
        };
        let before = sample(0.5);
        let after = sample(0.);
        let energy = |coverage: &[u8]| {
            coverage
                .iter()
                .map(|&alpha| u64::from(alpha).pow(2))
                .sum::<u64>()
        };
        assert!(
            energy(&after) > energy(&before),
            "snapped coverage keeps the original edge contrast"
        );
        // Optional diagnostic crop for visual QA; normal test runs write no files.
        if let Some(path) = std::env::var_os("VOBR_GLYPH_SAMPLING_PROBE") {
            let mut comparison = image::RgbaImage::from_pixel(
                (width * 2 + 8) as u32,
                height as u32,
                image::Rgba([250, 250, 247, 255]),
            );
            for (column, coverage) in [&before, &after].into_iter().enumerate() {
                for (index, &alpha) in coverage.iter().enumerate() {
                    let alpha = f32::from(alpha) / 255.;
                    let blend =
                        |paper: f32, ink: f32| (paper * (1. - alpha) + ink * alpha).round() as u8;
                    comparison.put_pixel(
                        (column * (width + 8) + index % width) as u32,
                        (index / width) as u32,
                        image::Rgba([blend(250., 41.), blend(250., 45.), blend(247., 44.), 255]),
                    );
                }
            }
            image::imageops::resize(
                &comparison,
                comparison.width() * 6,
                comparison.height() * 6,
                image::imageops::FilterType::Nearest,
            )
            .save(path)
            .unwrap();
        }
        eprintln!(
            "glyph coverage energy: fractional={}, snapped={}",
            energy(&before),
            energy(&after)
        );
        delta.clear();
    }
}
