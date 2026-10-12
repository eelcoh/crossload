//! The command line, run as a program on temporary folders. Its settings, state and cache are
//! kept in a scratch place, never the user's; no device or network is reached.
use std::{
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    process::Command,
};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// A program run with a private home, so nothing of the user's is read or written.
fn program(name: &str, home: &Path) -> Command {
    let binary = match name {
        "xteink" => env!("CARGO_BIN_EXE_xteink"),
        _ => env!("CARGO_BIN_EXE_crossload"),
    };
    let mut command = Command::new(binary);
    command
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_DATA_HOME", home.join("data"));
    command
}

/// The saved settings, read as the program reads them.
fn load(path: &Path) -> anyhow::Result<crossload::config::Defaults> {
    crossload::config::load(&store(path))
}

fn save(defaults: &mut crossload::config::Defaults, path: &Path) {
    defaults.save(&store(path)).unwrap();
}

fn store(path: &Path) -> crossload::config::Store {
    let parent = path.parent().unwrap();
    fs::create_dir_all(parent).unwrap();
    crossload::config::Store {
        dir: std::sync::Arc::new(ambit_caps_dir_native::NativeDir::open(parent).unwrap()),
        file: ambit_caps_dir::RelPath::new([path.file_name().unwrap().to_str().unwrap()]).unwrap(),
        display: path.display().to_string(),
        home: None,
    }
}

fn cli(path: &std::path::Path, args: &[&str]) -> std::process::Output {
    program("crossload", path.parent().unwrap())
        .arg("--config")
        .arg(path)
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn missing_config_is_optional_and_set_preserves_other_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    assert!(load(&path).unwrap().device.is_none());
    assert!(!path.exists());
    assert!(
        cli(&path, &["config", "set", "--reader", "crosspoint.local"])
            .status
            .success()
    );
    assert!(cli(
        &path,
        &["config", "set", "--output", dir.path().to_str().unwrap()]
    )
    .status
    .success());
    let saved = load(&path).unwrap();
    assert_eq!(saved.reader.as_deref(), Some("crosspoint.local"));
    assert_eq!(saved.output.as_deref(), dir.path().to_str());
    assert!(cli(&path, &["config", "unset", "reader"]).status.success());
    assert!(load(&path).unwrap().reader.is_none());
    assert!(load(&path).unwrap().output.is_some());
}
#[test]
fn invalid_settings_do_not_replace_saved_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let mut saved = crossload::config::Defaults {
        reader: Some("crosspoint.local".into()),
        ..Default::default()
    };
    save(&mut saved, &path);
    let original = fs::read(&path).unwrap();
    assert!(!cli(&path, &["config", "set", "--output", "relative/path"])
        .status
        .success());
    assert!(
        !cli(&path, &["config", "set", "--reader", "https://bad.example"])
            .status
            .success()
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    fs::write(&path, b"{\"outpt\":\"/tmp\"}").unwrap();
    assert!(load(&path).is_err());
}
#[test]
fn cli_device_overrides_config_and_missing_device_is_actionable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let missing = cli(&path, &["kobo", "list"]);
    assert!(String::from_utf8_lossy(&missing.stderr).contains("Missing --device"));
    // Empty but valid device fixture; the saved device intentionally does not exist.
    fs::create_dir_all(dir.path().join(".kobo")).unwrap();
    let db = rusqlite::Connection::open(dir.path().join(".kobo/KoboReader.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE content (ContentID TEXT, Title TEXT, Attribution TEXT); CREATE TABLE content_keys (volumeid TEXT, elementid TEXT, elementkey TEXT); CREATE TABLE user (UserID TEXT);").unwrap();
    let mut saved = crossload::config::Defaults {
        device: Some(dir.path().join("absent").display().to_string()),
        ..Default::default()
    };
    save(&mut saved, &path);
    assert!(!cli(&path, &["kobo", "list"]).status.success());
    let result = cli(
        &path,
        &[
            "kobo",
            "list",
            "--device",
            dir.path().to_str().unwrap(),
            "--json",
        ],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&result.stdout).trim(), "[]");
}
#[test]
fn import_uses_saved_paths_without_implicitly_sending_to_saved_reader() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let device = dir.path().join("device");
    fs::create_dir_all(device.join(".kobo/kepub")).unwrap();
    let db = rusqlite::Connection::open(device.join(".kobo/KoboReader.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE content (ContentID TEXT, Title TEXT, Attribution TEXT); CREATE TABLE content_keys (volumeid TEXT, elementid TEXT, elementkey TEXT); CREATE TABLE user (UserID TEXT); INSERT INTO content VALUES ('book', 'Book', 'Author');").unwrap();
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, value) in [
        ("mimetype", "application/epub+zip"),
        ("META-INF/container.xml", "<container><rootfiles><rootfile full-path='book.opf'/></rootfiles></container>"),
        ("book.opf", "<package><manifest><item id='c' href='chapter.xhtml' media-type='application/xhtml+xml'/></manifest><spine><itemref idref='c'/></spine></package>"),
        ("chapter.xhtml", "<html><head/><body><p>Test</p></body></html>"),
    ] { zip.start_file(name, SimpleFileOptions::default()).unwrap(); zip.write_all(value.as_bytes()).unwrap(); }
    let bytes = zip.finish().unwrap().into_inner();
    fs::write(device.join(".kobo/kepub/book"), &bytes).unwrap();
    let output = dir.path().join("books");
    let mut defaults = crossload::config::Defaults {
        device: Some(device.display().to_string()),
        output: Some(output.display().to_string()),
        reader: Some("127.0.0.1:9".into()),
        ..Default::default()
    };
    save(&mut defaults, &path);
    let result = cli(&path, &["kobo", "import", "book"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let imported = fs::read_dir(output)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(fs::read(imported).unwrap(), bytes);
    assert!(!String::from_utf8_lossy(&result.stderr).contains("Sending"));
    let result = cli(&path, &["import", "missing.acsm", "--send-to", "--copy-to"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("cannot be used with"));
}

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
fn copy_cli_and_mutually_exclusive_import_destinations() {
    let (dir, data) = fixture();
    let book = dir.path().join("Book.epub");
    let card = dir.path().join("card");
    let output = program("xteink", dir.path())
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
    let output = program("xteink", dir.path())
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
    let output = program("xteink", dir.path())
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
        program("crossload", self.temp.path())
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
    // A Kobo that is not mounted is not connected, not broken.
    assert!(status.contains("Kobo: Not connected"), "{status}");
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

#[test]
fn tui_rejects_redirected_io_without_terminal_escape_codes() {
    let result = program("xteink", &std::env::temp_dir().join("crossload-test-home"))
        .args(["tui", "--output", "/tmp/xteink-tui-unused"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("interactive terminal"));
    assert!(!result.stdout.contains(&27));
}
#[test]
fn tui_transfer_options_reject_conflicting_destinations() {
    let result = program("xteink", &std::env::temp_dir().join("crossload-test-home"))
        .args([
            "tui",
            "--output",
            "books",
            "--send-to",
            "reader.local",
            "--copy-to",
            "card",
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("cannot be used with"));
}

#[test]
fn import_folder_requires_send_to() {
    let result = program("xteink", &std::env::temp_dir().join("crossload-test-home"))
        .args([
            "import",
            "book.acsm",
            "--output",
            "books",
            "--folder",
            "/Books",
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("--send-to"));
}
