#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
fn main() -> anyhow::Result<()> {
    voice_over_book_reader::run()
}
