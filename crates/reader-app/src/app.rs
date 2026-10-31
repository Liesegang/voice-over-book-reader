#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;

#[path = "cef_reader.rs"]
mod cef_reader;

use crate::{
    fonts::{self, FontSystem},
    icons::{self, Icon},
    layout::{self, BookLayout, Hit},
    theme::Palette,
};
use eframe::egui::{
    self, Align2, Color32, FontId, Id, Pos2, Rect, RichText, Sense, Ui, UiBuilder, pos2, vec2,
};
use reader_core::{Book, Locator};
use reader_library::{
    BookEntry, Bookmark, CalibreLibrary, DictionaryCatalog, DictionaryHit, DictionaryInfo,
    LibraryStore, ReadingPosition, Settings, Theme, unix_millis,
};
use reader_speech::{
    ProviderConfig, SpeechConfig, SpeechController, SpeechEventKind, SpeechState, SpeechUnit,
};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Screen {
    Library,
    Reader,
    Settings,
    Audiobook,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Drawer {
    Toc,
    Bookmarks,
    Notes,
    Search,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingTab {
    Appearance,
    Reading,
    Speech,
    Dictionaries,
    Library,
    Keys,
}
enum Work {
    Pagination {
        key: String,
        generation: u64,
        result: Result<crate::pagination::PageIndex, String>,
    },
    Link {
        book_generation: u64,
        request: u64,
        result: Result<Option<reader_core::LinkedDocument>, String>,
        note: bool,
        anchor: Pos2,
    },
    SpeechSources {
        book_generation: u64,
        generation: u64,
        sources: Vec<(u64, reader_core::Atom)>,
    },
    Chapter {
        book_generation: u64,
        request: u64,
        index: usize,
        result: Result<reader_core::Chapter, String>,
    },
    Search {
        book_generation: u64,
        generation: u64,
        result: Result<Vec<reader_core::SearchHit>, String>,
    },
    DictionaryState {
        epoch: u64,
        data: serde_json::Value,
    },
    Opened {
        generation: u64,
        result: Result<(Book, PathBuf), String>,
    },
    Shelf(Result<Vec<BookEntry>, String>),
    Dictionaries(Result<Vec<DictionaryInfo>, String>),
    Lookup {
        query: String,
        hits: Result<Vec<DictionaryHit>, String>,
    },
}
struct DictionaryPopup {
    query: String,
    hits: Vec<DictionaryHit>,
    anchor: Pos2,
}

const READING_FOOTER_HEIGHT: f32 = crate::reading_surface::READER_METADATA_HEIGHT;
const MAX_JUMP_MARKS: usize = 5;

fn seek_slider_rect(row: Rect) -> Rect {
    Rect::from_min_max(
        pos2(row.left() + 108_f32.min(row.width() * 0.25), row.top()),
        row.max,
    )
}

fn seek_rail_position(rect: Rect, fraction: f32, rtl: bool) -> Pos2 {
    // egui 0.37 SliderGeometry::position_range: a circular handle's radius is
    // widget thickness / 2.5. History marks use that same center travel range.
    let inset = rect.height() / 2.5;
    let fraction = if rtl { 1. - fraction } else { fraction }.clamp(0., 1.);
    pos2(
        rect.left() + inset + fraction * (rect.width() - 2. * inset).max(0.),
        rect.center().y,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct JumpMark {
    locator: Locator,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ComposedFrameKey {
    revision: u64,
    page: usize,
    extent: [u32; 2],
    columns: usize,
    first_slot: usize,
    gap: u32,
    cross_chapter: bool,
    sibling_ready: bool,
    loaded_neighbors: u8,
}

struct ComposedFrame {
    key: ComposedFrameKey,
    page: Arc<layout::Page>,
    links: Arc<std::collections::HashSet<u64>>,
    prefetch: Arc<Vec<(usize, Vec<layout::Picture>)>>,
}

fn narration_texts<'a>(
    chapter: &'a reader_core::Chapter,
    rules: &std::collections::BTreeMap<String, String>,
) -> Vec<&'a str> {
    let atoms = chapter.atoms();
    let original = reader_speech::pronunciation_source_mask(
        atoms.iter().map(|atom| atom.text.as_str()),
        rules,
    );
    atoms
        .into_iter()
        .zip(original)
        .map(|(atom, custom)| {
            if custom {
                atom.text.as_str()
            } else {
                atom.spoken_text()
            }
        })
        .collect()
}

struct ChapterNarration {
    units: Vec<SpeechUnit>,
    start_index: usize,
    byte_offset: usize,
}

fn chapter_narration(
    chapter: &reader_core::Chapter,
    rules: &std::collections::BTreeMap<String, String>,
    hit: Option<&Hit>,
) -> ChapterNarration {
    let mut plan = ChapterNarration {
        units: Vec::new(),
        start_index: 0,
        byte_offset: 0,
    };
    for (index, (atom, text)) in chapter
        .atoms()
        .into_iter()
        .zip(narration_texts(chapter, rules))
        .enumerate()
    {
        if atom.locator_at(0).is_none() || text.trim().is_empty() {
            continue;
        }
        let id = layout::unit_id(chapter.id, index);
        if let Some(hit) = hit.filter(|hit| hit.unit_id == id) {
            plan.start_index = plan.units.len();
            if atom.ruby.is_none() && text.is_char_boundary(hit.offset) {
                plan.byte_offset = hit.offset;
            }
        }
        plan.units.push(SpeechUnit {
            id,
            text: text.to_owned(),
        });
    }
    plan
}

pub struct ReaderApp {
    cef_reader: Option<cef_reader::CefReader>,
    perf_driver: Option<crate::cef_perf::PerfDriver>,
    speech_smoke: Option<crate::speech_smoke::Driver>,
    first_book_frame: Option<crate::latency::Pending>,
    page_count_ready: Option<crate::latency::Pending>,
    pagination_index: Option<crate::pagination::PageIndex>,
    pagination_key: String,
    pagination_epoch: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pagination_pending: bool,
    pagination_error: Option<String>,
    pending_page_index: Option<usize>,
    pending_book_seek: Option<f32>,
    jump_marks: Vec<JumpMark>,
    link_request: u64,
    auxiliary_document: Option<reader_core::LinkedDocument>,
    auxiliary_layout: BookLayout,
    auxiliary_layout_key: String,
    auxiliary_scroll_target: Option<Locator>,
    worker_context: egui::Context,
    pub(crate) reading_extent: egui::Vec2,
    selected_shelf: Option<String>,
    pub(crate) new_shelf_name: String,
    pub(crate) new_shelf_open: bool,
    last_library_refresh: Instant,
    reader_window: bool,
    opening: bool,
    open_error: Option<String>,
    chapter_request: u64,
    chapter_loading: Option<usize>,
    chapter_prefetch: std::collections::HashSet<usize>,
    spread_pages: usize,
    cross_chapter_spread: bool,
    spread_plan: crate::reading_surface::ChapterSpread,
    sibling_layout: BookLayout,
    sibling_key: String,
    pending_page_fraction: Option<f32>,
    pending_fragment: Option<String>,
    seek_drag: Option<f32>,
    pending_page_turn: Option<(Instant, bool)>,
    wheel_accumulator: f32,
    last_wheel: Instant,
    pending_wheel_turns: std::collections::VecDeque<bool>,
    wheel_capture_frame: u64,
    wheel_paint_frame: u64,
    search_results: Vec<reader_core::SearchHit>,
    search_started_query: String,
    search_generation: u64,
    search_edited: Instant,
    search_busy: bool,
    pub(crate) binding_capture: Option<String>,
    pub(crate) binding_query: String,
    pub(crate) font_query: String,
    pub(crate) dictionary_drag: Option<String>,
    pub(crate) speech_instruction: String,
    pub(crate) speech_reference: Option<PathBuf>,
    pub(crate) speech_seed: u64,
    pub(crate) speech_progress: String,
    pub(crate) speech_error: Option<String>,
    voice_preview: bool,
    prefetched_audio: Option<String>,
    dictionary_pinned: bool,
    dictionary_epoch: u64,
    pub(crate) store: LibraryStore,
    pub(crate) settings: Settings,
    pub(crate) screen: Screen,
    pub(crate) previous_screen: Screen,
    pub(crate) setting_tab: SettingTab,
    pub(crate) fonts: FontSystem,
    book: Book,
    path: Option<PathBuf>,
    chapter: usize,
    page: usize,
    layout: BookLayout,
    layout_key: String,
    layout_revision: u64,
    composed_frame: Option<ComposedFrame>,
    applied_palette: Option<Palette>,
    annotation_book: String,
    annotations: Arc<Vec<reader_library::Annotation>>,
    annotations_updated: Option<Instant>,
    pending_restore: Option<Locator>,
    resume_locator: Option<Locator>,
    open_generation: u64,
    follow_playback: bool,
    last_runtime_speed: f32,
    focus: bool,
    // Remember the user's sidebar choice while chrome is hidden. The overlay
    // paint gate, rather than this state, determines whether it is visible.
    drawer_open: bool,
    drawer: Drawer,
    point_tool: bool,
    selected: Option<Hit>,
    selection_anchor: Option<Hit>,
    pending_center_tap: Option<Instant>,
    note_open: bool,
    note_text: String,
    footnote: Option<(String, Locator, Pos2)>,
    shelf_query: String,
    search_query: String,
    shelf: Vec<BookEntry>,
    shelf_progress: HashMap<String, f32>,
    shelf_filter: u8,
    covers: crate::covers::CoverCache,
    pictures: HashMap<String, Option<egui::TextureHandle>>,
    pub(crate) dictionary_query: String,
    pub(crate) dictionary_infos: Vec<DictionaryInfo>,
    dictionary_popup: Option<DictionaryPopup>,
    dictionary_child: Option<(Child, ChildStdin)>,
    dictionary_visible: bool,
    screen_origin: Pos2,
    pixels_per_point: f32,
    parent_hwnd: Option<isize>,
    pub(crate) notice: Option<(String, Instant)>,
    pub(crate) busy: bool,
    tx: Sender<Work>,
    rx: Receiver<Work>,
    pub(crate) speech_provider: String,
    pub(crate) speech_endpoint: String,
    pub(crate) speech_voice: String,
    pub(crate) speech_model: String,
    pub(crate) speech_api_key: String,
    pub(crate) speech_command: String,
    pub(crate) speech_command_args: String,
    pub(crate) speech_state: String,
    pub(crate) speaking: bool,
    speech: Option<SpeechController>,
    speech_config_key: String,
    speech_profiles: crate::speech_profiles::SpeechProfiles,
    speech_generation: u64,
    speech_sources: HashMap<u64, reader_core::Atom>,
    spoken_range: Option<(Locator, Locator)>,
    screenshot: Option<PathBuf>,
    frame_count: usize,
    services: crate::services_ui::ServicesUi,
    audiobook: crate::audiobook_ui::AudiobookUi,
}

impl ReaderApp {
    pub(crate) fn settings_preview_source(&self) -> (reader_core::Chapter, String, bool) {
        let chapter = self
            .book
            .chapters
            .get(self.chapter)
            .filter(|c| c.loaded && !c.blocks.is_empty())
            .cloned()
            .unwrap_or_else(|| Book::demo().chapters.remove(0));
        let rtl = crate::reading_surface::next_to_left(&self.book, self.chapter, &self.settings);
        (chapter, self.book.id.clone(), rtl)
    }
    pub(crate) fn settings_preview_spread(
        &self,
        extent: egui::Vec2,
    ) -> crate::reading_surface::ChapterSpread {
        crate::reading_surface::chapter_spread(&self.book, self.chapter, extent, &self.settings)
    }
    pub(crate) fn settings_preview_sibling(&self) -> Option<reader_core::Chapter> {
        crate::reading_surface::chapter_spread(
            &self.book,
            self.chapter,
            self.reading_extent,
            &self.settings,
        )
        .second
        .and_then(|i| self.book.chapters.get(i))
        .filter(|c| c.loaded)
        .cloned()
    }
    pub fn new(cc: &eframe::CreationContext<'_>, args: &[String]) -> anyhow::Result<Self> {
        let speech_smoke = crate::speech_smoke::Driver::from_args(args)?;
        let _init = crate::latency::span("app_initialize", 0);
        {
            let _fonts = crate::latency::span("ui_fonts_install", 0);
            fonts::install_ui(&cc.egui_ctx);
        }
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let store_timer = crate::latency::span("library_store_open", 0);
        let store = if let Some(i) = args.iter().position(|a| a == "--data-dir") {
            LibraryStore::open(
                args.get(i + 1)
                    .ok_or_else(|| anyhow::anyhow!("--data-dir requires a path"))?,
            )?
        } else {
            LibraryStore::open_default()?
        };
        drop(store_timer);
        let settings = store.settings().clone();
        let speech_profiles = crate::speech_profiles::SpeechProfiles::load(&settings);
        let services =
            crate::services_ui::ServicesUi::new(&settings, store.data_dir().to_path_buf());
        let (tx, rx) = mpsc::channel();
        let covers = crate::covers::CoverCache::new(store.data_dir());
        let mut app = Self {
            perf_driver: crate::cef_perf::PerfDriver::from_args(args),
            speech_smoke,
            cef_reader: if !cfg!(test) && !args.iter().any(|a| a == "--native-renderer") {
                cc.wgpu_render_state.clone().map(cef_reader::CefReader::new)
            } else {
                None
            },
            first_book_frame: None,
            page_count_ready: None,
            worker_context: cc.egui_ctx.clone(),
            pagination_index: None,
            pagination_key: String::new(),
            pagination_epoch: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            pagination_pending: false,
            pagination_error: None,
            pending_page_index: None,
            pending_book_seek: None,
            jump_marks: Vec::new(),
            link_request: 0,
            auxiliary_document: None,
            auxiliary_layout: Default::default(),
            auxiliary_layout_key: String::new(),
            auxiliary_scroll_target: None,
            reading_extent: vec2(1280., 760.),
            selected_shelf: None,
            new_shelf_name: String::new(),
            new_shelf_open: false,
            last_library_refresh: Instant::now(),
            reader_window: args.iter().any(|a| a == "--reader" || a == "--demo")
                || book_argument(args).is_some(),
            opening: false,
            open_error: None,
            chapter_request: 0,
            chapter_loading: None,
            chapter_prefetch: std::collections::HashSet::new(),
            spread_pages: 1,
            cross_chapter_spread: false,
            spread_plan: Default::default(),
            sibling_layout: BookLayout::default(),
            sibling_key: String::new(),
            pending_page_fraction: None,
            pending_fragment: None,
            seek_drag: None,
            pending_page_turn: None,
            wheel_accumulator: 0.,
            last_wheel: Instant::now(),
            pending_wheel_turns: std::collections::VecDeque::new(),
            wheel_capture_frame: u64::MAX,
            wheel_paint_frame: u64::MAX,
            search_results: Vec::new(),
            search_started_query: String::new(),
            search_generation: 0,
            search_edited: Instant::now(),
            search_busy: false,
            binding_capture: None,
            binding_query: String::new(),
            font_query: String::new(),
            dictionary_drag: None,
            speech_instruction: String::new(),
            speech_reference: None,
            speech_seed: 42,
            speech_progress: String::new(),
            speech_error: None,
            voice_preview: false,
            prefetched_audio: None,
            dictionary_pinned: false,
            dictionary_epoch: 0,
            shelf: store.books().to_vec(),
            store,
            settings,
            screen: if args.iter().any(|a| a == "--demo") {
                Screen::Reader
            } else {
                Screen::Library
            },
            previous_screen: Screen::Library,
            setting_tab: SettingTab::Appearance,
            fonts: FontSystem::new(),
            book: Book::demo(),
            path: None,
            chapter: 0,
            page: 0,
            layout: BookLayout::default(),
            layout_key: String::new(),
            layout_revision: 0,
            composed_frame: None,
            applied_palette: None,
            annotation_book: String::new(),
            annotations: Arc::new(Vec::new()),
            annotations_updated: None,
            pending_restore: None,
            resume_locator: None,
            open_generation: 0,
            follow_playback: true,
            last_runtime_speed: 1.,
            focus: true,
            drawer_open: false,
            drawer: Drawer::Toc,
            point_tool: false,
            selected: None,
            selection_anchor: None,
            pending_center_tap: None,
            note_open: false,
            note_text: String::new(),
            footnote: None,
            shelf_query: String::new(),
            shelf_progress: HashMap::new(),
            search_query: String::new(),
            shelf_filter: 0,
            covers,
            pictures: HashMap::new(),
            dictionary_query: String::new(),
            dictionary_infos: Vec::new(),
            dictionary_popup: None,
            dictionary_child: None,
            dictionary_visible: false,
            screen_origin: Pos2::ZERO,
            pixels_per_point: 1.,
            parent_hwnd: None,
            notice: None,
            busy: false,
            tx,
            rx,
            speech_provider: "irodori".into(),
            speech_endpoint: "http://127.0.0.1:8880/v1".into(),
            speech_voice: String::new(),
            speech_model: "kokoro".into(),
            speech_api_key: String::new(),
            speech_command: String::new(),
            speech_command_args: "[]".into(),
            speech_state: String::new(),
            speaking: false,
            speech: None,
            speech_config_key: String::new(),
            speech_profiles,
            speech_generation: 0,
            speech_sources: HashMap::new(),
            spoken_range: None,
            screenshot: args
                .iter()
                .position(|a| a == "--screenshot")
                .and_then(|i| args.get(i + 1))
                .map(PathBuf::from),
            frame_count: 0,
            services,
            audiobook: crate::audiobook_ui::AudiobookUi::new(),
        };
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            app.parent_hwnd = cc
                .winit_window()
                .and_then(|window| window.window_handle().ok())
                .and_then(|handle| match handle.as_raw() {
                    RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
                    _ => None,
                });
            #[cfg(windows)]
            if let Some(hwnd) = app.parent_hwnd {
                let color = windows_sys::Win32::Graphics::Dwm::DWMWA_COLOR_NONE;
                unsafe {
                    windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
                        hwnd as _,
                        windows_sys::Win32::Graphics::Dwm::DWMWA_BORDER_COLOR as u32,
                        (&color as *const u32).cast(),
                        std::mem::size_of::<u32>() as u32,
                    );
                }
            }
        }
        app.speech_provider = app.settings.tts.provider_id.clone();
        app.speech_voice = app.settings.tts.voice.clone().unwrap_or_default();
        if let Some(config) = app
            .settings
            .tts
            .provider_config
            .clone()
            .and_then(|value| serde_json::from_value::<ProviderConfig>(value).ok())
        {
            match config {
                ProviderConfig::OpenAi {
                    base_url,
                    voice,
                    model,
                    api_key,
                } => {
                    app.speech_endpoint = base_url;
                    app.speech_voice = voice;
                    app.speech_model = model;
                    app.speech_api_key = api_key;
                }
                ProviderConfig::Voicevox { base_url, speaker } => {
                    app.speech_endpoint = base_url;
                    app.speech_voice = speaker.to_string();
                }
                ProviderConfig::Command { program, args } => {
                    app.speech_command = program.to_string_lossy().into_owned();
                    app.speech_command_args =
                        serde_json::to_string(&args).unwrap_or_else(|_| "[]".into());
                }
                _ => {}
            }
        }
        if args.iter().any(|a| a == "--focus") {
            app.screen = Screen::Reader;
            app.focus = true;
        }
        app.load_profile_fields();
        if args.iter().any(|a| a == "--settings") {
            app.screen = Screen::Settings;
        }
        if args.iter().any(|a| a == "--sepia") {
            app.settings.set_theme(Theme::Sepia);
        }
        if args.iter().any(|a| a == "--dark") {
            app.settings.set_theme(Theme::Dark);
        }
        if args.iter().any(|a| a == "--system-voice") {
            app.speech_provider = "system".into();
        }
        if args.iter().any(|a| a == "--toc") {
            app.drawer_open = true;
        }
        app.load_profile_fields();
        if let Some(file) = book_argument(args) {
            app.open_path(file);
        }
        app.refresh_shelf();
        app.refresh_dictionaries();
        if !cfg!(test) && !app.reader_window {
            crate::library_ipc::listen(app.store.data_dir(), cc.egui_ctx.clone());
        }
        Ok(app)
    }
    pub(crate) fn notify(&mut self, message: impl Into<String>) {
        self.notice = Some((message.into(), Instant::now()));
    }
    pub(crate) fn save_settings(&mut self) {
        if self.last_runtime_speed != self.settings.tts.speed {
            if let Some(speech) = &self.speech {
                let _ = speech.set_speed(self.settings.tts.speed);
            }
            self.last_runtime_speed = self.settings.tts.speed;
        }
        self.sync_speech_settings();
        if let Err(e) = self.store.save_settings(&self.settings) {
            self.notify(format!("設定を保存できません: {e}"));
        }
        self.layout_key.clear();
        self.pictures.clear();
    }
    pub(crate) fn open_settings(&mut self, tab: SettingTab) {
        self.previous_screen = self.screen;
        self.screen = Screen::Settings;
        self.setting_tab = tab;
    }
    pub(crate) fn pick_book(&mut self) {
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter(
                "電子書籍",
                &[
                    "epub", "txt", "html", "htm", "md", "fb2", "cbz", "pdf", "mobi", "azw3", "mp3",
                    "m4a", "m4b", "wav",
                ],
            )
            .pick_files()
        {
            for path in paths {
                self.open_document(path);
            }
        }
    }
    fn open_document(&mut self, path: PathBuf) {
        if let Err(error) = self.launch_window(Some(&path)) {
            self.notify(format!("ウィンドウを開けません: {error:#}"));
        }
    }
    fn launch_window(&self, path: Option<&std::path::Path>) -> anyhow::Result<()> {
        let mut command = Command::new(std::env::current_exe()?);
        command.arg("--data-dir").arg(self.store.data_dir());
        if let Some(path) = path {
            command.arg("--reader").arg(path);
        } else {
            command.arg("--library");
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        command.spawn()?;
        Ok(())
    }
    fn open_path(&mut self, path: PathBuf) {
        if let Some(reader) = self.cef_reader.as_mut() {
            reader.close_book();
        }
        self.clear_frame_caches();
        if path.extension().is_some_and(|extension| {
            ["mp3", "m4a", "m4b", "wav"]
                .iter()
                .any(|value| extension.eq_ignore_ascii_case(value))
        }) {
            self.persist_position();
            self.stop_speech();
            self.audiobook.open(path);
            self.screen = Screen::Audiobook;
            return;
        }
        self.audiobook.stop();
        self.persist_position();
        self.stop_speech();
        self.pending_center_tap = None;
        self.pagination_epoch
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.pagination_key.clear();
        self.pagination_index = None;
        self.pagination_pending = false;
        self.pagination_error = None;
        self.pending_book_seek = None;
        self.pending_page_index = None;
        self.auxiliary_document = None;
        self.link_request = self.link_request.wrapping_add(1);
        self.pending_wheel_turns.clear();
        self.wheel_accumulator = 0.;
        self.open_generation = self.open_generation.wrapping_add(1);
        let generation = self.open_generation;
        self.first_book_frame = Some(crate::latency::pending(
            "open_to_first_frame_cpu_ready",
            generation,
        ));
        crate::latency::event("book_open_requested", generation);
        self.busy = true;
        self.opening = true;
        self.open_error = None;
        self.screen = Screen::Reader;
        self.focus = true;
        self.layout = BookLayout::default();
        self.composed_frame = None;
        self.layout_key.clear();
        let tx = self.tx.clone();
        let ctx = self.worker_context.clone();
        std::thread::spawn(move || {
            let _open = crate::latency::span("book_open_worker", generation);
            let result = reader_core::open(&path)
                .map(|book| (book, path))
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Work::Opened { generation, result });
            ctx.request_repaint();
        });
    }
    fn clear_frame_caches(&mut self) {
        self.composed_frame = None;
        self.annotation_book.clear();
        self.annotations = Arc::new(Vec::new());
        self.annotations_updated = None;
    }
    pub(crate) fn refresh_shelf(&mut self) {
        if !self.reader_window
            && let Ok(store) = LibraryStore::open(self.store.data_dir())
        {
            self.store = store;
        }
        self.shelf = self.store.books().to_vec();
        if let Some(path) = self.settings.calibre_path.clone() {
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(Work::Shelf(
                    CalibreLibrary::scan(path)
                        .map(|l| l.entries)
                        .map_err(|e| format!("{e:#}")),
                ));
            });
        }
    }
    pub(crate) fn refresh_dictionaries(&mut self) {
        let dir = self.store.data_dir().join("dictionaries");
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Work::Dictionaries(
                DictionaryCatalog::open(dir)
                    .map(|c| c.dictionaries().to_vec())
                    .map_err(|e| format!("{e:#}")),
            ));
        });
    }
    pub(crate) fn add_dictionaries(&mut self, folder: bool) {
        let path = if folder {
            rfd::FileDialog::new().pick_folder()
        } else {
            rfd::FileDialog::new()
                .add_filter("MDict", &["mdx"])
                .pick_file()
        };
        if let Some(path) = path {
            let dir = self.store.data_dir().join("dictionaries");
            let tx = self.tx.clone();
            self.busy = true;
            std::thread::spawn(move || {
                let result = (|| -> anyhow::Result<Vec<DictionaryInfo>> {
                    let mut c = DictionaryCatalog::open(dir)?;
                    let report = c.register(path)?;
                    if report.registered.is_empty() && !report.errors.is_empty() {
                        anyhow::bail!("{}", report.errors.join("\n"));
                    }
                    Ok(c.dictionaries().to_vec())
                })()
                .map_err(|e| format!("{e:#}"));
                let _ = tx.send(Work::Dictionaries(result));
            });
        }
    }
    fn handle_work(&mut self) {
        while let Ok(work) = self.rx.try_recv() {
            match work {
                Work::Pagination {
                    key,
                    generation,
                    result,
                } => {
                    if key != self.pagination_key
                        || generation
                            != self
                                .pagination_epoch
                                .load(std::sync::atomic::Ordering::Relaxed)
                    {
                        continue;
                    }
                    self.pagination_pending = false;
                    crate::latency::event("page_count_received", generation);
                    match result {
                        Ok(index) => {
                            if let Some(timer) = self.page_count_ready.take() {
                                timer.finish();
                            }
                            self.pagination_index = Some(index);
                            if let Some(fraction) = self.pending_book_seek.take() {
                                self.seek_book(fraction);
                            }
                        }
                        Err(error) => {
                            self.page_count_ready = None;
                            self.pagination_error = Some(error);
                        }
                    }
                }
                Work::Link {
                    book_generation,
                    request,
                    result,
                    note,
                    anchor,
                } => {
                    if book_generation != self.open_generation || request != self.link_request {
                        continue;
                    }
                    match result {
                        Ok(Some(target)) => {
                            if !note {
                                self.record_jump_origin();
                            }
                            if let Some(index) = target.spine_index {
                                if let Some(slot) = self.book.chapters.get_mut(index) {
                                    *slot = target.chapter.clone();
                                }
                                if note {
                                    if let Some(text) =
                                        target.chapter.text_at_anchor(target.locator)
                                    {
                                        self.footnote = Some((text, target.locator, anchor));
                                    }
                                } else {
                                    self.auxiliary_document = None;
                                    self.restore_locator(target.locator);
                                }
                            } else {
                                self.auxiliary_scroll_target = Some(target.locator);
                                self.auxiliary_document = Some(target);
                                self.auxiliary_layout_key.clear();
                                self.footnote = None;
                            }
                        }
                        Ok(None) => self.notify("リンク先が見つかりません"),
                        Err(error) => self.notify(format!("リンクを開けません: {error}")),
                    }
                }
                Work::SpeechSources {
                    book_generation,
                    generation,
                    sources,
                } => {
                    if book_generation == self.open_generation
                        && generation == self.speech_generation
                    {
                        self.speech_sources.extend(sources);
                    }
                }
                Work::DictionaryState { epoch, data } => {
                    if epoch == self.dictionary_epoch {
                        if let Some(visible) = data.get("visible").and_then(|v| v.as_bool()) {
                            self.dictionary_visible = visible;
                        }
                        if let Some(pinned) = data.get("pinned").and_then(|v| v.as_bool()) {
                            self.dictionary_pinned = pinned;
                        }
                    }
                }
                Work::Chapter {
                    book_generation,
                    request,
                    index,
                    result,
                } => {
                    if book_generation != self.open_generation {
                        continue;
                    }
                    if result.is_ok() {
                        self.chapter_prefetch.remove(&index);
                    }
                    match result {
                        Ok(chapter) => {
                            if let Some(slot) = self.book.chapters.get_mut(index) {
                                *slot = chapter;
                            }
                        }
                        Err(error) => {
                            if request == self.chapter_request {
                                self.open_error = Some(error);
                            }
                        }
                    }
                    if request > 0 && request == self.chapter_request {
                        self.chapter_loading = None;
                        self.layout_key.clear();
                        if index == self.chapter {
                            let chapter = &self.book.chapters[index];
                            if self.pending_page_index.is_none()
                                && self.pending_page_fraction.is_none()
                                && self.pending_book_seek.is_none()
                            {
                                self.pending_restore = self
                                    .pending_restore
                                    .filter(|l| l.chapter as usize == index)
                                    .or_else(|| {
                                        self.pending_fragment
                                            .take()
                                            .and_then(|f| chapter.anchors.get(&f).copied())
                                    })
                                    .or_else(|| chapter.start_locator());
                                self.resume_locator = self.pending_restore;
                            }
                            self.book
                                .unload_chapters_except(&[self.chapter, self.chapter + 1]);
                        }
                    }
                }
                Work::Search {
                    book_generation,
                    generation,
                    result,
                } => {
                    if book_generation != self.open_generation
                        || generation != self.search_generation
                    {
                        continue;
                    }
                    self.search_busy = false;
                    match result {
                        Ok(hits) => self.search_results = hits,
                        Err(error) => self.notify(error),
                    }
                }
                Work::Opened { generation, .. } if generation != self.open_generation => {}
                Work::Opened {
                    generation: _,
                    result: Ok((mut book, path)),
                } => {
                    let _apply = crate::latency::span("book_open_apply", self.open_generation);
                    crate::latency::event("book_open_received", self.open_generation);
                    self.stop_speech();
                    self.clear_frame_caches();
                    let linked = self
                        .shelf
                        .iter()
                        .find(|entry| same_path(&entry.epub_path, &path))
                        .cloned();
                    if let Some(entry) = &linked {
                        book.id = entry.id.clone();
                    }
                    self.book = book;
                    self.pending_book_seek = None;
                    self.pending_page_index = None;
                    self.path = Some(path.clone());
                    self.chapter = 0;
                    self.page = 0;
                    self.layout_key.clear();
                    self.layout = BookLayout::default();
                    self.composed_frame = None;
                    self.pending_restore = None;
                    self.resume_locator = None;
                    self.pending_center_tap = None;
                    self.pictures.clear();
                    self.selected = None;
                    self.selection_anchor = None;
                    self.footnote = None;
                    self.focus = true;
                    self.drawer_open = false;
                    self.drawer = Drawer::Toc;
                    self.jump_marks.clear();
                    self.opening = false;
                    self.open_error = None;
                    self.chapter_loading = None;
                    self.chapter_prefetch.clear();
                    self.search_results.clear();
                    self.search_started_query.clear();
                    self.screen = Screen::Reader;
                    self.busy = false;
                    if self
                        .notice
                        .as_ref()
                        .is_some_and(|(message, _)| message == "書籍を開いています…")
                    {
                        self.notice = None;
                    }
                    if let Err(e) = self.store.register_document(&path, &self.book) {
                        self.notify(format!("本棚への登録: {e}"));
                    }
                    if let Some(mut entry) = linked {
                        entry.id = self.book.id.clone();
                        let _ = self.store.save_book(&entry);
                    }
                    self.settings.last_book = Some(path);
                    self.save_settings();
                    self.refresh_shelf();
                    if let Ok(Some(position)) = self.store.position(&self.book.id) {
                        self.restore_locator(position.locator);
                    }
                }
                Work::Opened {
                    generation: _,
                    result: Err(e),
                } => {
                    self.busy = false;
                    self.opening = false;
                    self.open_error = Some(format!("書籍を開けません: {e}"));
                }
                Work::Shelf(Ok(entries)) => {
                    self.shelf_progress.clear();
                    self.shelf = self.store.books().to_vec();
                    for mut entry in entries {
                        if let Some(existing) = self.shelf.iter_mut().find(|e| {
                            e.epub_path == entry.epub_path
                                || (entry.uuid.is_some() && e.uuid == entry.uuid)
                        }) {
                            entry.id = existing.id.clone();
                            let _ = self.store.save_book(&entry);
                            *existing = entry;
                        } else {
                            self.shelf.push(entry);
                        }
                    }
                }
                Work::Shelf(Err(e)) => self.notify(format!("Calibre: {e}")),
                Work::Dictionaries(result) => {
                    self.busy = false;
                    match result {
                        Ok(infos) => self.dictionary_infos = infos,
                        Err(e) => self.notify(e),
                    }
                }
                Work::Lookup { query, hits } => {
                    if let Some(popup) = &mut self.dictionary_popup
                        && popup.query == query
                    {
                        match hits {
                            Ok(hits) => popup.hits = hits,
                            Err(e) => self.notify(e),
                        }
                    }
                }
            }
        }
    }
    fn restore_locator(&mut self, locator: Locator) {
        self.selection_anchor = None;
        self.persist_position();
        self.follow_playback = false;
        self.pending_center_tap = None;
        self.chapter = (locator.chapter as usize).min(self.book.chapters.len().saturating_sub(1));
        self.chapter_loading = None;
        self.page = 0;
        self.layout_key.clear();
        self.selected = None;
        self.pending_book_seek = None;
        self.pending_page_index = None;
        self.pending_page_fraction = None;
        self.pending_fragment = None;
        self.pending_restore = Some(locator);
        self.resume_locator = Some(locator);
    }
    fn current_book_locator(&self) -> Option<Locator> {
        (!self.layout_key.is_empty())
            .then(|| {
                self.layout
                    .pages
                    .get(self.page)
                    .and_then(|page| page.first_locator())
            })
            .flatten()
            .filter(|loc| loc.chapter as usize == self.chapter)
            .or(self.pending_restore)
            .or(self.resume_locator)
    }
    fn record_jump_origin(&mut self) {
        // A second seek while a chapter/page is still loading must not create
        // a marker for its unpainted provisional chapter-start position.
        if self.opening
            || self.chapter_loading.is_some()
            || self.pending_page_index.is_some()
            || self.layout_key.is_empty()
        {
            return;
        }
        let Some(locator) = self.current_book_locator() else {
            return;
        };
        self.jump_marks.retain(|mark| mark.locator != locator);
        self.jump_marks.push(JumpMark { locator });
        if self.jump_marks.len() > MAX_JUMP_MARKS {
            self.jump_marks.remove(0);
        }
    }
    fn jump_to_locator(&mut self, locator: Locator) {
        if self.current_book_locator() != Some(locator) {
            self.record_jump_origin();
        }
        self.restore_locator(locator);
    }
    fn persist_position(&mut self) {
        let _persist = crate::latency::span("reading_position_persist", self.open_generation);
        if let Some(locator) = self
            .layout
            .pages
            .get(self.page)
            .and_then(|p| p.first_locator())
        {
            let progress = self
                .page_numbers()
                .map(|(_, end, total)| end as f32 / total.max(1) as f32)
                .unwrap_or_else(|| {
                    self.shelf_progress
                        .get(&self.book.id)
                        .copied()
                        .unwrap_or(0.)
                });
            let _ = self.store.save_position(&ReadingPosition {
                book_id: self.book.id.clone(),
                locator,
                progress,
                updated_at: unix_millis(),
            });
            self.shelf_progress.insert(self.book.id.clone(), progress);
        }
    }
    fn turn_page(&mut self, forward: bool) {
        let _turn = crate::latency::span("page_turn_dispatch", self.open_generation);
        if self.book.epub.is_some()
            && let Some(reader) = self.cef_reader.as_mut()
        {
            if !reader.turn_ready() {
                self.pending_wheel_turns.push_back(forward);
                self.worker_context.request_repaint();
                return;
            }
            reader.mark_turn();
        }
        if self.opening || self.chapter_loading.is_some() {
            return;
        }
        self.selection_anchor = None;
        let old_chapter = self.chapter;
        self.pending_center_tap = None;
        self.pending_page_turn = None;
        self.resume_locator = None;
        self.follow_playback = false;
        let atomic = crate::reading_surface::is_image_chapter(&self.book.chapters[self.chapter])
            && self.layout.pages.len() == 1;
        if atomic {
            let target = if forward {
                self.chapter
                    + if self.cross_chapter_spread {
                        self.spread_plan.consumed()
                    } else {
                        1
                    }
            } else {
                crate::reading_surface::previous_chapter_spread(
                    &self.book,
                    self.chapter,
                    self.reading_extent,
                    &self.settings,
                )
            };
            if target >= self.book.chapters.len() || target == self.chapter {
                return;
            }
            self.persist_position();
            self.chapter = target;
            self.page = 0;
            self.layout = BookLayout::default();
            self.composed_frame = None;
            self.layout_key.clear();
            self.selected = None;
            self.pending_restore = None;
            self.chapter_loading = None;
            return;
        }
        let window = crate::reading_surface::page_window(
            &self.book.chapters[self.chapter],
            self.page,
            self.layout.pages.len(),
            self.spread_pages,
            self.next_to_left(),
        );
        if forward {
            if window.end < self.layout.pages.len() {
                self.page = window.end;
            } else if self.chapter + 1 < self.book.chapters.len() {
                self.persist_position();
                self.chapter += 1;
                self.page = 0;
                self.layout_key.clear();
            }
        } else if self.page > 0 {
            self.page = crate::reading_surface::page_window(
                &self.book.chapters[self.chapter],
                self.page - 1,
                self.layout.pages.len(),
                self.spread_pages,
                self.next_to_left(),
            )
            .start;
        } else if self.chapter > 0 {
            self.persist_position();
            self.chapter -= 1;
            self.page = usize::MAX;
            self.layout_key.clear();
        }
        self.selected = None;
        if old_chapter != self.chapter {
            self.layout = BookLayout::default();
            self.composed_frame = None;
            self.pending_restore = None;
        }
        self.persist_position();
    }
    fn add_bookmark(&mut self) {
        let locator = self.selected.as_ref().map(|h| h.locator).or_else(|| {
            self.layout
                .pages
                .get(self.page)
                .and_then(|p| p.first_locator())
        });
        if let Some(locator) = locator {
            let label = self
                .book
                .chapters
                .get(self.chapter)
                .map(|c| c.title.clone())
                .unwrap_or_default();
            let bookmark = Bookmark::new(&self.book.id, locator, label);
            match self.store.save_bookmark(&bookmark) {
                Ok(()) => self.notify("ブックマークを追加しました"),
                Err(e) => self.notify(e.to_string()),
            }
        }
    }
}

pub(crate) fn region<R>(ui: &mut Ui, id: &str, rect: Rect, show: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope_builder(UiBuilder::new().id_salt(id).max_rect(rect), |ui| {
        ui.set_clip_rect(ui.clip_rect().intersect(rect));
        show(ui)
    })
    .inner
}

impl ReaderApp {
    fn top_bar(&mut self, ui: &mut Ui, p: Palette) {
        ui.horizontal_centered(|ui| {
            if self.screen == Screen::Reader {
                if icons::button(ui, Icon::Menu, "読書メニュー", self.drawer_open, p.accent)
                    .clicked()
                {
                    self.drawer_open = !self.drawer_open;
                }
                if icons::button(ui, Icon::Library, "本棚ウィンドウへ", false, p.accent).clicked()
                {
                    let _ = self.launch_window(None);
                }
                let width = (ui.available_width() - 144.).max(80.);
                icons::label(
                    ui,
                    vec2(width, 32.),
                    RichText::new(&self.book.title).size(15.),
                    Sense::hover(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::button(ui, Icon::Settings, "設定", false, p.accent).clicked() {
                        self.open_settings(SettingTab::Appearance);
                    }
                    if icons::button(ui, Icon::Bookmark, "ブックマークを追加", false, p.accent)
                        .clicked()
                    {
                        self.add_bookmark();
                    }
                    if icons::button(ui, Icon::Search, "本文検索", false, p.accent).clicked() {
                        self.drawer = Drawer::Search;
                        self.drawer_open = true;
                    }
                });
            } else {
                ui.heading("Voice Over Book Reader");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::button(ui, Icon::Settings, "設定", false, p.accent).clicked() {
                        self.open_settings(SettingTab::Appearance);
                    }
                });
            }
        });
    }

    fn drawer(&mut self, ui: &mut Ui, p: Palette) {
        ui.spacing_mut().item_spacing = vec2(4., 4.);
        ui.horizontal(|ui| {
            for (tab, icon, label) in [
                (Drawer::Toc, Icon::Book, "目次"),
                (Drawer::Bookmarks, Icon::Bookmark, "ブックマーク"),
                (Drawer::Notes, Icon::Note, "注釈"),
                (Drawer::Search, Icon::Search, "検索"),
            ] {
                if icons::button(ui, icon, label, self.drawer == tab, p.accent).clicked() {
                    self.drawer = tab;
                }
            }
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .animated(true)
            .show(ui, |ui| match self.drawer {
                Drawer::Toc => {
                    let entries = self.book.toc.clone();
                    self.toc_entries(ui, &entries, 0, p);
                }
                Drawer::Bookmarks => {
                    let bookmarks = self.store.bookmarks(&self.book.id).unwrap_or_default();
                    if bookmarks.is_empty() {
                        ui.label(RichText::new("ブックマークはまだありません").color(p.muted));
                    }
                    for bookmark in bookmarks {
                        ui.horizontal(|ui| {
                            let width = (ui.available_width() - 40.).max(40.);
                            if icons::label(ui, vec2(width, 30.), &bookmark.label, Sense::click())
                                .clicked()
                            {
                                self.jump_to_locator(bookmark.locator);
                            }
                            if icons::button(ui, Icon::Close, "ブックマークを削除", false, p.accent)
                                .clicked()
                            {
                                let _ = self.store.delete_bookmark(&bookmark.id);
                            }
                        });
                    }
                }
                Drawer::Notes => {
                    for note in self.store.annotations(&self.book.id).unwrap_or_default() {
                        if icons::label(
                            ui,
                            vec2(ui.available_width(), 30.),
                            &note.quote,
                            Sense::click(),
                        )
                        .clicked()
                        {
                            self.jump_to_locator(note.start);
                        }
                        ui.label(RichText::new(note.note).small().color(p.muted));
                        ui.separator();
                    }
                }
                Drawer::Search => {
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.search_query)
                                .hint_text("本文を検索")
                                .desired_width(f32::INFINITY),
                        )
                        .changed()
                    {
                        self.search_edited = Instant::now();
                        self.search_generation = self.search_generation.wrapping_add(1);
                    }
                    if self.search_busy {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("検索中");
                        });
                    } else if !self.search_query.is_empty() {
                        ui.label(format!("{}件", self.search_results.len()));
                    }
                    for hit in self.search_results.clone().into_iter().take(500) {
                        if ui
                            .add_sized(
                                [ui.available_width(), 44.],
                                egui::Button::new(&hit.snippet).frame(false).wrap(),
                            )
                            .clicked()
                        {
                            self.jump_to_locator(hit.start);
                        }
                    }
                }
            });
    }

    fn toc_entries(
        &mut self,
        ui: &mut Ui,
        entries: &[reader_core::TocEntry],
        depth: usize,
        p: Palette,
    ) {
        for entry in entries {
            ui.horizontal(|ui| {
                ui.add_space(depth as f32 * 12.);
                let group = matches!(entry.target, Some(reader_core::TocTarget::Group));
                let active = entry.target.is_none()
                    && self.chapter == entry.chapter as usize
                    && entry.fragment.is_none();
                let response = icons::label(
                    ui,
                    vec2(ui.available_width(), 28.),
                    RichText::new(&entry.title).color(if active { p.accent } else { p.ink }),
                    if group {
                        Sense::hover()
                    } else {
                        Sense::click()
                    },
                );
                if response.clicked() {
                    match &entry.target {
                        Some(reader_core::TocTarget::Group) => {}
                        Some(reader_core::TocTarget::Document { href }) => self.request_book_link(
                            self.book.chapters[self.chapter].id,
                            href.clone(),
                            false,
                            response.rect.center(),
                            ui.ctx(),
                        ),
                        None => {
                            self.jump_to_chapter(entry.chapter as usize, entry.fragment.clone())
                        }
                    }
                }
            });
            self.toc_entries(ui, &entry.children, depth + 1, p);
        }
    }

    fn reading_seek(&mut self, ui: &mut Ui, p: Palette) {
        ui.spacing_mut().item_spacing.y = 0.;
        let rtl = self.next_to_left();
        let (row, _) = ui.allocate_exact_size(
            vec2(ui.available_width(), READING_FOOTER_HEIGHT),
            Sense::hover(),
        );
        let rail = seek_slider_rect(row);
        let label =
            Rect::from_min_max(row.min + vec2(8., 0.), pos2(rail.left() - 8., row.bottom()));
        let pages = self
            .page_numbers()
            .map(|(start, end, total)| {
                if start == end {
                    format!("{start} / {total}")
                } else {
                    format!("{start}–{end} / {total}")
                }
            })
            .unwrap_or_else(|| "— / —".into());
        icons::label_at(
            ui,
            label,
            RichText::new(&pages).size(11.).color(p.muted),
            Sense::hover(),
        )
        .on_hover_text(format!("ページ: {pages}"));
        let progress = self.seek_drag.unwrap_or_else(|| self.reading_fraction());
        let mut position = if rtl { 1. - progress } else { progress };
        ui.spacing_mut().slider_width = rail.width();
        ui.spacing_mut().interact_size.y = READING_FOOTER_HEIGHT;
        ui.spacing_mut().slider_rail_height = 6.;
        let seek = region(ui, "seek-slider", rail, |ui| {
            ui.add_enabled(
                self.pagination_index.is_some(),
                egui::Slider::new(&mut position, 0.0..=1.0)
                    .show_value(false)
                    .trailing_fill(false)
                    .handle_shape(egui::style::HandleShape::Circle),
            )
        });
        let dragging_slider = self.seek_drag.is_some();
        let (pressed, down, released, pointer_click, pointer_origin, pointer_pos, pointer_drag) =
            ui.input(|input| {
                (
                    input.pointer.primary_pressed(),
                    input.pointer.primary_down(),
                    input.pointer.primary_released(),
                    input.pointer.primary_clicked(),
                    input.pointer.press_origin().or_else(|| {
                        input.events.iter().find_map(|event| {
                            if let egui::Event::PointerButton {
                                pos,
                                button: egui::PointerButton::Primary,
                                pressed: true,
                                ..
                            } = event
                            {
                                Some(*pos)
                            } else {
                                None
                            }
                        })
                    }),
                    input.pointer.interact_pos(),
                    input.pointer.is_decidedly_dragging(),
                )
            });
        let frame = ui.ctx().cumulative_frame_nr();
        let capture_id = Id::unique("seek-history-pointer");
        let mut capture = ui
            .ctx()
            .data(|data| data.get_temp::<(Locator, Option<u64>)>(capture_id));
        if capture
            .is_some_and(|(_, completed)| completed.is_some_and(|completed| completed < frame))
            || (!down && !released)
            || pointer_drag
        {
            capture = None;
        }
        let release_processed = capture.is_some_and(|(_, completed)| completed == Some(frame));
        let mut clicked = None;
        let mut release_hit = false;
        if let Some(index) = &self.pagination_index {
            for (order, mark) in self.jump_marks.iter().enumerate() {
                let Some(fraction) = index.locator_fraction(mark.locator) else {
                    continue;
                };
                let center = seek_rail_position(seek.rect, fraction, rtl);
                let target = Rect::from_center_size(center, vec2(16., seek.rect.height()));
                // Defer applying slider changes until the overlaid dot has had
                // a chance to consume this click. A drag begun on the rail keeps
                // working when it passes across a history mark.
                if !dragging_slider
                    && pressed
                    && !pointer_drag
                    && !release_processed
                    && pointer_origin.is_some_and(|origin| target.contains(origin))
                {
                    capture = Some((mark.locator, None));
                }
                release_hit |= capture.is_some_and(|(locator, _)| locator == mark.locator)
                    && pointer_pos.is_some_and(|position| target.contains(position));
                let response =
                    ui.interact(target, Id::unique(("jump-mark", order)), Sense::click());
                let (chapter, page) = index.locate(fraction);
                let global = index.offsets[chapter] + page + 1;
                let title = self
                    .book
                    .chapters
                    .get(chapter)
                    .map(|c| c.title.as_str())
                    .unwrap_or("");
                let tooltip = format!("戻る: {title} · {global} / {}ページ", index.total);
                response
                    .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &tooltip));
                let color = if response.hovered() {
                    p.accent
                } else {
                    p.accent.gamma_multiply(
                        0.55 + 0.45 * (order + 1) as f32 / self.jump_marks.len().max(1) as f32,
                    )
                };
                ui.painter().circle_filled(center, 3.5, color);
                if response.clicked() && !dragging_slider && !released {
                    clicked = Some(mark.locator);
                }
                response.on_hover_text(tooltip);
            }
        }
        if released
            && !release_processed
            && let Some((locator, completed)) = &mut capture
        {
            if pointer_click && release_hit {
                clicked = Some(*locator);
            }
            *completed = Some(frame);
        }
        ui.ctx().data_mut(|data| {
            if let Some(capture) = capture {
                data.insert_temp(capture_id, capture);
            } else {
                data.remove::<(Locator, Option<u64>)>(capture_id);
            }
        });
        if let Some(locator) = clicked {
            self.seek_drag = None;
            self.jump_to_locator(locator);
        } else if capture.is_none() {
            let progress = if rtl { 1. - position } else { position };
            if seek.changed() {
                self.seek_drag = Some(progress);
            }
            if seek.drag_stopped() || (seek.changed() && !seek.dragged()) {
                self.seek_book(progress);
                self.seek_drag = None;
            }
        }
        let status = if self.pagination_error.is_some() {
            Some(("ページ数を取得できません", Color32::from_rgb(190, 70, 60)))
        } else if self.pagination_pending {
            Some(("全体のページ数を計算中…", p.muted))
        } else {
            None
        };
        if let Some((text, color)) = status {
            if self.pagination_pending && self.pagination_error.is_none() {
                let phase = ui.input(|input| (input.time / 1.8).fract()) as f32;
                let x = seek.rect.left() + phase * (seek.rect.width() - 24.).max(0.);
                ui.painter().line_segment(
                    [
                        pos2(x, seek.rect.center().y),
                        pos2(x + 24., seek.rect.center().y),
                    ],
                    egui::Stroke::new(2., p.accent.gamma_multiply(0.55)),
                );
                ui.ctx().request_repaint_after(Duration::from_millis(80));
            }
            let galley = ui
                .painter()
                .layout_no_wrap(text.into(), FontId::proportional(11.), color);
            let status_rect =
                Rect::from_center_size(seek.rect.center(), galley.size() + vec2(16., 4.));
            ui.painter().rect_filled(status_rect, 0., p.paper);
            ui.painter()
                .galley(seek.rect.center() - galley.size() * 0.5, galley, color);
            let response = ui.interact(status_rect, Id::unique("seek-page-status"), Sense::hover());
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text));
            if let Some(error) = &self.pagination_error {
                response.on_hover_text(error);
            }
        }
        seek.on_hover_text(self.pagination_error.as_deref().unwrap_or(
            if self.pagination_pending {
                "全体のページ数を計算中…"
            } else {
                "読書位置（ページ）"
            },
        ));
    }

    fn reading_bar(&mut self, ui: &mut Ui, p: Palette) {
        let bounds = ui.available_rect_before_wrap();
        let rtl = self.next_to_left();
        let left = bounds.center().x - 310.;
        let y = ui.cursor().min.y;
        let group = |x: f32, width: f32| Rect::from_min_size(pos2(left + x, y), vec2(width, 40.));
        region(ui, "paging-group", group(0., 80.), |ui| {
            ui.spacing_mut().item_spacing.x = 8.;
            ui.horizontal_centered(|ui| {
                if icons::button(
                    ui,
                    Icon::Back,
                    if rtl {
                        "次のページ"
                    } else {
                        "前のページ"
                    },
                    false,
                    p.accent,
                )
                .clicked()
                {
                    self.turn_page(rtl);
                }
                if icons::button(
                    ui,
                    Icon::Next,
                    if rtl {
                        "前のページ"
                    } else {
                        "次のページ"
                    },
                    false,
                    p.accent,
                )
                .clicked()
                {
                    self.turn_page(!rtl);
                }
            });
        });
        region(ui, "playback-group", group(100., 128.), |ui| {
            ui.spacing_mut().item_spacing.x = 8.;
            ui.horizontal_centered(|ui| {
                if icons::button(
                    ui,
                    Icon::Point,
                    "読み上げ開始位置を選ぶ（本文の右クリックからも開始できます）",
                    self.point_tool,
                    p.accent,
                )
                .clicked()
                {
                    self.point_tool = !self.point_tool;
                    if self.point_tool {
                        self.prefetch_visible();
                    }
                }
                if icons::button(
                    ui,
                    if self.speaking {
                        Icon::Pause
                    } else {
                        Icon::Play
                    },
                    "読み上げを再生・一時停止",
                    self.speaking,
                    p.accent,
                )
                .clicked()
                {
                    self.toggle_speech();
                }
                if icons::button(ui, Icon::Stop, "読み上げを停止", false, p.accent).clicked()
                {
                    self.stop_speech();
                }
            });
        });
        region(ui, "voice-options", group(252., 80.), |ui| {
            ui.spacing_mut().item_spacing.x = 8.;
            ui.horizontal_centered(|ui| {
                if icons::button(
                    ui,
                    Icon::Follow,
                    "読み上げ位置への追従",
                    self.follow_playback && self.settings.tts.auto_follow,
                    p.accent,
                )
                .clicked()
                {
                    self.follow_playback = !self.follow_playback;
                    self.settings.tts.auto_follow = self.follow_playback;
                    self.save_settings();
                }
                if icons::button(ui, Icon::AudioSettings, "音声と声色の設定", false, p.accent)
                    .clicked()
                {
                    self.open_settings(SettingTab::Speech);
                }
            });
        });
        region(ui, "voice-and-speed", group(356., 264.), |ui| {
            ui.spacing_mut().item_spacing.x = 8.;
            ui.horizontal_centered(|ui| {
                let before = self.speech_provider.clone();
                egui::ComboBox::from_id_salt("voice-choice")
                    .selected_text(if self.speech_provider == "kokoro" {
                        format!("Kokoro · {}", self.speech_voice)
                    } else {
                        self.provider_config().name().to_owned()
                    })
                    .width(168.)
                    .show_ui(ui, |ui| {
                        for (id, label) in [
                            ("irodori", "Irodori"),
                            ("kokoro", "Kokoro"),
                            ("aivis", "AivisSpeech"),
                            ("aivis_native", "AivisSpeech · 組み込み"),
                            ("voiceroid2", "VOICEROID2"),
                            ("system", "システム音声"),
                            ("openai", "OpenAI互換"),
                            ("voicevox", "VOICEVOX"),
                        ] {
                            ui.selectable_value(&mut self.speech_provider, id.into(), label);
                        }
                    });
                if before != self.speech_provider {
                    self.finish_provider_change(&before);
                    self.save_settings();
                }
                let previous = self.settings.tts.speed;
                egui::ComboBox::from_id_salt("speed-choice")
                    .selected_text(format!("{:.2}×", self.settings.tts.speed))
                    .width(88.)
                    .show_ui(ui, |ui| {
                        for speed in [0.5, 0.75, 1., 1.25, 1.5, 2., 2.5, 3., 4., 5., 7.5, 10.] {
                            ui.selectable_value(
                                &mut self.settings.tts.speed,
                                speed,
                                format!("{speed}×"),
                            );
                        }
                    });
                if previous != self.settings.tts.speed {
                    self.save_settings();
                }
            });
        });
        for x in [90., 240., 344.] {
            ui.painter().line_segment(
                [pos2(left + x, y + 10.), pos2(left + x, y + 30.)],
                egui::Stroke::new(1., p.border),
            );
        }
        if !self.speech_progress.is_empty() && (self.speech_state == "準備中" || !self.speaking)
        {
            ui.painter().text(
                pos2(bounds.center().x, y + 46.),
                Align2::CENTER_CENTER,
                self.speech_progress.chars().take(80).collect::<String>(),
                FontId::proportional(10.),
                p.muted,
            );
        }
    }

    fn library(&mut self, ui: &mut Ui, p: Palette) {
        let bounds = ui.available_rect_before_wrap();
        let side = Rect::from_min_max(bounds.min, pos2(bounds.left() + 212., bounds.bottom()));
        ui.painter().rect_filled(side, 0., p.panel);
        let nav = side.shrink2(vec2(12., 10.));
        let footer =
            Rect::from_min_size(pos2(nav.left(), nav.bottom() - 40.), vec2(nav.width(), 40.));
        region(
            ui,
            "library-nav",
            Rect::from_min_max(nav.min, pos2(nav.right(), footer.top() - 12.)),
            |ui| {
                ui.spacing_mut().item_spacing = vec2(4., 4.);
                let (brand, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 60.), Sense::hover());
                icons::paint(
                    ui.painter(),
                    pos2(brand.left() + 18., brand.top() + 23.),
                    Icon::Book,
                    p.accent,
                );
                ui.painter().text(
                    pos2(brand.left() + 42., brand.top() + 9.),
                    Align2::LEFT_TOP,
                    "Voice Over",
                    FontId::proportional(18.),
                    p.ink,
                );
                ui.painter().text(
                    pos2(brand.left() + 42., brand.top() + 32.),
                    Align2::LEFT_TOP,
                    "Book Reader",
                    FontId::proportional(13.),
                    p.muted,
                );
                for (index, label) in ["すべての本", "読書中", "読了"].into_iter().enumerate()
                {
                    if icons::nav_row(
                        ui,
                        if index == 0 {
                            Icon::Library
                        } else {
                            Icon::Book
                        },
                        label,
                        ui.available_width(),
                        self.selected_shelf.is_none() && self.shelf_filter == index as u8,
                        p.accent,
                    )
                    .clicked()
                    {
                        self.selected_shelf = None;
                        self.shelf_filter = index as u8;
                    }
                }
                ui.add_space(18.);
                let (header, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 36.), Sense::hover());
                ui.painter().text(
                    pos2(header.left() + 12., header.center().y),
                    Align2::LEFT_CENTER,
                    "自分の本棚",
                    FontId::proportional(12.),
                    p.muted,
                );
                region(
                    ui,
                    "create-shelf-button",
                    Rect::from_min_size(pos2(header.right() - 36., header.top()), vec2(36., 36.)),
                    |ui| {
                        if icons::button(
                            ui,
                            Icon::Plus,
                            "本棚を作成",
                            self.new_shelf_open,
                            p.accent,
                        )
                        .clicked()
                        {
                            self.new_shelf_open = !self.new_shelf_open;
                        }
                    },
                );
                if self.new_shelf_open {
                    ui.horizontal(|ui| {
                        let edit = ui.add(
                            egui::TextEdit::singleline(&mut self.new_shelf_name)
                                .hint_text("本棚の名前")
                                .desired_width((ui.available_width() - 54.).max(70.)),
                        );
                        let enter =
                            edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if (ui.button("作成").clicked() || enter)
                            && !self.new_shelf_name.trim().is_empty()
                        {
                            let name = self.new_shelf_name.trim().to_owned();
                            self.settings
                                .custom_shelves
                                .entry(name.clone())
                                .or_default();
                            self.selected_shelf = Some(name);
                            self.new_shelf_name.clear();
                            self.new_shelf_open = false;
                            self.save_settings();
                        }
                    });
                }
                let tail = 124.;
                egui::ScrollArea::vertical()
                    .id_salt("custom-shelves")
                    .max_height((ui.available_height() - tail).max(42.))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        if self.settings.custom_shelves.is_empty() && !self.new_shelf_open {
                            ui.add_space(6.);
                            ui.add(
                                egui::Label::new(
                                    RichText::new("好きな本を集める本棚を\n＋から作成できます")
                                        .size(12.)
                                        .color(p.muted),
                                )
                                .halign(egui::Align::Min),
                            );
                            ui.add_space(10.);
                        }
                        for name in self
                            .settings
                            .custom_shelves
                            .keys()
                            .cloned()
                            .collect::<Vec<_>>()
                        {
                            let response = icons::nav_row(
                                ui,
                                Icon::Book,
                                &name,
                                ui.available_width(),
                                self.selected_shelf.as_ref() == Some(&name),
                                p.accent,
                            );
                            if response.clicked() {
                                self.selected_shelf = Some(name.clone());
                            }
                            response.context_menu(|ui| {
                                if ui.button("本棚を削除").clicked() {
                                    self.settings.custom_shelves.remove(&name);
                                    if self.selected_shelf.as_ref() == Some(&name) {
                                        self.selected_shelf = None;
                                    }
                                    self.save_settings();
                                    ui.close();
                                }
                            });
                        }
                    });
                ui.add_space(16.);
                ui.separator();
                ui.add_space(8.);
                if icons::nav_row(
                    ui,
                    Icon::Book,
                    "オンライン本棚",
                    ui.available_width(),
                    false,
                    p.accent,
                )
                .clicked()
                {
                    self.services.open_catalog();
                }
                if icons::nav_row(
                    ui,
                    Icon::Note,
                    "Web記事を取り込む",
                    ui.available_width(),
                    false,
                    p.accent,
                )
                .clicked()
                {
                    self.services.open_web_import();
                }
                if icons::nav_row(
                    ui,
                    Icon::Refresh,
                    "本棚を更新",
                    ui.available_width(),
                    false,
                    p.accent,
                )
                .clicked()
                {
                    self.refresh_shelf();
                }
            },
        );
        region(ui, "library-settings", footer, |ui| {
            if icons::nav_row(
                ui,
                Icon::Settings,
                "設定",
                ui.available_width(),
                false,
                p.accent,
            )
            .clicked()
            {
                self.open_settings(SettingTab::Appearance);
            }
        });
        let content = Rect::from_min_max(
            pos2(side.right() + 24., bounds.top() + 20.),
            pos2(bounds.right(), bounds.bottom()),
        );
        region(ui, "library-grid", content, |ui| {
            ui.horizontal(|ui| {
                ui.heading(self.selected_shelf.as_deref().unwrap_or("ライブラリ"));
                ui.add_space(16.);
                ui.add(
                    egui::TextEdit::singleline(&mut self.shelf_query)
                        .hint_text("タイトル・著者を検索")
                        .desired_width((ui.available_width() - 220.).clamp(140., 380.)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(14.);
                    if icons::button(
                        ui,
                        Icon::List,
                        "行リスト表示",
                        self.settings.library_list,
                        p.accent,
                    )
                    .clicked()
                    {
                        self.settings.library_list = true;
                        self.save_settings();
                    }
                    if icons::button(
                        ui,
                        Icon::Grid,
                        "表紙表示",
                        !self.settings.library_list,
                        p.accent,
                    )
                    .clicked()
                    {
                        self.settings.library_list = false;
                        self.save_settings();
                    }
                    if ui.button("本を追加").clicked() {
                        self.pick_book();
                    }
                });
            });
            let query = self.shelf_query.to_lowercase();
            let membership = self
                .selected_shelf
                .as_ref()
                .and_then(|name| self.settings.custom_shelves.get(name));
            let entries = self
                .shelf
                .iter()
                .enumerate()
                .filter(|(_, entry)| {
                    if membership.is_some_and(|ids| !ids.contains(&entry.id)) {
                        return false;
                    }
                    let found = query.is_empty()
                        || format!(
                            "{} {} {}",
                            entry.title,
                            entry.authors.join(" "),
                            entry.tags.join(" ")
                        )
                        .to_lowercase()
                        .contains(&query);
                    let progress = if self.shelf_filter == 0 || self.selected_shelf.is_some() {
                        0.
                    } else {
                        *self
                            .shelf_progress
                            .entry(entry.id.clone())
                            .or_insert_with(|| {
                                self.store
                                    .position(&entry.id)
                                    .ok()
                                    .flatten()
                                    .map(|p| p.progress)
                                    .unwrap_or(0.)
                            })
                    };
                    found
                        && (self.selected_shelf.is_some()
                            || match self.shelf_filter {
                                1 => progress > 0. && progress < 0.99,
                                2 => progress >= 0.99,
                                _ => true,
                            })
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            ui.label(
                RichText::new(format!("{}冊", entries.len()))
                    .small()
                    .color(p.muted),
            );
            ui.add_space(12.);
            if entries.is_empty() {
                ui.add_space(32.);
                ui.label("本を追加するか、本のメニューからこの本棚へ登録してください。");
                return;
            }
            let mut open = None;
            let columns = (ui.available_width() / 180.).floor().max(1.) as usize;
            let width = (ui.available_width() - 24. - 16. * columns.saturating_sub(1) as f32)
                / columns as f32;
            let list = self.settings.library_list;
            let row_height = if list { 64. } else { width * 1.35 + 76. };
            let rows = if list {
                entries.len()
            } else {
                entries.len().div_ceil(columns)
            };
            egui::ScrollArea::vertical()
                .id_salt(("books", self.selected_shelf.clone(), list))
                .auto_shrink([false, false])
                .animated(true)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                .show_rows(ui, row_height, rows, |ui, range| {
                    for index in range {
                        if list {
                            let entry = self.shelf[entries[index]].clone();
                            let (rect, response) = ui.allocate_exact_size(
                                vec2(ui.available_width() - 20., row_height),
                                Sense::click(),
                            );
                            if response.hovered() {
                                ui.painter().rect_filled(rect, 4., p.selection);
                            }
                            let cover =
                                Rect::from_min_size(rect.min + vec2(4., 4.), vec2(38., 54.));
                            self.paint_cover(ui, &entry, cover, p);
                            let author_width = (rect.width() * 0.24).clamp(80., 240.);
                            let text = Rect::from_min_max(
                                rect.min + vec2(58., 0.),
                                pos2(rect.right() - author_width - 16., rect.bottom()),
                            );
                            icons::label_at(ui, text, &entry.title, Sense::hover());
                            ui.painter().text(
                                pos2(rect.right() - author_width, rect.center().y),
                                Align2::LEFT_CENTER,
                                entry
                                    .authors
                                    .join("、")
                                    .chars()
                                    .take(24)
                                    .collect::<String>(),
                                FontId::proportional(13.),
                                p.muted,
                            );
                            ui.painter().line_segment(
                                [rect.left_bottom(), rect.right_bottom()],
                                egui::Stroke::new(1., p.border),
                            );
                            if response.clicked() {
                                open = Some(entry.epub_path.clone());
                            }
                            self.book_context_menu(response, &entry);
                        } else {
                            ui.horizontal_top(|ui| {
                                ui.spacing_mut().item_spacing.x = 16.;
                                for &entry_index in &entries
                                    [index * columns..((index + 1) * columns).min(entries.len())]
                                {
                                    let entry = self.shelf[entry_index].clone();
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 4.;
                                        ui.set_width(width);
                                        let (rect, response) = ui.allocate_exact_size(
                                            vec2(width, width * 1.35),
                                            Sense::click(),
                                        );
                                        self.paint_cover(ui, &entry, rect, p);
                                        if response.clicked() {
                                            open = Some(entry.epub_path.clone());
                                        }
                                        self.book_context_menu(response, &entry);
                                        ui.add_space(6.);
                                        let title = icons::label(
                                            ui,
                                            vec2(width, 24.),
                                            RichText::new(&entry.title).size(14.),
                                            Sense::click(),
                                        );
                                        if title.clicked() {
                                            open = Some(entry.epub_path.clone());
                                        }
                                        self.book_context_menu(title, &entry);
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(entry.authors.join("、"))
                                                    .small()
                                                    .color(p.muted),
                                            )
                                            .truncate(),
                                        );
                                        ui.add_space(12.);
                                    });
                                }
                            });
                        }
                    }
                });
            if let Some(path) = open {
                self.open_document(path);
            }
        });
    }
    fn paint_cover(&mut self, ui: &mut Ui, entry: &BookEntry, rect: Rect, p: Palette) {
        if let Some(texture) = self.cover_texture(ui.ctx(), entry) {
            let size = texture.size_vec2();
            let scale = (rect.width() / size.x).min(rect.height() / size.y);
            ui.painter().rect_filled(rect, 4., p.panel);
            ui.painter().image(
                texture.id(),
                Rect::from_center_size(rect.center(), size * scale),
                Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
                Color32::WHITE,
            );
        } else {
            ui.painter().rect_filled(rect, 4., p.panel);
            if rect.width() > 70. {
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    entry.title.chars().take(16).collect::<String>(),
                    FontId::proportional(16.),
                    p.muted,
                );
            }
        }
    }
    fn book_context_menu(&mut self, response: egui::Response, entry: &BookEntry) {
        response.context_menu(|ui| {
            if ui.button("別ウィンドウで開く").clicked() {
                self.open_document(entry.epub_path.clone());
                ui.close();
            }
            ui.separator();
            ui.label("自分の本棚");
            if self.settings.custom_shelves.is_empty() {
                ui.label("左の＋から本棚を作成できます。");
            }
            for name in self
                .settings
                .custom_shelves
                .keys()
                .cloned()
                .collect::<Vec<_>>()
            {
                let mut included = self.settings.custom_shelves[&name].contains(&entry.id);
                if ui.checkbox(&mut included, &name).changed() {
                    let ids = self.settings.custom_shelves.get_mut(&name).unwrap();
                    if included {
                        ids.push(entry.id.clone());
                    } else {
                        ids.retain(|id| id != &entry.id);
                    }
                    self.save_settings();
                }
            }
        });
    }

    fn cover_texture(
        &mut self,
        ctx: &egui::Context,
        entry: &BookEntry,
    ) -> Option<egui::TextureHandle> {
        self.covers.get(ctx, entry.cover_path.as_deref()?)
    }
}

impl ReaderApp {
    fn reader(&mut self, ui: &mut Ui, p: Palette) {
        if self.cef_reader.is_some() && self.book.epub.is_some() {
            self.chromium_reader(ui, p);
            return;
        }
        let rect = ui.available_rect_before_wrap();
        self.reading_extent = rect.size();
        let interaction_rect = Rect::from_min_max(
            rect.min,
            pos2(
                rect.right(),
                (rect.bottom() - READING_FOOTER_HEIGHT).max(rect.top()),
            ),
        );
        let response = ui.interact(
            interaction_rect,
            Id::unique("book-page"),
            Sense::click_and_drag(),
        );
        if response.hovered() && self.wheel_capture_frame != ui.ctx().cumulative_frame_nr() {
            self.wheel_capture_frame = ui.ctx().cumulative_frame_nr();
            let wheel = ui.input(|i| {
                i.events
                    .iter()
                    .filter_map(|event| {
                        if let egui::Event::MouseWheel {
                            unit,
                            delta,
                            modifiers,
                            ..
                        } = event
                        {
                            if modifiers.command {
                                None
                            } else {
                                Some((*unit, *delta))
                            }
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
            });
            self.queue_wheel_pages(&wheel);
        }
        if self.opening {
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                "本を読み込んでいます",
                FontId::proportional(15.),
                p.muted,
            );
            return;
        }
        if let Some(error) = &self.open_error {
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                error,
                FontId::proportional(15.),
                p.ink,
            );
            return;
        }
        if self.book.chapters.is_empty() {
            return;
        }
        let rect = crate::reading_surface::content_rect(
            rect,
            &self.book.chapters[self.chapter],
            &self.settings,
        );
        if self.layout_key.is_empty() {
            let normalized = crate::reading_surface::chapter_spread_start(
                &self.book,
                self.chapter,
                rect.size(),
                &self.settings,
            );
            if normalized != self.chapter
                && (self.book.chapters[self.chapter].presentation.layout
                    == Some(reader_core::RenditionLayout::PrePaginated)
                    || self.book.chapters[self.chapter].presentation.placement
                        != reader_core::PagePlacement::Auto)
            {
                self.chapter = normalized;
                self.page = 0;
            }
        }
        if !self.book.chapters[self.chapter].loaded {
            self.request_chapter();
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                "章を読み込んでいます",
                FontId::proportional(14.),
                p.muted,
            );
            return;
        }
        if let Some(diagnostic) =
            layout::epub_fixed_layout_diagnostic(&self.book.chapters[self.chapter])
        {
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                diagnostic,
                FontId::proportional(14.),
                p.muted,
            );
            // Keep a source-backed page so wheel/arrow navigation can leave it.
            self.layout = BookLayout {
                pages: vec![layout::Page {
                    anchor: self.book.chapters[self.chapter].start_locator(),
                    ..Default::default()
                }],
            };
            self.layout_key = format!("unsupported-fixed:{}:{}", self.book.id, self.chapter);
            self.spread_pages = 1;
            self.cross_chapter_spread = false;
            self.spread_plan = Default::default();
            return;
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
        let extent = plan.extent(rect.size());
        let key = format!(
            "{}:{}:{:.0}:{:.0}:{:?}:{:.1}:{:.2}:{:.2}:{:.0}:{}:{}:{:.2}:{:?}:{:?}",
            self.book.id,
            self.chapter,
            rect.width(),
            rect.height(),
            self.settings.reading_mode,
            self.settings.font_size,
            self.settings.line_spacing,
            self.settings.letter_spacing,
            self.settings.margins,
            self.settings.show_ruby,
            self.settings.font_family,
            self.settings.ruby_scale,
            self.settings.theme,
            self.settings.accent
        );
        let key = format!(
            "{key}:{}:{:?}:{:?}:{:?}:css:{}",
            self.spread_pages,
            plan,
            self.book.chapters[self.chapter].presentation,
            (
                rect.size(),
                self.settings.font_size,
                self.settings.line_spacing,
                self.settings.letter_spacing,
                (
                    self.settings.horizontal_margin(),
                    self.settings.vertical_margin(),
                    self.settings.page_gap,
                    self.settings.custom_foreground
                ),
                self.settings.ruby_scale
            ),
            reader_core::default_reader_css_revision()
        );
        if key != self.layout_key {
            let _layout = crate::latency::span("visible_chapter_layout", self.open_generation);
            self.layout_revision = self.layout_revision.wrapping_add(1);
            self.composed_frame = None;
            let anchor = self
                .pending_restore
                .take()
                .or_else(|| self.selected.as_ref().map(|h| h.locator))
                .or_else(|| {
                    self.layout
                        .pages
                        .get(self.page)
                        .and_then(|p| p.first_locator())
                });
            self.layout = layout::chapter_layout(
                &mut self.fonts,
                &self.book.chapters[self.chapter],
                extent,
                &crate::reading_surface::effective_layout_settings(
                    &self.settings,
                    &self.book.chapters[self.chapter],
                ),
            );
            self.layout_key = key;
            self.page = if self.page == usize::MAX {
                self.layout.pages.len() - 1
            } else {
                self.page.min(self.layout.pages.len() - 1)
            };
            if let Some(locator) = anchor.filter(|l| l.chapter as usize == self.chapter)
                && let Some(page) = self
                    .layout
                    .pages
                    .iter()
                    .position(|p| p.contains_locator(locator))
            {
                self.page = page;
            }
            if let Some(page) = self.pending_page_index.take() {
                self.page = page.min(self.layout.pages.len().saturating_sub(1));
            }
            if let Some(fraction) = self.pending_page_fraction.take() {
                self.page = ((self.layout.pages.len().saturating_sub(1) as f32) * fraction).round()
                    as usize;
            }
            self.page = crate::reading_surface::page_window(
                &self.book.chapters[self.chapter],
                self.page,
                self.layout.pages.len(),
                self.spread_pages,
                self.next_to_left(),
            )
            .start;
        }
        self.cross_chapter_spread &= self.layout.pages.len() == 1;
        let sibling_key = format!("{}:{:?}", self.layout_key, plan.second);
        if self.cross_chapter_spread {
            if sibling_key != self.sibling_key
                && let Some(next) = plan
                    .second
                    .and_then(|i| self.book.chapters.get(i))
                    .filter(|c| c.loaded)
            {
                self.sibling_layout = layout::chapter_layout(
                    &mut self.fonts,
                    next,
                    extent,
                    &crate::reading_surface::effective_layout_settings(&self.settings, next),
                );
                self.sibling_key = sibling_key.clone();
                self.layout_revision = self.layout_revision.wrapping_add(1);
                self.composed_frame = None;
            }
            if self.sibling_key == sibling_key && self.sibling_layout.pages.len() != 1 {
                self.cross_chapter_spread = false;
            }
        }
        let window = crate::reading_surface::page_window(
            &self.book.chapters[self.chapter],
            self.page,
            self.layout.pages.len(),
            self.spread_pages,
            self.next_to_left(),
        );
        let first_slot = if self.cross_chapter_spread {
            plan.first_slot
        } else {
            window.first_slot
        };
        let loaded_neighbors = (self.chapter + 1
            ..(self.chapter + 1 + self.spread_pages * 2).min(self.book.chapters.len()))
            .enumerate()
            .fold(0u8, |mask, (offset, index)| {
                mask | (u8::from(self.book.chapters[index].loaded) << offset)
            });
        let frame_key = ComposedFrameKey {
            revision: self.layout_revision,
            page: self.page,
            extent: [rect.width().to_bits(), rect.height().to_bits()],
            columns: self.spread_pages,
            first_slot,
            gap: plan.gap.to_bits(),
            cross_chapter: self.cross_chapter_spread,
            sibling_ready: self.sibling_key == sibling_key,
            loaded_neighbors,
        };
        if self
            .composed_frame
            .as_ref()
            .is_none_or(|frame| frame.key != frame_key)
        {
            let second = if self.cross_chapter_spread {
                if self.sibling_key == sibling_key {
                    self.sibling_layout.pages.first().cloned()
                } else {
                    None
                }
            } else if window.end > self.page + 1 {
                self.layout.pages.get(self.page + 1).cloned()
            } else {
                None
            };
            let page = crate::reading_surface::compose(
                self.layout
                    .pages
                    .get(self.page)
                    .cloned()
                    .unwrap_or_default(),
                second,
                rect.size(),
                self.spread_pages,
                first_slot,
                plan.gap,
            );
            let links = std::iter::once(self.chapter)
                .chain(if self.cross_chapter_spread {
                    plan.second
                } else {
                    None
                })
                .filter_map(|i| self.book.chapters.get(i))
                .flat_map(|chapter| {
                    chapter
                        .atoms()
                        .into_iter()
                        .enumerate()
                        .filter(|(_, atom)| atom.link.is_some())
                        .map(|(index, _)| layout::unit_id(chapter.id, index))
                        .collect::<Vec<_>>()
                })
                .collect();
            let mut prefetch = vec![(
                self.chapter,
                self.layout
                    .pages
                    .iter()
                    .skip(self.page + self.spread_pages)
                    .take(self.spread_pages * 2)
                    .chain(
                        self.layout
                            .pages
                            .iter()
                            .skip(self.page.saturating_sub(self.spread_pages))
                            .take(self.spread_pages),
                    )
                    .flat_map(|page| page.pictures.iter())
                    .take(8)
                    .cloned()
                    .collect(),
            )];
            for index in self.chapter + 1
                ..(self.chapter + 1 + self.spread_pages * 2).min(self.book.chapters.len())
            {
                let chapter = &self.book.chapters[index];
                if chapter.loaded && crate::reading_surface::is_image_chapter(chapter) {
                    let content = crate::reading_surface::content_rect(
                        Rect::from_min_size(Pos2::ZERO, self.reading_extent),
                        chapter,
                        &self.settings,
                    );
                    let next_plan = crate::reading_surface::chapter_spread(
                        &self.book,
                        index,
                        content.size(),
                        &self.settings,
                    );
                    let next = layout::chapter_layout(
                        &mut self.fonts,
                        chapter,
                        next_plan.extent(content.size()),
                        &crate::reading_surface::effective_layout_settings(&self.settings, chapter),
                    );
                    prefetch.push((
                        index,
                        next.pages
                            .iter()
                            .take(self.spread_pages)
                            .flat_map(|page| page.pictures.iter())
                            .take(4)
                            .cloned()
                            .collect(),
                    ));
                }
            }
            self.composed_frame = Some(ComposedFrame {
                key: frame_key,
                page: Arc::new(page),
                links: Arc::new(links),
                prefetch: Arc::new(prefetch),
            });
        }
        let frame = self.composed_frame.as_ref().unwrap();
        let page = Arc::clone(&frame.page);
        let linked_units = Arc::clone(&frame.links);
        let prefetch = Arc::clone(&frame.prefetch);
        let painter = ui.painter_at(rect);
        for fill in &page.header_fills {
            painter.rect_filled(fill.translate(rect.min.to_vec2()), 0., p.panel);
        }
        for picture in &page.pictures {
            let target = picture.rect.translate(rect.min.to_vec2());
            let physical_slot = usize::from(picture.rect.center().x >= rect.width() * 0.5);
            let image_chapter = if self.cross_chapter_spread && physical_slot != plan.first_slot {
                plan.second.unwrap_or(self.chapter)
            } else {
                self.chapter
            };
            if let Some(texture) = self.picture_texture_for(ui.ctx(), picture, image_chapter) {
                let size = texture.size_vec2();
                let scale = (target.width() / size.x).min(target.height() / size.y);
                let dst = Rect::from_center_size(target.center(), size * scale);
                painter.image(
                    texture.id(),
                    dst,
                    Rect::from_min_max(pos2(0., 0.), pos2(1., 1.)),
                    Color32::WHITE,
                );
            } else {
                painter.text(
                    target.center(),
                    Align2::CENTER_CENTER,
                    &picture.alt,
                    FontId::proportional(18.),
                    p.muted,
                );
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
        let annotations = Arc::clone(&self.annotations);
        for hit in &page.hits {
            let selected = self
                .selected
                .as_ref()
                .is_some_and(|s| hit.locator >= s.locator && hit.locator < s.end);
            let speaking = self
                .spoken_range
                .is_some_and(|(a, b)| hit.locator >= a && hit.locator < b);
            let noted = annotations
                .iter()
                .any(|a| hit.locator >= a.start && hit.locator < a.end);
            if selected || speaking || noted {
                painter.rect_filled(
                    hit.rect.translate(rect.min.to_vec2()).expand(2.),
                    3.,
                    p.selection,
                );
            }
        }
        for hit in &page.hits {
            if linked_units.contains(&hit.unit_id) {
                let target = hit.rect.translate(rect.min.to_vec2());
                let ends = if self.book.chapters[self.chapter].writing_mode.is_vertical() {
                    [target.right_top(), target.right_bottom()]
                } else {
                    [target.left_bottom(), target.right_bottom()]
                };
                painter.line_segment(ends, egui::Stroke::new(1., p.accent));
            }
        }
        self.fonts
            .draw_batch(&painter, &page.glyphs, rect.min.to_vec2(), p.ink);
        for rule in &page.rules {
            painter.rect_stroke(
                rule.translate(rect.min.to_vec2()),
                0.,
                egui::Stroke::new(1., p.border),
                egui::StrokeKind::Inside,
            );
        }
        self.ensure_pagination_index();
        self.prefetch_next_chapter();
        self.maybe_prefetch_audio(ui.ctx());
        for (chapter, pictures) in prefetch.iter() {
            crate::book_images::prefetch(
                ui.ctx(),
                &self.book,
                &self.book.chapters[*chapter].href,
                pictures,
                &mut self.pictures,
                self.settings.adapt_monochrome_images.then_some(p.ink),
            );
        }

        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Label,
                true,
                page.hits
                    .iter()
                    .map(|hit| hit.text.as_str())
                    .collect::<String>(),
            )
        });
        if let Some(pos) = response.interact_pointer_pos() {
            let local = pos2(pos.x - rect.left(), pos.y - rect.top());
            let hit = page.hit(local).cloned();
            if hit
                .as_ref()
                .is_some_and(|h| linked_units.contains(&h.unit_id))
            {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.double_clicked() && !self.point_tool {
                self.pending_center_tap = None;
                self.pending_page_turn = None;
                if let Some(hit) = hit {
                    let hit = layout::expand_word(&self.book.chapters[self.chapter], &hit);
                    let query =
                        reader_core::word_at(&self.book.chapters[self.chapter], hit.locator)
                            .ok()
                            .flatten()
                            .and_then(|word| word.lemma)
                            .unwrap_or_else(|| hit.text.clone());
                    self.selected = Some(hit);
                    self.lookup_dictionary(query, pos);
                }
            } else if response.clicked() {
                if self.point_tool {
                    if let Some(hit) = hit {
                        let hit = layout::expand_word(&self.book.chapters[self.chapter], &hit);
                        self.selected = Some(hit.clone());
                        self.start_speech(Some(hit));
                        self.point_tool = false;
                    }
                } else {
                    if self.dictionary_visible && !self.dictionary_pinned {
                        self.hide_dictionary();
                    }
                    self.dictionary_popup = None;
                    self.footnote = None;
                    self.selected = None;
                    if !hit
                        .as_ref()
                        .is_some_and(|hit| self.follow_link(hit, pos, ui.ctx()))
                    {
                        if local.x < rect.width() * 0.30 || local.x > rect.width() * 0.70 {
                            self.turn_page((local.x < rect.width() * 0.5) == self.next_to_left());
                            self.pending_center_tap = None;
                        } else {
                            self.pending_center_tap = Some(Instant::now());
                            self.pending_page_turn = None;
                        }
                    }
                }
            } else if response.dragged() {
                self.pending_center_tap = None;
                self.pending_page_turn = None;
                if response.drag_started() {
                    let origin =
                        ui.input(|i| i.pointer.press_origin()).unwrap_or(pos) - rect.min.to_vec2();
                    if page.hit(origin).is_none()
                        && !page.pictures.iter().any(|p| p.rect.contains(origin))
                    {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
                }
                if self.selection_anchor.is_none() {
                    let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
                    self.selection_anchor = page
                        .hit(pos2(origin.x - rect.left(), origin.y - rect.top()))
                        .cloned();
                }
                if let (Some(anchor), Some(hit)) = (&self.selection_anchor, hit) {
                    let (start, end) = if anchor.locator <= hit.locator {
                        (anchor, &hit)
                    } else {
                        (&hit, anchor)
                    };
                    let mut selected = start.clone();
                    selected.end = end.end;
                    selected.text = page
                        .hits
                        .iter()
                        .filter(|h| h.locator >= selected.locator && h.locator < selected.end)
                        .map(|h| h.text.as_str())
                        .collect();
                    self.selected = Some(selected);
                }
            } else if response.secondary_clicked() {
                self.pending_center_tap = None;
                if let Some(hit) = hit
                    && !self
                        .selected
                        .as_ref()
                        .is_some_and(|s| hit.locator >= s.locator && hit.locator < s.end)
                {
                    self.selected =
                        Some(layout::expand_word(&self.book.chapters[self.chapter], &hit));
                }
            }
        }
        if response.drag_stopped() {
            self.selection_anchor = None;
        }
        response.context_menu(|ui| {
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
                if let Some(hit) = &self.selected {
                    ui.ctx().copy_text(hit.text.clone());
                }
                ui.close();
            }
        });
    }
    fn queue_wheel_pages(&mut self, events: &[(egui::MouseWheelUnit, egui::Vec2)]) {
        if events.is_empty() {
            return;
        }
        if self.last_wheel.elapsed() > Duration::from_millis(200) {
            self.wheel_accumulator = 0.;
        }
        self.last_wheel = Instant::now();
        for (unit, delta) in events {
            let distance = if delta.y.abs() >= delta.x.abs() {
                -delta.y
            } else if self.next_to_left() {
                delta.x
            } else {
                -delta.x
            };
            if !distance.is_finite() {
                continue;
            }
            self.wheel_accumulator += match unit {
                egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => distance,
                egui::MouseWheelUnit::Point => distance / 80.,
            };
            let count = self.wheel_accumulator.abs().floor().min(1024.) as usize;
            let forward = self.wheel_accumulator > 0.;
            self.wheel_accumulator -= count as f32 * self.wheel_accumulator.signum();
            self.pending_wheel_turns
                .extend(std::iter::repeat_n(forward, count));
        }
    }
    pub(crate) fn picture_texture_for(
        &mut self,
        ctx: &egui::Context,
        picture: &layout::Picture,
        chapter: usize,
    ) -> Option<egui::TextureHandle> {
        match crate::book_images::load_texture_themed(
            ctx,
            &self.book,
            &self.book.chapters[chapter].href,
            picture,
            &mut self.pictures,
            self.settings
                .adapt_monochrome_images
                .then(|| Palette::for_settings(&self.settings).ink),
        ) {
            Ok(texture) => texture,
            Err(error) => {
                self.notify(format!("画像の描画: {error}"));
                None
            }
        }
    }
    fn lookup_dictionary(&mut self, query: String, anchor: Pos2) {
        let query = query.trim().to_owned();
        if query.is_empty() {
            return;
        }
        let helper = std::env::current_exe().ok().and_then(|p| {
            p.parent().map(|p| {
                p.join(if cfg!(windows) {
                    "reader-dictionary-popup.exe"
                } else {
                    "reader-dictionary-popup"
                })
            })
        });
        if let Some(helper) = helper.filter(|p| p.is_file()) {
            let screen = (self.screen_origin + anchor.to_vec2()) * self.pixels_per_point;
            let palette = Palette::for_settings(&self.settings);
            let theme = if palette.is_dark() {
                "dark"
            } else if self.settings.theme == Theme::Sepia {
                "sepia"
            } else {
                "light"
            };
            let paper = format!(
                "{:02X}{:02X}{:02X}",
                palette.paper.r(),
                palette.paper.g(),
                palette.paper.b()
            );
            let ink = format!(
                "{:02X}{:02X}{:02X}",
                palette.ink.r(),
                palette.ink.g(),
                palette.ink.b()
            );
            let accent = format!(
                "{:02X}{:02X}{:02X}",
                self.settings.accent[0], self.settings.accent[1], self.settings.accent[2]
            );
            let message = serde_json::json!({"type":"lookup","query":query,"x":screen.x as i32,"y":screen.y as i32,"theme":theme,"accent":accent,"paper":paper,"ink":ink});
            let mut reused = false;
            if let Some((child, input)) = &mut self.dictionary_child
                && child.try_wait().ok().flatten().is_none()
            {
                reused = writeln!(input, "{message}").is_ok();
            }
            if reused {
                self.dictionary_visible = true;
                return;
            }
            let mut command = Command::new(helper);
            command
                .arg("--data-dir")
                .arg(self.store.data_dir())
                .arg("--query")
                .arg(&query)
                .arg("--x")
                .arg((screen.x as i32).to_string())
                .arg("--y")
                .arg((screen.y as i32).to_string())
                .arg("--theme")
                .arg(theme)
                .arg("--accent")
                .arg(accent)
                .arg("--paper")
                .arg(paper)
                .arg("--ink")
                .arg(ink)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped());
            if let Some(hwnd) = self.parent_hwnd {
                command.arg("--parent-hwnd").arg(hwnd.to_string());
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000);
            }
            if let Ok(mut child) = command.spawn()
                && let Some(input) = child.stdin.take()
            {
                self.dictionary_epoch = self.dictionary_epoch.wrapping_add(1);
                let epoch = self.dictionary_epoch;
                if let Some(output) = child.stdout.take() {
                    let tx = self.tx.clone();
                    std::thread::spawn(move || {
                        for line in BufReader::new(output).lines().map_while(Result::ok) {
                            if let Ok(data) = serde_json::from_str(&line) {
                                let _ = tx.send(Work::DictionaryState { epoch, data });
                            }
                        }
                    });
                }
                self.dictionary_child = Some((child, input));
                self.dictionary_visible = true;
                return;
            }
        }
        self.dictionary_popup = Some(DictionaryPopup {
            query: query.clone(),
            hits: Vec::new(),
            anchor,
        });
        let dir = self.store.data_dir().join("dictionaries");
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let hits = (|| -> anyhow::Result<Vec<DictionaryHit>> {
                let mut c = DictionaryCatalog::open(dir)?;
                c.lookup(&query, 20)
            })()
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Work::Lookup { query, hits });
        });
    }
    fn popup(&mut self, ctx: &egui::Context) {
        let Some(popup) = &self.dictionary_popup else {
            return;
        };
        let query = popup.query.clone();
        let anchor = popup.anchor;
        let hits = popup.hits.clone();
        let mut open = true;
        egui::Window::new(format!("辞書 · {query}"))
            .id(Id::unique("dictionary-fallback"))
            .default_pos(anchor + vec2(12., 16.))
            .default_width(360.)
            .resizable(true)
            .collapsible(false)
            .open(&mut open)
            .show(ctx, |ui| {
                if hits.is_empty() {
                    ui.label("該当する語がありません。設定からMDX辞書を追加できます。");
                }
                for hit in &hits {
                    ui.label(RichText::new(&hit.dictionary_title).small());
                    ui.heading(&hit.headword);
                    ui.label(strip_tags(&hit.html));
                    ui.separator();
                }
            });
        if !open {
            self.dictionary_popup = None;
        }
    }
    pub(crate) fn set_voiceroid2_profile(&mut self, config: ProviderConfig) {
        self.speech_profiles.set("voiceroid2", config);
    }
    pub(crate) fn set_aivis_runtime(&mut self, engine_path: Option<PathBuf>, auto_start: bool) {
        let mut config = self.provider_config();
        if let ProviderConfig::AivisSpeech {
            engine_path: stored,
            auto_start: enabled,
            ..
        } = &mut config
        {
            *stored = engine_path;
            *enabled = auto_start;
        }
        self.speech_profiles.set("aivis", config);
        self.sync_speech_settings();
    }
    pub(crate) fn set_kokoro_profile(&mut self, config: ProviderConfig) {
        if matches!(config, ProviderConfig::Kokoro { .. }) {
            self.speech_profiles.set("kokoro", config);
            self.sync_speech_settings();
        }
    }
    pub(crate) fn provider_config(&self) -> ProviderConfig {
        match self.speech_provider.as_str() {
            "aivis_native" => self.speech_profiles.get("aivis_native"),
            "irodori" => {
                let mut config = self.speech_profiles.get("irodori");
                if let ProviderConfig::Irodori {
                    reference_audio,
                    voice_instruction,
                    seed,
                    ..
                } = &mut config
                {
                    *reference_audio = self.speech_reference.clone();
                    *voice_instruction = self.speech_instruction.clone();
                    *seed = self.speech_seed;
                }
                config
            }
            "kokoro" => {
                let mut config = self.speech_profiles.get("kokoro");
                if let ProviderConfig::Kokoro { voice, .. } = &mut config {
                    *voice = self.speech_voice.clone();
                }
                config
            }
            "aivis" => {
                let mut config = self.speech_profiles.get("aivis");
                if let ProviderConfig::AivisSpeech {
                    base_url, speaker, ..
                } = &mut config
                {
                    *base_url = self.speech_endpoint.clone();
                    *speaker = self.speech_voice.parse().unwrap_or(0);
                }
                config
            }
            "voiceroid2" => {
                let mut config = self.speech_profiles.get("voiceroid2");
                if let ProviderConfig::Voiceroid2 { voice, .. } = &mut config {
                    *voice = self.speech_voice.clone();
                }
                config
            }
            "system" => ProviderConfig::System {
                voice: (!self.speech_voice.is_empty()).then(|| self.speech_voice.clone()),
            },
            "openai" => ProviderConfig::OpenAi {
                base_url: self.speech_endpoint.clone(),
                api_key: self.speech_api_key.clone(),
                model: self.speech_model.clone(),
                voice: if self.speech_voice.is_empty() {
                    "jf_alpha".into()
                } else {
                    self.speech_voice.clone()
                },
            },
            "voicevox" => ProviderConfig::Voicevox {
                base_url: self.speech_endpoint.clone(),
                speaker: self.speech_voice.parse().unwrap_or(0),
            },
            "command" => ProviderConfig::Command {
                program: PathBuf::from(&self.speech_command),
                args: serde_json::from_str(&self.speech_command_args).unwrap_or_default(),
            },
            _ => self.speech_profiles.get("irodori"),
        }
    }
    fn start_speech(&mut self, hit: Option<Hit>) {
        let _request = crate::latency::span("speech_request_prepare", self.open_generation);
        self.voice_preview = false;
        self.speech_error = None;
        self.audiobook.stop();
        if !self.ensure_speech() {
            return;
        }
        if !self
            .book
            .chapters
            .get(self.chapter)
            .is_some_and(|c| c.loaded)
        {
            self.request_chapter();
            return;
        }
        if let Some(speech) = &self.speech {
            let _ = speech.set_speed(self.settings.tts.speed);
        }
        self.speech_sources.clear();
        let hit = hit.or_else(|| self.speech_start_hit());
        let chapter = &self.book.chapters[self.chapter];
        let ChapterNarration {
            units,
            start_index,
            byte_offset,
        } = chapter_narration(chapter, &self.settings.tts.pronunciations, hit.as_ref());
        let atoms = chapter.atoms();
        for unit in &units {
            let index = (unit.id & 0xFFFF_FFFF) as usize;
            self.speech_sources.insert(unit.id, atoms[index].clone());
        }
        if units.is_empty() {
            self.notify("読み上げる本文がありません");
            return;
        }
        match self
            .speech
            .as_ref()
            .unwrap()
            .speak_at(units, start_index, byte_offset)
        {
            Ok(generation) => {
                self.speech_generation = generation;
                self.selected = None;
                self.speech_state = "準備中".into();
                self.speaking = true;
                self.follow_playback = self.settings.tts.auto_follow;
                self.feed_remaining_chapters(generation, self.chapter + 1);
            }
            Err(e) => self.notify(e.to_string()),
        }
    }
    fn toggle_speech(&mut self) {
        if self.speech_state == "一時停止" {
            if let Some(speech) = &self.speech {
                speech.resume();
            }
        } else if self.speaking {
            if let Some(speech) = &self.speech {
                speech.pause();
            }
        } else {
            self.start_speech(self.selected.clone());
        }
    }
    fn stop_speech(&mut self) {
        if let Some(speech) = &self.speech {
            speech.stop();
            self.speech_generation = speech.generation();
        }
        self.speaking = false;
        self.spoken_range = None;
        self.speech_state.clear();
    }
    fn poll_speech(&mut self) {
        loop {
            let event = self.speech.as_ref().and_then(|s| s.try_recv());
            let Some(event) = event else {
                break;
            };
            if event.generation != self.speech_generation {
                continue;
            }
            match event.kind {
                SpeechEventKind::State(state) => {
                    self.speaking = matches!(state, SpeechState::Playing | SpeechState::Preparing);
                    self.speech_state = match state {
                        SpeechState::Preparing => "準備中",
                        SpeechState::Playing => "再生中",
                        SpeechState::Paused => "一時停止",
                        _ => "",
                    }
                    .into();
                    if matches!(state, SpeechState::Finished | SpeechState::Stopped) {
                        self.spoken_range = None;
                        if self.voice_preview {
                            self.voice_preview = false;
                            if let Some(speech) = &self.speech {
                                let _ = speech.set_speed(self.settings.tts.speed);
                            }
                        }
                    }
                }
                SpeechEventKind::Boundary { ranges, .. } => {
                    let mapped: Vec<_> = ranges
                        .iter()
                        .filter_map(|range| {
                            let atom = self.speech_sources.get(&range.id)?;
                            let offsets = if atom.ruby.is_some() {
                                (0, atom.text.len())
                            } else {
                                (range.start, range.end)
                            };
                            Some((atom.locator_at(offsets.0)?, atom.locator_at(offsets.1)?))
                        })
                        .collect();
                    if let (Some(first), Some(last)) = (mapped.first(), mapped.last()) {
                        let (start, end) = (first.0, last.1);
                        let chromium = self.cef_reader.is_some() && self.book.epub.is_some();
                        let before = [self.chapter, self.page];
                        self.apply_spoken_range(start, end, chromium);
                        if let Some(driver) = self.speech_smoke.as_mut() {
                            driver.boundary(start, before, [self.chapter, self.page]);
                        }
                    }
                }
                SpeechEventKind::Progress(message) => self.speech_progress = message,
                SpeechEventKind::Error(error) => {
                    self.speaking = false;
                    self.speech_error = Some(error.clone());
                    self.notify(error);
                }
            }
        }
    }
    fn apply_spoken_range(&mut self, start: Locator, end: Locator, chromium: bool) {
        self.resume_locator = Some(start);
        self.spoken_range = Some((start, end));
        if !(self.settings.tts.auto_follow && self.follow_playback) {
            return;
        }
        if self.chapter != start.chapter as usize {
            self.persist_position();
            self.chapter = start.chapter as usize;
            self.page = 0;
            self.layout = BookLayout::default();
            self.composed_frame = None;
            self.layout_key.clear();
            self.pending_restore = Some(start);
        }
        // Chromium's compatibility layout contains one start anchor per page,
        // not native hit geometry. Only its correlated DOM result may move it.
        if chromium {
            if self.layout_key.is_empty() && self.pending_restore.is_some() {
                self.pending_restore = Some(start);
            }
            return;
        }
        if let Some(page) = self
            .layout
            .pages
            .iter()
            .position(|p| p.hits.iter().any(|h| h.locator >= start && h.locator < end))
        {
            self.page = crate::reading_surface::page_window(
                &self.book.chapters[self.chapter],
                page,
                self.layout.pages.len(),
                self.spread_pages,
                self.next_to_left(),
            )
            .start;
        }
    }
    fn shortcuts(&mut self, ctx: &egui::Context) {
        if let Some(action_id) = self.binding_capture.clone() {
            if let Some(chord) = ctx.input(|i| {
                i.events.iter().find_map(|event| {
                    if let egui::Event::Key {
                        key,
                        pressed: true,
                        repeat: false,
                        modifiers,
                        ..
                    } = event
                    {
                        Some(crate::actions::chord(*key, *modifiers))
                    } else {
                        None
                    }
                })
            }) {
                for action in crate::actions::Action::ALL {
                    if action.id() != action_id
                        && crate::actions::binding(*action, &self.settings.key_bindings) == chord
                    {
                        self.settings
                            .key_bindings
                            .insert(action.id().into(), String::new());
                    }
                }
                let conflicts = self
                    .settings
                    .key_bindings
                    .iter()
                    .filter(|(id, value)| **id != action_id && **value == chord)
                    .map(|(id, _)| id.clone())
                    .collect::<Vec<_>>();
                for id in conflicts {
                    self.settings.key_bindings.insert(id, String::new());
                }
                self.settings.key_bindings.insert(action_id, chord);
                self.binding_capture = None;
                self.save_settings();
            }
            return;
        }
        let book_has_focus = self.screen == Screen::Reader
            && ctx.memory(|memory| memory.focused()) == Some(Id::unique("book-page"));
        if ctx.egui_wants_keyboard_input() && !book_has_focus {
            return;
        }
        for action in crate::actions::pressed(ctx, &self.settings.key_bindings) {
            self.execute_action(action, ctx);
        }
        let dynamic = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|event| {
                    if let egui::Event::Key {
                        key,
                        pressed: true,
                        repeat: false,
                        modifiers,
                        ..
                    } = event
                    {
                        self.settings
                            .key_bindings
                            .iter()
                            .find(|(id, chord)| {
                                id.starts_with("Shelf:")
                                    && crate::actions::matches(chord, *key, *modifiers)
                            })
                            .map(|(id, _)| id[6..].to_owned())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        });
        for name in dynamic {
            self.selected_shelf = Some(name);
            if self.reader_window {
                let _ = self.launch_window(None);
            } else {
                self.screen = Screen::Library;
            }
        }
    }
    fn execute_action(&mut self, action: crate::actions::Action, ctx: &egui::Context) {
        use crate::actions::Action as A;
        match action {
            A::Open => self.pick_book(),
            A::Library => {
                let _ = self.launch_window(None);
            }
            A::CloseWindow => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            A::MinimizeWindow => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            A::MaximizeWindow => ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(
                !ctx.input(|i| i.viewport().maximized.unwrap_or(false)),
            )),
            A::CloseTransient => {
                if let Some(reader) = self.cef_reader.as_mut() {
                    reader.clear_selection();
                }
                self.link_request = self.link_request.wrapping_add(1);
                self.auxiliary_document = None;
                self.force_hide_dictionary();
                self.footnote = None;
                self.note_open = false;
                self.selected = None;
                self.selection_anchor = None;
                self.point_tool = false;
                self.drawer_open = false;
                self.pending_center_tap = None;
                self.pending_page_turn = None;
                if self.screen == Screen::Settings {
                    self.screen = self.previous_screen;
                } else if self.screen == Screen::Reader {
                    self.focus = true;
                }
            }
            A::ToggleChrome => {
                self.focus = !self.focus;
            }
            A::NextPage => self.turn_page(true),
            A::PreviousPage => self.turn_page(false),
            A::PageLeft => self.turn_page(self.next_to_left()),
            A::PageRight => self.turn_page(!self.next_to_left()),
            A::FirstPage => self.seek_book(0.),
            A::LastPage => self.seek_book(1.),
            A::NextChapter => self.jump_to_chapter(self.chapter + 1, None),
            A::PreviousChapter => self.jump_to_chapter(self.chapter.saturating_sub(1), None),
            A::Toc | A::Bookmarks | A::Notes | A::Search => {
                self.focus = false;
                self.drawer_open = true;
                self.drawer = match action {
                    A::Toc => Drawer::Toc,
                    A::Bookmarks => Drawer::Bookmarks,
                    A::Notes => Drawer::Notes,
                    _ => Drawer::Search,
                };
            }
            A::AddBookmark => self.add_bookmark(),
            A::AddNote => {
                self.note_open = true;
                self.note_text.clear();
            }
            A::Copy => {
                if let Some(hit) = &self.selected {
                    ctx.copy_text(hit.text.clone());
                }
            }
            A::Dictionary => {
                if let Some(hit) = &self.selected {
                    self.lookup_dictionary(
                        hit.text.clone(),
                        ctx.pointer_hover_pos().unwrap_or(pos2(
                            self.reading_extent.x * 0.5,
                            self.reading_extent.y * 0.5,
                        )),
                    );
                }
            }
            A::PinDictionary => {
                self.dictionary_pinned = !self.dictionary_pinned;
                if let Some((_, input)) = &mut self.dictionary_child {
                    let _ = writeln!(
                        input,
                        "{}",
                        serde_json::json!({"type":"set_pinned","pinned":self.dictionary_pinned})
                    );
                }
            }
            A::PlayPause => self.toggle_speech(),
            A::Stop => self.stop_speech(),
            A::ReadSelection => self.start_speech(self.selected.clone()),
            A::PickReadPosition => {
                self.point_tool = !self.point_tool;
                if self.point_tool {
                    self.prefetch_visible();
                }
            }
            A::FollowSpeech => {
                self.follow_playback = !self.follow_playback;
                self.settings.tts.auto_follow = self.follow_playback;
                self.save_settings();
            }
            A::Faster | A::Slower => {
                self.settings.tts.speed = (self.settings.tts.speed
                    + if action == A::Faster { 0.25 } else { -0.25 })
                .clamp(0.5, 10.);
                self.save_settings();
            }
            A::PreviewVoice => self.preview_voice(),
            A::Appearance
            | A::ReadingSettings
            | A::SpeechSettings
            | A::DictionarySettings
            | A::LibrarySettings
            | A::KeySettings => self.open_settings(match action {
                A::Appearance => SettingTab::Appearance,
                A::ReadingSettings => SettingTab::Reading,
                A::SpeechSettings => SettingTab::Speech,
                A::DictionarySettings => SettingTab::Dictionaries,
                A::LibrarySettings => SettingTab::Library,
                _ => SettingTab::Keys,
            }),
            A::Light | A::Dark | A::Sepia => {
                let theme = match action {
                    A::Light => Theme::Light,
                    A::Dark => Theme::Dark,
                    _ => Theme::Sepia,
                };
                self.settings.set_theme(theme);
                self.save_settings();
            }
            A::Vertical | A::Horizontal | A::AutoWriting => {
                self.settings.reading_mode = match action {
                    A::Vertical => reader_library::ReadingMode::Vertical,
                    A::Horizontal => reader_library::ReadingMode::Horizontal,
                    _ => reader_library::ReadingMode::Auto,
                };
                self.save_settings();
            }
            A::ToggleRuby => {
                self.settings.show_ruby = !self.settings.show_ruby;
                self.save_settings();
            }
            A::FontLarger | A::FontSmaller => {
                self.settings.font_size = (self.settings.font_size
                    + if action == A::FontLarger { 1. } else { -1. })
                .clamp(10., 96.);
                self.save_settings();
            }
            A::AutoSpread | A::SinglePage | A::DoublePage => {
                self.settings.spread = match action {
                    A::AutoSpread => reader_library::SpreadMode::Auto,
                    A::SinglePage => reader_library::SpreadMode::Single,
                    _ => reader_library::SpreadMode::Double,
                };
                self.save_settings();
            }
            A::ToggleTextSpread => {
                self.settings.text_spread = !self.settings.text_spread;
                self.save_settings();
            }
            A::RefreshLibrary => self.refresh_shelf(),
            A::AllBooks | A::ReadingBooks | A::FinishedBooks => {
                self.selected_shelf = None;
                self.shelf_filter = match action {
                    A::AllBooks => 0,
                    A::ReadingBooks => 1,
                    _ => 2,
                };
            }
            A::LibraryCards | A::LibraryList => {
                self.settings.library_list = action == A::LibraryList;
                self.save_settings();
            }
            A::CreateShelf => {
                if self.reader_window {
                    let _ = self.launch_window(None);
                } else {
                    self.screen = Screen::Library;
                    self.new_shelf_open = true;
                }
            }
            A::Opds => self.services.open_catalog(),
            A::ImportWeb => self.services.open_web_import(),
            A::AddDictionary => self.add_dictionaries(false),
            A::AddDictionaryFolder => self.add_dictionaries(true),
            A::RefreshDictionaries => self.refresh_dictionaries(),
        }
    }
}

impl eframe::App for ReaderApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_work();
        self.poll_speech();
        self.shortcuts(&ctx);
        if self.screen == Screen::Reader
            && self.wheel_paint_frame != ctx.cumulative_frame_nr()
            && !self.opening
            && self.chapter_loading.is_none()
            && !self.layout_key.is_empty()
            && !self.layout.pages.is_empty()
            && (!self.book.epub.is_some()
                || self
                    .cef_reader
                    .as_ref()
                    .is_none_or(|reader| reader.turn_ready()))
            && let Some(forward) = self.pending_wheel_turns.pop_front()
        {
            self.wheel_paint_frame = ctx.cumulative_frame_nr();
            self.turn_page(forward);
        }
        self.frame_count += 1;
        let tap_delay = Duration::from_secs_f64(
            ctx.options(|o| o.input_options.max_double_click_delay) + 0.025,
        );
        if self
            .pending_center_tap
            .is_some_and(|time| time.elapsed() > tap_delay)
        {
            self.pending_center_tap = None;
            if !self.point_tool {
                self.focus = !self.focus;
                self.selected = None;
            }
        }
        if self
            .pending_page_turn
            .is_some_and(|(time, _)| time.elapsed() > tap_delay)
        {
            let (_, forward) = self.pending_page_turn.take().unwrap();
            self.turn_page(forward);
        }
        self.poll_search();
        if !self.reader_window
            && self.screen == Screen::Library
            && self.last_library_refresh.elapsed() > Duration::from_secs(3)
        {
            self.last_library_refresh = Instant::now();
            if let Ok(store) = LibraryStore::open(self.store.data_dir()) {
                for entry in store.books() {
                    if !self.shelf.iter().any(|existing| existing.id == entry.id) {
                        self.shelf.push(entry.clone());
                    }
                }
                self.store = store;
                self.shelf_progress.clear();
            }
        }
        self.screen_origin =
            ctx.input(|i| i.viewport().inner_rect.map(|r| r.min).unwrap_or(Pos2::ZERO));
        self.pixels_per_point = ctx.pixels_per_point();
        let p = Palette::for_settings(&self.settings);
        if self.applied_palette != Some(p) {
            p.apply(&ctx, p.is_dark());
            self.applied_palette = Some(p);
        }
        ctx.options_mut(|options| {
            options.input_options.line_scroll_speed = 40.
                * if self.screen == Screen::Library {
                    self.settings.scroll_speed
                } else {
                    1.
                }
        });
        ui.reset_style();
        let all = ui.max_rect();
        ui.painter().rect_filled(all, 0., p.paper);
        let blank_drag = if self.screen == Screen::Library {
            Some(ui.interact(
                all.shrink(5.),
                Id::unique("library-blank-drag"),
                Sense::drag(),
            ))
        } else {
            None
        };
        let settings = self.screen == Screen::Settings;
        if settings {
            let body = Rect::from_min_max(pos2(all.left(), all.top() + 32.), all.max);
            region(ui, "settings-surface", body, |ui| self.settings_view(ui, p));
        } else if self.screen == Screen::Library {
            region(
                ui,
                "library",
                Rect::from_min_max(pos2(all.left(), all.top() + 32.), all.max),
                |ui| self.library(ui, p),
            );
        } else if self.screen == Screen::Audiobook {
            region(
                ui,
                "audiobook",
                Rect::from_min_max(pos2(all.left(), all.top() + 32.), all.max),
                |ui| {
                    for action in self.audiobook.show(ui, p) {
                        match action {
                            crate::audiobook_ui::AudiobookAction::Back => {
                                let _ = self.launch_window(None);
                            }
                            crate::audiobook_ui::AudiobookAction::Notice(message) => {
                                self.notify(message)
                            }
                        }
                    }
                },
            );
        } else {
            region(ui, "native-book", all, |ui| self.reader(ui, p));
            if !self.focus {
                egui::Area::new(Id::unique("reading-top-overlay"))
                    .fixed_pos(pos2(all.left(), all.top() + 32.))
                    .order(egui::Order::Foreground)
                    .show(&ctx, |ui| {
                        let rect = Rect::from_min_size(ui.cursor().min, vec2(all.width(), 52.));
                        ui.allocate_rect(rect, Sense::hover());
                        ui.painter().rect_filled(rect, 0., p.panel);
                        region(ui, "top-controls", rect.shrink2(vec2(16., 8.)), |ui| {
                            self.top_bar(ui, p)
                        });
                    });
                egui::Area::new(Id::unique("reading-bottom-overlay"))
                    .fixed_pos(pos2(all.left(), all.bottom() - READING_FOOTER_HEIGHT - 72.))
                    .order(egui::Order::Foreground)
                    .show(&ctx, |ui| {
                        let rect = Rect::from_min_size(ui.cursor().min, vec2(all.width(), 72.));
                        ui.allocate_rect(rect, Sense::hover());
                        ui.painter().rect_filled(rect, 0., p.panel);
                        region(ui, "reading-controls", rect.shrink2(vec2(20., 6.)), |ui| {
                            self.reading_bar(ui, p)
                        });
                    });
                if self.drawer_open {
                    egui::Area::new(Id::unique("reading-drawer-overlay"))
                        .fixed_pos(pos2(all.left(), all.top() + 84.))
                        .order(egui::Order::Foreground)
                        .show(&ctx, |ui| {
                            let rect = Rect::from_min_size(
                                ui.cursor().min,
                                vec2(
                                    296.,
                                    (all.height() - READING_FOOTER_HEIGHT - 156.).max(180.),
                                ),
                            );
                            ui.allocate_rect(rect, Sense::hover());
                            ui.painter().rect_filled(rect, 0., p.panel);
                            region(ui, "drawer-controls", rect.shrink2(vec2(12., 8.)), |ui| {
                                self.drawer(ui, p)
                            });
                        });
                }
            }
            let footer = Rect::from_min_max(
                pos2(all.left(), all.bottom() - READING_FOOTER_HEIGHT),
                all.max,
            );
            ui.painter().rect_filled(footer, 0., p.paper);
            region(ui, "reading-seek-footer", footer, |ui| {
                self.reading_seek(ui, p)
            });
        }
        let title_visible = self.screen != Screen::Reader
            || !self.focus
            || self.opening
            || ctx
                .pointer_hover_pos()
                .is_some_and(|pos| pos.y < all.top() + 32.);
        if title_visible {
            egui::Area::new(Id::unique("native-window-title"))
                .fixed_pos(all.min)
                .order(egui::Order::Foreground)
                .show(&ctx, |ui| {
                    let rect = Rect::from_min_size(ui.cursor().min, vec2(all.width(), 32.));
                    ui.allocate_rect(rect, Sense::hover());
                    self.window_title_bar(ui, rect, p);
                });
        }
        if self.screen == Screen::Reader && self.focus && !self.opening {
            let title = Rect::from_min_max(
                all.min + vec2(24., 6.),
                pos2(all.right() - 150., all.top() + 26.),
            );
            ui.painter().with_clip_rect(title).text(
                title.left_top(),
                Align2::LEFT_TOP,
                &self.book.title,
                FontId::proportional(11.),
                p.muted,
            );
        }
        if blank_drag.is_some_and(|r| r.drag_started()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        self.resize_edges(ui, all);
        self.popup(&ctx);
        self.note_popup(&ctx);
        self.pronunciation_editor(&ctx);
        self.footnote_popup(&ctx);
        self.auxiliary_popup(&ctx, p);
        self.service_dialogs(&ctx);
        if let Some((message, time)) = &self.notice
            && time.elapsed() < Duration::from_millis(2600)
        {
            egui::Area::new(Id::unique("notification"))
                .anchor(Align2::CENTER_BOTTOM, vec2(0., -78.))
                .order(egui::Order::Foreground)
                .show(&ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_max_width(700.);
                        ui.label(message);
                    });
                });
        }
        for file in ctx.input(|i| i.raw.dropped_files.clone()) {
            self.open_document(file.path().to_path_buf());
        }
        if self.screen == Screen::Reader
            && !self.opening
            && !self.layout.pages.is_empty()
            && let Some(timer) = self.first_book_frame.take()
        {
            timer.finish();
        }
        if self.frame_count == 1 {
            crate::latency::event("first_native_frame_cpu_ready", 0);
        }
        if self.frame_count >= 8
            && !self.busy
            && (self.screen != Screen::Reader
                || (!self.opening
                    && !self.layout.pages.is_empty()
                    && self
                        .cef_reader
                        .as_ref()
                        .is_none_or(|reader| reader.turn_ready())))
            && let Some(path) = self.screenshot.take()
        {
            ctx.request_screenshot(move |image| {
                let bytes: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
                let _ = image::save_buffer(
                    path,
                    &bytes,
                    image.width() as u32,
                    image.height() as u32,
                    image::ColorType::Rgba8,
                );
            });
        }
        if let Some(mut driver) = self.perf_driver.take() {
            let chromium = self.book.epub.is_some() && self.cef_reader.is_some();
            let marker = if chromium {
                self.cef_reader
                    .as_ref()
                    .and_then(|reader| reader.measurement_marker())
            } else if !self.layout.pages.is_empty() {
                Some([
                    16 + ((self.page & 7) * 32) as u8,
                    16 + (((self.page >> 3) & 7) * 32) as u8,
                    128,
                ])
            } else {
                None
            };
            if !chromium && let Some(rgb) = marker {
                let marker_rect =
                    Rect::from_min_size(pos2(all.right() - 6., all.bottom() - 38.), vec2(4., 4.));
                ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    Id::unique("measurement-marker"),
                ))
                .rect_filled(
                    marker_rect,
                    0.,
                    Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
                );
            }
            let ready = !self.opening
                && !self.layout.pages.is_empty()
                && (!chromium
                    || self
                        .cef_reader
                        .as_ref()
                        .is_some_and(|reader| reader.turn_ready()));
            let action = driver.tick(
                &ctx,
                [
                    (all.width() * ctx.pixels_per_point()).round() as usize,
                    (all.height() * ctx.pixels_per_point()).round() as usize,
                ],
                marker,
                self.chapter,
                self.page,
                self.pagination_index.as_ref().map(|index| index.total),
                ready,
            );
            self.perf_driver = Some(driver);
            match action {
                Some(crate::cef_perf::PerfAction::Turn(forward)) => self.turn_page(forward),
                Some(crate::cef_perf::PerfAction::WheelBurst(count)) => {
                    self.queue_wheel_pages(&[(
                        egui::MouseWheelUnit::Line,
                        vec2(0., -(count as f32)),
                    )]);
                }
                Some(crate::cef_perf::PerfAction::ToggleChrome) => {
                    self.focus = !self.focus;
                    self.drawer_open = true;
                }
                Some(crate::cef_perf::PerfAction::Close) => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close)
                }
                None => {}
            }
        }
        if let Some(mut driver) = self.speech_smoke.take() {
            let reader = self.cef_reader.as_ref();
            let action = driver.tick(
                &ctx,
                crate::speech_smoke::Frame {
                    ready: self.screen == Screen::Reader
                        && !self.opening
                        && reader.is_some_and(|reader| reader.turn_ready())
                        && self.pagination_index.is_some(),
                    total: self.pagination_index.as_ref().map(|index| index.total),
                    layout: &self.layout_key,
                    chapter: self.chapter,
                    page: self.page,
                    error: self.open_error.is_some()
                        || self.speech_error.is_some()
                        || reader.is_some_and(|reader| reader.speech_has_error()),
                    panes: reader
                        .map(|reader| reader.speech_snapshot())
                        .unwrap_or_default(),
                },
            );
            self.speech_smoke = Some(driver);
            match action {
                Some(crate::speech_smoke::Action::Start) => {
                    self.stop_speech();
                    self.speech_provider = "system".into();
                    self.speech_voice.clear();
                    self.settings.tts.speed = 2.;
                    self.settings.tts.auto_follow = true;
                    self.follow_playback = true;
                    self.start_speech(None);
                }
                Some(crate::speech_smoke::Action::Close) => {
                    self.stop_speech();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                None => {}
            }
        }
        ctx.request_repaint_after(
            if self.speaking
                || !self.pending_wheel_turns.is_empty()
                || self.pending_center_tap.is_some()
                || self.pending_page_turn.is_some()
            {
                Duration::from_millis(16)
            } else if self.busy {
                Duration::from_millis(50)
            } else {
                Duration::from_millis(250)
            },
        );
    }
}
impl Drop for ReaderApp {
    fn drop(&mut self) {
        self.pagination_epoch
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.persist_position();
        if let Some((_, input)) = &mut self.dictionary_child {
            let _ = writeln!(input, "{{\"type\":\"close\"}}");
        }
    }
}
fn strip_tags(html: &str) -> String {
    let mut text = String::new();
    let mut tag = false;
    for c in html.chars() {
        match c {
            '<' => tag = true,
            '>' => {
                tag = false;
                text.push(' ');
            }
            _ if !tag => text.push(c),
            _ => {}
        }
    }
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn book_argument(args: &[String]) -> Option<PathBuf> {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if matches!(
            arg.as_str(),
            "--data-dir" | "--screenshot" | "--perf-output" | "--speech-smoke-output"
        ) {
            args.next();
            continue;
        }
        if !arg.starts_with('-') {
            return Some(PathBuf::from(arg));
        }
    }
    None
}

impl ReaderApp {
    fn service_dialogs(&mut self, ctx: &egui::Context) {
        for action in self.services.show(ctx, &mut self.settings) {
            match action {
                crate::services_ui::ServiceAction::OpenBook(path) => self.open_document(path),
                crate::services_ui::ServiceAction::SettingsChanged => self.save_settings(),
                crate::services_ui::ServiceAction::Notice(message) => self.notify(message),
            }
        }
    }
    pub(crate) fn open_catalog_dialog(&mut self) {
        self.services.open_catalog();
    }
    fn note_popup(&mut self, ctx: &egui::Context) {
        if !self.note_open {
            return;
        }
        let mut open = true;
        let selected = self.selected.clone();
        let mut save = false;
        egui::Window::new("注釈")
            .id(Id::unique("annotation-editor"))
            .open(&mut open)
            .collapsible(false)
            .default_width(400.)
            .show(ctx, |ui| {
                if let Some(hit) = &selected {
                    ui.label(RichText::new(&hit.text).strong());
                    ui.separator();
                }
                ui.add(
                    egui::TextEdit::multiline(&mut self.note_text)
                        .hint_text("メモを書く")
                        .desired_width(f32::INFINITY)
                        .desired_rows(6),
                );
                if ui.button("保存").clicked() {
                    save = true;
                }
            });
        self.note_open = open;
        if save {
            if let Some(hit) = selected {
                let annotation = reader_library::Annotation {
                    id: format!("note-{}-{}", unix_millis(), hit.locator.node),
                    book_id: self.book.id.clone(),
                    start: hit.locator,
                    end: hit.end,
                    quote: hit.text,
                    note: self.note_text.clone(),
                    color: self.settings.accent,
                    created_at: unix_millis(),
                };
                match self.store.save_annotation(&annotation) {
                    Ok(()) => {
                        self.note_open = false;
                        self.annotations_updated = None;
                        self.notify("注釈を保存しました");
                    }
                    Err(e) => self.notify(e.to_string()),
                }
            } else {
                self.notify("本文の文字を選んでから注釈を追加してください");
            }
        }
    }
    fn prefetch_visible(&mut self) {
        if !self.ensure_speech() {
            return;
        }
        let Some(chapter) = self.book.chapters.get(self.chapter).filter(|c| c.loaded) else {
            return;
        };
        let hit = self.speech_start_hit();
        let ChapterNarration {
            units,
            start_index,
            byte_offset,
        } = chapter_narration(chapter, &self.settings.tts.pronunciations, hit.as_ref());
        if units.is_empty() {
            return;
        }
        if let Some(speech) = &self.speech {
            if let Err(error) = speech.prefetch_at(units, start_index, byte_offset) {
                self.speech_error = Some(error.to_string());
            } else {
                self.speech_progress = "音声を先読みしています".into();
            }
        }
    }
    fn maybe_prefetch_audio(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().focused.unwrap_or(true)) {
            return;
        }
        if !self.settings.tts.pre_cache
            || self.speaking
            || self.voice_preview
            || self.opening
            || self.layout.pages.is_empty()
        {
            return;
        }
        let config = self.provider_config();
        if !matches!(
            config,
            ProviderConfig::Irodori { .. }
                | ProviderConfig::Kokoro { .. }
                | ProviderConfig::AivisSpeech { .. }
                | ProviderConfig::AivisNative { .. }
        ) || !config.capability().available
        {
            return;
        }
        let key = format!(
            "{}:{}:{}:{}:{}",
            self.book.id,
            self.chapter,
            self.page,
            self.layout_key,
            serde_json::to_string(&(&config, &self.settings.tts.pronunciations))
                .unwrap_or_default()
        );
        if self.prefetched_audio.as_ref() == Some(&key) {
            return;
        }
        self.prefetched_audio = Some(key);
        self.prefetch_visible();
    }
    fn ensure_speech(&mut self) -> bool {
        if self.speech_provider == "command"
            && serde_json::from_str::<Vec<String>>(&self.speech_command_args).is_err()
        {
            self.notify("コマンド引数は文字列のJSON配列で指定してください");
            return false;
        }
        let provider = self.provider_config();
        let key = serde_json::to_string(&(&provider, &self.settings.tts.pronunciations))
            .unwrap_or_default();
        if self.speech.is_none() || key != self.speech_config_key {
            match SpeechController::new(SpeechConfig {
                provider,
                cache_dir: self.store.data_dir().join("speech/cache"),
                speed: self.settings.tts.speed,
                pronunciations: self.settings.tts.pronunciations.clone(),
            }) {
                Ok(speech) => {
                    self.speech_generation = speech.generation();
                    self.last_runtime_speed = self.settings.tts.speed;
                    self.speech = Some(speech);
                    self.speech_config_key = key;
                }
                Err(error) => {
                    self.notify(error.to_string());
                    return false;
                }
            }
        }
        true
    }
    fn linked_atom(&self, hit: &Hit) -> Option<&reader_core::Atom> {
        self.book
            .chapters
            .iter()
            .find(|c| c.id == hit.locator.chapter)
            .or_else(|| {
                self.auxiliary_document
                    .as_ref()
                    .map(|d| &d.chapter)
                    .filter(|c| c.id == hit.locator.chapter)
            })?
            .atoms()
            .get((hit.unit_id & 0xFFFF_FFFF) as usize)
            .copied()
    }
    fn follow_link(&mut self, hit: &Hit, anchor: Pos2, ctx: &egui::Context) -> bool {
        let Some(atom) = self.linked_atom(hit) else {
            return false;
        };
        let Some(link) = atom.link.clone() else {
            return false;
        };
        let note = atom.link_is_note || atom.style.superscript;
        if link.starts_with("https://")
            || link.starts_with("http://")
            || link.starts_with("mailto:")
        {
            ctx.open_url(egui::OpenUrl::new_tab(link));
            return true;
        }
        self.request_book_link(hit.locator.chapter, link, note, anchor, ctx);
        true
    }
    fn request_book_link(
        &mut self,
        source: u32,
        link: String,
        note: bool,
        anchor: Pos2,
        ctx: &egui::Context,
    ) {
        self.link_request = self.link_request.wrapping_add(1);
        let request = self.link_request;
        let book_generation = self.open_generation;
        let book = self.book.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = book
                .load_link_target(source, &link)
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Work::Link {
                book_generation,
                request,
                result,
                note,
                anchor,
            });
            ctx.request_repaint();
        });
    }
    fn auxiliary_popup(&mut self, ctx: &egui::Context, p: Palette) {
        let Some(document) = self.auxiliary_document.clone() else {
            if let Some(reader) = self.cef_reader.as_mut() {
                reader.close_auxiliary();
            }
            return;
        };
        if self.cef_reader.is_some() && self.book.epub.is_some() {
            self.chromium_auxiliary_popup(ctx, p, &document);
            return;
        }
        let mut open = true;
        let mut clicked = None;
        egui::Window::new(if document.chapter.title.is_empty() {
            "補足"
        } else {
            &document.chapter.title
        })
        .id(Id::unique("linked-auxiliary-document"))
        .default_size(vec2(620., 600.))
        .constrain(true)
        .collapsible(false)
        .open(&mut open)
        .show(ctx, |ui| {
            let extent = vec2(ui.available_width().max(240.), 540.);
            let key = format!(
                "{}:{:?}:{:?}:{:.0}:{}:{}:{}",
                document.chapter.href,
                document.locator,
                document.chapter.presentation,
                extent.x,
                self.settings.font_family,
                self.settings.font_size,
                self.settings.margins
            );
            let key = format!(
                "{key}:{:?}:css:{}",
                (
                    self.settings.horizontal_margin(),
                    self.settings.vertical_margin(),
                    self.settings.page_gap
                ),
                reader_core::default_reader_css_revision()
            );
            if key != self.auxiliary_layout_key {
                self.auxiliary_layout = layout::chapter_layout(
                    &mut self.fonts,
                    &document.chapter,
                    extent,
                    &self.settings,
                );
                self.auxiliary_layout_key = key;
            }
            egui::ScrollArea::vertical()
                .id_salt("auxiliary-document-scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for page in self.auxiliary_layout.pages.clone() {
                        let (rect, response) = ui.allocate_exact_size(extent, Sense::click());
                        if let Some(locator) = self.auxiliary_scroll_target
                            && page.contains_locator(locator)
                        {
                            let target = page
                                .hit_for_locator(locator)
                                .map(|h| h.rect.translate(rect.min.to_vec2()))
                                .unwrap_or(rect);
                            ui.scroll_to_rect(target, Some(egui::Align::Min));
                            self.auxiliary_scroll_target = None;
                        }
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        let painter = ui.painter_at(rect);
                        for fill in &page.header_fills {
                            painter.rect_filled(fill.translate(rect.min.to_vec2()), 0., p.panel);
                        }
                        for picture in &page.pictures {
                            if let Ok(Some(texture)) = crate::book_images::load_texture(
                                ctx,
                                &self.book,
                                &document.chapter.href,
                                picture,
                                &mut self.pictures,
                            ) {
                                painter.image(
                                    texture.id(),
                                    picture.rect.translate(rect.min.to_vec2()),
                                    Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
                                    Color32::WHITE,
                                );
                            }
                        }
                        self.fonts
                            .draw_batch(&painter, &page.glyphs, rect.min.to_vec2(), p.ink);
                        for rule in &page.rules {
                            painter.rect_stroke(
                                rule.translate(rect.min.to_vec2()),
                                0.,
                                egui::Stroke::new(1., p.border),
                                egui::StrokeKind::Inside,
                            );
                        }
                        if let Some(position) = response
                            .hover_pos()
                            .or_else(|| response.interact_pointer_pos())
                            && let Some(hit) = page.hit(position - rect.min.to_vec2())
                            && document
                                .chapter
                                .atoms()
                                .get((hit.unit_id & 0xFFFF_FFFF) as usize)
                                .is_some_and(|a| a.link.is_some())
                        {
                            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                            if response.clicked() {
                                clicked = Some((hit.clone(), position));
                            }
                        }
                    }
                });
        });
        if !open {
            self.auxiliary_document = None;
        }
        if let Some((hit, position)) = clicked {
            self.follow_link(&hit, position, ctx);
        }
    }
    fn footnote_popup(&mut self, ctx: &egui::Context) {
        let Some((text, target, anchor)) = self.footnote.clone() else {
            return;
        };
        let mut open = true;
        let mut jump = false;
        egui::Window::new("脚注")
            .id(Id::unique("footnote"))
            .default_pos(anchor + vec2(12., 16.))
            .default_width(370.)
            .collapsible(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(text);
                if ui.button("本文の位置を開く").clicked() {
                    jump = true;
                }
            });
        if !open {
            self.footnote = None;
        }
        if jump {
            self.jump_to_locator(target);
            self.footnote = None;
        }
    }
    pub(crate) fn sync_speech_settings(&mut self) {
        let config = self.provider_config();
        let key = serde_json::to_string(&(&config, &self.settings.tts.pronunciations))
            .unwrap_or_default();
        if self.speech.is_some() && self.speech_config_key != key {
            self.stop_speech();
            self.speech = None;
            self.speech_config_key.clear();
            self.prefetched_audio = None;
            self.speech_progress.clear();
        }
        self.speech_profiles.set(&self.speech_provider, config);
        self.speech_profiles
            .write(&self.speech_provider, &mut self.settings);
    }
    pub(crate) fn set_aivis_native_config(&mut self, config: ProviderConfig) {
        if matches!(config, ProviderConfig::AivisNative { .. }) {
            self.speech_profiles.set("aivis_native", config);
        }
    }
    pub(crate) fn finish_provider_change(&mut self, previous: &str) {
        let selected = self.speech_provider.clone();
        self.speech_provider = previous.into();
        let previous_config = self.provider_config();
        self.speech_profiles.set(previous, previous_config);
        self.speech_provider = selected;
        self.load_profile_fields();
        self.sync_speech_settings();
        self.speech = None;
        self.speech_config_key.clear();
        self.speaking = false;
        self.speech_state.clear();
    }
    fn load_profile_fields(&mut self) {
        match self.speech_profiles.get(&self.speech_provider) {
            ProviderConfig::AivisNative { .. } => self.speech_voice.clear(),
            ProviderConfig::System { voice } => self.speech_voice = voice.unwrap_or_default(),
            ProviderConfig::OpenAi {
                base_url,
                api_key,
                model,
                voice,
            } => {
                self.speech_endpoint = base_url;
                self.speech_api_key = api_key;
                self.speech_model = model;
                self.speech_voice = voice;
            }
            ProviderConfig::Voicevox { base_url, speaker } => {
                self.speech_endpoint = base_url;
                self.speech_voice = speaker.to_string();
            }
            ProviderConfig::AivisSpeech {
                base_url, speaker, ..
            } => {
                self.speech_endpoint = base_url;
                self.speech_voice = speaker.to_string();
            }
            ProviderConfig::Voiceroid2 { voice, .. } => {
                self.speech_voice = voice;
            }
            ProviderConfig::Command { program, args } => {
                self.speech_command = program.to_string_lossy().into_owned();
                self.speech_command_args =
                    serde_json::to_string(&args).unwrap_or_else(|_| "[]".into());
            }
            ProviderConfig::Irodori {
                reference_audio,
                voice_instruction,
                seed,
                ..
            } => {
                self.speech_voice.clear();
                self.speech_reference = reference_audio;
                self.speech_instruction = voice_instruction;
                self.speech_seed = seed;
            }
            ProviderConfig::Kokoro { voice, .. } => self.speech_voice = voice,
        }
    }

    fn speech_start_hit(&self) -> Option<Hit> {
        let page = self.layout.pages.get(self.page)?;
        if self.cef_reader.is_some()
            && self.book.epub.is_some()
            && let Some(locator) = self.resume_locator
            && page.first_locator().is_some_and(|start| start <= locator)
            && self
                .layout
                .pages
                .get(self.page + 1)
                .and_then(|page| page.first_locator())
                .is_none_or(|end| locator < end)
            && locator.chapter as usize == self.chapter
            && let Some(hit) = self.chromium_hit(locator, Pos2::ZERO)
        {
            return Some(hit);
        }
        if let Some(locator) = self.resume_locator
            && let Some(hit) = page.hit_for_locator(locator)
        {
            return Some(hit.clone());
        }
        self.layout
            .pages
            .iter()
            .skip(self.page)
            .find_map(|page| page.hits.first().cloned())
    }
    pub(crate) fn preview_voice(&mut self) {
        self.stop_speech();
        self.speech_error = None;
        if !self.ensure_speech() {
            return;
        }
        self.speech_sources.clear();
        self.spoken_range = None;
        self.voice_preview = true;
        if let Some(speech) = &self.speech {
            let _ = speech.set_speed(1.);
            match speech.speak(
                vec![SpeechUnit {
                    id: u64::MAX,
                    text: reader_speech::IRODORI_VOICE_SAMPLE_TEXT.into(),
                }],
                0,
            ) {
                Ok(generation) => {
                    self.speech_generation = generation;
                    self.speaking = true;
                    self.speech_state = "準備中".into();
                }
                Err(error) => {
                    self.speech_error = Some(error.to_string());
                }
            }
        }
    }
    fn feed_remaining_chapters(&self, generation: u64, start: usize) {
        let Some(speech) = &self.speech else {
            return;
        };
        let feeder = speech.feeder();
        let book = self.book.clone();
        let book_generation = self.open_generation;
        let tx = self.tx.clone();
        let pronunciations = self.settings.tts.pronunciations.clone();
        std::thread::spawn(move || {
            for index in start..book.chapters.len() {
                if !feeder.is_current() {
                    return;
                }
                let Ok(chapter) = book.load_chapter(index) else {
                    return;
                };
                let mut units = Vec::new();
                let mut sources = Vec::new();
                for (index, (atom, text)) in chapter
                    .atoms()
                    .into_iter()
                    .zip(narration_texts(&chapter, &pronunciations))
                    .enumerate()
                {
                    let text = text.to_owned();
                    if text.trim().is_empty() || atom.locator_at(0).is_none() {
                        continue;
                    }
                    let id = layout::unit_id(chapter.id, index);
                    units.push(SpeechUnit { id, text });
                    sources.push((id, atom.clone()));
                }
                if tx
                    .send(Work::SpeechSources {
                        book_generation,
                        generation,
                        sources,
                    })
                    .is_err()
                    || feeder.append(units).is_err()
                {
                    return;
                }
            }
        });
    }
    pub(crate) fn accept_voice_sample(&mut self) {
        if let Some(path) =
            reader_speech::irodori_reference_path(&self.provider_config()).filter(|p| p.is_file())
        {
            self.speech_reference = Some(path);
            self.save_settings();
            self.speech_progress = "この声で読み上げます".into();
        } else {
            self.speech_error = Some("先に声を試聴してください".into());
        }
    }
    fn hide_dictionary(&mut self) {
        if let Some((_, input)) = &mut self.dictionary_child {
            let _ = writeln!(input, "{{\"type\":\"hide\"}}");
        }
        if !self.dictionary_pinned {
            self.dictionary_visible = false;
        }
        self.dictionary_popup = None;
        self.pending_center_tap = None;
    }
    fn next_to_left(&self) -> bool {
        crate::reading_surface::next_to_left(&self.book, self.chapter, &self.settings)
    }
    fn page_numbers(&self) -> Option<(usize, usize, usize)> {
        let index = self.pagination_index.as_ref()?;
        let start = *index.offsets.get(self.chapter)? + self.page;
        let end = if self.cross_chapter_spread {
            index
                .offsets
                .get(self.chapter + 2)
                .copied()
                .unwrap_or(index.total)
        } else {
            index.offsets[self.chapter]
                + crate::reading_surface::page_window(
                    &self.book.chapters[self.chapter],
                    self.page,
                    self.layout.pages.len(),
                    self.spread_pages,
                    self.next_to_left(),
                )
                .end
        };
        Some((start + 1, end.max(start + 1).min(index.total), index.total))
    }
    fn reading_fraction(&self) -> f32 {
        self.pagination_index
            .as_ref()
            .map(|index| {
                if self
                    .page_numbers()
                    .is_some_and(|(_, end, total)| end == total)
                {
                    1.
                } else {
                    index.fraction(self.chapter, self.page)
                }
            })
            .unwrap_or(0.)
    }
    fn seek_book(&mut self, fraction: f32) {
        let Some(index) = &self.pagination_index else {
            self.pending_restore = None;
            self.resume_locator = None;
            self.pending_fragment = None;
            self.pending_page_index = None;
            self.pending_page_fraction = None;
            self.pending_book_seek = Some(fraction);
            return;
        };
        let (chapter, page) = index.locate(fraction);
        if (chapter, page) != (self.chapter, self.page) {
            self.record_jump_origin();
        }
        self.jump_to_chapter(chapter, None);
        self.pending_page_index = Some(page);
        self.pending_page_fraction = None;
        self.pending_restore = None;
        self.resume_locator = None;
        self.pending_fragment = None;
    }
    fn ensure_pagination_index(&mut self) {
        if self.opening || self.book.chapters.is_empty() || self.layout.pages.is_empty() {
            return;
        }
        let e = self.reading_extent;
        let key = format!(
            "native-pages-v022:css:{}:{:?}",
            reader_core::default_reader_css_revision(),
            (
                self.book.content_identity(),
                e,
                self.settings.reading_mode,
                self.settings.spread,
                self.settings.text_spread,
                (
                    &self.settings.font_family,
                    self.settings.font_size,
                    self.settings.letter_spacing,
                    self.settings.line_spacing,
                    (
                        self.settings.horizontal_margin(),
                        self.settings.vertical_margin(),
                        self.settings.page_gap
                    ),
                    self.settings.show_ruby,
                    self.settings.ruby_scale,
                    self.book.chapters.len()
                )
            )
        );
        if key == self.pagination_key
            && (self.pagination_pending
                || self.pagination_index.is_some()
                || self.pagination_error.is_some())
        {
            return;
        }
        let generation = self
            .pagination_epoch
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        let debounce = !self.pagination_key.is_empty();
        self.page_count_ready = Some(crate::latency::pending(
            "page_count_request_to_ready",
            generation,
        ));
        crate::latency::event("page_count_requested", generation);
        self.pagination_key = key.clone();
        self.pagination_index = None;
        self.pagination_error = None;
        self.pagination_pending = true;
        // Run the same paginator synchronously in headless fixtures, so no worker
        // retains their temporary archives after the test has ended.
        if cfg!(test) {
            self.pagination_index = crate::pagination::compute(
                &self.book,
                e,
                &self.settings,
                &self.pagination_epoch,
                generation,
            )
            .ok();
            self.pagination_pending = false;
            if self.pagination_index.is_some()
                && let Some(fraction) = self.pending_book_seek.take()
            {
                self.seek_book(fraction);
            }
            return;
        }
        let book = self.book.clone();
        let settings = self.settings.clone();
        let epoch = self.pagination_epoch.clone();
        let tx = self.tx.clone();
        let ctx = self.worker_context.clone();
        let directory = self.store.data_dir().join("pagination-v022");
        std::thread::spawn(move || {
            let _worker = crate::latency::span("page_count_worker", generation);
            use std::hash::{Hash, Hasher};
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            key.hash(&mut hash);
            let path = directory.join(format!("{:016x}.json", hash.finish()));
            let result = (|| -> anyhow::Result<crate::pagination::PageIndex> {
                let cache_timer = crate::latency::span("page_count_cache_read", generation);
                if let Ok(bytes) = std::fs::read(&path)
                    && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
                    && value.get("key").and_then(|v| v.as_str()) == Some(key.as_str())
                    && let Ok(index) = serde_json::from_value::<crate::pagination::PageIndex>(
                        value["index"].clone(),
                    )
                    && index.counts.len() == book.chapters.len()
                {
                    crate::latency::event("page_count_cache_hit", generation);
                    return Ok(index);
                }
                drop(cache_timer);
                // A resize emits many intermediate geometries. Wait briefly off
                // the UI thread and discard superseded work before loading fonts
                // or parsing chapters for a size the user has already left.
                // The first layout has no resize burst to debounce.
                if debounce {
                    let _wait = crate::latency::span("page_count_resize_debounce", generation);
                    std::thread::sleep(Duration::from_millis(120));
                }
                if epoch.load(std::sync::atomic::Ordering::Relaxed) != generation {
                    anyhow::bail!("Pagination superseded");
                }
                let compute_timer = crate::latency::span("page_count_compute", generation);
                let index = crate::pagination::compute(&book, e, &settings, &epoch, generation)?;
                drop(compute_timer);
                if epoch.load(std::sync::atomic::Ordering::Relaxed) != generation {
                    anyhow::bail!("Pagination superseded");
                }
                if std::fs::create_dir_all(&directory).is_ok() {
                    let _write = crate::latency::span("page_count_cache_write", generation);
                    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
                    if let Ok(bytes) =
                        serde_json::to_vec(&serde_json::json!({"key":key,"index":index}))
                        && std::fs::write(&temp, bytes).is_ok()
                    {
                        let _ = std::fs::rename(&temp, &path);
                    }
                }
                Ok(index)
            })()
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Work::Pagination {
                key,
                generation,
                result,
            });
            ctx.request_repaint();
        });
    }
    fn jump_to_chapter(&mut self, index: usize, fragment: Option<String>) {
        let index = index.min(self.book.chapters.len().saturating_sub(1));
        if index != self.chapter || self.page != 0 || fragment.is_some() {
            self.record_jump_origin();
        }
        self.persist_position();
        self.pending_center_tap = None;
        self.pending_page_turn = None;
        self.follow_playback = false;
        self.chapter = index;
        self.page = 0;
        self.layout = BookLayout::default();
        self.composed_frame = None;
        self.layout_key.clear();
        self.selected = None;
        self.selection_anchor = None;
        self.chapter_loading = None;
        self.pending_book_seek = None;
        self.pending_page_index = None;
        self.pending_page_fraction = None;
        self.pending_fragment = fragment;
        self.pending_restore = self
            .book
            .chapters
            .get(self.chapter)
            .filter(|c| c.loaded)
            .and_then(|c| {
                self.pending_fragment
                    .as_ref()
                    .and_then(|f| c.anchors.get(f))
                    .copied()
                    .or_else(|| c.start_locator())
            });
        self.resume_locator = self.pending_restore;
    }
    fn request_chapter(&mut self) {
        if self.chapter_loading == Some(self.chapter) {
            return;
        }
        self.chapter_request = self.chapter_request.wrapping_add(1);
        let request = self.chapter_request;
        let index = self.chapter;
        let book_generation = self.open_generation;
        let book = self.book.clone();
        let tx = self.tx.clone();
        let ctx = self.worker_context.clone();
        self.chapter_loading = Some(index);
        std::thread::spawn(move || {
            let result = book.load_chapter(index).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Work::Chapter {
                book_generation,
                request,
                index,
                result,
            });
            ctx.request_repaint();
        });
    }
    fn prefetch_next_chapter(&mut self) {
        let ahead = if self.cross_chapter_spread { 4 } else { 2 };
        let keep = (self.chapter.saturating_sub(1)
            ..=(self.chapter + ahead).min(self.book.chapters.len().saturating_sub(1)))
            .collect::<Vec<_>>();
        self.book.unload_chapters_except(&keep);
        for index in keep {
            if index == self.chapter
                || self.book.chapters[index].loaded
                || !self.chapter_prefetch.insert(index)
            {
                continue;
            }
            let book = self.book.clone();
            let tx = self.tx.clone();
            let book_generation = self.open_generation;
            let ctx = self.worker_context.clone();
            std::thread::spawn(move || {
                let result = book.load_chapter(index).map_err(|e| format!("{e:#}"));
                let _ = tx.send(Work::Chapter {
                    book_generation,
                    request: 0,
                    index,
                    result,
                });
                ctx.request_repaint();
            });
        }
    }
    fn poll_search(&mut self) {
        if self.search_query == self.search_started_query
            || self.search_edited.elapsed() < Duration::from_millis(200)
            || self.opening
        {
            return;
        }
        self.search_started_query = self.search_query.clone();
        self.search_results.clear();
        if self.search_query.is_empty() {
            self.search_busy = false;
            return;
        }
        self.search_busy = true;
        let query = self.search_query.clone();
        let book = self.book.clone();
        let generation = self.search_generation;
        let book_generation = self.open_generation;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = book.search_all(&query).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Work::Search {
                book_generation,
                generation,
                result,
            });
        });
    }
    fn window_title_bar(&mut self, ui: &mut Ui, rect: Rect, p: Palette) {
        ui.painter().rect_filled(rect, 0., p.paper);
        if !self.reader_window {
            ui.painter().rect_filled(
                Rect::from_min_size(rect.min, vec2(212., rect.height())),
                0.,
                p.panel,
            );
        }
        let drag = Rect::from_min_max(rect.min, pos2(rect.right() - 126., rect.bottom()));
        let response = ui.interact(
            drag,
            Id::unique("window-title-drag"),
            Sense::click_and_drag(),
        );
        if response.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
        if response.double_clicked() {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        }
        if self.reader_window {
            ui.painter()
                .with_clip_rect(drag.shrink2(vec2(10., 0.)))
                .text(
                    pos2(rect.left() + 16., rect.center().y),
                    Align2::LEFT_CENTER,
                    &self.book.title,
                    FontId::proportional(12.),
                    p.muted,
                );
        }
        for (index, icon, label) in [
            (0, Icon::Minimize, "最小化"),
            (
                1,
                if maximized {
                    Icon::Restore
                } else {
                    Icon::Maximize
                },
                if maximized {
                    "元のサイズに戻す"
                } else {
                    "最大化"
                },
            ),
            (2, Icon::Close, "ウィンドウを閉じる"),
        ] {
            let button = Rect::from_min_size(
                pos2(rect.right() - 126. + index as f32 * 42., rect.top()),
                vec2(42., rect.height()),
            );
            let response = ui.interact(
                button,
                Id::unique(("window-control", index)),
                Sense::click(),
            );
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
            if response.hovered() {
                ui.painter().rect_filled(
                    button,
                    0.,
                    if index == 2 {
                        Color32::from_rgb(196, 43, 47)
                    } else {
                        p.selection
                    },
                );
            }
            icons::paint(
                ui.painter(),
                button.center(),
                icon,
                if index == 2 && response.hovered() {
                    Color32::WHITE
                } else {
                    p.ink
                },
            );
            if response.on_hover_text(label).clicked() {
                ui.ctx().send_viewport_cmd(match index {
                    0 => egui::ViewportCommand::Minimized(true),
                    1 => egui::ViewportCommand::Maximized(!maximized),
                    _ => egui::ViewportCommand::Close,
                });
            }
        }
    }
    fn resize_edges(&self, ui: &mut Ui, all: Rect) {
        if ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false)) {
            return;
        }
        use egui::viewport::ResizeDirection as D;
        let w = 5.;
        for (index, rect, direction, cursor) in [
            (
                0,
                Rect::from_min_size(all.min, vec2(w, all.height())),
                D::West,
                egui::CursorIcon::ResizeHorizontal,
            ),
            (
                1,
                Rect::from_min_size(pos2(all.right() - w, all.top()), vec2(w, all.height())),
                D::East,
                egui::CursorIcon::ResizeHorizontal,
            ),
            (
                2,
                Rect::from_min_size(all.min, vec2(all.width(), w)),
                D::North,
                egui::CursorIcon::ResizeVertical,
            ),
            (
                3,
                Rect::from_min_size(pos2(all.left(), all.bottom() - w), vec2(all.width(), w)),
                D::South,
                egui::CursorIcon::ResizeVertical,
            ),
            (
                4,
                Rect::from_min_size(all.min, vec2(10., 10.)),
                D::NorthWest,
                egui::CursorIcon::ResizeNwSe,
            ),
            (
                5,
                Rect::from_min_size(pos2(all.right() - 10., all.top()), vec2(10., 10.)),
                D::NorthEast,
                egui::CursorIcon::ResizeNeSw,
            ),
            (
                6,
                Rect::from_min_size(pos2(all.left(), all.bottom() - 10.), vec2(10., 10.)),
                D::SouthWest,
                egui::CursorIcon::ResizeNeSw,
            ),
            (
                7,
                Rect::from_min_size(all.max - vec2(10., 10.), vec2(10., 10.)),
                D::SouthEast,
                egui::CursorIcon::ResizeNwSe,
            ),
        ] {
            let ctx = ui.ctx().clone();
            let area = egui::Area::new(Id::unique(("resize-edge-overlay", index)))
                .fixed_pos(rect.min)
                .order(egui::Order::Foreground)
                .show(&ctx, |ui| {
                    let (_, response) = ui.allocate_exact_size(rect.size(), Sense::drag());
                    let response = response.on_hover_cursor(cursor);
                    if response.drag_started() {
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
                    }
                });
            ctx.move_to_top(area.response.layer_id);
        }
    }
    fn force_hide_dictionary(&mut self) {
        if let Some((_, input)) = &mut self.dictionary_child {
            let _ = writeln!(input, "{{\"type\":\"force_hide\"}}");
        }
        self.dictionary_visible = false;
        self.dictionary_popup = None;
    }
}

fn same_path(a: &std::path::Path, b: &std::path::Path) -> bool {
    if a == b {
        return true;
    }
    #[cfg(windows)]
    {
        let normalize = |path: &std::path::Path| {
            path.to_string_lossy()
                .replace('/', "\\")
                .trim_start_matches("\\\\?\\")
                .to_lowercase()
        };
        normalize(a) == normalize(b)
    }
    #[cfg(not(windows))]
    {
        false
    }
}
