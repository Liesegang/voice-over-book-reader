//! Opt-in Windows GPU/CEF regression. The default test run never launches UI.
#![cfg(windows)]

use reader_library::{LibraryStore, Settings, SpreadMode};
use serde_json::Value;
use std::{
    fs::{self, File},
    io::Write,
    os::windows::process::CommandExt,
    path::Path,
    process::Command,
};
use zip::{ZipWriter, write::SimpleFileOptions};

fn write_density_fixture(path: &Path) {
    let mut archive = ZipWriter::new(File::create(path).unwrap());
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut put = |name: &str, content: &str| {
        archive.start_file(name, options).unwrap();
        archive.write_all(content.as_bytes()).unwrap();
    };
    put("mimetype", "application/epub+zip");
    put(
        "META-INF/container.xml",
        r#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OPS/package.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
    );
    put(
        "OPS/package.opf",
        r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="book-id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="book-id">urn:vobr:test:cef-dpi</dc:identifier><dc:title>Density regression</dc:title><dc:language>ja</dc:language><meta property="dcterms:modified">2026-01-01T00:00:00Z</meta></metadata><manifest><item id="c0" href="c0.xhtml" media-type="application/xhtml+xml"/><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="c2.xhtml" media-type="application/xhtml+xml"/><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/></manifest><spine><itemref idref="c0"/><itemref idref="c1"/><itemref idref="c2"/></spine></package>"#,
    );
    put(
        "OPS/nav.xhtml",
        r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>Contents</title></head><body><nav epub:type="toc"><ol><li><a href="c0.xhtml">One</a></li><li><a href="c1.xhtml">Two</a></li><li><a href="c2.xhtml">Three</a></li></ol></nav></body></html>"#,
    );
    // All three chapter resources are byte-identical. The author media query
    // must therefore produce the same pagination in visible and counter views.
    let paragraphs = "<p>窓の外には青い空が広がり、静かな風が木々の葉を揺らしている。読書の時間を楽しみながら、一頁ずつ物語を読み進める。</p>".repeat(240);
    let chapter = format!(
        r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml" lang="ja"><head><title>Identical chapter</title><style>body{{writing-mode:vertical-rl}}p{{font-size:1em}}@media (min-resolution:1.2dppx){{p{{font-size:1.6em}}}}</style></head><body>{paragraphs}</body></html>"#
    );
    for index in 0..3 {
        put(&format!("OPS/c{index}.xhtml"), &chapter);
    }
    archive.finish().unwrap();
}

// This script is static. Paths are environment values and individual native
// arguments; neither Rust nor PowerShell interpolates a path into shell code.
const HIDDEN_NATIVE_RUN: &str = r#"
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) { throw 'PowerShell 7 is required' }
$start = [Diagnostics.ProcessStartInfo]::new($env:VOBR_NATIVE_EXE)
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
$start.WorkingDirectory = [IO.Path]::GetDirectoryName($env:VOBR_NATIVE_EXE)
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
foreach ($argument in @('--reader', $env:VOBR_DPI_BOOK, '--data-dir', $env:VOBR_DPI_DATA, '--perf-output', $env:VOBR_DPI_REPORT, '--perf-count-only')) {
    $start.ArgumentList.Add($argument)
}
$start.Environment['VOBR_DIAGNOSTIC_VIEWPORT'] = '1281x761@1.25'
$reader = [Diagnostics.Process]::Start($start)
try {
    $stdout = $reader.StandardOutput.ReadToEndAsync()
    $stderr = $reader.StandardError.ReadToEndAsync()
    if (-not $reader.WaitForExit(60000)) {
        $reader.Kill($true)
        $null = $reader.WaitForExit(5000)
        throw 'Owned native DPI reader exceeded its 60 second timeout'
    }
    if (-not $stdout.Wait(5000) -or -not $stderr.Wait(5000)) {
        throw 'Native DPI reader exited without closing its diagnostic streams'
    }
    [IO.File]::WriteAllText($env:VOBR_DPI_STDOUT, $stdout.GetAwaiter().GetResult())
    [IO.File]::WriteAllText($env:VOBR_DPI_STDERR, $stderr.GetAwaiter().GetResult())
    if ($reader.ExitCode -ne 0) { throw "Native DPI reader exited with code $($reader.ExitCode)" }
} finally {
    $reader.Dispose()
}
"#;

#[test]
#[ignore = "requires Windows GPU, a complete CEF bundle in VOBR_NATIVE_EXE, and pwsh.exe (PowerShell 7)"]
fn resolution_media_queries_have_identical_visible_and_background_page_counts() {
    let executable = fs::canonicalize(
        std::env::var_os("VOBR_NATIVE_EXE")
            .expect("Set VOBR_NATIVE_EXE to the bundled reader executable"),
    )
    .expect("VOBR_NATIVE_EXE must identify an existing bundled executable");
    assert!(executable.is_file());
    let scratch = tempfile::Builder::new()
        .prefix("vobr-cef-dpi-")
        .tempdir()
        .unwrap();
    let book = scratch.path().join("density.epub");
    let data = scratch.path().join("data");
    let report = scratch.path().join("report.json");
    write_density_fixture(&book);
    {
        let mut store = LibraryStore::open(&data).unwrap();
        let mut settings = Settings {
            font_family: "Noto Serif CJK JP".into(),
            font_size: 23.,
            spread: SpreadMode::Single,
            text_spread: false,
            // Isolate even library discovery from the user's normal settings.
            calibre_path: Some(scratch.path().join("unused-library")),
            ..Settings::default()
        };
        settings.tts.provider_id = "system".into();
        settings.tts.pre_cache = false;
        store.save_settings(&settings).unwrap();
    }
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let result = Command::new("pwsh.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            HIDDEN_NATIVE_RUN,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .env("VOBR_NATIVE_EXE", &executable)
        .env("VOBR_DPI_BOOK", &book)
        .env("VOBR_DPI_DATA", &data)
        .env("VOBR_DPI_REPORT", &report)
        .env("VOBR_DPI_STDOUT", scratch.path().join("stdout.log"))
        .env("VOBR_DPI_STDERR", scratch.path().join("stderr.log"))
        .output()
        .expect("Launch pwsh.exe (PowerShell 7) for the hidden native regression");
    assert!(
        result.status.success(),
        "Hidden native launch failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: Value =
        serde_json::from_slice(&fs::read(report).expect("Native performance report")).unwrap();
    assert_eq!(
        report["success"], true,
        "Native GPU/count verification failed"
    );
    assert_eq!(report["complete_index_observed"], true);
    assert_eq!(report["requested_backend"], "chromium");
    let total = report["total_pages"].as_u64().unwrap();
    let indexes: Vec<Value> = fs::read_dir(data.join("pagination-chromium-v1"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .map(|path| serde_json::from_slice::<Value>(&fs::read(path).unwrap()).unwrap())
        .filter(|index| {
            index["counts"]
                .as_array()
                .is_some_and(|counts| counts.len() == 3)
                && index["total"].as_u64() == Some(total)
        })
        .collect();
    assert_eq!(
        indexes.len(),
        1,
        "Expected one complete three-chapter pagination cache"
    );
    let counts: Vec<u64> = indexes[0]["counts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|count| count.as_u64().unwrap())
        .collect();
    assert!(
        counts.iter().all(|count| *count > 1),
        "Fixture must span multiple pages"
    );
    assert!(
        counts.windows(2).all(|pair| pair[0] == pair[1]),
        "Identical chapters differ between visible and background DPI: {counts:?}"
    );
    assert_eq!(counts.iter().sum::<u64>(), total);
    eprintln!("CEF 125% DPI regression: counts={counts:?}, total={total}");
}
