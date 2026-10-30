use reader_library::{DictionaryCatalog, DictionaryStatus};

#[test]
fn real_mdx_mdd_lookup_redirect_prefix_resources_and_reopen() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let sources = directory.path().join("辞書");
    let cache = directory.path().join("app-cache");
    std::fs::create_dir(&sources)?;
    let mut mdx = mdictlib::MdxBuilder::new();
    mdx.header_attribute("Title", "試験辞書")?;
    mdx.add_entry("apple", "<b>りんご</b><img src='fruit.png'>")?;
    mdx.add_entry("applet", "<p>small app</p>")?;
    mdx.add_entry("alias", "@@@LINK=apple")?;
    mdx.add_entry("cycle", "@@@LINK=cycle")?;
    let mdx_path = sources.join("test.mdx");
    mdx.finish(std::fs::File::create(&mdx_path)?)?;
    let mut mdd = mdictlib::MddBuilder::new();
    mdd.add_resource("\\fruit.png", b"fake image fixture")?;
    mdd.finish(std::fs::File::create(sources.join("test.mdd"))?)?;
    let source_before = std::fs::read(&mdx_path)?;
    let mut catalog = DictionaryCatalog::open(&cache)?;
    let report = catalog.register(&sources)?;
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let id = &report.registered[0].id;
    assert_eq!(report.registered[0].mdd_paths.len(), 1);
    assert_eq!(
        catalog.lookup("apple", 20)?[0].html,
        "<b>りんご</b><img src='fruit.png'>"
    );
    let aliases = catalog.lookup("alias", 20)?;
    assert_eq!(aliases[0].headword, "apple");
    assert!(aliases[0].redirected_from.is_some());
    assert_eq!(catalog.prefix("app", 20)?.len(), 2);
    assert_eq!(
        catalog.resource(id, "fruit.png")?.unwrap(),
        b"fake image fixture"
    );
    assert!(catalog.resource(id, "../../metadata.db").is_err());
    assert!(catalog.lookup("cycle", 20)?.is_empty());
    assert!(!catalog.last_errors().is_empty());
    drop(catalog);
    let mut reopened = DictionaryCatalog::open(&cache)?;
    assert_eq!(reopened.lookup("apple", 20)?[0].headword, "apple");
    assert_eq!(std::fs::read(&mdx_path)?, source_before);
    assert!(!sources.join("resources").exists());
    Ok(())
}

#[test]
fn invalid_dictionary_gets_diagnostic_without_aborting_directory_import() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    std::fs::write(directory.path().join("broken.mdx"), b"not a dictionary")?;
    let mut catalog = DictionaryCatalog::open(directory.path().join("cache"))?;
    let report = catalog.register(directory.path())?;
    assert_eq!(report.registered.len(), 1);
    assert!(!report.errors.is_empty());
    assert!(matches!(
        report.registered[0].status,
        DictionaryStatus::Unavailable(_)
    ));
    Ok(())
}

#[test]
fn dictionary_names_fall_back_for_placeholder_html_and_legacy_catalog_titles() -> anyhow::Result<()>
{
    let directory = tempfile::tempdir()?;
    let cache = directory.path().join("cache");
    let mut catalog = DictionaryCatalog::open(&cache)?;
    let mut sources = Vec::new();
    for (filename, title, expected) in [
        (
            "研究社英和辞典",
            "Title (No HTML code allowed)",
            "研究社英和辞典",
        ),
        ("大辞林", "<font color='red'>Dictionary</font>", "大辞林"),
        ("japanese", "  日本語　辞典  ", "日本語 辞典"),
    ] {
        let path = directory.path().join(format!("{filename}.mdx"));
        let mut builder = mdictlib::MdxBuilder::new();
        builder.header_attribute("Title", title)?;
        builder.add_entry("猫", "ねこ")?;
        builder.finish(std::fs::File::create(&path)?)?;
        let bytes = std::fs::read(&path)?;
        let report = catalog.register(&path)?;
        assert_eq!(report.registered[0].title, expected);
        sources.push((path, bytes));
    }
    let manifest = cache.join("dictionaries.json");
    let mut json: serde_json::Value = serde_json::from_slice(&std::fs::read(&manifest)?)?;
    json[0]["title"] = serde_json::json!("Title (No HTML code allowed)");
    std::fs::write(&manifest, serde_json::to_vec(&json)?)?;
    let reopened = DictionaryCatalog::open(&cache)?;
    assert_eq!(reopened.dictionaries()[0].title, "研究社英和辞典");
    for (path, bytes) in sources {
        assert_eq!(std::fs::read(path)?, bytes);
    }
    Ok(())
}
