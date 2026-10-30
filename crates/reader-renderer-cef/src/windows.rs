use crate::{Command, NavigatorEvent, RendererError, command_script, decode_events};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};
use welding::{CefSurfaceEvent, CefSurfaceProducer, NavigationEvent, WebRequestId};

/// CEF's bootstrap owns these pointers until RunWinMain returns. This wrapper
/// deliberately has no direct/unsandboxed entry point.
pub struct SandboxBootstrap<'a> {
    context: welding::CefWindowsSandboxContext<'a>,
    cef_dir: PathBuf,
}
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
    _bootstrap: SandboxBootstrap<'a>,
}
#[derive(Clone)]
pub struct Runtime<'a>(Rc<RuntimeInner<'a>>);

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
    DocumentLoaded { url: String },
    Navigator(NavigatorEvent),
    ExternalLink { url: String, user_gesture: bool },
    Error(String),
}
pub struct GpuBookSurface<'a> {
    producer: welding::windows_cef::WindowsCefProducer,
    current: Option<welding::ImportedTexture>,
    host: welding::HostWgpuContext,
    pending: VecDeque<SurfaceEvent>,
    poll_in_flight: bool,
    poll_since: Instant,
    next_poll: Instant,
    navigator_installed: bool,
    _runtime: Runtime<'a>,
}
impl<'a> GpuBookSurface<'a> {
    pub fn new(
        runtime: &Runtime<'a>,
        device: wgpu::Device,
        queue: wgpu::Queue,
        options: SurfaceOptions,
    ) -> Result<Self, RendererError> {
        let host = welding::HostWgpuContext::new(device, queue);
        if host.backend != welding::InteropBackend::Dx12 {
            return Err(RendererError::Platform(
                "Windows book GPU surface requires the host DX12 backend".into(),
            ));
        }
        let producer = welding::windows_cef::WindowsCefProducer::new(
            &runtime.0.cef,
            welding::windows_cef::WindowsCefConfig {
                surface: welding::CefSurfaceConfig {
                    initial_url: options.initial_url,
                    initial_size: winit_size(options.physical_size),
                    scale_factor: options.scale_factor,
                    background_color: Some(options.background),
                    user_data_dir: options.profile,
                    devtools_protocol: false,
                    ..Default::default()
                },
            },
            &host,
        )
        .map_err(platform)?;
        Ok(Self {
            producer,
            current: None,
            host,
            pending: VecDeque::new(),
            poll_in_flight: false,
            poll_since: Instant::now(),
            next_poll: Instant::now(),
            navigator_installed: false,
            _runtime: runtime.clone(),
        })
    }
    pub fn navigate(&mut self, url: &str) -> Result<(), RendererError> {
        self.navigator_installed = false;
        self.poll_in_flight = false;
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
            .map_err(platform)
    }
    /// Nonblocking: call from the native frame loop. ImportedTexture remains
    /// alive until replaced; register/update its view with egui-wgpu, not via a
    /// CPU ColorImage. Book bounds are independent of native chrome visibility.
    pub fn poll(&mut self) -> Result<bool, RendererError> {
        while let Some(event) = self.producer.poll_web_event() {
            match event {
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
                _ => {}
            }
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
            self.next_poll = Instant::now() + Duration::from_millis(16);
        }
        let Some(frame) = self.producer.acquire_frame(&self.host).map_err(platform)? else {
            return Ok(false);
        };
        self.current = Some(frame);
        Ok(true)
    }
    pub fn frame(&self) -> Option<&welding::ImportedTexture> {
        self.current.as_ref()
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
        self.producer.close().map_err(platform)
    }
    pub fn is_closed(&self) -> bool {
        self.producer.is_closed()
    }
}
const DRAIN_ID: u64 = 0x564f425200000001;
fn platform(error: impl std::fmt::Display) -> RendererError {
    RendererError::Platform(error.to_string())
}
fn winit_size(size: [u32; 2]) -> dpi::PhysicalSize<u32> {
    dpi::PhysicalSize::new(size[0].max(1), size[1].max(1))
}
