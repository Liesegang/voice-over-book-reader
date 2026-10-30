use super::{EntryPages, Options, emit, play_sound, resource_response};
#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "android"
)))]
use anyhow::Context;
use anyhow::Result;
use reader_dictionary_popup::{
    DictionaryTab, PopupCommand,
    geometry::{Bounds, beside_word, fit_popup},
    presentation::popup_update_script,
    render_popup_tabs,
};
use reader_library::{DictionaryCatalog, DictionaryHit};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::BufRead,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime},
};
use tao::{
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{ElementState, Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    keyboard::Key,
    window::{ResizeDirection, Window, WindowBuilder},
};
use wry::{WebView, WebViewBuilder};

#[derive(Clone, Debug)]
enum UserEvent {
    Command(PopupCommand),
    Results {
        sequence: u64,
        query: String,
        dictionary_id: String,
        hits: Vec<DictionaryHit>,
        errors: Vec<String>,
    },
    Action(BridgeAction),
    PageReady,
    Error(String),
}

#[derive(Clone, Debug, Deserialize)]
struct BridgeAction {
    action: String,
    dictionary_id: Option<String>,
    query: Option<String>,
    key: Option<String>,
    direction: Option<String>,
}

struct Work {
    sequence: u64,
    query: String,
    dictionary_id: String,
    priority: bool,
}

#[derive(Default)]
struct WorkQueue {
    sequence: Option<u64>,
    pending: VecDeque<Work>,
    completed: HashSet<String>,
}
impl WorkQueue {
    fn enqueue(&mut self, work: Work) {
        if self.sequence != Some(work.sequence) {
            self.pending.clear();
            self.completed.clear();
            self.sequence = Some(work.sequence);
        }
        if self.completed.contains(&work.dictionary_id) {
            return;
        }
        if let Some(index) = self
            .pending
            .iter()
            .position(|queued| queued.dictionary_id == work.dictionary_id)
        {
            if !work.priority {
                return;
            }
            self.pending.remove(index);
        }
        if work.priority {
            self.pending.push_front(work);
        } else {
            self.pending.push_back(work);
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
struct Preferences {
    width: f64,
    height: f64,
    x: i32,
    y: i32,
    positioned: bool,
    pinned: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            width: 560.0,
            height: 420.0,
            x: 0,
            y: 0,
            positioned: false,
            pinned: false,
        }
    }
}
impl Preferences {
    fn load(path: &Path) -> Self {
        let mut value = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .unwrap_or_default();
        value.width = if value.width.is_finite() {
            value.width.clamp(320.0, 1600.0)
        } else {
            560.0
        };
        value.height = if value.height.is_finite() {
            value.height.clamp(180.0, 1400.0)
        } else {
            420.0
        };
        value
    }
    fn save(&self, path: &Path) {
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(path, bytes);
        }
    }
}

struct Popup {
    window: Window,
    webview: WebView,
    data_dir: PathBuf,
    preferences_path: PathBuf,
    preferences: Preferences,
    theme: String,
    accent: String,
    query: String,
    tabs: Vec<DictionaryTab>,
    selected: Option<String>,
    hits: HashMap<String, Vec<Arc<DictionaryHit>>>,
    diagnostics: HashMap<String, Vec<String>>,
    pages: EntryPages,
    catalog: Arc<Mutex<DictionaryCatalog>>,
    current_request: Arc<AtomicU64>,
    work_tx: mpsc::Sender<Work>,
    proxy: tao::event_loop::EventLoopProxy<UserEvent>,
    sequence: u64,
    visible: bool,
    paper: Option<String>,
    ink: Option<String>,
    shown_at: Instant,
    blur_since: Option<Instant>,
    focus_acquired: bool,
    dismiss_on_blur: bool,
    geometry_dirty: Option<Instant>,
}

pub(super) fn run(options: Options) -> Result<()> {
    let dictionary_dir = options.data_dir.join("dictionaries");
    let initial_catalog = DictionaryCatalog::open(&dictionary_dir)?;
    let tabs = tabs(&initial_catalog);
    let catalog = Arc::new(Mutex::new(initial_catalog));
    let pages = Arc::new(Mutex::new(
        HashMap::<(String, u64), Arc<DictionaryHit>>::new(),
    ));
    let preferences_path = options.data_dir.join("dictionary-popup.json");
    let preferences = Preferences::load(&preferences_path);
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let mut builder = WindowBuilder::new()
        .with_title("辞書")
        .with_decorations(false)
        .with_resizable(true)
        .with_transparent(false)
        .with_inner_size(LogicalSize::new(preferences.width, preferences.height))
        .with_min_inner_size(LogicalSize::new(320.0, 180.0))
        .with_visible(false)
        .with_position(PhysicalPosition::new(options.x, options.y));
    #[cfg(target_os = "windows")]
    {
        use tao::platform::windows::WindowBuilderExtWindows;
        builder = builder
            .with_skip_taskbar(true)
            .with_undecorated_shadow(false);
        if let Some(hwnd) = options.parent_hwnd {
            builder = builder.with_owner_window(hwnd);
        }
    }
    let window = builder.build(&event_loop)?;
    remove_native_border(&window);
    let resource_catalog = Arc::clone(&catalog);
    let resource_pages = Arc::clone(&pages);
    let ipc_proxy = proxy.clone();
    let webview_builder = WebViewBuilder::new()
        .with_transparent(false)
        .with_background_color((250, 250, 247, 255))
        .with_devtools(false)
        .with_html(render_popup_tabs(
            "",
            &tabs,
            None,
            &[],
            &[],
            &options.theme,
            &options.accent,
            preferences.pinned,
            false,
        ))
        .with_navigation_handler(|url| {
            url == "about:blank"
                || url.starts_with("data:text/html;charset=utf-8;base64,")
                || url.starts_with("dictionary://localhost/")
                || url.starts_with("http://dictionary.localhost/")
                || url.starts_with("https://dictionary.localhost/")
        })
        .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
        .with_ipc_handler(move |request| {
            if request.body().len() > 32 * 1024 {
                return;
            }
            if let Ok(action) = serde_json::from_str::<BridgeAction>(request.body()) {
                let root = request.uri() == "about:blank"
                    || matches!(request.uri().scheme_str(), Some("about" | "data"));
                if matches!(action.action.as_str(), "entry" | "sound" | "close")
                    || (root
                        && matches!(
                            action.action.as_str(),
                            "pin" | "dictionary" | "tab" | "drag" | "resize" | "lookup" | "ready"
                        ))
                {
                    let _ = ipc_proxy.send_event(UserEvent::Action(action));
                }
            }
        })
        .with_asynchronous_custom_protocol("dictionary".into(), move |_, request, responder| {
            let catalog = Arc::clone(&resource_catalog);
            let pages = Arc::clone(&resource_pages);
            std::thread::spawn(move || {
                responder.respond(resource_response(
                    &request.uri().to_string(),
                    &catalog,
                    &pages,
                ))
            });
        });
    #[cfg(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    ))]
    let webview = webview_builder.build(&window)?;
    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    )))]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        webview_builder.build_gtk(
            window
                .default_vbox()
                .context("GTK window container unavailable")?,
        )?
    };
    let current_request = Arc::new(AtomicU64::new(0));
    let (work_tx, work_rx) = mpsc::channel::<Work>();
    worker(
        work_rx,
        Arc::clone(&catalog),
        dictionary_dir,
        Arc::clone(&current_request),
        proxy.clone(),
    );
    let stdin_proxy = proxy.clone();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else {
                break;
            };
            if line.len() > 32 * 1024 {
                continue;
            }
            match serde_json::from_str::<PopupCommand>(&line) {
                Ok(command) => {
                    let _ = stdin_proxy.send_event(UserEvent::Command(command));
                }
                Err(error) => {
                    let _ = stdin_proxy
                        .send_event(UserEvent::Error(format!("Invalid popup command: {error}")));
                }
            }
        }
        let _ = stdin_proxy.send_event(UserEvent::Command(PopupCommand::Close));
    });
    let mut popup = Popup {
        window,
        webview,
        data_dir: options.data_dir,
        preferences_path,
        preferences,
        theme: options.theme.clone(),
        accent: options.accent.clone(),
        query: String::new(),
        tabs,
        selected: None,
        hits: HashMap::new(),
        diagnostics: HashMap::new(),
        pages,
        catalog,
        current_request,
        work_tx,
        proxy: proxy.clone(),
        sequence: 0,
        visible: false,
        paper: options.paper,
        ink: options.ink,
        shown_at: Instant::now(),
        blur_since: None,
        focus_acquired: false,
        dismiss_on_blur: options.dismiss_on_blur,
        geometry_dirty: None,
    };
    emit(serde_json::json!({"event":"ready"}));
    if !options.query.is_empty() {
        let _ = proxy.send_event(UserEvent::Command(PopupCommand::Lookup {
            query: options.query,
            x: Some(options.x),
            y: Some(options.y),
            theme: Some(options.theme),
            accent: Some(options.accent),
            paper: None,
            ink: None,
        }));
    }
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        popup.handle(event, flow);
    });
}

fn tabs(catalog: &DictionaryCatalog) -> Vec<DictionaryTab> {
    catalog
        .dictionaries()
        .iter()
        .filter(|info| info.enabled)
        .map(|info| DictionaryTab {
            id: info.id.clone(),
            title: info.title.clone(),
            count: None,
        })
        .collect()
}

fn worker(
    rx: mpsc::Receiver<Work>,
    catalog: Arc<Mutex<DictionaryCatalog>>,
    dictionary_dir: PathBuf,
    current: Arc<AtomicU64>,
    proxy: tao::event_loop::EventLoopProxy<UserEvent>,
) {
    std::thread::spawn(move || {
        // Searching may build a large MDX index. Keep that service separate from
        // the resource catalog so an already visible definition never waits on it.
        let mut search_catalog = None;
        let mut stamp: Option<(u64, Option<SystemTime>)> = None;
        let mut queue = WorkQueue::default();
        loop {
            if queue.pending.is_empty() {
                let Ok(work) = rx.recv() else {
                    break;
                };
                if work.sequence == current.load(Ordering::Relaxed) {
                    queue.enqueue(work);
                }
            }
            while let Ok(work) = rx.try_recv() {
                if work.sequence == current.load(Ordering::Relaxed) {
                    queue.enqueue(work);
                }
            }
            let Some(work) = queue.pending.pop_front() else {
                continue;
            };
            if current.load(Ordering::Relaxed) != work.sequence {
                continue;
            }
            let result = (|| -> Result<Vec<DictionaryHit>> {
                let next = std::fs::metadata(dictionary_dir.join("dictionaries.json"))
                    .ok()
                    .map(|m| (m.len(), m.modified().ok()));
                if search_catalog.is_none() || next != stamp {
                    search_catalog = Some(DictionaryCatalog::open(&dictionary_dir)?);
                    if let Ok(mut resources) = catalog.lock() {
                        *resources = DictionaryCatalog::open(&dictionary_dir)?;
                    }
                    stamp = next;
                }
                search_catalog
                    .as_mut()
                    .expect("opened above")
                    .lookup_in_cancellable(&work.dictionary_id, &work.query, 40, || {
                        current.load(Ordering::Relaxed) != work.sequence
                    })
            })();
            if current.load(Ordering::Relaxed) != work.sequence {
                continue;
            }
            queue.completed.insert(work.dictionary_id.clone());
            let (hits, errors) = match result {
                Ok(hits) => (hits, vec![]),
                Err(error) => (vec![], vec![format!("{error:#}")]),
            };
            let _ = proxy.send_event(UserEvent::Results {
                sequence: work.sequence,
                query: work.query,
                dictionary_id: work.dictionary_id,
                hits,
                errors,
            });
        }
    });
}

impl Popup {
    fn render(&self) {
        let hits: Vec<_> = self
            .tabs
            .iter()
            .filter_map(|tab| self.hits.get(&tab.id))
            .flat_map(|hits| hits.iter().map(AsRef::as_ref))
            .collect();
        let diagnostics: Vec<_> = self
            .tabs
            .iter()
            .flat_map(|tab| {
                self.diagnostics
                    .get(&tab.id)
                    .into_iter()
                    .flatten()
                    .map(|error| format!("{}: {error}", tab.title))
            })
            .collect();
        let script = popup_update_script(
            &self.query,
            &self.tabs,
            self.selected.as_deref(),
            hits,
            &diagnostics,
            &self.theme,
            &self.accent,
            self.preferences.pinned,
            self.paper.as_deref(),
            self.ink.as_deref(),
        );
        let _ = self.webview.evaluate_script(&script);
    }
    fn visibility(&self) {
        emit(
            serde_json::json!({"event":"visibility","visible":self.visible,"pinned":self.preferences.pinned}),
        );
    }
    fn hide(&mut self, force: bool) {
        if force || !self.preferences.pinned {
            self.remember_geometry();
            self.preferences.save(&self.preferences_path);
            self.geometry_dirty = None;
            self.visible = false;
            self.window.set_visible(false);
            self.blur_since = None;
        }
        self.visibility();
    }
    fn select(&mut self, id: String) {
        if !self.tabs.iter().any(|tab| tab.id == id) {
            return;
        }
        self.selected = Some(id.clone());
        // A jump never cancels other dictionaries or rebuilds the page. It only
        // moves a pending dictionary to the front of the background queue.
        if !self.hits.contains_key(&id) {
            let _ = self.work_tx.send(Work {
                sequence: self.sequence,
                query: self.query.clone(),
                dictionary_id: id,
                priority: true,
            });
        }
    }
    fn lookup(&mut self, query: String, preferred: Option<String>) {
        let query = query.trim().chars().take(1024).collect::<String>();
        if query.is_empty() {
            return;
        }
        let previous: Vec<_> = self.tabs.iter().map(|tab| tab.id.clone()).collect();
        if let Ok(catalog) = DictionaryCatalog::open(self.data_dir.join("dictionaries")) {
            self.tabs = tabs(&catalog);
        }
        let changed = query != self.query
            || previous
                != self
                    .tabs
                    .iter()
                    .map(|tab| tab.id.clone())
                    .collect::<Vec<_>>();
        if changed {
            self.sequence = self.sequence.wrapping_add(1);
            self.current_request.store(self.sequence, Ordering::Relaxed);
            self.hits.clear();
            self.diagnostics.clear();
            if let Ok(mut pages) = self.pages.lock() {
                pages.clear();
            }
        }
        self.query = query;
        self.selected = preferred
            .or_else(|| self.selected.clone())
            .filter(|id| self.tabs.iter().any(|tab| tab.id == *id))
            .or_else(|| self.tabs.first().map(|tab| tab.id.clone()));
        for tab in &mut self.tabs {
            tab.count = self.hits.get(&tab.id).map(Vec::len);
        }
        self.render();
        // Names and pending sections are painted first; results arrive one by one.
        let mut ids: Vec<_> = self.tabs.iter().map(|tab| tab.id.clone()).collect();
        if let Some(selected) = &self.selected
            && let Some(index) = ids.iter().position(|id| id == selected)
        {
            let preferred = ids.remove(index);
            ids.insert(0, preferred);
        }
        for id in ids {
            if !self.hits.contains_key(&id) {
                let _ = self.work_tx.send(Work {
                    sequence: self.sequence,
                    query: self.query.clone(),
                    dictionary_id: id,
                    priority: false,
                });
            }
        }
    }
    fn remember_geometry(&mut self) {
        let logical = self
            .window
            .inner_size()
            .to_logical::<f64>(self.window.scale_factor());
        self.preferences.width = logical.width;
        self.preferences.height = logical.height;
        if let Ok(position) = self.window.outer_position() {
            self.preferences.x = position.x;
            self.preferences.y = position.y;
        }
        self.geometry_dirty = Some(Instant::now());
    }
    fn bounds(&self) -> Bounds {
        let position = self
            .window
            .outer_position()
            .unwrap_or(PhysicalPosition::new(0, 0));
        let size = self.window.outer_size();
        Bounds {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
        }
    }
    fn set_bounds(&self, geometry: Bounds) {
        let current = self.bounds();
        if current.width != geometry.width || current.height != geometry.height {
            self.window
                .set_inner_size(PhysicalSize::new(geometry.width, geometry.height));
        }
        if current.x != geometry.x || current.y != geometry.y {
            self.window
                .set_outer_position(PhysicalPosition::new(geometry.x, geometry.y));
        }
    }
    fn clamp(&self) {
        let current = self.bounds();
        self.set_bounds(fit_popup(
            current,
            work_area(&self.window, (current.x, current.y)),
        ));
    }
    fn handle(&mut self, event: Event<'_, UserEvent>, flow: &mut ControlFlow) {
        match event {
            Event::UserEvent(UserEvent::Command(PopupCommand::Lookup {
                query,
                x,
                y,
                theme,
                accent,
                paper,
                ink,
            })) => {
                if let Some(theme) = theme {
                    self.theme = theme;
                }
                if let Some(accent) = accent {
                    self.accent = accent;
                }
                if let Some(paper) = paper {
                    self.paper = Some(paper);
                }
                if let Some(ink) = ink {
                    self.ink = Some(ink);
                }
                let anchor = (
                    x.unwrap_or(self.preferences.x),
                    y.unwrap_or(self.preferences.y),
                );
                let size = self.window.outer_size();
                let keep_position = self.preferences.positioned || self.preferences.pinned;
                let work = work_area(
                    &self.window,
                    if keep_position {
                        (self.preferences.x, self.preferences.y)
                    } else {
                        anchor
                    },
                );
                let inset_width = work.width.saturating_sub(12).max(1);
                let inset_height = work.height.saturating_sub(12).max(1);
                let dpi = self.window.scale_factor();
                self.window.set_min_inner_size(Some(PhysicalSize::new(
                    ((320.0 * dpi) as u32).min(inset_width),
                    ((180.0 * dpi) as u32).min(inset_height),
                )));
                self.window
                    .set_max_inner_size(Some(PhysicalSize::new(inset_width, inset_height)));
                let target = if keep_position {
                    fit_popup(
                        Bounds {
                            x: self.preferences.x,
                            y: self.preferences.y,
                            width: size.width,
                            height: size.height,
                        },
                        work,
                    )
                } else {
                    beside_word(anchor, (size.width, size.height), work)
                };
                self.set_bounds(target);
                self.lookup(query, None);
                self.visible = true;
                self.shown_at = Instant::now();
                self.blur_since = None;
                self.focus_acquired = false;
                self.window.set_visible(true);
                self.window.set_focus();
                let _ = self.webview.focus();
                self.visibility();
            }
            Event::UserEvent(UserEvent::Command(PopupCommand::Hide)) => self.hide(false),
            Event::UserEvent(UserEvent::Command(PopupCommand::ForceHide)) => self.hide(true),
            Event::UserEvent(UserEvent::Command(PopupCommand::SetPinned { pinned })) => {
                self.pin(pinned)
            }
            Event::UserEvent(UserEvent::Command(PopupCommand::Close)) => {
                self.remember_geometry();
                self.preferences.save(&self.preferences_path);
                *flow = ControlFlow::Exit;
            }
            Event::UserEvent(UserEvent::Results {
                sequence,
                query,
                dictionary_id,
                hits,
                errors,
            }) if sequence == self.sequence && query == self.query => {
                let hits: Vec<_> = hits.into_iter().map(Arc::new).collect();
                if let Ok(mut pages) = self.pages.lock() {
                    for hit in &hits {
                        pages.insert((hit.dictionary_id.clone(), hit.ordinal), Arc::clone(hit));
                    }
                }
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == dictionary_id) {
                    tab.count = Some(hits.len());
                }
                emit(
                    serde_json::json!({"event":"results","query":query,"dictionary_id":dictionary_id,"count":hits.len(),"errors":errors}),
                );
                self.hits.insert(dictionary_id.clone(), hits);
                self.diagnostics.insert(dictionary_id, errors);
                self.render();
            }
            Event::UserEvent(UserEvent::PageReady) => self.render(),
            Event::UserEvent(UserEvent::Action(action)) => self.action(action),
            Event::UserEvent(UserEvent::Error(error)) => {
                emit(serde_json::json!({"event":"error","message":error}))
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => self.hide(true),
            Event::WindowEvent {
                event: WindowEvent::KeyboardInput { event, .. },
                ..
            } if event.logical_key == Key::Escape && event.state == ElementState::Pressed => {
                self.hide(true)
            }
            Event::WindowEvent {
                event: WindowEvent::Focused(focused),
                ..
            } => {
                if focused {
                    self.focus_acquired = true;
                    self.blur_since = None;
                }
                #[cfg(not(target_os = "windows"))]
                if !focused && self.focus_acquired {
                    self.blur_since = Some(Instant::now());
                }
            }
            Event::WindowEvent {
                event: WindowEvent::Resized(_),
                ..
            } => {
                let _ = self.webview.set_bounds(wry::Rect {
                    position: PhysicalPosition::new(0, 0).into(),
                    size: self.window.inner_size().into(),
                });
                if self.visible {
                    self.clamp();
                    self.remember_geometry();
                }
            }
            Event::WindowEvent {
                event: WindowEvent::Moved(_),
                ..
            } if self.visible => {
                self.clamp();
                self.remember_geometry();
            }
            _ => {}
        }
        if self.visible && !matches!(*flow, ControlFlow::Exit) {
            if self
                .geometry_dirty
                .is_some_and(|time| time.elapsed() > Duration::from_millis(200))
            {
                self.preferences.save(&self.preferences_path);
                self.geometry_dirty = None;
            }
            #[cfg(target_os = "windows")]
            {
                if is_popup_foreground(&self.window) {
                    self.focus_acquired = true;
                    self.blur_since = None;
                } else if self.focus_acquired && self.blur_since.is_none() {
                    self.blur_since = Some(Instant::now());
                }
            }
            if self.dismiss_on_blur
                && !self.preferences.pinned
                && self.shown_at.elapsed() > Duration::from_millis(600)
                && self
                    .blur_since
                    .is_some_and(|time| time.elapsed() > Duration::from_millis(300))
            {
                self.hide(false);
            }
            if self.visible {
                *flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(100));
            }
        }
    }
    fn pin(&mut self, pinned: bool) {
        self.preferences.pinned = pinned;
        self.preferences.save(&self.preferences_path);
        self.blur_since = None;
        let _ = self
            .webview
            .evaluate_script(&format!("window.setPinned({pinned})"));
        emit(serde_json::json!({"event":"pinned","pinned":pinned,"visible":self.visible}));
    }
    fn action(&mut self, action: BridgeAction) {
        match action.action.as_str() {
            "close" => self.hide(true),
            "pin" => self.pin(!self.preferences.pinned),
            "ready" => {
                let _ = self.proxy.send_event(UserEvent::PageReady);
            }
            "dictionary" | "tab" => {
                if let Some(id) = action.dictionary_id {
                    self.select(id);
                }
            }
            "lookup" => {
                if let Some(query) = action.query {
                    self.lookup(query, None);
                }
            }
            "entry" => {
                if let (Some(id), Some(query)) = (action.dictionary_id, action.query) {
                    self.lookup(query, Some(id));
                }
            }
            "sound" => {
                if let (Some(id), Some(key)) = (action.dictionary_id, action.key) {
                    let catalog = Arc::clone(&self.catalog);
                    let proxy = self.proxy.clone();
                    std::thread::spawn(move || {
                        if let Err(error) = play_sound(&catalog, &id, &key) {
                            let _ = proxy.send_event(UserEvent::Error(format!("{error:#}")));
                        }
                    });
                }
            }
            "drag" => {
                self.preferences.positioned = true;
                let _ = self.window.drag_window();
                self.clamp();
                self.remember_geometry();
            }
            "resize" => {
                if let Some(direction) = action.direction.as_deref().and_then(resize_direction) {
                    let _ = self.window.drag_resize_window(direction);
                    self.clamp();
                    self.remember_geometry();
                }
            }
            _ => {}
        }
    }
}

fn resize_direction(value: &str) -> Option<ResizeDirection> {
    Some(match value {
        "n" => ResizeDirection::North,
        "ne" => ResizeDirection::NorthEast,
        "e" => ResizeDirection::East,
        "se" => ResizeDirection::SouthEast,
        "s" => ResizeDirection::South,
        "sw" => ResizeDirection::SouthWest,
        "w" => ResizeDirection::West,
        "nw" => ResizeDirection::NorthWest,
        _ => return None,
    })
}

fn work_area(window: &Window, anchor: (i32, i32)) -> Bounds {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::{
            Foundation::POINT,
            Graphics::Gdi::{
                GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
            },
        };
        let monitor = unsafe {
            MonitorFromPoint(
                POINT {
                    x: anchor.0,
                    y: anchor.1,
                },
                MONITOR_DEFAULTTONEAREST,
            )
        };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
            return Bounds {
                x: info.rcWork.left,
                y: info.rcWork.top,
                width: (info.rcWork.right - info.rcWork.left).max(1) as u32,
                height: (info.rcWork.bottom - info.rcWork.top).max(1) as u32,
            };
        }
    }
    let monitor = window
        .available_monitors()
        .find(|m| {
            let p = m.position();
            let s = m.size();
            anchor.0 >= p.x
                && anchor.1 >= p.y
                && anchor.0 < i64::from(p.x).saturating_add(i64::from(s.width)) as i32
                && anchor.1 < i64::from(p.y).saturating_add(i64::from(s.height)) as i32
        })
        .or_else(|| window.current_monitor());
    if let Some(monitor) = monitor {
        let p = monitor.position();
        let s = monitor.size();
        Bounds {
            x: p.x,
            y: p.y,
            width: s.width,
            height: s.height,
        }
    } else {
        Bounds {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }
    }
}

#[cfg(target_os = "windows")]
fn is_popup_foreground(window: &Window) -> bool {
    use tao::platform::windows::WindowExtWindows;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GA_ROOT, GetAncestor, GetForegroundWindow, IsChild,
    };
    let hwnd = window.hwnd() as windows_sys::Win32::Foundation::HWND;
    let foreground = unsafe { GetForegroundWindow() };
    foreground == hwnd
        || unsafe { IsChild(hwnd, foreground) } != 0
        || unsafe { GetAncestor(foreground, GA_ROOT) } == hwnd
}

fn remove_native_border(window: &Window) {
    #[cfg(target_os = "windows")]
    {
        use tao::platform::windows::WindowExtWindows;
        use windows_sys::Win32::Graphics::Dwm::{
            DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DwmSetWindowAttribute,
        };
        window.set_undecorated_shadow(false);
        let color = DWMWA_COLOR_NONE;
        // Older Windows versions may not implement this attribute; Tao's
        // decoration/shadow settings still remove the classic non-client frame.
        unsafe {
            DwmSetWindowAttribute(
                window.hwnd() as _,
                DWMWA_BORDER_COLOR as u32,
                (&color as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            );
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = window;
}

#[cfg(test)]
mod tests {
    use super::*;
    fn work(sequence: u64, id: &str, priority: bool) -> Work {
        Work {
            sequence,
            query: "猫".into(),
            dictionary_id: id.into(),
            priority,
        }
    }
    #[test]
    fn background_queue_retains_all_dictionaries_and_can_prioritize_a_jump() {
        let mut queue = WorkQueue::default();
        for id in ["first", "second", "third"] {
            queue.enqueue(work(1, id, false));
        }
        queue.enqueue(work(1, "third", true));
        assert_eq!(
            queue
                .pending
                .iter()
                .map(|w| w.dictionary_id.as_str())
                .collect::<Vec<_>>(),
            ["third", "first", "second"]
        );
        queue.completed.insert("third".into());
        queue.pending.pop_front();
        queue.enqueue(work(1, "third", true));
        assert_eq!(queue.pending.len(), 2);
    }
    #[test]
    fn new_word_cancels_old_queued_work_without_losing_new_dictionaries() {
        let mut queue = WorkQueue::default();
        queue.enqueue(work(4, "old", false));
        queue.completed.insert("first".into());
        queue.enqueue(work(5, "first", false));
        queue.enqueue(work(5, "next", false));
        assert!(queue.completed.is_empty());
        assert_eq!(queue.pending.len(), 2);
        assert!(queue.pending.iter().all(|work| work.sequence == 5));
    }
    #[test]
    fn saved_preferences_reject_nonfinite_and_oversized_dimensions() {
        let directory =
            std::env::temp_dir().join(format!("yomi-popup-pref-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&directory);
        let path = directory.join("popup.json");
        std::fs::write(&path, br#"{"width":99999,"height":-5,"pinned":true}"#).unwrap();
        let value = Preferences::load(&path);
        assert_eq!(value.width, 1600.0);
        assert_eq!(value.height, 180.0);
        assert!(value.pinned);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir(directory);
    }
    #[test]
    fn all_eight_resize_handles_have_a_native_direction() {
        for direction in ["n", "ne", "e", "se", "s", "sw", "w", "nw"] {
            assert!(resize_direction(direction).is_some());
        }
        assert!(resize_direction("outside").is_none());
    }
}
