use crossload::{
    inventory::{self, Destination},
    kobo::Library,
    sync::{self, Options, Status},
};
use std::{
    fs,
    io::{Cursor, Write},
    path::PathBuf,
    process::Command,
};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
struct Fixture {
    _temp: tempfile::TempDir,
    device: PathBuf,
    card: PathBuf,
    output: PathBuf,
}
fn epub(text: &str, compression: CompressionMethod) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(vec![]));
    for (name, value) in [
        ("mimetype", "application/epub+zip".to_owned()),
        ("META-INF/container.xml", "<container><rootfiles><rootfile full-path='book.opf'/></rootfiles></container>".into()),
        ("book.opf", "<package xmlns:dc='http://purl.org/dc/elements/1.1/'><metadata><dc:title>Book</dc:title><dc:creator>Author</dc:creator></metadata><manifest><item id='c' href='chapter.xhtml' media-type='application/xhtml+xml'/></manifest><spine><itemref idref='c'/></spine></package>".into()),
        ("chapter.xhtml", format!("<html><head/><body><p>{text}</p></body></html>")),
    ] { zip.start_file(name, SimpleFileOptions::default().compression_method(compression)).unwrap(); zip.write_all(value.as_bytes()).unwrap(); }
    zip.finish().unwrap().into_inner()
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let device = temp.path().join("kobo");
        let card = temp.path().join("card");
        let output = temp.path().join("books");
        fs::create_dir_all(device.join(".kobo/kepub")).unwrap();
        fs::create_dir(&card).unwrap();
        let db = rusqlite::Connection::open(device.join(".kobo/KoboReader.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE content (ContentID TEXT, Title TEXT, Attribution TEXT, Accessibility INTEGER); CREATE TABLE content_keys (volumeid TEXT, elementid TEXT, elementkey TEXT); CREATE TABLE user (UserID TEXT);").unwrap();
        Self {
            _temp: temp,
            device,
            card,
            output,
        }
    }
    fn add(&self, id: &str, bytes: &[u8], preview: bool) {
        let db = rusqlite::Connection::open(self.device.join(".kobo/KoboReader.sqlite")).unwrap();
        db.execute(
            "INSERT INTO content VALUES (?1, 'Book', 'Author', ?2)",
            rusqlite::params![id, if preview { 6 } else { 1 }],
        )
        .unwrap();
        fs::write(self.device.join(".kobo/kepub").join(id), bytes).unwrap();
    }
    fn options(&self, apply: bool) -> Options {
        Options {
            output: self.output.clone(),
            serial: None,
            apply,
            optimize: Some(crossload::profile::X4),
            organized: true,
            repair: false,
            exclude: vec![],
        }
    }
    fn run(&self, apply: bool) -> sync::Report {
        sync::run(
            &Library::open(&self.device).unwrap(),
            &sync::card(&self.card).unwrap(),
            &self.options(apply),
            |_| {},
        )
        .unwrap()
    }
}
#[test]
fn dry_run_has_no_published_writes_and_apply_is_repeatable() {
    let f = Fixture::new();
    let bytes = epub("text", CompressionMethod::Stored);
    f.add("one", &bytes, false);
    f.add("duplicate", &bytes, false);
    f.add("preview", &bytes, true);
    let report = f.run(false);
    assert_eq!(report.previews_skipped, 1);
    assert_eq!(report.books[0].status, Status::Transfer);
    assert_eq!(report.books[1].status, Status::Duplicate);
    assert!(!f.output.exists());
    assert_eq!(fs::read_dir(&f.card).unwrap().count(), 0);
    let first = f.run(true);
    assert_eq!(first.failures(), 0);
    assert_eq!(first.books[0].status, Status::Sent);
    assert_eq!(fs::read(f.card.join("Author/Book.epub")).unwrap(), bytes);
    let second = f.run(true);
    assert!(second.books.iter().all(|b| b.status == Status::Present));
    assert_eq!(fs::read_dir(&f.output).unwrap().count(), 1);
}
#[test]
fn renamed_repacked_book_is_recognized_but_same_title_different_text_is_not() {
    let f = Fixture::new();
    let original = epub("original", CompressionMethod::Stored);
    f.add("one", &original, false);
    let repacked = epub("original", CompressionMethod::Deflated);
    assert_ne!(original, repacked);
    assert!(inventory::identity(&original).matches(&inventory::identity(&repacked)));
    fs::write(f.card.join("old-name.epub"), repacked).unwrap();
    assert_eq!(f.run(false).books[0].status, Status::Present);
    fs::write(
        f.card.join("old-name.epub"),
        epub("different edition", CompressionMethod::Stored),
    )
    .unwrap();
    assert_eq!(f.run(false).books[0].status, Status::Transfer);
}
#[test]
fn collision_preserves_both_books_and_local_recovery_checks_bytes() {
    let f = Fixture::new();
    let bytes = epub("one", CompressionMethod::Stored);
    f.add("one", &bytes, false);
    fs::create_dir(f.card.join("Author")).unwrap();
    fs::write(f.card.join("Author/Book.epub"), b"unrelated").unwrap();
    assert_eq!(f.run(true).failures(), 1);
    assert!(!f.output.exists());
    assert_eq!(
        fs::read(f.card.join("Author/Book.epub")).unwrap(),
        b"unrelated"
    );
    let lib = Library::open(&f.device).unwrap();
    let imported = lib.import_reusing("one", &f.output, None).unwrap();
    assert_eq!(
        lib.import_reusing("one", &f.output, None).unwrap(),
        imported
    );
    fs::write(&imported, b"other local file").unwrap();
    assert!(lib.import_reusing("one", &f.output, None).is_err());
    assert_eq!(fs::read(imported).unwrap(), b"other local file");
}
#[test]
fn distinct_books_with_same_destination_are_reported_without_overwrite() {
    let f = Fixture::new();
    f.add("one", &epub("one", CompressionMethod::Stored), false);
    f.add("two", &epub("two", CompressionMethod::Stored), false);
    let plan = f.run(false);
    assert_eq!(plan.books[0].status, Status::Transfer);
    assert_eq!(plan.books[1].status, Status::Failed);
    let applied = f.run(true);
    assert_eq!(applied.books[0].status, Status::Sent);
    assert_eq!(applied.books[1].status, Status::Failed);
}
#[test]
fn inventory_failure_and_symlinks_never_mean_empty_reader() {
    let f = Fixture::new();
    f.add("one", &epub("one", CompressionMethod::Stored), false);
    std::os::unix::fs::symlink(&f.device, f.card.join("external")).unwrap();
    assert!(sync::run(
        &Library::open(&f.device).unwrap(),
        &Destination::Card(f.card.clone()),
        &f.options(true),
        |_| {}
    )
    .is_err());
    assert!(!f.output.exists());
    let unavailable =
        Destination::Reader(crossload::crosspoint::Reader::new("127.0.0.1:1", "/").unwrap());
    assert!(sync::run(
        &Library::open(&f.device).unwrap(),
        &unavailable,
        &f.options(true),
        |_| {}
    )
    .is_err());
}
#[test]
fn cli_defaults_to_dry_run_and_json_stays_machine_readable() {
    let f = Fixture::new();
    f.add("one", &epub("one", CompressionMethod::Stored), false);
    let result = Command::new(env!("CARGO_BIN_EXE_crossload"))
        .arg("--config")
        .arg(f._temp.path().join("missing.json"))
        .args(["kobo", "sync", "--device"])
        .arg(&f.device)
        .arg("--copy-to")
        .arg(&f.card)
        .arg("--output")
        .arg(&f.output)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["apply"], false);
    assert_eq!(report["books"][0]["status"], "transfer");
    assert!(!f.output.exists());
    assert_eq!(fs::read_dir(f.card).unwrap().count(), 0);
}

#[test]
fn explicit_exclusion_keeps_existing_variant_and_syncs_remaining_books() {
    let f = Fixture::new();
    f.add(
        "store",
        &epub("store variant", CompressionMethod::Stored),
        false,
    );
    let existing = epub("calibre variant", CompressionMethod::Stored);
    f.add("sideload", &existing, false);
    fs::create_dir(f.card.join("Author")).unwrap();
    fs::write(f.card.join("Author/Book.epub"), &existing).unwrap();
    let report = f.run(false);
    let failure = report
        .books
        .iter()
        .find(|b| b.status == Status::Failed)
        .unwrap();
    assert!(failure.detail.contains("--exclude store"));
    assert!(!failure.detail.contains("--repair"));
    let mut options = f.options(true);
    options.exclude = vec!["store".into()];
    let report = sync::run(
        &Library::open(&f.device).unwrap(),
        &Destination::Card(f.card.clone()),
        &options,
        |_| {},
    )
    .unwrap();
    assert_eq!(report.excluded, 1);
    assert_eq!(report.failures(), 0);
    assert_eq!(report.books.len(), 1);
    assert_eq!(report.books[0].status, Status::Present);
    assert_eq!(fs::read(f.card.join("Author/Book.epub")).unwrap(), existing);
    options.exclude = vec!["typo".into()];
    assert!(sync::run(
        &Library::open(&f.device).unwrap(),
        &Destination::Card(f.card.clone()),
        &options,
        |_| {}
    )
    .is_err());
}
