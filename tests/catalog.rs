//! The catalog commands: the same discovery the TUI uses, without a terminal.
use std::{
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    process::Command,
};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
fn epub(title: &str, author: &str) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(vec![]));
    for (name, value) in [
        ("mimetype", "application/epub+zip".to_owned()),
        ("META-INF/container.xml", "<container><rootfiles><rootfile full-path='book.opf'/></rootfiles></container>".into()),
        ("book.opf", format!("<package xmlns:dc='http://purl.org/dc/elements/1.1/'><metadata><dc:title>{title}</dc:title><dc:creator>{author}</dc:creator></metadata><manifest><item id='c' href='chapter.xhtml' media-type='application/xhtml+xml'/></manifest><spine><itemref idref='c'/></spine></package>")),
        ("chapter.xhtml", format!("<html><head/><body><p>{title}</p></body></html>")),
    ] {
        zip.start_file(name, SimpleFileOptions::default().compression_method(CompressionMethod::Stored)).unwrap();
        zip.write_all(value.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}
struct Fixture {
    temp: tempfile::TempDir,
    browse: PathBuf,
    output: PathBuf,
    card: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let (browse, output, card) = (
            temp.path().join("browse"),
            temp.path().join("output"),
            temp.path().join("card"),
        );
        for path in [&browse, &output, &card] {
            fs::create_dir_all(path).unwrap();
        }
        fs::write(browse.join("dune.epub"), epub("Dune", "Frank Herbert")).unwrap();
        fs::write(
            browse.join("piranesi.epub"),
            epub("Piranesi", "Susanna Clarke"),
        )
        .unwrap();
        Self {
            temp,
            browse,
            output,
            card,
        }
    }
    /// A saved configuration and a real device are never consulted: every
    /// location is explicit, and the cache stays inside the fixture.
    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_crossload"))
            .arg("--config")
            .arg(self.temp.path().join("missing.json"))
            .args(args)
            .arg("--browse")
            .arg(&self.browse)
            .arg("--output")
            .arg(&self.output)
            .arg("--device")
            .arg(self.temp.path().join("absent-kobo"))
            .arg("--copy-to")
            .arg(&self.card)
            .env("XDG_CACHE_HOME", self.temp.path())
            .output()
            .unwrap()
    }
}
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
fn files(path: &Path) -> usize {
    fs::read_dir(path).unwrap().count()
}
#[test]
fn books_reports_the_catalog_as_a_table_and_as_json() {
    let f = Fixture::new();
    let result = f.run(&["books"]);
    assert!(result.status.success(), "{}", text(&result.stderr));
    let out = text(&result.stdout);
    // Redirected output is data: a header and one line per book.
    assert!(
        out.starts_with("TITLE\tAUTHOR\tWHERE\tSOURCE\tBYTES\n"),
        "{out}"
    );
    assert!(
        out.contains("Dune\tFrank Herbert\tLocal\tLocal (original)\t"),
        "{out}"
    );
    // Location states belong on stderr, so they never pollute the data.
    let status = text(&result.stderr);
    assert!(
        status.contains("Local: Ready (2 books, 0 unreadable)"),
        "{status}"
    );
    assert!(status.contains("Kobo: Unavailable"), "{status}");
    let result = f.run(&["books", "--json"]);
    assert!(result.status.success(), "{}", text(&result.stderr));
    let catalog: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(catalog[0]["title"], "Dune");
    assert_eq!(catalog[0]["copies"][0]["place"], "Local");
    // Matching internals are not part of the reported catalog.
    assert!(catalog[0]["copies"][0].get("variants").is_none());
}
#[test]
fn sync_plans_before_it_copies_and_repeats_without_duplicating() {
    let f = Fixture::new();
    // The old name still selects the same place, so saved scripts keep working.
    let plan = f.run(&["sync", "--from", "local", "--to", "xteink"]);
    assert!(plan.status.success(), "{}", text(&plan.stderr));
    let out = text(&plan.stdout);
    assert!(out.contains("Dune\tFrank Herbert\tLocal\tplanned"), "{out}");
    assert!(
        out.contains("2 book(s) would be copied to CrossPoint"),
        "{out}"
    );
    // A dry run is the default and writes nothing at all.
    assert_eq!(files(&f.card), 0);
    let applied = f.run(&["sync", "--from", "local", "--to", "crosspoint", "--apply"]);
    assert!(applied.status.success(), "{}", text(&applied.stderr));
    assert!(text(&applied.stdout).contains("2 book(s) copied to CrossPoint"));
    assert!(f.card.join("Frank Herbert/Dune.epub").is_file());
    assert!(f.card.join("Susanna Clarke/Piranesi.epub").is_file());
    // Copies already at the destination are recognized, not duplicated.
    let again = f.run(&["sync", "--from", "local", "--to", "xteink"]);
    assert!(text(&again.stdout).contains("Nothing to copy"));
    assert_eq!(files(&f.card), 2);
}
#[test]
fn sync_refuses_one_location_and_an_unavailable_source() {
    let f = Fixture::new();
    let same = f.run(&["sync", "--from", "local", "--to", "local"]);
    assert!(!same.status.success());
    assert!(text(&same.stderr).contains("must differ"));
    let missing = f.run(&["sync", "--from", "kobo", "--to", "local"]);
    assert!(!missing.status.success());
    assert!(
        text(&missing.stderr).contains("Kobo is unavailable"),
        "{}",
        text(&missing.stderr)
    );
    assert_eq!(files(&f.output), 0);
}
