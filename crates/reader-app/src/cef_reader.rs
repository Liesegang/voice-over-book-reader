//! Chromium draws book content into the same GPU device as the native controls.
use super::*;
use crate::cef_resources::BookOrigin;
use anyhow::{Context, Result};
use reader_renderer_cef::{
    Command as BookCommand, Flow, GpuBookSurface, LayoutOptions, NavigatorEvent, Revision, Runtime,
    SourceLocator, SourceRange, SurfaceEvent, SurfaceOptions, Writing,
};

pub(super) struct CefReader {
    render: eframe::egui_wgpu::RenderState,
    runtime: Option<Runtime<'static>>,
    origin: Option<BookOrigin>,
    book_key: String,
    layout_key: String,
    revision: u64,
    panes: [Option<Pane>; 2],
    expected_panes: usize,
    auxiliary: Option<Pane>,
    counter: Option<Counter>,
    count_plan: Option<Receiver<Result<Vec<CountItem>, String>>>,
    counter_url: String,
    counter_script: Option<String>,
    font_css: String,
    counted: Vec<Option<Vec<Option<Locator>>>>,
    count_epoch: Arc<std::sync::atomic::AtomicU64>,
    cache_path: Option<PathBuf>,
    error: Option<String>,
    highlight: Option<(Locator, Locator)>,
    turn_pending: bool,
    hit_action: Option<Pos2>,
    context_pending: bool,
    suppress_click_until: Instant,
}
struct Counter {
    surface: GpuBookSurface<'static>,
    started: bool,
    polling: bool,
    next_poll: Instant,
}
#[derive(serde::Serialize)]
struct CountItem {
    chapter: usize,
    url: String,
    width: f32,
    height: f32,
    options: LayoutOptions,
}
struct Pane {
    surface: GpuBookSurface<'static>,
    texture: Option<egui::TextureId>,
    chapter: usize,
    url: String,
    revision: Revision,
    options: LayoutOptions,
    size: [u32; 2],
    scale: f32,
    ready: bool,
    page: usize,
    requested_page: usize,
    starts: Vec<Option<Locator>>,
    waiting_paint: Option<u64>,
    displayed: bool,
    awaiting_location: bool,
    loaded: bool,
    settings_revision: u64,
    turns: std::collections::VecDeque<bool>,
    wheel: f32,
    annotations: Vec<SourceRange>,
    highlight: Option<(Locator, Locator)>,
    captured_buttons: u8,
    cursor: egui::CursorIcon,
    auxiliary_location: Option<Locator>,
}

fn auxiliary_regions(ui: &mut Ui) -> (Rect, egui::Response, Ui) {
    // Reserve the window content once. A horizontal row otherwise contributes
    // its own minimum height to Window/Resize, which expands on every pass if
    // that row is even slightly taller than the reserved footer.
    let available = ui.available_size_before_wrap().max(vec2(1., 1.));
    let (bounds, _) = ui.allocate_exact_size(available, Sense::hover());
    let text_height = ui
        .text_style_height(&egui::TextStyle::Button)
        .max(ui.text_style_height(&egui::TextStyle::Body));
    let footer_height = ui
        .spacing()
        .interact_size
        .y
        .max(text_height + 2. * ui.spacing().button_padding.y)
        .min(bounds.height());
    let footer_rect = Rect::from_min_max(
        pos2(bounds.left(), bounds.bottom() - footer_height),
        bounds.max,
    );
    let rect = Rect::from_min_max(
        bounds.min,
        pos2(
            bounds.right(),
            (footer_rect.top() - ui.spacing().item_spacing.y).max(bounds.top() + 1.),
        ),
    );
    let response = ui.interact(
        rect,
        ui.scope_id().with("auxiliary-book-page"),
        Sense::click_and_drag(),
    );
    let mut footer = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("auxiliary-book-footer")
            .max_rect(footer_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    footer.set_clip_rect(ui.clip_rect().intersect(footer_rect));
    (rect, response, footer)
}

impl CefReader {
    pub(super) fn new(render: eframe::egui_wgpu::RenderState) -> Self {
        Self {
            render,
            runtime: None,
            origin: None,
            book_key: String::new(),
            layout_key: String::new(),
            revision: 0,
            panes: [None, None],
            expected_panes: 1,
            auxiliary: None,
            counter: None,
            counted: Vec::new(),
            count_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            cache_path: None,
            error: None,
            highlight: None,
            count_plan: None,
            counter_url: String::new(),
            counter_script: None,
            font_css: String::new(),
            turn_pending: false,
            hit_action: None,
            context_pending: false,
            suppress_click_until: Instant::now(),
        }
    }
    pub(super) fn turn_ready(&self) -> bool {
        !self.turn_pending && self.panes_ready()
    }
    fn panes_ready(&self) -> bool {
        self.panes.iter().flatten().count() == self.expected_panes
            && self
                .panes
                .iter()
                .flatten()
                .all(|p| p.ready && p.waiting_paint.is_none() && p.displayed)
    }
    pub(super) fn mark_turn(&mut self) {
        self.turn_pending = true;
    }
    pub(super) fn measurement_marker(&self) -> Option<[u8; 3]> {
        let pane = self.panes[0].as_ref()?;
        if !pane.options.measurement_marker || !pane.ready {
            return None;
        }
        Some([
            16 + ((pane.requested_page & 7) * 32) as u8,
            16 + (((pane.requested_page >> 3) & 7) * 32) as u8,
            128 + ((pane.revision.0 & 3) * 32) as u8,
        ])
    }
    fn release_pane(&self, pane: Option<Pane>) {
        if let Some(mut pane) = pane {
            if let Some(texture) = pane.texture.take() {
                self.render.renderer.write().free_texture(&texture);
            }
            let _ = pane.surface.close();
        }
    }
    fn reset(&mut self) {
        self.count_epoch
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let panes = std::mem::take(&mut self.panes);
        for pane in panes {
            self.release_pane(pane);
        }
        let auxiliary = self.auxiliary.take();
        self.release_pane(auxiliary);
        self.counter = None;
        self.count_plan = None;
        if let Some(origin) = self.origin.take() {
            let _ = origin.unregister_global();
        }
        self.layout_key.clear();
        self.error = None;
        self.highlight = None;
        self.book_key.clear();
        self.turn_pending = false;
        self.expected_panes = 1;
    }
    pub(super) fn close_book(&mut self) {
        self.reset();
    }
    pub(super) fn close_auxiliary(&mut self) {
        let pane = self.auxiliary.take();
        self.release_pane(pane);
    }
    pub(super) fn clear_selection(&mut self) {
        self.hit_action = None;
        self.context_pending = false;
        self.suppress_click_until = Instant::now() + Duration::from_millis(150);
        for pane in self
            .panes
            .iter_mut()
            .flatten()
            .chain(self.auxiliary.iter_mut())
            .filter(|pane| pane.ready)
        {
            if let Err(error) = pane.surface.command(&BookCommand::ClearSelection {
                revision: pane.revision,
            }) {
                tracing::warn!(%error, "Could not clear book selection");
            }
        }
    }
    fn bind_book(&mut self, app: &ReaderApp) -> Result<()> {
        let key = format!("{}:{}", app.book.id, app.book.content_identity());
        if self.book_key == key {
            return Ok(());
        }
        self.reset();
        let _load = crate::latency::span("chromium_book_bind", app.open_generation);
        let runtime = crate::bootstrap::runtime(app.store.data_dir())?;
        let mut origin = BookOrigin::new(app.book.clone())?;
        if let Some(types) = app
            .book
            .epub
            .as_ref()
            .and_then(|source| source.media_types())
        {
            origin.set_mime_types(types.clone())?;
        }
        let counter_path = format!("__vobr_{}/count.xhtml", origin.token());
        origin.add_control_document(counter_path.clone(),b"<?xml version='1.0'?><html xmlns='http://www.w3.org/1999/xhtml'><head><title>Pagination</title></head><body style='margin:0'/></html>".to_vec())?;
        self.font_css.clear();
        for (file, family, data, format) in [
            (
                "serif.otf",
                "Noto Serif CJK JP",
                crate::fonts::SERIF,
                "opentype",
            ),
            (
                "sans.otf",
                "Noto Sans CJK JP",
                crate::fonts::SANS,
                "opentype",
            ),
            ("mono.ttf", "JetBrains Mono", crate::fonts::MONO, "truetype"),
            ("math.otf", "STIX Two Math", crate::fonts::MATH, "opentype"),
        ] {
            let path = format!("__vobr_{}/{}", origin.token(), file);
            origin.add_static_resource(path.clone(), data)?;
            let url = serde_json::to_string(&origin.url(&path)?)?;
            self.font_css.push_str(&format!("@font-face{{font-family:'{family}';src:url({url}) format('{format}');font-display:block;}}"));
        }
        self.counter_url = origin.url(&counter_path)?;
        origin.register_global()?;
        self.runtime = Some(runtime);
        self.origin = Some(origin);
        self.book_key = key;
        Ok(())
    }
    fn make_pane(
        &self,
        app: &ReaderApp,
        chapter: usize,
        extent: egui::Vec2,
        scale: f32,
        p: Palette,
    ) -> Result<Pane> {
        self.make_document_pane(app, &app.book.chapters[chapter], extent, scale, p)
    }
    fn make_document_pane(
        &self,
        app: &ReaderApp,
        chapter: &reader_core::Chapter,
        extent: egui::Vec2,
        scale: f32,
        p: Palette,
    ) -> Result<Pane> {
        let size = [
            (extent.x * scale).round().max(1.) as u32,
            (extent.y * scale).round().max(1.) as u32,
        ];
        let url = self
            .origin
            .as_ref()
            .context("Book origin unavailable")?
            .url(&chapter.href)?;
        let mut surface = GpuBookSurface::new(
            self.runtime.as_ref().context("Book runtime unavailable")?,
            self.render.device.clone(),
            self.render.queue.clone(),
            SurfaceOptions {
                initial_url: url.clone(),
                physical_size: size,
                scale_factor: scale,
                background: [p.paper.r(), p.paper.g(), p.paper.b()],
                profile: None,
            },
        )?;
        let ctx = app.worker_context.clone();
        surface.set_repaint_wake(Some(Arc::new(move || ctx.request_repaint())));
        Ok(Pane {
            surface,
            texture: None,
            chapter: chapter.id as usize,
            url,
            revision: Revision(self.revision),
            options: options(chapter, &app.settings, p, &self.font_css),
            size,
            scale,
            ready: false,
            page: 0,
            requested_page: 0,
            starts: Vec::new(),
            waiting_paint: None,
            displayed: false,
            awaiting_location: false,
            loaded: false,
            settings_revision: self.revision,
            turns: Default::default(),
            wheel: 0.,
            annotations: Vec::new(),
            highlight: None,
            captured_buttons: 0,
            cursor: egui::CursorIcon::Default,
            auxiliary_location: None,
        })
    }
    fn poll_pane(
        render: &eframe::egui_wgpu::RenderState,
        pane: &mut Pane,
        events: &mut Vec<NavigatorEvent>,
        visible: bool,
    ) -> Result<()> {
        let changed = pane.surface.poll()?;
        while let Some(event) = pane.surface.event() {
            match event {
                SurfaceEvent::DocumentLoaded { url }
                    if url.split('#').next() == Some(pane.url.as_str()) =>
                {
                    pane.surface.install_navigator()?;
                    pane.loaded = true;
                    pane.surface.command(&BookCommand::Configure {
                        revision: pane.revision,
                        chapter: pane.chapter as u32,
                        nodes: Vec::new(),
                        options: Box::new(pane.options.clone()),
                    })?;
                }
                SurfaceEvent::Navigator(event) => {
                    let current = match &event {
                        NavigatorEvent::Pagination { revision, .. }
                        | NavigatorEvent::Position { revision, .. }
                        | NavigatorEvent::Invalidated { revision, .. }
                        | NavigatorEvent::Hit { revision, .. }
                        | NavigatorEvent::Rects { revision, .. }
                        | NavigatorEvent::Selection { revision, .. }
                        | NavigatorEvent::Pointer { revision, .. }
                        | NavigatorEvent::Link { revision, .. }
                        | NavigatorEvent::Error { revision, .. } => *revision == pane.revision,
                    };
                    if !current {
                        continue;
                    }
                    match &event {
                        NavigatorEvent::Pagination { page_starts, .. } => {
                            pane.annotations.clear();
                            pane.highlight = None;
                            pane.starts = page_starts.iter().map(|p| p.map(from_source)).collect();
                            if pane.starts.is_empty() {
                                pane.starts.push(None);
                            }
                            pane.ready = true;
                            if pane.requested_page > 0 {
                                pane.surface.command(&BookCommand::Page {
                                    revision: pane.revision,
                                    index: pane.requested_page.min(pane.starts.len() - 1) as u32,
                                })?;
                            }
                        }
                        NavigatorEvent::Position { page, .. } => {
                            pane.page = *page as usize;
                            pane.waiting_paint = Some(pane.surface.request_repaint()?);
                        }
                        NavigatorEvent::Invalidated { .. } => {
                            pane.ready = false;
                            pane.revision.0 += 1;
                            pane.surface.command(&BookCommand::Configure {
                                revision: pane.revision,
                                chapter: pane.chapter as u32,
                                nodes: Vec::new(),
                                options: Box::new(pane.options.clone()),
                            })?;
                        }
                        NavigatorEvent::Error { message, .. } => anyhow::bail!("{message}"),
                        _ => {}
                    }
                    if visible {
                        events.push(event);
                    }
                }
                SurfaceEvent::Cursor(shape) => {
                    use reader_renderer_cef::welding::CursorShape;
                    pane.cursor = match shape {
                        CursorShape::Pointer => egui::CursorIcon::PointingHand,
                        CursorShape::Text => egui::CursorIcon::Text,
                        CursorShape::Wait => egui::CursorIcon::Wait,
                        CursorShape::Crosshair => egui::CursorIcon::Crosshair,
                        CursorShape::Move | CursorShape::ResizeAll => egui::CursorIcon::Move,
                        CursorShape::NotAllowed => egui::CursorIcon::NotAllowed,
                        CursorShape::Help => egui::CursorIcon::Help,
                        CursorShape::Progress => egui::CursorIcon::Progress,
                        CursorShape::ResizeNs => egui::CursorIcon::ResizeVertical,
                        CursorShape::ResizeEw => egui::CursorIcon::ResizeHorizontal,
                        CursorShape::ResizeNeSw => egui::CursorIcon::ResizeNeSw,
                        CursorShape::ResizeNwSe => egui::CursorIcon::ResizeNwSe,
                        CursorShape::Grab => egui::CursorIcon::Grab,
                        CursorShape::Grabbing => egui::CursorIcon::Grabbing,
                        CursorShape::ZoomIn => egui::CursorIcon::ZoomIn,
                        CursorShape::ZoomOut => egui::CursorIcon::ZoomOut,
                        _ => egui::CursorIcon::Default,
                    };
                }
                SurfaceEvent::Error(error) => anyhow::bail!("{error}"),
                _ => {}
            }
        }
        if changed && let Some(frame) = pane.surface.frame() {
            let mut renderer = render.renderer.write();
            if let Some(id) = pane.texture {
                renderer.update_egui_texture_from_wgpu_texture(
                    &render.device,
                    &frame.view,
                    eframe::wgpu::FilterMode::Linear,
                    id,
                );
            } else if visible {
                pane.texture = Some(renderer.register_native_texture(
                    &render.device,
                    &frame.view,
                    eframe::wgpu::FilterMode::Linear,
                ));
            }
            if pane
                .waiting_paint
                .is_some_and(|barrier| pane.surface.frame_sequence() > barrier)
            {
                pane.waiting_paint = None;
            }
        }
        Ok(())
    }
    fn configure(&mut self, app: &ReaderApp, extent: egui::Vec2, p: Palette) {
        let key = format!(
            "chromium-layout-3:{}:{}:{:?}:{}:{}:{:?}",
            reader_core::default_reader_css_revision(),
            self.book_key,
            extent,
            app.settings.horizontal_margin(),
            app.settings.vertical_margin(),
            (
                app.settings.reading_mode,
                app.settings.spread,
                app.settings.text_spread,
                app.settings.page_gap,
                app.settings.font_family.as_str(),
                app.settings.font_size,
                app.settings.letter_spacing,
                app.settings.line_spacing,
                app.settings.ruby_scale,
                app.settings.show_ruby,
                app.settings.adapt_monochrome_images,
                (
                    [p.paper.r(), p.paper.g(), p.paper.b()],
                    [p.ink.r(), p.ink.g(), p.ink.b()],
                    [p.accent.r(), p.accent.g(), p.accent.b()]
                )
            )
        );
        if key == self.layout_key {
            return;
        }
        self.revision += 1;
        self.count_epoch
            .store(self.revision, std::sync::atomic::Ordering::Relaxed);
        self.layout_key = key;
        self.counter = None;
        self.count_plan = None;
        self.counted = vec![None; app.book.chapters.len()];
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        self.layout_key.hash(&mut hash);
        let path = app
            .store
            .data_dir()
            .join("pagination-chromium-v1")
            .join(format!("{:016x}.json", hash.finish()));
        if let Ok(bytes) = std::fs::read(&path)
            && let Ok(index) = serde_json::from_slice::<crate::pagination::PageIndex>(&bytes)
            && index.starts.len() == self.counted.len()
        {
            self.counted = index.starts.into_iter().map(Some).collect();
        }
        self.cache_path = Some(path);
        if self.counted.iter().any(Option::is_none) {
            let book = app.book.clone();
            let settings = app.settings.clone();
            let origin = self.origin.as_ref().unwrap().clone();
            let existing = self.counted.clone();
            let font_css = self.font_css.clone();
            let epoch = self.count_epoch.clone();
            let generation = self.revision;
            let debounce = self.panes[0].is_some();
            let (tx, rx) = mpsc::channel();
            self.count_plan = Some(rx);
            std::thread::spawn(move || {
                let result = (|| -> Result<Vec<CountItem>> {
                    if debounce {
                        std::thread::sleep(Duration::from_millis(120));
                    }
                    let mut items = Vec::new();
                    for (chapter, cached) in existing.iter().enumerate() {
                        anyhow::ensure!(
                            epoch.load(std::sync::atomic::Ordering::Relaxed) == generation,
                            "Page calculation superseded"
                        );
                        if cached.is_some() {
                            continue;
                        }
                        let mut metadata = book.clone();
                        metadata.chapters[chapter] = book.load_chapter(chapter)?;
                        let plan = crate::reading_surface::chapter_spread(
                            &metadata, chapter, extent, &settings,
                        );
                        let size = plan.extent(extent);
                        let mut layout_options =
                            options(&metadata.chapters[chapter], &settings, p, &font_css);
                        // Ink adaptation preserves geometry; pagination never
                        // needs the additional pixel scan or image copies.
                        layout_options.adapt_monochrome_images = false;
                        layout_options.measurement_marker = false;
                        items.push(CountItem {
                            chapter,
                            url: origin.url(&metadata.chapters[chapter].href)?,
                            width: size.x,
                            height: size.y,
                            options: layout_options,
                        });
                    }
                    Ok(items)
                })()
                .map_err(|error| error.to_string());
                let _ = tx.send(result);
            });
        }
    }
    fn count(
        &mut self,
        app: &ReaderApp,
        rect: Rect,
        p: Palette,
    ) -> Result<Option<crate::pagination::PageIndex>> {
        let _ = (rect, p);
        if let Some(rx) = self.count_plan.as_ref()
            && let Ok(items) = rx.try_recv()
        {
            let mut items = items.map_err(anyhow::Error::msg)?;
            self.count_plan = None;
            items.retain(|item| self.counted[item.chapter].is_none());
            if !items.is_empty() {
                let mut surface = GpuBookSurface::new(
                    self.runtime.as_ref().unwrap(),
                    self.render.device.clone(),
                    self.render.queue.clone(),
                    SurfaceOptions {
                        initial_url: self.counter_url.clone(),
                        physical_size: [2, 2],
                        scale_factor: 1.,
                        background: [250, 248, 242],
                        profile: None,
                    },
                )?;
                let ctx = app.worker_context.clone();
                surface.set_repaint_wake(Some(Arc::new(move || ctx.request_repaint())));
                let serialized = serde_json::to_string(&items)?;
                // Keep the script until LoadEnd. The data is encoded, never interpreted as source.
                self.counter = Some(Counter {
                    surface,
                    started: false,
                    polling: false,
                    next_poll: Instant::now(),
                });
                self.counter_script = Some(format!(
                    "({})({serialized})",
                    include_str!("cef_counter.js")
                ));
            }
        }
        if let Some(counter) = self.counter.as_mut() {
            counter.surface.poll()?;
            while let Some(event) = counter.surface.event() {
                match event {
                    SurfaceEvent::DocumentLoaded { .. } if !counter.started => {
                        counter.surface.install_navigator()?;
                        let script = self.counter_script.take().unwrap();
                        counter.surface.request_script_result(7000, &script)?;
                        counter.started = true;
                    }
                    SurfaceEvent::ScriptResult { id: 7001, result } => {
                        counter.polling = false;
                        let value: serde_json::Value =
                            serde_json::from_str(&result.map_err(anyhow::Error::msg)?)?;
                        if let Some(error) =
                            value["errors"].as_array().and_then(|errors| errors.first())
                        {
                            anyhow::bail!("Page calculation: {error}");
                        }
                        if let Some(results) = value["results"].as_array() {
                            for result in results {
                                let chapter = result["chapter"]
                                    .as_u64()
                                    .context("Missing counted chapter")?
                                    as usize;
                                let starts: Vec<Option<SourceLocator>> =
                                    serde_json::from_value(result["starts"].clone())?;
                                if chapter < self.counted.len() {
                                    self.counted[chapter] = Some(
                                        starts.into_iter().map(|p| p.map(from_source)).collect(),
                                    );
                                }
                            }
                        }
                    }
                    SurfaceEvent::Error(error) => anyhow::bail!("{error}"),
                    _ => {}
                }
            }
            if counter.started && !counter.polling && Instant::now() >= counter.next_poll {
                counter.surface.request_script_result(7001,"({results:globalThis.__vobrCountResults?.splice(0)??[],errors:globalThis.__vobrCountErrors??[]})")?;
                counter.polling = true;
                counter.next_poll = Instant::now() + Duration::from_millis(8);
            }
        }
        if self.counted.iter().all(Option::is_some) {
            self.counter = None;
            self.count_plan = None;
            self.counter_script = None;
            let starts: Vec<_> = self.counted.iter().map(|p| p.clone().unwrap()).collect();
            let index = crate::pagination::PageIndex::new_with_starts(
                starts.iter().map(Vec::len).collect(),
                starts,
            )?;
            if let Some(path) = self.cache_path.take() {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
                if let Ok(bytes) = serde_json::to_vec(&index)
                    && std::fs::write(&temporary, bytes).is_ok()
                {
                    let _ = std::fs::rename(temporary, path);
                }
            }
            return Ok(Some(index));
        }
        Ok(None)
    }
}

fn options(
    chapter: &reader_core::Chapter,
    settings: &Settings,
    p: Palette,
    font_css: &str,
) -> LayoutOptions {
    let fixed = chapter.presentation.layout == Some(reader_core::RenditionLayout::PrePaginated);
    LayoutOptions {
        flow: if fixed { Flow::Fixed } else { Flow::Reflowable },
        writing: match settings.reading_mode {
            reader_library::ReadingMode::Auto => Writing::Author,
            reader_library::ReadingMode::Vertical => Writing::VerticalRl,
            _ => Writing::Horizontal,
        },
        margins: [
            settings.vertical_margin().max(READING_FOOTER_HEIGHT) as f64,
            settings.horizontal_margin() as f64,
            settings.vertical_margin().max(READING_FOOTER_HEIGHT) as f64,
            settings.horizontal_margin() as f64,
        ],
        font_family: Some(settings.font_family.clone()),
        font_size: Some(settings.font_size as f64),
        line_height: Some(settings.line_spacing as f64),
        letter_spacing: Some(settings.letter_spacing as f64),
        ruby_scale: Some(settings.ruby_scale as f64),
        show_ruby: settings.show_ruby,
        adapt_monochrome_images: settings.adapt_monochrome_images,
        fixed_viewport: chapter
            .presentation
            .viewport
            .map(|[w, h]| [w as f64, h as f64]),
        background: Some([p.paper.r(), p.paper.g(), p.paper.b()]),
        foreground: Some([p.ink.r(), p.ink.g(), p.ink.b()]),
        highlight_color: Some([p.accent.r(), p.accent.g(), p.accent.b()]),
        default_css: Some(format!("{font_css}\n{}", reader_core::DEFAULT_READER_CSS)),
        measurement_marker: std::env::args().any(|a| a == "--perf-output"),
        ..Default::default()
    }
}
fn from_source(value: SourceLocator) -> Locator {
    Locator {
        chapter: value.chapter,
        node: value.node,
        offset: value.offset,
    }
}
fn to_source(value: Locator) -> SourceLocator {
    SourceLocator {
        chapter: value.chapter,
        node: value.node,
        offset: value.offset,
    }
}

impl ReaderApp {
    pub(super) fn chromium_reader(&mut self, ui: &mut Ui, p: Palette) {
        let mut reader = self.cef_reader.take().unwrap();
        if let Err(error) = self.chromium_reader_inner(&mut reader, ui, p) {
            reader.error = Some(error.to_string());
        }
        if let Some(error) = reader.error.as_ref() {
            ui.painter().text(
                ui.max_rect().center(),
                Align2::CENTER_CENTER,
                format!("本文を表示できません: {error}"),
                FontId::proportional(14.),
                p.ink,
            );
        }
        self.cef_reader = Some(reader);
    }
    fn chromium_reader_inner(
        &mut self,
        reader: &mut CefReader,
        ui: &mut Ui,
        p: Palette,
    ) -> Result<()> {
        let rect = ui.available_rect_before_wrap();
        let interaction = Rect::from_min_max(
            rect.min,
            pos2(rect.right(), rect.bottom() - READING_FOOTER_HEIGHT),
        );
        let response = ui.interact(
            interaction,
            Id::unique("book-page"),
            Sense::click_and_drag(),
        );
        if response.secondary_clicked() {
            reader.context_pending = true;
            self.selected = None;
        }
        if response.dragged() {
            reader.suppress_click_until = Instant::now() + Duration::from_millis(150);
            self.pending_center_tap = None;
            self.pending_page_turn = None;
        }
        if response.hovered() && self.wheel_capture_frame != ui.ctx().cumulative_frame_nr() {
            self.wheel_capture_frame = ui.ctx().cumulative_frame_nr();
            let wheel = ui.input(|input| {
                input
                    .events
                    .iter()
                    .filter_map(|event| match event {
                        egui::Event::MouseWheel {
                            unit,
                            delta,
                            modifiers,
                            ..
                        } if !modifiers.command => Some((*unit, *delta)),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            });
            self.queue_wheel_pages(&wheel);
        }
        self.reading_extent = rect.size();
        if self.opening || self.book.chapters.is_empty() {
            return Ok(());
        }
        if !self.book.chapters[self.chapter].loaded {
            self.request_chapter();
            return Ok(());
        }
        reader.bind_book(self)?;
        self.prefetch_next_chapter();
        reader.runtime.as_ref().unwrap().pump();
        let previous_key = reader.layout_key.clone();
        reader.configure(self, rect.size(), p);
        if previous_key != reader.layout_key {
            self.pending_restore = self.current_book_locator().or(self.pending_restore);
            self.pagination_index = None;
            self.pagination_pending = true;
            self.pagination_error = None;
            self.layout = BookLayout::default();
            self.layout_key.clear();
        }
        let plan = crate::reading_surface::chapter_spread(
            &self.book,
            self.chapter,
            rect.size(),
            &self.settings,
        );
        self.spread_plan = plan;
        self.spread_pages = plan.columns;
        self.cross_chapter_spread = plan.second.is_some();
        reader.expected_panes = plan.columns;
        let extent = plan.extent(rect.size());
        let mut events = Vec::new();
        for slot in 0..plan.columns {
            let target = if slot == 0 {
                self.chapter
            } else {
                plan.second.unwrap_or(self.chapter)
            };
            let target_page = if slot == 0 || target != self.chapter {
                self.page
            } else {
                self.page.saturating_add(1)
            };
            if slot > 0
                && target == self.chapter
                && !self.layout.pages.is_empty()
                && target_page >= self.layout.pages.len()
            {
                reader.expected_panes -= 1;
                let old = reader.panes[slot].take();
                reader.release_pane(old);
                continue;
            }
            let physical_slot = if slot == 0 {
                plan.first_slot
            } else {
                1 - plan.first_slot
            };
            let target_rect = Rect::from_min_size(
                rect.min + vec2(physical_slot as f32 * (extent.x + plan.gap), 0.),
                extent,
            );
            if !self.book.chapters[target].loaded {
                let old = reader.panes[slot].take();
                reader.release_pane(old);
                continue;
            }
            let replace = reader.panes[slot]
                .as_ref()
                .is_none_or(|pane| pane.chapter != target);
            if replace {
                let old = reader.panes[slot].take();
                reader.release_pane(old);
                reader.panes[slot] =
                    Some(reader.make_pane(self, target, extent, ui.ctx().pixels_per_point(), p)?);
            }
            let pane = reader.panes[slot].as_mut().unwrap();
            let fixed_alignment = if matches!(pane.options.flow, Flow::Fixed) && plan.columns == 2 {
                if physical_slot == 0 { 1 } else { -1 }
            } else {
                0
            };
            if pane.settings_revision != reader.revision
                || pane.options.fixed_horizontal_align != fixed_alignment
            {
                pane.settings_revision = reader.revision;
                pane.revision.0 = pane.revision.0.max(reader.revision) + 1;
                pane.options = options(
                    &self.book.chapters[target],
                    &self.settings,
                    p,
                    &reader.font_css,
                );
                pane.options.fixed_horizontal_align = fixed_alignment;
                if fixed_alignment != 0 {
                    pane.options.margins[if physical_slot == 0 { 1 } else { 3 }] = 0.;
                }
                pane.ready = false;
                if pane.loaded {
                    pane.surface.command(&BookCommand::Configure {
                        revision: pane.revision,
                        chapter: target as u32,
                        nodes: Vec::new(),
                        options: Box::new(pane.options.clone()),
                    })?;
                }
            }
            let scale = ui.ctx().pixels_per_point();
            let pixels = [
                (extent.x * scale).round().max(1.) as u32,
                (extent.y * scale).round().max(1.) as u32,
            ];
            if pane.size != pixels || (pane.scale - scale).abs() > f32::EPSILON {
                pane.surface.resize(pixels, scale)?;
                pane.size = pixels;
                pane.scale = scale;
            }
            pane.requested_page = target_page;
            Self::poll_chromium_input(ui, target_rect, pane)?;
            let mut pane_events = Vec::new();
            CefReader::poll_pane(&reader.render, pane, &mut pane_events, true)?;
            if ui.rect_contains_pointer(target_rect) {
                ui.ctx().set_cursor_icon(pane.cursor);
            }
            for mut event in pane_events {
                if slot > 0 && matches!(event, NavigatorEvent::Position { .. }) {
                    continue;
                }
                match &mut event {
                    NavigatorEvent::Pointer { x, y, .. } | NavigatorEvent::Link { x, y, .. } => {
                        *x += (target_rect.left() - rect.left()) as f64;
                        *y += (target_rect.top() - rect.top()) as f64;
                    }
                    _ => {}
                }
                events.push(event);
            }
            if pane.ready {
                reader.counted[target] = Some(pane.starts.clone());
                if slot == 0 {
                    self.layout = BookLayout {
                        pages: pane
                            .starts
                            .iter()
                            .map(|start| {
                                let mut page = layout::Page {
                                    anchor: *start,
                                    ..Default::default()
                                };
                                if let Some(hit) = start
                                    .and_then(|start| self.chromium_hit(start, target_rect.min))
                                {
                                    page.hits.push(hit);
                                }
                                page
                            })
                            .collect(),
                    };
                    self.layout_key = reader.layout_key.clone();
                    self.page = self.page.min(self.layout.pages.len().saturating_sub(1));
                    if let Some(page) = self.pending_page_index.take() {
                        self.page = page.min(self.layout.pages.len() - 1);
                    }
                    if let Some(fraction) = self.pending_page_fraction.take() {
                        self.page = (fraction * self.layout.pages.len().saturating_sub(1) as f32)
                            .round() as usize;
                    }
                    if let Some(location) = self.pending_restore.take() {
                        pane.awaiting_location = true;
                        events.retain(|event| !matches!(event, NavigatorEvent::Position { .. }));
                        pane.surface.command(&BookCommand::Locate {
                            revision: pane.revision,
                            location: to_source(location),
                        })?;
                    } else if let Some(fragment) = self.pending_fragment.take() {
                        pane.awaiting_location = true;
                        events.retain(|event| !matches!(event, NavigatorEvent::Position { .. }));
                        pane.surface.command(&BookCommand::Fragment {
                            revision: pane.revision,
                            fragment,
                        })?;
                    }
                    pane.requested_page = self.page;
                }
                if pane.page != pane.requested_page && pane.waiting_paint.is_none() {
                    pane.surface.command(&BookCommand::Page {
                        revision: pane.revision,
                        index: pane.requested_page as u32,
                    })?;
                    pane.waiting_paint = Some(pane.surface.frame_sequence());
                }
            }
            if let Some(texture) = pane.texture
                && (pane.displayed || (pane.ready && pane.waiting_paint.is_none()))
            {
                ui.painter().image(
                    texture,
                    target_rect,
                    Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
                    Color32::WHITE,
                );
                pane.displayed = true;
            }
        }
        if plan.columns == 1 {
            let old = reader.panes[1].take();
            reader.release_pane(old);
        }
        // Letterbox areas of fixed pages may lie outside the author DOM.
        // Schedule the same native gesture; a DOM link response cancels it
        // before the double-click interval expires.
        if response.clicked()
            && !response.double_clicked()
            && !self.point_tool
            && Instant::now() >= reader.suppress_click_until
            && let Some(position) = response.interact_pointer_pos()
        {
            let x = position.x - rect.left();
            if x < rect.width() * 0.3 || x > rect.width() * 0.7 {
                self.pending_page_turn = Some((
                    Instant::now(),
                    (x < rect.width() * 0.5) == self.next_to_left(),
                ));
            } else {
                self.pending_center_tap = Some(Instant::now());
            }
        }
        for event in events {
            self.chromium_event(reader, ui, rect, event)?;
        }
        if reader.panes_ready()
            && reader.panes[0].as_ref().is_some_and(|pane| {
                pane.page == self.page
                    && pane.ready
                    && pane.waiting_paint.is_none()
                    && pane.displayed
            })
        {
            reader.turn_pending = false;
        }
        if response.double_clicked()
            && let Some(position) = response.interact_pointer_pos()
        {
            reader.suppress_click_until = Instant::now() + Duration::from_millis(500);
            reader.hit_action = Some(position);
            self.pending_center_tap = None;
            self.pending_page_turn = None;
            for (slot, pane) in reader
                .panes
                .iter_mut()
                .enumerate()
                .filter_map(|(slot, pane)| {
                    pane.as_mut()
                        .filter(|pane| pane.ready)
                        .map(|pane| (slot, pane))
                })
            {
                let physical_slot = if slot == 0 {
                    plan.first_slot
                } else {
                    1 - plan.first_slot
                };
                let target_rect = Rect::from_min_size(
                    rect.min + vec2(physical_slot as f32 * (extent.x + plan.gap), 0.),
                    extent,
                );
                if !target_rect.contains(position) {
                    continue;
                }
                pane.surface.command(&BookCommand::HitTest {
                    revision: pane.revision,
                    x: (position.x - target_rect.left()) as f64,
                    y: (position.y - target_rect.top()) as f64,
                })?;
            }
        }
        if response.drag_started()
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
        {
            let local = origin - rect.min;
            if local.x < self.settings.horizontal_margin()
                || local.x > rect.width() - self.settings.horizontal_margin()
                || local.y < self.settings.vertical_margin()
                || local.y > rect.height() - self.settings.vertical_margin()
            {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
        }
        response.context_menu(|ui| {
            if reader.context_pending {
                ui.disable();
            }
            if ui.button("ここから読み始める").clicked() {
                self.start_speech(self.selected.clone());
                ui.close();
            }
            if ui.button("辞書を引く").clicked() {
                if let Some(hit) = self.selected.clone() {
                    self.lookup_dictionary(hit.text, response.hover_pos().unwrap_or(rect.center()));
                }
                ui.close();
            }
            if ui.button("読みを登録").clicked() {
                if let Some(word) = self.selected.as_ref().map(|hit| hit.text.clone()) {
                    self.open_pronunciation_editor(ui.ctx(), &word);
                }
                ui.close();
            }
            if ui.button("ブックマーク").clicked() {
                self.add_bookmark();
                ui.close();
            }
            if ui.button("注釈を書く").clicked() {
                self.note_open = true;
                self.note_text.clear();
                ui.close();
            }
            if ui.button("選択文字をコピー").clicked() {
                if let Some(hit) = self.selected.as_ref() {
                    ui.ctx().copy_text(hit.text.clone());
                }
                ui.close();
            }
        });
        if self.pagination_index.is_none()
            && self.pagination_error.is_none()
            && reader.panes[0]
                .as_ref()
                .is_some_and(|pane| pane.ready && pane.displayed && pane.waiting_paint.is_none())
        {
            match reader.count(self, rect, p) {
                Ok(Some(index)) => {
                    self.pagination_index = Some(index);
                    self.pagination_pending = false;
                    crate::latency::event("chromium_total_pages_ready", self.open_generation);
                    if let Some(fraction) = self.pending_book_seek.take() {
                        self.seek_book(fraction);
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    self.pagination_error = Some(error.to_string());
                    self.pagination_pending = false;
                    reader.counter = None;
                    reader.count_plan = None;
                }
            }
        }
        if self.annotation_book != self.book.id
            || self
                .annotations_updated
                .is_none_or(|updated| updated.elapsed() >= Duration::from_secs(1))
        {
            self.annotations = Arc::new(self.store.annotations(&self.book.id).unwrap_or_default());
            self.annotation_book.clone_from(&self.book.id);
            self.annotations_updated = Some(Instant::now());
        }
        for pane in reader.panes.iter_mut().flatten().filter(|pane| pane.ready) {
            let annotations: Vec<_> = self
                .annotations
                .iter()
                .flat_map(|note| self.chromium_ranges(note.start, note.end, pane.chapter))
                .collect();
            if pane.annotations != annotations {
                pane.surface.command(&BookCommand::Annotations {
                    revision: pane.revision,
                    ranges: annotations.clone(),
                })?;
                pane.annotations = annotations;
            }
        }
        if reader.highlight != self.spoken_range
            || reader
                .panes
                .iter()
                .flatten()
                .any(|pane| pane.highlight != self.spoken_range)
        {
            reader.highlight = self.spoken_range;
            for pane in reader.panes.iter_mut().flatten().filter(|pane| pane.ready) {
                pane.highlight = self.spoken_range;
                let ranges = self
                    .spoken_range
                    .map(|(start, end)| self.chromium_ranges(start, end, pane.chapter))
                    .unwrap_or_default();
                pane.surface.command(&BookCommand::Highlight {
                    revision: pane.revision,
                    ranges,
                })?;
                if self.settings.tts.auto_follow
                    && self.follow_playback
                    && let Some((start, _)) = self.spoken_range
                    && start.chapter as usize == pane.chapter
                {
                    pane.awaiting_location = true;
                    pane.surface.command(&BookCommand::Locate {
                        revision: pane.revision,
                        location: to_source(start),
                    })?;
                }
            }
        }
        self.maybe_prefetch_audio(ui.ctx());
        self.prefetch_next_chapter();
        ui.ctx()
            .request_repaint_after(if reader.turn_ready() && !self.pagination_pending {
                Duration::from_millis(100)
            } else {
                Duration::from_millis(8)
            });
        Ok(())
    }
    pub(super) fn chromium_auxiliary_popup(
        &mut self,
        ctx: &egui::Context,
        p: Palette,
        document: &reader_core::LinkedDocument,
    ) {
        let Some(mut reader) = self.cef_reader.take() else {
            return;
        };
        let mut open = true;
        egui::Window::new(if document.chapter.title.is_empty() {
            "補足"
        } else {
            &document.chapter.title
        })
        .id(Id::unique("linked-auxiliary-document"))
        .default_size(vec2(620., 600.))
        .min_size(vec2(360., 260.))
        .constrain(true)
        .collapsible(false)
        .open(&mut open)
        .show(ctx, |ui| {
            if let Err(error) = self.chromium_auxiliary_content(&mut reader, ui, p, document) {
                ui.label(format!("補足を表示できません: {error}"));
            }
        });
        if !open {
            self.auxiliary_document = None;
            let pane = reader.auxiliary.take();
            reader.release_pane(pane);
        }
        self.cef_reader = Some(reader);
    }
    fn chromium_auxiliary_content(
        &mut self,
        reader: &mut CefReader,
        ui: &mut Ui,
        p: Palette,
        document: &reader_core::LinkedDocument,
    ) -> Result<()> {
        let (rect, response, mut footer) = auxiliary_regions(ui);
        let extent = rect.size();
        let chapter = &document.chapter;
        if reader
            .auxiliary
            .as_ref()
            .is_none_or(|pane| pane.chapter != chapter.id as usize)
        {
            let old = reader.auxiliary.take();
            reader.release_pane(old);
            reader.auxiliary = Some(reader.make_document_pane(
                self,
                chapter,
                extent,
                ui.ctx().pixels_per_point(),
                p,
            )?);
        }
        let pane = reader.auxiliary.as_mut().unwrap();
        pane.auxiliary_location.get_or_insert(document.locator);
        let mut layout_options = options(chapter, &self.settings, p, &reader.font_css);
        layout_options.measurement_marker = false;
        layout_options.margins = [20.; 4];
        let scale = ui.ctx().pixels_per_point();
        let size = [
            (extent.x * scale).round().max(1.) as u32,
            (extent.y * scale).round().max(1.) as u32,
        ];
        if pane.size != size
            || (pane.scale - scale).abs() > f32::EPSILON
            || serde_json::to_string(&pane.options)? != serde_json::to_string(&layout_options)?
        {
            self.auxiliary_scroll_target = self
                .auxiliary_scroll_target
                .or(pane.auxiliary_location)
                .or_else(|| pane.starts.get(pane.page).copied().flatten());
            pane.requested_page = 0;
            pane.awaiting_location = false;
            pane.surface.resize(size, scale)?;
            pane.size = size;
            pane.scale = scale;
            pane.options = layout_options;
            pane.revision.0 += 1;
            pane.ready = false;
            if pane.loaded {
                pane.surface.command(&BookCommand::Configure {
                    revision: pane.revision,
                    chapter: chapter.id,
                    nodes: Vec::new(),
                    options: Box::new(pane.options.clone()),
                })?;
            }
        }
        Self::poll_chromium_input(ui, rect, pane)?;
        let mut events = Vec::new();
        let previous_revision = pane.revision;
        CefReader::poll_pane(&reader.render, pane, &mut events, true)?;
        if pane.revision != previous_revision {
            // DOM/font invalidation also rebuilds pagination. Keep the original
            // source position instead of applying a page number from the old layout.
            self.auxiliary_scroll_target = self.auxiliary_scroll_target.or(pane.auxiliary_location);
            pane.requested_page = 0;
            pane.awaiting_location = false;
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(pane.cursor);
        }
        if pane.ready
            && let Some(locator) = self.auxiliary_scroll_target.take()
        {
            pane.auxiliary_location = Some(locator);
            pane.awaiting_location = true;
            events.retain(|event| !matches!(event, NavigatorEvent::Position { .. }));
            pane.surface.command(&BookCommand::Locate {
                revision: pane.revision,
                location: to_source(locator),
            })?;
        }
        for event in events {
            match event {
                NavigatorEvent::Position { page, start, .. } => {
                    pane.requested_page = page as usize;
                    if pane.awaiting_location {
                        pane.awaiting_location = false;
                    } else if self.auxiliary_scroll_target.is_none() {
                        pane.auxiliary_location =
                            start.map(from_source).or(pane.auxiliary_location);
                    }
                }
                NavigatorEvent::Link { href, x, y, .. } => {
                    let anchor = rect.min + vec2(x as f32, y as f32);
                    if let Some(origin) = reader.origin.as_ref()
                        && let Ok(path) = origin.path_from_url(&href)
                    {
                        let suffix = href
                            .split_once('#')
                            .map(|(_, fragment)| format!("#{fragment}"))
                            .unwrap_or_default();
                        self.request_book_link(
                            chapter.id,
                            format!("/{path}{suffix}"),
                            false,
                            anchor,
                            ui.ctx(),
                        );
                    } else if href.starts_with("https://")
                        || href.starts_with("http://")
                        || href.starts_with("mailto:")
                    {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(href));
                    }
                }
                NavigatorEvent::Selection {
                    start: Some(start),
                    end: Some(end),
                    text,
                    ..
                } => {
                    if let Some(mut hit) = self.chromium_hit(from_source(start), rect.min) {
                        hit.end = from_source(end);
                        hit.text = text;
                        self.selected = Some(hit);
                    }
                }
                _ => {}
            }
        }
        if let Some(texture) = pane.texture
            && (pane.displayed || (pane.ready && pane.waiting_paint.is_none()))
        {
            ui.painter().image(
                texture,
                rect,
                Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
                Color32::WHITE,
            );
            pane.displayed = true;
        }
        if response.hovered() {
            for event in ui.input(|input| input.events.clone()) {
                if let egui::Event::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                    ..
                } = event
                    && !modifiers.command
                {
                    pane.wheel += match unit {
                        egui::MouseWheelUnit::Point => -delta.y / 80.,
                        _ => -delta.y,
                    };
                    let count = pane.wheel.abs().floor().min(1024.) as usize;
                    let forward = pane.wheel > 0.;
                    pane.wheel -= count as f32 * pane.wheel.signum();
                    pane.turns.extend(std::iter::repeat_n(forward, count));
                }
            }
        }
        {
            let ui = &mut footer;
            if ui
                .add_enabled(pane.page > 0, egui::Button::new("前のページ"))
                .clicked()
            {
                pane.turns.push_back(false);
            }
            ui.label(format!("{} / {}", pane.page + 1, pane.starts.len().max(1)));
            if ui
                .add_enabled(
                    pane.page + 1 < pane.starts.len(),
                    egui::Button::new("次のページ"),
                )
                .clicked()
            {
                pane.turns.push_back(true);
            }
        }
        if pane.ready && !pane.awaiting_location && pane.waiting_paint.is_none() {
            if let Some(forward) = pane.turns.pop_front() {
                pane.requested_page = if forward {
                    (pane.page + 1).min(pane.starts.len().saturating_sub(1))
                } else {
                    pane.page.saturating_sub(1)
                };
            }
            if pane.page != pane.requested_page {
                pane.surface.command(&BookCommand::Page {
                    revision: pane.revision,
                    index: pane.requested_page as u32,
                })?;
                pane.waiting_paint = Some(pane.surface.frame_sequence());
            }
        }
        if !pane.ready || pane.waiting_paint.is_some() || !pane.turns.is_empty() {
            ui.ctx().request_repaint_after(Duration::from_millis(8));
        }
        Ok(())
    }
    fn poll_chromium_input(ui: &mut Ui, rect: Rect, pane: &mut Pane) -> Result<()> {
        use reader_renderer_cef::welding::{EventModifiers, MouseAction, MouseButton, MouseEvent};
        let input = ui.input(|input| input.events.clone());
        let modifiers = ui.input(|input| input.modifiers);
        let held = ui.input(|input| (input.pointer.primary_down(), input.pointer.secondary_down()));
        let flags = EventModifiers {
            shift: modifiers.shift,
            ctrl: modifiers.ctrl,
            alt: modifiers.alt,
            meta: modifiers.mac_cmd,
            left_mouse_button: held.0,
            right_mouse_button: held.1,
            ..Default::default()
        };
        for event in input {
            let (position, action, button) = match event {
                egui::Event::PointerMoved(pos) => (pos, MouseAction::Moved, MouseButton::Left),
                egui::Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    ..
                } => (
                    pos,
                    if pressed {
                        MouseAction::Pressed
                    } else {
                        MouseAction::Released
                    },
                    if button == egui::PointerButton::Secondary {
                        MouseButton::Right
                    } else {
                        MouseButton::Left
                    },
                ),
                _ => continue,
            };
            let button_mask = match button {
                MouseButton::Left => 1,
                MouseButton::Right => 2,
                MouseButton::Middle => 4,
            };
            let inside =
                rect.contains(position) && ui.ctx().layer_id_at(position) == Some(ui.layer_id());
            match action {
                MouseAction::Pressed if inside => pane.captured_buttons |= button_mask,
                MouseAction::Pressed => continue,
                MouseAction::Released if pane.captured_buttons & button_mask != 0 => {
                    pane.captured_buttons &= !button_mask
                }
                MouseAction::Released => continue,
                MouseAction::Moved if inside || pane.captured_buttons != 0 => {}
                _ => continue,
            }
            let local = position - rect.min;
            pane.surface.send_mouse(MouseEvent {
                x: (local.x * pane.scale).round() as i32,
                y: (local.y * pane.scale).round() as i32,
                button,
                action,
                modifiers: flags,
            })?;
        }
        Ok(())
    }
    fn chromium_event(
        &mut self,
        reader: &mut CefReader,
        ui: &mut Ui,
        rect: Rect,
        event: NavigatorEvent,
    ) -> Result<()> {
        match event {
            NavigatorEvent::Position { chapter, page, .. } if chapter as usize == self.chapter => {
                if let Some(pane) = reader.panes[0].as_mut()
                    && (pane.awaiting_location || pane.requested_page == page as usize)
                {
                    self.page = page as usize;
                    pane.requested_page = self.page;
                    pane.awaiting_location = false;
                }
            }
            NavigatorEvent::Pagination {
                chapter,
                writing_mode,
                ..
            } => {
                if let Some(chapter) = self.book.chapters.get_mut(chapter as usize) {
                    chapter.writing_mode = if writing_mode.starts_with("vertical") {
                        reader_core::WritingMode::VerticalRl
                    } else {
                        reader_core::WritingMode::Horizontal
                    };
                }
                crate::latency::event("chromium_visible_pagination_ready", self.open_generation);
            }
            NavigatorEvent::Pointer {
                kind,
                x,
                y,
                location,
                selection_start,
                selection_end,
                text,
                ..
            } => {
                let position = rect.min + vec2(x as f32, y as f32);
                let hit = location
                    .and_then(|location| self.chromium_hit(from_source(location), position));
                if let (Some(start), Some(end)) = (selection_start, selection_end)
                    && !text.is_empty()
                    && let Some(mut selected) = self.chromium_hit(from_source(start), position)
                {
                    selected.end = from_source(end);
                    selected.text = text;
                    self.selected = Some(selected);
                }
                match kind.as_str() {
                    "double_click" => {
                        if let Some(hit) = hit {
                            self.lookup_chromium_hit(hit, position);
                        }
                    }
                    "context_menu" => {
                        reader.context_pending = false;
                        if let Some(hit) = hit {
                            self.selected =
                                Some(layout::expand_word(&self.book.chapters[self.chapter], &hit));
                        }
                    }
                    "click" if Instant::now() < reader.suppress_click_until => {}
                    "click" if self.point_tool => {
                        if let Some(hit) = hit {
                            self.start_speech(Some(hit));
                            self.point_tool = false;
                        }
                    }
                    "click" => {
                        if self.dictionary_visible && !self.dictionary_pinned {
                            self.hide_dictionary();
                        }
                        self.dictionary_popup = None;
                        self.footnote = None;
                        self.selected = None;
                        if x < rect.width() as f64 * 0.3 || x > rect.width() as f64 * 0.7 {
                            let forward = (x < rect.width() as f64 * 0.5) == self.next_to_left();
                            if hit.is_some() {
                                self.pending_page_turn = Some((Instant::now(), forward));
                            } else if reader.turn_ready() {
                                reader.mark_turn();
                                self.turn_page(forward);
                            } else {
                                self.pending_wheel_turns.push_back(forward);
                            }
                        } else {
                            self.pending_center_tap = Some(Instant::now());
                        }
                    }
                    _ => {}
                }
            }
            NavigatorEvent::Selection {
                start: Some(start),
                end: Some(end),
                text,
                ..
            } => {
                if let Some(mut hit) = self.chromium_hit(from_source(start), rect.min) {
                    hit.end = from_source(end);
                    hit.text = text;
                    self.selected = Some(hit);
                }
            }
            NavigatorEvent::Hit {
                location: Some(location),
                ..
            } => {
                if let Some(position) = reader.hit_action.take()
                    && let Some(hit) = self.chromium_hit(from_source(location), position)
                {
                    self.lookup_chromium_hit(hit, position);
                    self.pending_center_tap = None;
                }
            }
            NavigatorEvent::Link {
                href,
                x,
                y,
                location,
                ..
            } => {
                self.pending_center_tap = None;
                self.pending_page_turn = None;
                let position = pos2(rect.left() + x as f32, rect.top() + y as f32);
                if self.point_tool
                    && let Some(hit) =
                        location.and_then(|loc| self.chromium_hit(from_source(loc), position))
                {
                    self.start_speech(Some(hit));
                    self.point_tool = false;
                    return Ok(());
                }
                if let Some(hit) =
                    location.and_then(|loc| self.chromium_hit(from_source(loc), position))
                    && self.follow_link(&hit, position, ui.ctx())
                {
                    return Ok(());
                }
                if let Some(origin) = reader.origin.as_ref()
                    && let Ok(path) = origin.path_from_url(&href)
                {
                    let target = self
                        .book
                        .chapters
                        .iter()
                        .position(|chapter| chapter.href == path);
                    let fragment = href
                        .split_once('#')
                        .map(|(_, fragment)| fragment.to_owned());
                    if let Some(target) = target {
                        self.jump_to_chapter(target, fragment);
                    } else {
                        self.follow_chromium_aux_link(
                            &path,
                            fragment,
                            pos2(rect.left() + x as f32, rect.top() + y as f32),
                            ui.ctx(),
                        );
                    }
                } else if href.starts_with("https://") || href.starts_with("http://") {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(href));
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn lookup_chromium_hit(&mut self, hit: Hit, position: Pos2) {
        let Some(chapter) = self
            .book
            .chapters
            .iter()
            .find(|chapter| chapter.id == hit.locator.chapter)
        else {
            return;
        };
        let hit = layout::expand_word(chapter, &hit);
        let query = reader_core::word_at(chapter, hit.locator)
            .ok()
            .flatten()
            .and_then(|word| word.lemma)
            .unwrap_or_else(|| hit.text.clone());
        self.selected = Some(hit);
        self.lookup_dictionary(query, position);
    }
    pub(super) fn chromium_hit(&self, locator: Locator, position: Pos2) -> Option<Hit> {
        let chapter = self
            .book
            .chapters
            .iter()
            .find(|chapter| chapter.id == locator.chapter)
            .or_else(|| {
                self.auxiliary_document
                    .as_ref()
                    .map(|document| &document.chapter)
                    .filter(|chapter| chapter.id == locator.chapter)
            })?;
        for (index, atom) in chapter.atoms().into_iter().enumerate() {
            let mut preceding = 0;
            for span in &atom.sources {
                if span.node == locator.node
                    && locator.offset >= span.start
                    && locator.offset < span.end
                {
                    let offset = preceding + (locator.offset - span.start) as usize;
                    if !atom.text.is_char_boundary(offset) {
                        return None;
                    }
                    let count = atom.text[offset..].chars().next()?.len_utf8();
                    return Some(Hit {
                        rect: Rect::from_min_size(position, vec2(1., self.settings.font_size)),
                        locator,
                        end: atom.locator_at(offset + count)?,
                        unit_id: layout::unit_id(chapter.id, index),
                        offset,
                        text: atom.text[offset..offset + count].to_owned(),
                    });
                }
                preceding += span.len();
            }
        }
        None
    }
    fn chromium_ranges(&self, start: Locator, end: Locator, chapter: usize) -> Vec<SourceRange> {
        let Some(content) = self.book.chapters.get(chapter) else {
            return Vec::new();
        };
        content
            .atoms()
            .iter()
            .flat_map(|atom| atom.sources.iter())
            .filter_map(|span| {
                let a = span.start_locator().max(start);
                let b = span.end_locator().min(end);
                (a < b && a.node == b.node).then_some(SourceRange {
                    chapter: span.chapter,
                    node: span.node,
                    start: a.offset,
                    end: b.offset,
                })
            })
            .collect()
    }
    fn follow_chromium_aux_link(
        &mut self,
        path: &str,
        fragment: Option<String>,
        position: Pos2,
        ctx: &egui::Context,
    ) {
        let href = format!(
            "/{}{}",
            path,
            fragment.map(|f| format!("#{f}")).unwrap_or_default()
        );
        self.request_book_link(
            self.book.chapters[self.chapter].id,
            href,
            false,
            position,
            ctx,
        );
    }
}

impl Drop for CefReader {
    fn drop(&mut self) {
        self.reset();
    }
}

#[cfg(test)]
mod auxiliary_tests {
    use super::auxiliary_regions;
    use eframe::egui::{self, Id, Pos2, Rect, TextStyle, vec2};

    #[test]
    fn auxiliary_window_does_not_expand_between_frames() {
        for body_size in [14., 28.] {
            let ctx = egui::Context::default();
            ctx.global_style_mut(|style| {
                style.spacing.item_spacing = vec2(10., 10.);
                style.spacing.button_padding = vec2(12., 7.);
                style.text_styles.get_mut(&TextStyle::Body).unwrap().size = body_size;
                style.text_styles.get_mut(&TextStyle::Button).unwrap().size = 11.;
            });
            let mut initial = None;
            for frame in 0..60 {
                let input = egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600., 1200.))),
                    time: Some(frame as f64 / 8.),
                    ..Default::default()
                };
                let mut window_size = None;
                ctx.run_ui(input, |_| {
                    let window = egui::Window::new("Supplement")
                        .id(Id::unique("linked-auxiliary-document"))
                        .default_size(vec2(620., 600.))
                        .min_size(vec2(360., 260.))
                        .constrain(true)
                        .collapsible(false)
                        .show(&ctx, |ui| {
                            let (page, _, mut footer) = auxiliary_regions(ui);
                            let footer_rect = footer.max_rect();
                            let _ = footer.button("Previous page");
                            footer.label("38 / 113");
                            let _ = footer.button("Next page");
                            assert!(page.bottom() < footer_rect.top());
                            assert!(ui.min_rect().bottom() <= footer_rect.bottom() + 0.1);
                            assert!(footer.min_rect().bottom() <= footer_rect.bottom() + 0.1);
                        })
                        .unwrap();
                    window_size = Some(window.response.rect.size());
                })
                .drop_without_applying_deltas();
                // Allow the initial sizing/font pass to settle, then check the
                // same interactive Window repeatedly without user resizing.
                if frame == 4 {
                    initial = window_size;
                } else if frame > 4 {
                    let size = window_size.unwrap();
                    assert!(
                        (size - initial.unwrap()).length() <= 0.1,
                        "window grew without user input: {:?} -> {size:?}",
                        initial.unwrap()
                    );
                }
            }
        }
    }
}
