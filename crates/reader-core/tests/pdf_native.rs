use reader_core::{open, render_pdf_page};
use std::io::Write;

fn fixture(rotation: u32) -> tempfile::NamedTempFile {
    let stream = "BT /F1 18 Tf 20 50 Td (Native PDF) Tj ET";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Rotate {rotation} /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>"
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        format!(
            "<< /Length {} >>\nstream\n{stream}\nendstream",
            stream.len()
        ),
        "<< /Title (Native PDF Fixture) /Author (Reader tests) >>".to_owned(),
    ];
    let mut bytes = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        write!(&mut bytes, "{} 0 obj\n{}\nendobj\n", index + 1, object).unwrap();
    }
    let xref = bytes.len();
    write!(
        &mut bytes,
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    )
    .unwrap();
    for offset in offsets.iter().skip(1) {
        writeln!(&mut bytes, "{offset:010} 00000 n ").unwrap();
    }
    write!(
        &mut bytes,
        "trailer\n<< /Size {} /Root 1 0 R /Info 6 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    )
    .unwrap();
    let mut file = tempfile::Builder::new().suffix(".pdf").tempfile().unwrap();
    file.write_all(&bytes).unwrap();
    file
}

fn require_local_runtime() {
    if let Some(path) = std::env::var_os("PDFIUM_LIBRARY_PATH") {
        assert!(
            std::path::Path::new(&path).exists(),
            "PDFIUM_LIBRARY_PATH must already exist; no test-time downloads"
        );
        return;
    }
    let (platform, library) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => ("win-x64", "pdfium.dll"),
        ("macos", "x86_64") => ("mac-x64", "libpdfium.dylib"),
        ("macos", "aarch64") => ("mac-arm64", "libpdfium.dylib"),
        ("linux", "x86_64") => ("linux-x64", "libpdfium.so"),
        ("linux", "aarch64") => ("linux-arm64", "libpdfium.so"),
        _ => panic!("Prepare PDFIUM_LIBRARY_PATH on this platform"),
    };
    let directory = directories::ProjectDirs::from("", "Liesegang", "EpubReader").unwrap();
    assert!(
        directory
            .data_local_dir()
            .join("runtimes/pdfium/chromium-8125")
            .join(platform)
            .join(library)
            .is_file(),
        "Run cargo run -p reader-core --example prepare_pdfium before this test. The test never downloads its runtime."
    );
}

#[test]
#[ignore = "requires explicitly prepared local PDFium runtime; no test-time network"]
fn native_pdf_text_geometry_and_lazy_rasterization() {
    require_local_runtime();
    for rotation in [0, 90] {
        let file = fixture(rotation);
        let original = std::fs::read(file.path()).unwrap();
        let book = open(file.path()).unwrap();
        assert_eq!(book.title, "Native PDF Fixture");
        assert!(
            book.resources.is_empty(),
            "import must not eagerly rasterize pages"
        );
        assert!(book.chapters[0].plain_text().contains("Native PDF"));
        assert_eq!(book.search("Native").len(), 1);
        let geometry = book.chapters[0].fixed_page.as_ref().unwrap();
        assert!(geometry.chars.len() >= 9);
        for character in &geometry.chars {
            assert!(character.rect[0] >= 0.0 && character.rect[1] >= 0.0);
            assert!(
                character.rect[2] <= geometry.width + 1.0
                    && character.rect[3] <= geometry.height + 1.0
            );
        }
        let image_ref = book.chapters[0].blocks[0]
            .image
            .as_ref()
            .unwrap()
            .pdf_page
            .as_ref()
            .unwrap();
        let png = render_pdf_page(image_ref, 400).unwrap();
        let image = image::load_from_memory(&png).unwrap();
        assert_eq!(image.width(), 400);
        assert_eq!(image.height(), if rotation == 0 { 200 } else { 800 });
        assert_eq!(std::fs::read(file.path()).unwrap(), original);
    }
}
