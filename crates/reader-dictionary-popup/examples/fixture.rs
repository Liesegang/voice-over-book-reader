//! Generate an original, tiny MDX/MDD corpus for local popup QA.
//! Usage: cargo run -p reader-dictionary-popup --example fixture -- APP_DATA_DIR
use anyhow::{Context, Result};

fn main() -> Result<()> {
    let data_dir = std::path::PathBuf::from(
        std::env::args_os()
            .nth(1)
            .context("Pass an application data directory for test fixtures")?,
    );
    let sources = data_dir.join("qa-dictionary-source");
    std::fs::create_dir_all(&sources)?;
    let mdx_path = sources.join("qa-japanese.mdx");
    let mut builder = mdictlib::MdxBuilder::new();
    builder.header_attribute("Title", "表示確認用・日本語辞書")?;
    for (key, definition) in [
        (
            "猫",
            "<ruby>猫<rt>ねこ</rt></ruby>。人と暮らす小さな哺乳類。",
        ),
        (
            "吾輩",
            "<ruby>吾輩<rt>わがはい</rt></ruby>。自分を指すことば。",
        ),
        (
            "日本語",
            "日本で使われる言語。漢字と仮名を組み合わせて書く。",
        ),
        (
            "cat",
            "A small animal that lives with people; <a href='entry://猫'>猫</a>.",
        ),
        (
            "test",
            "A small original entry used to verify the dictionary popup.",
        ),
    ] {
        let body = format!(
            r#"<link rel="stylesheet" href="style.css"><article><img src="cat.svg" class="icon"><h2>{key}</h2><p>{definition}</p><p><a href="sound://sample.wav">▶ 音声を再生</a> · <a href="entry://猫">「猫」を引く</a></p><details><summary>用例を開く</summary><p>猫が本のとなりで眠っている。</p></details><p id="script-proof"></p></article><script>document.getElementById('script-proof').textContent='CSS・画像・辞書 JavaScript を読み込みました。';</script>"#
        );
        builder.add_entry(key, &body)?;
    }
    builder.add_entry("ねこ", "@@@LINK=猫")?;
    builder.finish(std::fs::File::create(&mdx_path)?)?;
    let mut resources = mdictlib::MddBuilder::new();
    resources.add_resource("\\style.css",b"article{color:#243245}h2{font-size:24px;margin:0 0 12px;color:#2563eb}.icon{float:right;width:64px;height:64px}ruby rt{font-size:.5em}details{padding:8px;background:#eff6ff;border-radius:6px}#script-proof{font-size:11px;color:#64748b}")?;
    resources.add_resource("\\cat.svg",br##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><path fill="#dbeafe" stroke="#2563eb" stroke-width="2" d="M10 24V7l16 10h12L54 7v25a22 22 0 1 1-44-8Z"/><circle cx="24" cy="32" r="2"/><circle cx="42" cy="32" r="2"/><path d="m28 40 5 4 5-4" fill="none" stroke="#2563eb" stroke-width="2"/></svg>"##)?;
    resources.add_resource("\\sample.wav", &wav())?;
    resources.finish(std::fs::File::create(sources.join("qa-japanese.mdd"))?)?;
    let mut catalog = reader_library::DictionaryCatalog::open(data_dir.join("dictionaries"))?;
    let report = catalog.register(&mdx_path)?;
    anyhow::ensure!(report.errors.is_empty(), "{}", report.errors.join("\n"));
    for (filename, title, entries) in [
        (
            "qa-examples.mdx",
            "用例辞典",
            vec![
                (
                    "猫",
                    "<h2>猫の用例</h2><p>猫が窓辺で本の続きを待っている。</p>",
                ),
                ("cat", "<p>A cat waits by the window.</p>"),
            ],
        ),
        (
            "qa-english.mdx",
            "英語だけの辞典",
            vec![(
                "test",
                "<p>A test entry; 「猫」 has deliberately no match in this dictionary.</p>",
            )],
        ),
    ] {
        let mut builder = mdictlib::MdxBuilder::new();
        builder.header_attribute("Title", title)?;
        for (key, definition) in entries {
            builder.add_entry(key, definition)?;
        }
        let path = sources.join(filename);
        builder.finish(std::fs::File::create(&path)?)?;
        let report = catalog.register(path)?;
        anyhow::ensure!(report.errors.is_empty(), "{}", report.errors.join("\n"));
    }
    println!(
        "Registered three original QA dictionaries in {}",
        data_dir.display()
    );
    Ok(())
}

fn wav() -> Vec<u8> {
    let samples = 8000u32;
    let bytes = samples * 2;
    let mut data = Vec::new();
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&(bytes + 36).to_le_bytes());
    data.extend_from_slice(b"WAVEfmt ");
    data.extend_from_slice(&16u32.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&16000u32.to_le_bytes());
    data.extend_from_slice(&32000u32.to_le_bytes());
    data.extend_from_slice(&2u16.to_le_bytes());
    data.extend_from_slice(&16u16.to_le_bytes());
    data.extend_from_slice(b"data");
    data.extend_from_slice(&bytes.to_le_bytes());
    for sample in 0..samples {
        let amplitude =
            ((sample as f32 / 16000.0 * 440.0 * std::f32::consts::TAU).sin() * 2000.0) as i16;
        data.extend_from_slice(&amplitude.to_le_bytes());
    }
    data
}
