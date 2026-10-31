use crate::{Command, NavigatorEvent, RendererError, command_script, decode_events};
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
pub struct GpuBookSurface<'a> {
    producer: welding::PlatformCefProducer,
    current: Option<welding::ImportedTexture>,
    popup: Option<welding::PopupSurface>,
    host: welding::HostWgpuContext,
    pending: VecDeque<SurfaceEvent>,
    poll_in_flight: bool,
    poll_since: Instant,
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
            next_poll: Instant::now(),
            active_until: Instant::now() + Duration::from_secs(2),
            navigator_installed: false,
            closing: false,
            closed: false,
            _runtime: runtime.clone(),
        })
    }
    pub fn navigate(&mut self, url: &str) -> Result<(), RendererError> {
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
        self.producer
            .execute_script(&command_script(command)?, "vobr://native-command")
            .map_err(platform)?;
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
                    self.closed = true;
                }
                CefSurfaceEvent::Navigation(NavigationEvent::LoadEnd { url, .. }) => {
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
        if self.navigator_installed && !self.poll_in_flight && Instant::now() >= self.next_poll {
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
        if id == DRAIN_ID {
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
