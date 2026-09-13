use crossload::copy;
use std::{
    fs,
    io::{Cursor, Write},
    os::unix::fs::symlink,
    process::Command,
};
use zip::{write::SimpleFileOptions, ZipWriter};
fn fixture() -> (tempfile::TempDir, Vec<u8>) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("card")).unwrap();
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in [
        ("mimetype", "application/epub+zip"),
        ("META-INF/container.xml", "<container><rootfiles><rootfile full-path='book.opf'/></rootfiles></container>"),
        ("book.opf", "<package><manifest><item id='c' href='chapter.xhtml' media-type='application/xhtml+xml'/></manifest><spine><itemref idref='c'/></spine></package>"),
        ("chapter.xhtml", "<html xmlns='http://www.w3.org/1999/xhtml'><head><title>Copy test</title></head><body><p>Test</p></body></html>"),
    ] {
        zip.start_file(name, SimpleFileOptions::default()).unwrap(); zip.write_all(data.as_bytes()).unwrap();
    }
    let data = zip.finish().unwrap().into_inner();
    fs::write(dir.path().join("Book.epub"), &data).unwrap();
    (dir, data)
}
#[test]
fn copy_preserves_source_and_skips_identical_destination() {
    let (dir, data) = fixture();
    let book = dir.path().join("Book.epub");
    let card = dir.path().join("card");
    let result = copy::copy(&book, &card).unwrap();
    assert!(!result.already_present);
    assert_eq!(result.bytes, data.len());
    assert_eq!(fs::read(result.path).unwrap(), data);
    assert!(copy::copy(&book, &card).unwrap().already_present);
    assert_eq!(fs::read(book).unwrap(), data);
    assert_eq!(fs::read_dir(card).unwrap().count(), 1);
}
#[test]
fn missing_mount_directory_is_not_created() {
    let (dir, _) = fixture();
    let missing = dir.path().join("not-mounted/Books");
    assert!(copy::copy(&dir.path().join("Book.epub"), &missing).is_err());
    assert!(!missing.exists());
}
#[test]
fn different_and_case_colliding_files_are_preserved() {
    let (dir, _) = fixture();
    let card = dir.path().join("card");
    let existing = card.join("book.EPUB");
    fs::write(&existing, b"existing book").unwrap();
    assert!(copy::copy(&dir.path().join("Book.epub"), &card).is_err());
    assert_eq!(fs::read(existing).unwrap(), b"existing book");
    assert_eq!(fs::read_dir(card).unwrap().count(), 1);
}
#[test]
fn destination_symlink_is_not_followed() {
    let (dir, _) = fixture();
    let card = dir.path().join("card");
    let outside = dir.path().join("outside");
    fs::write(&outside, b"untouched").unwrap();
    symlink(&outside, card.join("Book.epub")).unwrap();
    assert!(copy::copy(&dir.path().join("Book.epub"), &card).is_err());
    assert_eq!(fs::read(outside).unwrap(), b"untouched");
}
#[test]
fn malformed_book_does_not_reach_card() {
    let (dir, _) = fixture();
    let book = dir.path().join("Book.epub");
    fs::write(&book, b"broken ZIP").unwrap();
    let card = dir.path().join("card");
    assert!(copy::copy(&book, &card).is_err());
    assert_eq!(fs::read_dir(card).unwrap().count(), 0);
}
#[test]
fn copy_cli_and_mutually_exclusive_import_destinations() {
    let (dir, data) = fixture();
    let book = dir.path().join("Book.epub");
    let card = dir.path().join("card");
    let output = Command::new(env!("CARGO_BIN_EXE_xteink"))
        .arg("copy")
        .arg(&book)
        .arg("--to")
        .arg(&card)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read(card.join("Unknown author/Book.epub")).unwrap(),
        data
    );
    let output = Command::new(env!("CARGO_BIN_EXE_xteink"))
        .args([
            "import",
            "book.acsm",
            "--output",
            "books",
            "--send-to",
            "crosspoint.local",
            "--copy-to",
            "card",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
}
#[test]
fn kobo_import_can_copy_directly_to_card() {
    let (dir, data) = fixture();
    let device = dir.path().join("kobo");
    fs::create_dir_all(device.join(".kobo/kepub")).unwrap();
    let db = rusqlite::Connection::open(device.join(".kobo/KoboReader.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE content (ContentID TEXT PRIMARY KEY, Title TEXT, Attribution TEXT); CREATE TABLE content_keys (volumeid TEXT, elementid TEXT, elementkey TEXT); CREATE TABLE user (UserID TEXT); INSERT INTO content VALUES ('test-book', 'Test', 'Author');").unwrap();
    fs::write(device.join(".kobo/kepub/test-book"), &data).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_xteink"))
        .args(["kobo", "import", "test-book", "--device"])
        .arg(&device)
        .arg("--output")
        .arg(dir.path().join("books"))
        .arg("--copy-to")
        .arg(dir.path().join("card"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let card_file = fs::read_dir(dir.path().join("card/Unknown author"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(fs::read(card_file).unwrap(), data);
}
