fn main() -> anyhow::Result<()> {
    println!("{}", reader_core::pdfium_runtime_path()?.display());
    Ok(())
}
