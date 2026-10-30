use reader_library::CalibreLibrary;

#[test]
fn calibre_snapshot_keeps_database_and_original_bytes_unchanged() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("日本語の本棚");
    std::fs::create_dir_all(root.join("著者/書名 (1)"))?;
    let db_path = root.join("metadata.db");
    let writer = rusqlite::Connection::open(&db_path)?;
    writer.execute_batch("CREATE TABLE library_id(uuid TEXT); INSERT INTO library_id VALUES('library-uuid');
      CREATE TABLE books(id INTEGER,uuid TEXT,title TEXT,path TEXT,has_cover INTEGER);
      INSERT INTO books VALUES(1,'book-uuid','日本語の本','著者/書名 (1)',1);
      INSERT INTO books VALUES(2,'other-uuid','PDFだけ','another',0);
      CREATE TABLE authors(id INTEGER,name TEXT); INSERT INTO authors VALUES(4,'著者');
      CREATE TABLE books_authors_link(id INTEGER,book INTEGER,author INTEGER); INSERT INTO books_authors_link VALUES(1,1,4);
      CREATE TABLE tags(id INTEGER,name TEXT); INSERT INTO tags VALUES(5,'小説');
      CREATE TABLE books_tags_link(id INTEGER,book INTEGER,tag INTEGER); INSERT INTO books_tags_link VALUES(1,1,5);
      CREATE TABLE data(book INTEGER,format TEXT,name TEXT); INSERT INTO data VALUES(1,'EPUB','original'); INSERT INTO data VALUES(2,'PDF','other');")?;
    let source = root.join("著者/書名 (1)/original.epub");
    let original = b"PK\x03\x04 original EPUB fixture kept unchanged";
    std::fs::write(&source, original)?;
    std::fs::write(root.join("著者/書名 (1)/cover.jpg"), b"cover unchanged")?;
    let before = std::fs::read(&db_path)?;
    // Leave the writer connection open, as when the main Calibre program is open.
    let snapshot = CalibreLibrary::scan(&root)?;
    assert_eq!(snapshot.entries.len(), 1);
    let book = &snapshot.entries[0];
    assert_eq!(book.title, "日本語の本");
    assert_eq!(book.authors, ["著者"]);
    assert_eq!(book.tags, ["小説"]);
    assert!(book.epub_path.ends_with("original.epub"));
    assert!(book.cover_path.is_some());
    assert_eq!(std::fs::read(&db_path)?, before);
    assert_eq!(std::fs::read(&source)?, original);
    drop(writer);
    Ok(())
}

#[test]
fn nonexistent_calibre_path_does_not_create_a_database() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    assert!(CalibreLibrary::scan(directory.path()).is_err());
    assert!(!directory.path().join("metadata.db").exists());
    Ok(())
}
