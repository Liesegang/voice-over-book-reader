mod actions;
mod app;
mod audiobook_ui;
mod book_images;
mod bootstrap;
mod cef_perf;
mod cef_resources;
mod covers;
mod fonts;
mod icons;
mod latency;
mod layout;
mod library_ipc;
mod pagination;
mod pronunciation_settings;
mod reading_surface;
mod services_ui;
mod settings;
mod settings_controls;
mod speech_profiles;
mod speech_settings;
mod speech_smoke;
mod theme;

pub fn run_native_shell() -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    latency::initialize(started);
    latency::event("process_started", 0);
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|a| {
        a == "--reader"
            || a == "--demo"
            || a == "--settings"
            || a == "--screenshot"
            || (!a.starts_with('-') && std::path::Path::new(a).is_file())
    }) {
        let store = if let Some(i) = args.iter().position(|a| a == "--data-dir") {
            reader_library::LibraryStore::open(
                args.get(i + 1)
                    .ok_or_else(|| anyhow::anyhow!("--data-dir requires a path"))?,
            )?
        } else {
            reader_library::LibraryStore::open_default()?
        };
        if library_ipc::focus_existing(store.data_dir()) {
            return Ok(());
        }
    }
    let mut native = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 760.0])
            .with_decorations(false)
            .with_min_inner_size([820.0, 580.0]),
        ..Default::default()
    };
    if !args.iter().any(|arg| arg == "--native-renderer") {
        bootstrap::configure_gpu(&mut native)?;
    }
    eframe::run_native(
        "Voice Over Book Reader",
        native,
        Box::new(move |cc| Ok(Box::new(app::ReaderApp::new(cc, &args)?))),
    )
    .map_err(|error| anyhow::anyhow!("{error}"))
}

/// Windows CEF bootstrap owns the sandbox context throughout this call.
///
/// # Safety
/// Both pointers are passed unchanged from CEF's matching bootstrap executable.
#[cfg(windows)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn RunWinMain(
    instance: reader_renderer_cef::cef::sys::HINSTANCE,
    _command_line: *const u8,
    _show: i32,
    sandbox_info: *mut u8,
) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        bootstrap::windows_entry(instance.0.cast(), sandbox_info)
    }))
    .unwrap_or(199)
}

pub fn run() -> anyhow::Result<()> {
    bootstrap::direct_entry()
}

pub fn run_cef_helper() -> anyhow::Result<()> {
    bootstrap::run_helper()
}
