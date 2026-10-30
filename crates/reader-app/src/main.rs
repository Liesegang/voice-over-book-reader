#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod actions;
mod app;
mod audiobook_ui;
mod book_images;
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
mod theme;

fn main() -> anyhow::Result<()> {
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
    let native = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 760.0])
            .with_decorations(false)
            .with_min_inner_size([820.0, 580.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Voice Over Book Reader",
        native,
        Box::new(move |cc| Ok(Box::new(app::ReaderApp::new(cc, &args)?))),
    )
    .map_err(|error| anyhow::anyhow!("{error}"))
}
