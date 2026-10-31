use crate::{Command, NavigatorEvent, RendererError, Revision, command_script, decode_events};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};
use welding::{CefSurfaceEvent, CefSurfaceProducer, NavigationEvent, WebRequestId};

/// Build the one Vulkan device shared by the native UI and imported book
/// textures. Graft enables the foreign-queue and DMA-BUF native extensions;
/// ordinary wgpu::Adapter::request_device is insufficient for this import.
#[cfg(target_os = "linux")]
pub fn create_linux_gpu_context()
-> Result<(wgpu::Instance, wgpu::Adapter, wgpu::Device, wgpu::Queue), RendererError> {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .map_err(platform)?;
        let features = adapter.features() & wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF;
        let (device, queue) = welding::build_dmabuf_capable_device(
            &adapter,
            &wgpu::DeviceDescriptor {
                label: Some("vobr-native-and-book-device"),
                required_features: features,
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
            },
        )
        .map_err(platform)?;
        Ok((instance, adapter, device, queue))
    })
}

/// CEF's bootstrap owns these pointers until RunWinMain returns. This wrapper
/// deliberately has no direct/unsandboxed entry point.
#[cfg(windows)]
pub struct SandboxBootstrap<'a> {
    context: welding::CefWindowsSandboxContext<'a>,
    cef_dir: PathBuf,
}
#[cfg(windows)]
impl<'a> SandboxBootstrap<'a> {
    /// # Safety
    /// Both pointers must come unchanged from CEF RunWinMain and remain valid
    /// for 'a. Drop all returned runtimes/surfaces before that entry point ends.
    pub unsafe fn from_entrypoint(
        instance: *mut std::ffi::c_void,
        sandbox_info: *mut u8,
        cef_dir: impl Into<PathBuf>,
    ) -> Result<Self, RendererError> {
        let context =
            unsafe { welding::CefWindowsSandboxContext::from_raw(instance, sandbox_info) }
                .map_err(platform)?;
        Ok(Self {
            context,
            cef_dir: cef_dir.into(),
        })
    }
    /// Call before native GUI initialization in every CEF subprocess. Some(code)
    /// is a child-process exit code; it must never enter the application's UI.
    pub fn dispatch_subprocess(&self) -> Result<Option<i32>, RendererError> {
        self.context
            .execute_process(&self.cef_dir)
            .map_err(platform)
    }
    /// May be deferred until the native shell has painted. CEF stays alive across
    /// chapter/book navigation. Register resource factories after this returns,
    /// before creating the first GpuBookSurface.
    pub fn initialize(self, cache_root: impl AsRef<Path>) -> Result<Runtime<'a>, RendererError> {
        let mut config =
            welding::CefRuntimeConfig::new(&self.cef_dir, welding::CefSandboxMode::Sandboxed);
        config.cache_path = Some(cache_root.as_ref().to_path_buf());
        config.user_agent_product = Some("VoiceOverBookReader/0.3".into());
        config.command_line_switches = book_runtime_switches();
        let cef = self.context.initialize(config).map_err(platform)?;
        Ok(Runtime(Rc::new(RuntimeInner {
            cef,
            _bootstrap: self,
        })))
    }
}
// CefRuntime's derived Clone must NOT be used: every clone calls shutdown on
// Drop upstream. One Rc owns exactly one runtime and one shutdown.
struct RuntimeInner<'a> {
    cef: welding::CefRuntime,
    #[cfg(windows)]
    _bootstrap: SandboxBootstrap<'a>,
    #[cfg(not(windows))]
    _lifetime: std::marker::PhantomData<&'a ()>,
}
#[derive(Clone)]
pub struct Runtime<'a>(Rc<RuntimeInner<'a>>);

impl Runtime<'_> {
    /// Pump CEF from the native host event loop on macOS/Linux. Windows uses
    /// CEF's dedicated UI thread and this is intentionally a no-op there.
    pub fn pump(&self) {
        self.0.cef.do_message_loop_work();
    }
}

#[cfg(not(windows))]
impl Runtime<'static> {
    /// Entry point for the dedicated bundled CEF helper, before native UI or
    /// application settings are created. This also initializes macOS sandbox.
    pub fn run_subprocess_helper(cef_dir: &Path) -> Result<i32, RendererError> {
        welding::CefRuntime::execute_helper_from(cef_dir).map_err(platform)
    }
    /// Dispatch CEF children before constructing any native application state.
    pub fn dispatch_subprocess(cef_dir: &Path) -> Result<Option<i32>, RendererError> {
        welding::CefRuntime::execute_process_from(cef_dir, welding::CefSandboxMode::Sandboxed)
            .map_err(platform)
    }

    /// Initialize the bundled, sandboxed engine in the browser process. macOS
    /// callers pass the matching helper executable from the application bundle.
    pub fn initialize_sandboxed(
        cef_dir: impl Into<PathBuf>,
        cache_root: impl AsRef<Path>,
        subprocess_path: Option<PathBuf>,
    ) -> Result<Self, RendererError> {
        let mut config =
            welding::CefRuntimeConfig::new(cef_dir.into(), welding::CefSandboxMode::Sandboxed);
        config.cache_path = Some(cache_root.as_ref().to_path_buf());
        config.browser_subprocess_path = subprocess_path;
        config.user_agent_product = Some("VoiceOverBookReader/0.3".into());
        config.command_line_switches = book_runtime_switches();
        let cef = welding::CefRuntime::initialize(config).map_err(platform)?;
        Ok(Self(Rc::new(RuntimeInner {
            cef,
            _lifetime: std::marker::PhantomData,
        })))
    }
}

pub struct SurfaceOptions {
    pub initial_url: String,
    pub physical_size: [u32; 2],
    pub scale_factor: f32,
    pub background: [u8; 3],
    pub profile: Option<PathBuf>,
}
impl Default for SurfaceOptions {
    fn default() -> Self {
        Self {
            initial_url: "about:blank".into(),
            physical_size: [1, 1],
            scale_factor: 1.,
            background: [250, 248, 242],
            profile: None,
        }
    }
}
#[derive(Debug)]
pub enum SurfaceEvent {
    DocumentLoaded {
        url: String,
    },
    Navigator(NavigatorEvent),
    Cursor(welding::CursorShape),
    ExternalLink {
        url: String,
        user_gesture: bool,
    },
    ScriptResult {
        id: u64,
        result: Result<String, String>,
    },
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PageRequest {
    id: u64,
    revision: Revision,
    index: u32,
}
struct PendingPage {
    request: PageRequest,
    sent: Instant,
}
#[derive(Default)]
struct PageRequests {
    sequence: u32,
    pending: Option<PendingPage>,
}
impl PageRequests {
    fn begin(
        &mut self,
        revision: Revision,
        index: u32,
        now: Instant,
    ) -> Result<Option<PageRequest>, RendererError> {
        if self.pending.as_ref().is_some_and(|pending| {
            pending.request.revision == revision && pending.request.index == index
        }) {
            return Ok(None);
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| RendererError::Protocol("Page request identifiers exhausted".into()))?;
        let request = PageRequest {
            id: PAGE_REQUEST_PREFIX | u64::from(self.sequence),
            revision,
            index,
        };
        self.pending = Some(PendingPage { request, sent: now });
        Ok(Some(request))
    }
    fn complete(&mut self, id: u64) -> Option<PageRequest> {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.request.id == id)
        {
            self.pending.take().map(|pending| pending.request)
        } else {
            None
        }
    }
    fn expire(&mut self, now: Instant) -> Option<PageRequest> {
        if self.pending.as_ref().is_some_and(|pending| {
            now.saturating_duration_since(pending.sent) >= Duration::from_secs(5)
        }) {
            self.pending.take().map(|pending| pending.request)
        } else {
            None
        }
    }
    fn cancel(&mut self) {
        self.pending = None;
    }
    fn reserved(id: u64) -> bool {
        id & 0xffff_ffff_0000_0000 == PAGE_REQUEST_PREFIX
    }
}

pub struct GpuBookSurface<'a> {
    producer: welding::PlatformCefProducer,
    current: Option<welding::ImportedTexture>,
    popup: Option<welding::PopupSurface>,
    host: welding::HostWgpuContext,
    pending: VecDeque<SurfaceEvent>,
    poll_in_flight: bool,
    poll_since: Instant,
    page_requests: PageRequests,
    next_poll: Instant,
    active_until: Instant,
    navigator_installed: bool,
    closing: bool,
    closed: bool,
    _runtime: Runtime<'a>,
}
impl<'a> GpuBookSurface<'a> {
    pub fn new(
        runtime: &Runtime<'a>,
        device: wgpu::Device,
        queue: wgpu::Queue,
        options: SurfaceOptions,
    ) -> Result<Self, RendererError> {
        if !options.scale_factor.is_finite() || options.scale_factor <= 0. {
            return Err(RendererError::Protocol("Invalid display scale".into()));
        }
        let host = welding::HostWgpuContext::new(device, queue);
        #[cfg(windows)]
        if host.backend != welding::InteropBackend::Dx12 {
            return Err(RendererError::Platform(
                "Windows book GPU surface requires the host DX12 backend".into(),
            ));
        }
        #[cfg(target_os = "macos")]
        if host.backend != welding::InteropBackend::Metal {
            return Err(RendererError::Platform(
                "macOS book GPU surface requires Metal".into(),
            ));
        }
        #[cfg(target_os = "linux")]
        if host.backend != welding::InteropBackend::Vulkan {
            return Err(RendererError::Platform(
                "Linux book GPU surface requires a DMABUF-capable Vulkan device".into(),
            ));
        }
        let config = welding::PlatformCefConfig {
            surface: welding::CefSurfaceConfig {
                initial_url: options.initial_url,
                initial_size: winit_size(options.physical_size),
                scale_factor: options.scale_factor,
                background_color: Some(options.background),
                user_data_dir: options.profile,
                devtools_protocol: false,
                ..Default::default()
            },
        };
        #[cfg(windows)]
        let producer =
            welding::PlatformCefProducer::new(&runtime.0.cef, config, &host).map_err(platform)?;
        #[cfg(not(windows))]
        let producer =
            welding::PlatformCefProducer::new(&runtime.0.cef, config).map_err(platform)?;
        Ok(Self {
            producer,
            current: None,
            popup: None,
            host,
            pending: VecDeque::new(),
            poll_in_flight: false,
            poll_since: Instant::now(),
            page_requests: PageRequests::default(),
            next_poll: Instant::now(),
            active_until: Instant::now() + Duration::from_secs(2),
            navigator_installed: false,
            closing: false,
            closed: false,
            _runtime: runtime.clone(),
        })
    }
    pub fn navigate(&mut self, url: &str) -> Result<(), RendererError> {
        self.page_requests.cancel();
        self.navigator_installed = false;
        self.poll_in_flight = false;
        self.current = None;
        self.popup = None;
        self.active_until = Instant::now() + Duration::from_secs(2);
        self.producer.navigate_to_url(url).map_err(platform)
    }
    pub fn resize(
        &mut self,
        physical_size: [u32; 2],
        scale_factor: f32,
    ) -> Result<(), RendererError> {
        if !scale_factor.is_finite() || scale_factor <= 0. {
            return Err(RendererError::Protocol("Invalid display scale".into()));
        }
        self.page_requests.cancel();
        self.producer
            .set_scale_factor(scale_factor)
            .map_err(platform)?;
        self.producer
            .resize(winit_size(physical_size))
            .map_err(platform)
    }
    /// Original DOM has loaded. This only injects the isolated reader helper;
    /// Configure supplies source bindings and triggers font/image-ready layout.
    pub fn install_navigator(&mut self) -> Result<(), RendererError> {
        self.page_requests.cancel();
        self.producer
            .execute_script(crate::NAVIGATOR_SCRIPT, "vobr://navigator")
            .map_err(platform)?;
        self.navigator_installed = true;
        self.next_poll = Instant::now();
        self.active_until = Instant::now() + Duration::from_secs(2);
        Ok(())
    }
    pub fn command(&mut self, command: &Command) -> Result<(), RendererError> {
        if !self.navigator_installed {
            return Err(RendererError::Protocol(
                "Navigator is not installed on this document".into(),
            ));
        }
        if let Command::Page { revision, index } = command {
            if let Some(request) = self
                .page_requests
                .begin(*revision, *index, Instant::now())?
            {
                let script = page_reply_script(command)?;
                if let Err(error) = self
                    .producer
                    .request_script_result(WebRequestId::new(request.id), &script)
                {
                    self.page_requests.cancel();
                    return Err(platform(error));
                }
            }
        } else {
            if matches!(
                command,
                Command::Configure { .. }
                    | Command::Locate { .. }
                    | Command::Fragment { .. }
                    | Command::Refresh { .. }
            ) {
                self.page_requests.cancel();
            }
            self.producer
                .execute_script(&command_script(command)?, "vobr://native-command")
                .map_err(platform)?;
        }
        self.next_poll = Instant::now();
        self.active_until = Instant::now() + Duration::from_secs(2);
        Ok(())
    }
    /// Nonblocking: call from the native frame loop. ImportedTexture remains
    /// alive until replaced; register/update its view with egui-wgpu, not via a
    /// CPU ColorImage. Book bounds are independent of native chrome visibility.
    pub fn poll(&mut self) -> Result<bool, RendererError> {
        self._runtime.pump();
        while let Some(event) = self.producer.poll_web_event() {
            match event {
                CefSurfaceEvent::Navigation(NavigationEvent::BrowserClosed) => {
                    self.page_requests.cancel();
                    self.closed = true;
                }
                CefSurfaceEvent::Navigation(NavigationEvent::LoadEnd { url, .. }) => {
                    self.page_requests.cancel();
                    self.navigator_installed = false;
                    self.pending.push_back(SurfaceEvent::DocumentLoaded { url });
                }
                CefSurfaceEvent::Navigation(NavigationEvent::LoadError { error_text, .. }) => {
                    self.pending.push_back(SurfaceEvent::Error(error_text))
                }
                CefSurfaceEvent::Navigation(NavigationEvent::NewWindowRequested {
                    url,
                    user_gesture,
                }) => self
                    .pending
                    .push_back(SurfaceEvent::ExternalLink { url, user_gesture }),
                CefSurfaceEvent::Navigation(NavigationEvent::ContentProcessTerminated {
                    error_string,
                    ..
                }) => self.pending.push_back(SurfaceEvent::Error(format!(
                    "Book renderer stopped: {error_string}"
                ))),
                CefSurfaceEvent::ScriptCompleted { id, result } if id.get() == DRAIN_ID => {
                    self.poll_in_flight = false;
                    match result {
                        Ok(json) => match decode_events(&json) {
                            Ok(events) => self
                                .pending
                                .extend(events.into_iter().map(SurfaceEvent::Navigator)),
                            Err(e) => self.pending.push_back(SurfaceEvent::Error(e.to_string())),
                        },
                        Err(e) => self.pending.push_back(SurfaceEvent::Error(e)),
                    }
                }
                CefSurfaceEvent::ScriptCompleted { id, result }
                    if PageRequests::reserved(id.get()) =>
                {
                    // A cancelled/superseded request must not complete a newer
                    // page, nor be exposed as an unrelated ScriptResult.
                    if let Some(request) = self.page_requests.complete(id.get()) {
                        match result {
                            Ok(json) => match decode_page_reply(request, &json) {
                                Ok(events) => self
                                    .pending
                                    .extend(events.into_iter().map(SurfaceEvent::Navigator)),
                                Err(error) => self
                                    .pending
                                    .push_back(SurfaceEvent::Error(error.to_string())),
                            },
                            Err(error) => self.pending.push_back(SurfaceEvent::Error(error)),
                        }
                    }
                }
                CefSurfaceEvent::ScriptCompleted { id, result } => {
                    self.pending.push_back(SurfaceEvent::ScriptResult {
                        id: id.get(),
                        result,
                    });
                }
                _ => {}
            }
        }
        if let Some(cursor) = self.producer.poll_cursor_shape() {
            self.pending.push_back(SurfaceEvent::Cursor(cursor));
        }
        if self.poll_in_flight && self.poll_since.elapsed() > Duration::from_secs(5) {
            self.poll_in_flight = false;
            self.pending.push_back(SurfaceEvent::Error(
                "Book navigator response timed out".into(),
            ));
        }
        if self.page_requests.expire(Instant::now()).is_some() {
            self.pending.push_back(SurfaceEvent::Error(
                "Book page acknowledgement timed out".into(),
            ));
        }
        if self.navigator_installed
            && !self.poll_in_flight
            && self.page_requests.pending.is_none()
            && Instant::now() >= self.next_poll
        {
            self.producer
                .request_script_result(WebRequestId::new(DRAIN_ID), crate::DRAIN_SCRIPT)
                .map_err(platform)?;
            self.poll_in_flight = true;
            self.poll_since = Instant::now();
            self.next_poll = Instant::now()
                + if Instant::now() < self.active_until {
                    Duration::from_millis(8)
                } else {
                    Duration::from_millis(100)
                };
        }
        let mut changed = false;
        if let Some(frame) = self.producer.acquire_frame(&self.host).map_err(platform)? {
            self.current = Some(frame);
            self.next_poll = Instant::now();
            changed = true;
        }
        if self.producer.popup_rect().is_none() {
            changed |= self.popup.take().is_some();
        } else if let Some(popup) = self.producer.acquire_popup(&self.host).map_err(platform)? {
            self.popup = Some(popup);
            changed = true;
        }
        Ok(changed)
    }
    pub fn frame(&self) -> Option<&welding::ImportedTexture> {
        self.current.as_ref()
    }
    pub fn popup(&self) -> Option<&welding::PopupSurface> {
        self.popup.as_ref()
    }
    /// This is an imported callback generation, not a monitor-present timestamp.
    pub fn frame_sequence(&self) -> u64 {
        self.current.as_ref().map_or(0, |frame| frame.generation)
    }
    /// Drop a queued callback before asking CEF for another paint. Return the
    /// generation barrier the host must exceed before presenting another turn.
    /// A JS acknowledgement alone is never treated as a painted book page.
    pub fn request_repaint(&mut self) -> Result<u64, RendererError> {
        let mut barrier = self.frame_sequence();
        if let Some(frame) = self.producer.acquire_native_frame() {
            barrier = barrier.max(frame.generation());
        }
        self.producer.request_repaint().map_err(platform)?;
        self.active_until = Instant::now() + Duration::from_secs(2);
        self.next_poll = Instant::now();
        Ok(barrier)
    }
    pub fn request_script_result(&mut self, id: u64, script: &str) -> Result<(), RendererError> {
        if id == DRAIN_ID || PageRequests::reserved(id) {
            return Err(RendererError::Protocol(
                "Reserved navigator request ID".into(),
            ));
        }
        self.producer
            .request_script_result(WebRequestId::new(id), script)
            .map_err(platform)
    }
    pub fn set_visible(&mut self, visible: bool) -> Result<(), RendererError> {
        self.producer.set_visible(visible).map_err(platform)
    }
    pub fn set_repaint_wake(&mut self, wake: Option<std::sync::Arc<dyn Fn() + Send + Sync>>) {
        self.producer.set_repaint_wake(wake);
    }
    pub fn move_focus(&mut self, direction: welding::FocusDirection) -> Result<(), RendererError> {
        self.producer.move_focus(direction).map_err(platform)
    }
    pub fn event(&mut self) -> Option<SurfaceEvent> {
        self.pending.pop_front()
    }
    pub fn send_mouse(&mut self, event: welding::MouseEvent) -> Result<(), RendererError> {
        self.producer.send_mouse_input(event).map_err(platform)
    }
    pub fn send_key(&mut self, event: welding::KeyEvent) -> Result<(), RendererError> {
        self.producer.send_keyboard_input(event).map_err(platform)
    }
    pub fn close(&mut self) -> Result<(), RendererError> {
        self.page_requests.cancel();
        if !self.closing && !self.closed {
            self.producer.close().map_err(platform)?;
            self.closing = true;
        }
        Ok(())
    }
    pub fn is_closed(&self) -> bool {
        #[cfg(windows)]
        if self.producer.is_closed() {
            return true;
        }
        self.closed
    }
}

impl Drop for GpuBookSurface<'_> {
    fn drop(&mut self) {
        self.producer.set_repaint_wake(None);
        let _ = self.close();
        // Closing is asynchronous. Keep the sole CefRuntime owner alive until
        // the callback releases this browser; no stale browser at CefShutdown.
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.is_closed() && Instant::now() < deadline {
            self._runtime.pump();
            while let Some(event) = self.producer.poll_web_event() {
                if matches!(
                    event,
                    CefSurfaceEvent::Navigation(NavigationEvent::BrowserClosed)
                ) {
                    self.closed = true;
                }
            }
            if !self.is_closed() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        if !self.is_closed() {
            // A stuck Chromium child must not make us call CefShutdown while
            // its browser is still alive. Keep its runtime until process exit
            // rather than invoking the CEF API with an invalid lifetime.
            std::mem::forget(self._runtime.clone());
            eprintln!("Book renderer did not close within its shutdown deadline");
        }
    }
}
const DRAIN_ID: u64 = 0x564f425200000001;
const PAGE_REQUEST_PREFIX: u64 = 0x564f425300000000;

fn page_reply_script(command: &Command) -> Result<String, RendererError> {
    if !matches!(command, Command::Page { .. }) {
        return Err(RendererError::Protocol(
            "Only synchronous Page commands support an immediate reply".into(),
        ));
    }
    let json = serde_json::to_string(command)
        .map_err(|error| RendererError::Protocol(error.to_string()))?;
    // Page's async-declared dispatch branch executes turn()/Position without
    // awaiting anything. A single evaluation task prevents a normal drain
    // from consuming the acknowledgement between dispatch and this drain.
    Ok(format!(
        "(()=>{{const book=globalThis.__vobrBook;if(!book)throw new Error('Book navigator is unavailable');book.dispatch({json});return book.drain();}})()"
    ))
}
fn decode_page_reply(
    request: PageRequest,
    json: &str,
) -> Result<Vec<NavigatorEvent>, RendererError> {
    let events = decode_events(json)?;
    let acknowledged=events.iter().any(|event|matches!(event,NavigatorEvent::Position{revision,page,..} if *revision==request.revision && *page==request.index));
    if !acknowledged
        && !events
            .iter()
            .any(|event| matches!(event, NavigatorEvent::Error { revision,.. } if *revision==request.revision))
    {
        return Err(RendererError::Protocol(
            "Page reply did not acknowledge its requested revision and target".into(),
        ));
    }
    Ok(events)
}
fn book_runtime_switches() -> Vec<(String, Option<String>)> {
    [
        "disable-background-networking",
        "disable-component-update",
        "disable-sync",
    ]
    .into_iter()
    .map(|name| (name.to_owned(), None))
    .collect()
}
fn platform(error: impl std::fmt::Display) -> RendererError {
    RendererError::Platform(error.to_string())
}
fn winit_size(size: [u32; 2]) -> dpi::PhysicalSize<u32> {
    dpi::PhysicalSize::new(size[0].max(1), size[1].max(1))
}

#[cfg(test)]
mod page_reply_tests {
    use super::*;

    #[test]
    fn page_request_coalesces_duplicates_and_rejects_superseded_or_repeated_replies() {
        let now = Instant::now();
        let mut requests = PageRequests::default();
        let first = requests.begin(Revision(3), 1, now).unwrap().unwrap();
        assert!(requests.begin(Revision(3), 1, now).unwrap().is_none());
        let second = requests.begin(Revision(3), 2, now).unwrap().unwrap();
        assert_ne!(first.id, second.id);
        assert!(requests.complete(first.id).is_none());
        assert_eq!(requests.complete(second.id), Some(second));
        assert!(requests.complete(second.id).is_none());
        assert!(PageRequests::reserved(first.id));
        assert!(!PageRequests::reserved(DRAIN_ID));
        assert!(!PageRequests::reserved(7001));
    }

    #[test]
    fn cancellation_and_timeout_do_not_allow_old_replies_to_complete_a_new_page() {
        let now = Instant::now();
        let mut requests = PageRequests::default();
        let old = requests.begin(Revision(1), 4, now).unwrap().unwrap();
        requests.cancel();
        let current = requests.begin(Revision(2), 4, now).unwrap().unwrap();
        assert!(requests.complete(old.id).is_none());
        assert!(requests.expire(now + Duration::from_secs(4)).is_none());
        assert_eq!(requests.expire(now + Duration::from_secs(5)), Some(current));
        let next = requests
            .begin(Revision(2), 5, now + Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert!(requests.complete(current.id).is_none());
        assert_eq!(requests.complete(next.id), Some(next));
        requests.sequence = u32::MAX;
        assert!(requests.begin(Revision(3), 1, now).is_err());
    }

    #[test]
    fn direct_page_reply_requires_its_target_but_preserves_other_queued_navigator_events() {
        let request = PageRequest {
            id: PAGE_REQUEST_PREFIX | 1,
            revision: Revision(9),
            index: 2,
        };
        let position = |revision, page| serde_json::json!({"type":"position","revision":revision,"chapter":0,"page":page,"pages":5,"start":null});
        let valid = serde_json::json!([position(9, 0), position(9, 2)]).to_string();
        assert_eq!(decode_page_reply(request, &valid).unwrap().len(), 2);
        for invalid in [position(8, 2), position(9, 3)] {
            assert!(decode_page_reply(request, &serde_json::json!([invalid]).to_string()).is_err());
        }
        assert!(decode_page_reply(request, "[]").is_err());
        let failed = r#"[{"type":"error","revision":9,"message":"invalid page"}]"#;
        assert!(matches!(
            &decode_page_reply(request, failed).unwrap()[0],
            NavigatorEvent::Error { .. }
        ));
        let stale_error = r#"[{"type":"error","revision":8,"message":"old error"}]"#;
        assert!(decode_page_reply(request, stale_error).is_err());
    }

    #[test]
    fn only_synchronous_page_commands_are_encoded_with_an_atomic_dispatch_and_drain() {
        let script = page_reply_script(&Command::Page {
            revision: Revision(12),
            index: 4,
        })
        .unwrap();
        assert!(script.contains(
            r#"book.dispatch({"type":"page","revision":12,"index":4});return book.drain();"#
        ));
        assert!(
            page_reply_script(&Command::Refresh {
                revision: Revision(12)
            })
            .is_err()
        );
    }
}
