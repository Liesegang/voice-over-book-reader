#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
mod runtime;

use anyhow::{Context, Result};
use reader_dictionary_popup::{parse_resource_route, render_entry};
use reader_library::{DictionaryCatalog, DictionaryHit};
use std::{
    collections::HashMap,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use wry::http::{Response, header::CONTENT_TYPE};

type EntryPages = Arc<Mutex<HashMap<(String, u64), Arc<DictionaryHit>>>>;

struct Options {
    data_dir: PathBuf,
    query: String,
    x: i32,
    y: i32,
    parent_hwnd: Option<isize>,
    theme: String,
    accent: String,
    paper: Option<String>,
    ink: Option<String>,
    dismiss_on_blur: bool,
}

fn options() -> Result<Options> {
    let mut options = Options {
        data_dir: PathBuf::new(),
        query: String::new(),
        x: 160,
        y: 160,
        parent_hwnd: None,
        theme: "light".into(),
        accent: "2563eb".into(),
        paper: None,
        ink: None,
        dismiss_on_blur: true,
    };
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let value = arguments
            .next()
            .with_context(|| format!("Missing value after {argument}"))?;
        match argument.as_str() {
            "--data-dir" => options.data_dir = PathBuf::from(value),
            "--query" => options.query = value,
            "--x" => options.x = value.parse()?,
            "--y" => options.y = value.parse()?,
            "--parent-hwnd" => options.parent_hwnd = Some(value.parse()?),
            "--theme" => options.theme = value,
            "--accent" => options.accent = value.trim_start_matches('#').into(),
            "--paper" => options.paper = Some(value.trim_start_matches('#').into()),
            "--ink" => options.ink = Some(value.trim_start_matches('#').into()),
            "--dismiss-on-blur" => options.dismiss_on_blur = value.parse()?,
            _ => anyhow::bail!("Unknown option {argument}"),
        }
    }
    anyhow::ensure!(
        !options.data_dir.as_os_str().is_empty(),
        "--data-dir APP_DATA_DIR is required"
    );
    Ok(options)
}

fn main() -> Result<()> {
    if let Err(error) = runtime::run(options()?) {
        emit(serde_json::json!({"event":"error","message":format!("{error:#}")}));
        return Err(error);
    }
    Ok(())
}

fn resource_response(
    uri: &str,
    catalog: &Arc<Mutex<DictionaryCatalog>>,
    pages: &EntryPages,
) -> Response<Vec<u8>> {
    let result = (|| -> Result<(Vec<u8>, String)> {
        let route = parse_resource_route(uri)?;
        if let Some(ordinal) = route.resource.strip_prefix("__entry/") {
            let ordinal = ordinal.parse::<u64>()?;
            let pages = pages
                .lock()
                .map_err(|_| anyhow::anyhow!("Dictionary pages lock failed"))?;
            let hit = pages
                .get(&(route.dictionary_id, ordinal))
                .context("Dictionary entry is no longer active")?;
            return Ok((
                render_entry(hit).into_bytes(),
                "text/html; charset=utf-8".into(),
            ));
        }
        let bytes = catalog
            .lock()
            .map_err(|_| anyhow::anyhow!("Dictionary service lock failed"))?
            .resource(&route.dictionary_id, &route.resource)?
            .context("Dictionary resource not found")?;
        let mime = mime_guess::from_path(&route.resource)
            .first_or_octet_stream()
            .to_string();
        Ok((bytes, mime))
    })();
    let (status, bytes, mime) = match result {
        Ok((bytes, mime)) => (200, bytes, mime),
        Err(error) => (
            404,
            error.to_string().into_bytes(),
            "text/plain; charset=utf-8".into(),
        ),
    };
    Response::builder().status(status).header(CONTENT_TYPE,mime)
        .header("Content-Security-Policy","default-src 'self' data: blob: dictionary: http://dictionary.localhost https://dictionary.localhost; script-src 'self' 'unsafe-inline' 'unsafe-eval' dictionary: http://dictionary.localhost https://dictionary.localhost; style-src 'self' 'unsafe-inline' data: dictionary: http://dictionary.localhost https://dictionary.localhost; connect-src 'self'; frame-src 'self'; object-src 'none'")
        .body(bytes).unwrap()
}

fn play_sound(catalog: &Arc<Mutex<DictionaryCatalog>>, id: &str, key: &str) -> Result<()> {
    let bytes = catalog
        .lock()
        .map_err(|_| anyhow::anyhow!("Dictionary service lock failed"))?
        .resource(id, key)?
        .context("Dictionary audio resource not found")?;
    let stream = rodio::OutputStreamBuilder::open_default_stream()
        .context("Open dictionary audio output")?;
    let sink = rodio::Sink::connect_new(stream.mixer());
    sink.append(rodio::Decoder::try_from(std::io::Cursor::new(bytes))?);
    sink.sleep_until_end();
    Ok(())
}

fn emit(value: serde_json::Value) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{value}");
    let _ = stdout.flush();
}
