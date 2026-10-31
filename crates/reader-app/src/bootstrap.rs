//! Native application entry and the process-wide, main-thread CEF lifetime.
use anyhow::{Context, Result};
use reader_renderer_cef::Runtime;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
};

thread_local! {
    static RUNTIME: RefCell<Option<Runtime<'static>>> = const { RefCell::new(None) };
    #[cfg(windows)]
    static BOOTSTRAP: RefCell<Option<reader_renderer_cef::SandboxBootstrap<'static>>> = const { RefCell::new(None) };
}

pub fn runtime(data_dir: &Path) -> Result<Runtime<'static>> {
    RUNTIME.with(|slot| {
        if let Some(runtime) = slot.borrow().as_ref() {
            return Ok(runtime.clone());
        }
        let cache = data_dir
            .join("chromium")
            .join(std::process::id().to_string());
        std::fs::create_dir_all(&cache)?;
        #[cfg(windows)]
        let runtime = BOOTSTRAP.with(|bootstrap| {
            bootstrap
                .borrow_mut()
                .take()
                .context("The book renderer requires the bundled CEF bootstrap executable")?
                .initialize(&cache)
                .map_err(anyhow::Error::from)
        })?;
        #[cfg(not(windows))]
        let runtime =
            Runtime::initialize_sandboxed(cef_directory()?, &cache, Some(helper_path()?))?;
        *slot.borrow_mut() = Some(runtime.clone());
        crate::latency::event("chromium_runtime_ready", 0);
        Ok(runtime)
    })
}

fn shutdown() {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while crate::cef_resources::pending_resource_reads() > 0 && std::time::Instant::now() < deadline
    {
        RUNTIME.with(|slot| {
            if let Some(runtime) = slot.borrow().as_ref() {
                runtime.pump();
            }
        });
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    if crate::cef_resources::pending_resource_reads() > 0 {
        // Process exit will reclaim this state. CEF must outlive a resource
        // callback even when a local file read is unexpectedly blocked.
        RUNTIME.with(|slot| {
            if let Some(runtime) = slot.borrow_mut().take() {
                std::mem::forget(runtime);
            }
        });
        #[cfg(windows)]
        BOOTSTRAP.with(|slot| {
            if let Some(bootstrap) = slot.borrow_mut().take() {
                std::mem::forget(bootstrap);
            }
        });
        return;
    }
    RUNTIME.with(|slot| {
        slot.borrow_mut().take();
    });
    #[cfg(windows)]
    BOOTSTRAP.with(|slot| {
        slot.borrow_mut().take();
    });
}

fn cef_directory() -> Result<PathBuf> {
    let executable = std::env::current_exe()?;
    let directory = executable
        .parent()
        .context("Application directory is unavailable")?;
    #[cfg(target_os = "macos")]
    {
        let contents = directory
            .parent()
            .context("App Contents directory is unavailable")?;
        let enclosing = contents.parent().and_then(Path::parent);
        if let Some(frameworks) =
            enclosing.filter(|path| path.file_name().is_some_and(|name| name == "Frameworks"))
        {
            return Ok(frameworks.to_owned());
        }
        return Ok(contents.join("Frameworks"));
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(directory.to_owned())
    }
}

#[cfg(windows)]
pub unsafe fn windows_entry(instance: *mut std::ffi::c_void, sandbox_info: *mut u8) -> i32 {
    let result = (|| -> Result<i32> {
        // SAFETY: RunWinMain retains the bootstrap-owned context until the
        // native loop and every runtime/surface below have been dropped.
        let bootstrap = unsafe {
            reader_renderer_cef::SandboxBootstrap::from_entrypoint(
                instance,
                sandbox_info,
                cef_directory()?,
            )?
        };
        if let Some(code) = bootstrap.dispatch_subprocess()? {
            return Ok(code);
        }
        BOOTSTRAP.with(|slot| {
            *slot.borrow_mut() = Some(bootstrap);
        });
        let result = crate::run_native_shell();
        shutdown();
        result?;
        Ok(0)
    })();
    result.unwrap_or_else(|error| {
        eprintln!("{error:#}");
        1
    })
}

pub fn direct_entry() -> Result<()> {
    if std::env::args().any(|argument| argument == "--native-renderer") {
        return crate::run_native_shell();
    }
    #[cfg(windows)]
    {
        // The shipped executable is CEF's sandbox bootstrap, which enters the
        // client DLL above. A developer binary never silently disables it.
        anyhow::bail!("Run the bundled Voice Over Book Reader executable (scripts/ci-package.py)");
    }
    #[cfg(not(windows))]
    {
        if let Some(code) = Runtime::dispatch_subprocess(&cef_directory()?)? {
            std::process::exit(code);
        }
        let result = crate::run_native_shell();
        shutdown();
        result
    }
}

pub fn configure_gpu(native: &mut eframe::NativeOptions) -> Result<()> {
    use eframe::{
        egui_wgpu::{WgpuSetup, WgpuSetupCreateNew},
        wgpu,
    };
    let mut setup = WgpuSetupCreateNew::without_display_handle();
    #[cfg(windows)]
    {
        setup.instance_descriptor.backends = wgpu::Backends::DX12;
    }
    #[cfg(target_os = "macos")]
    {
        setup.instance_descriptor.backends = wgpu::Backends::METAL;
    }
    #[cfg(target_os = "linux")]
    {
        setup.instance_descriptor.backends = wgpu::Backends::VULKAN;
    }
    // Match the native frame import device. No optional per-driver feature is
    // needed by the book compositor, or by egui's standard renderer.
    setup.device_descriptor = std::sync::Arc::new(|_| wgpu::DeviceDescriptor {
        label: Some("reader-native-and-chromium"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
    });
    native.wgpu_options.wgpu_setup = WgpuSetup::CreateNew(setup);
    #[cfg(target_os = "linux")]
    {
        let (instance, adapter, device, queue) = reader_renderer_cef::create_linux_gpu_context()?;
        native.wgpu_options.wgpu_setup =
            WgpuSetup::Existing(eframe::egui_wgpu::WgpuSetupExisting {
                instance,
                adapter,
                device,
                queue,
            });
    }
    Ok(())
}

#[cfg(not(windows))]
fn helper_path() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        return Ok(cef_directory()?.join(
            "voice-over-book-reader Helper.app/Contents/MacOS/voice-over-book-reader Helper",
        ));
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(cef_directory()?.join("voice-over-book-reader-cef-helper"))
    }
}

pub fn run_helper() -> Result<()> {
    #[cfg(not(windows))]
    std::process::exit(Runtime::run_subprocess_helper(&cef_directory()?)?);
    #[cfg(windows)]
    anyhow::bail!("Windows renderer subprocesses use the sandbox bootstrap executable");
}
