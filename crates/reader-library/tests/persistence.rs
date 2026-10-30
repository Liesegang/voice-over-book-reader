use reader_core::Locator;
use reader_library::*;

#[test]
fn settings_position_bookmarks_annotations_survive_reopen() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let locator = Locator {
        chapter: 2,
        node: 7,
        offset: 12,
    };
    let bookmark = Bookmark::new("book-one", locator, "続きを読む");
    {
        let mut store = LibraryStore::open(directory.path())?;
        let mut settings = store.settings().clone();
        settings.theme = Theme::Sepia;
        settings.accent = [2, 100, 200];
        settings.tts.speed = 10.0;
        store.save_settings(&settings)?;
        store.save_position(&ReadingPosition {
            book_id: "book-one".into(),
            locator,
            progress: 0.7,
            updated_at: unix_millis(),
        })?;
        store.save_bookmark(&bookmark)?;
        store.save_annotation(&Annotation {
            id: "note-one".into(),
            book_id: "book-one".into(),
            start: locator,
            end: Locator {
                offset: 18,
                ..locator
            },
            quote: "日本語".into(),
            note: "メモ".into(),
            color: [255, 220, 50],
            created_at: unix_millis(),
        })?;
    }
    let store = LibraryStore::open(directory.path())?;
    assert_eq!(store.settings().theme, Theme::Sepia);
    assert_eq!(store.settings().tts.speed, 10.0);
    assert_eq!(store.position("book-one")?.unwrap().locator, locator);
    assert_eq!(store.bookmarks("book-one")?[0].id, bookmark.id);
    assert_eq!(store.annotations("book-one")?[0].quote, "日本語");
    store.delete_bookmark(&bookmark.id)?;
    assert!(store.bookmarks("book-one")?.is_empty());
    assert!(store.bookmarks("different-book")?.is_empty());
    Ok(())
}

#[test]
fn importing_and_removing_never_changes_original() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("原本.txt");
    let content = "元の文書です。";
    std::fs::write(&path, content)?;
    let mut store = LibraryStore::open(directory.path().join("app"))?;
    let entry = store.import_book(&path)?;
    assert_eq!(store.books().len(), 1);
    assert_eq!(store.import_book(&path)?.id, entry.id);
    assert_eq!(store.books().len(), 1);
    store.remove_book(&entry.id)?;
    assert_eq!(std::fs::read_to_string(&path)?, content);
    assert!(store.books().is_empty());
    Ok(())
}

#[test]
fn nonfinite_settings_and_out_of_range_speed_are_rejected() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = LibraryStore::open(directory.path())?;
    let mut settings = store.settings().clone();
    settings.font_size = f32::NAN;
    assert!(store.save_settings(&settings).is_err());
    settings.font_size = 24.0;
    settings.tts.speed = 10.1;
    assert!(store.save_settings(&settings).is_err());
    Ok(())
}

#[test]
fn custom_shelves_list_view_and_key_bindings_survive_reopen_without_changing_sources()
-> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let first_path = directory.path().join("一冊目.txt");
    let second_path = directory.path().join("二冊目.txt");
    let first_source = "最初の本。原本はそのままです。";
    let second_source = "次の本。棚の移動でも書き換えません。";
    std::fs::write(&first_path, first_source)?;
    std::fs::write(&second_path, second_source)?;
    let data = directory.path().join("app");
    let (first, second) = {
        let mut store = LibraryStore::open(&data)?;
        let first = store.import_book(&first_path)?;
        let second = store.import_book(&second_path)?;
        let mut settings = store.settings().clone();
        settings.library_list = true;
        settings.scroll_speed = 2.75;
        settings.tts.pre_cache = false;
        settings.spread = SpreadMode::Double;
        assert!(
            !settings.text_spread,
            "text starts as a continuous reading surface"
        );
        settings.text_spread = true;
        settings.custom_shelves.insert(
            "読みたい本".into(),
            vec![first.id.clone(), second.id.clone()],
        );
        settings.custom_shelves.insert("空の本棚".into(), vec![]);
        settings
            .key_bindings
            .insert("Shelf:読みたい本".into(), "Alt+J".into());
        settings
            .key_bindings
            .insert("PageLeft".into(), String::new());
        store.save_settings(&settings)?;
        (first, second)
    };
    {
        let mut store = LibraryStore::open(&data)?;
        assert_eq!(store.books().len(), 2);
        assert!(store.settings().library_list);
        assert_eq!(store.settings().scroll_speed, 2.75);
        assert!(!store.settings().tts.pre_cache);
        assert_eq!(store.settings().spread, SpreadMode::Double);
        assert!(store.settings().text_spread);
        assert_eq!(
            store.settings().custom_shelves["読みたい本"],
            [first.id.clone(), second.id.clone()]
        );
        assert!(store.settings().custom_shelves["空の本棚"].is_empty());
        assert_eq!(store.settings().key_bindings["Shelf:読みたい本"], "Alt+J");
        assert_eq!(store.settings().key_bindings["PageLeft"], "");
        let mut settings = store.settings().clone();
        settings
            .custom_shelves
            .get_mut("読みたい本")
            .unwrap()
            .retain(|id| id != &first.id);
        settings.custom_shelves.remove("空の本棚");
        store.save_settings(&settings)?;
    }
    let reopened = LibraryStore::open(&data)?;
    assert_eq!(
        reopened.settings().custom_shelves["読みたい本"],
        [second.id]
    );
    assert!(!reopened.settings().custom_shelves.contains_key("空の本棚"));
    assert_eq!(
        reopened.books().len(),
        2,
        "removing a book from a shelf does not remove its library entry"
    );
    assert_eq!(std::fs::read_to_string(first_path)?, first_source);
    assert_eq!(std::fs::read_to_string(second_path)?, second_source);
    Ok(())
}
