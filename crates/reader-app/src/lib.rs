mod actions;
mod app;
mod audiobook_ui;
mod book_images;
mod bootstrap;
mod cef_geometry;
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

#[derive(Clone, Copy, Debug, PartialEq)]
struct DiagnosticViewport {
    size: [f32; 2],
    zoom: f32,
}

fn parse_diagnostic_viewport(value: &str) -> anyhow::Result<DiagnosticViewport> {
    let (size, zoom) = value
        .split_once('@')
        .ok_or_else(|| anyhow::anyhow!("Diagnostic viewport requires WIDTHxHEIGHT@ZOOM"))?;
    let (width, height) = size
        .split_once('x')
        .ok_or_else(|| anyhow::anyhow!("Diagnostic viewport requires WIDTHxHEIGHT@ZOOM"))?;
    let (width, height, zoom) = (
        width.parse::<u32>()?,
        height.parse::<u32>()?,
        zoom.parse::<f32>()?,
    );
    anyhow::ensure!(
        (820..=4096).contains(&width)
            && (580..=4096).contains(&height)
            && (0.75..=3.0).contains(&zoom),
        "Diagnostic viewport dimensions or zoom are outside the test range"
    );
    Ok(DiagnosticViewport {
        size: [width as f32, height as f32],
        zoom,
    })
}

pub fn run_native_shell() -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    latency::initialize(started);
    latency::event("process_started", 0);
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Only isolated screenshot/performance runs may alter the diagnostic
    // viewport. Normal reader launches retain the platform's native scale.
    let diagnostic = if args.iter().any(|arg| arg == "--data-dir")
        && args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--screenshot" | "--perf-output"))
    {
        std::env::var("VOBR_DIAGNOSTIC_VIEWPORT")
            .ok()
            .map(|value| parse_diagnostic_viewport(&value))
            .transpose()?
    } else {
        None
    };
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
            .with_inner_size(diagnostic.map_or([1280.0, 760.0], |value| value.size))
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
        Box::new(move |cc| {
            if let Some(diagnostic) = diagnostic {
                cc.egui_ctx.set_zoom_factor(diagnostic.zoom);
            }
            Ok(Box::new(app::ReaderApp::new(cc, &args)?))
        }),
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

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    #[test]
    fn diagnostic_viewport_accepts_odd_dimensions_and_fractional_zoom() {
        assert_eq!(
            parse_diagnostic_viewport("1281x761@1.25").unwrap(),
            DiagnosticViewport {
                size: [1281., 761.],
                zoom: 1.25
            }
        );
        for invalid in [
            "1281x761",
            "0x761@1",
            "1281x100@1",
            "9000x761@1",
            "1281x761@NaN",
            "1281x761@0",
            "1281x761@5",
        ] {
            assert!(parse_diagnostic_viewport(invalid).is_err(), "{invalid}");
        }
    }
}
