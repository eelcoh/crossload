use crossload::config::{self, Defaults};
use std::{fs, process::Command};
fn cli(path: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_crossload"))
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
    assert!(config::load(&path).unwrap().device.is_none());
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
    let saved = config::load(&path).unwrap();
    assert_eq!(saved.reader.as_deref(), Some("crosspoint.local"));
    assert_eq!(saved.output.as_deref(), Some(dir.path()));
    assert!(cli(&path, &["config", "unset", "reader"]).status.success());
    assert!(config::load(&path).unwrap().reader.is_none());
    assert!(config::load(&path).unwrap().output.is_some());
}
#[test]
fn invalid_settings_do_not_replace_saved_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let mut saved = Defaults {
        reader: Some("crosspoint.local".into()),
        ..Defaults::default()
    };
    saved.save(&path).unwrap();
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
    assert!(config::load(&path).is_err());
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
    let mut saved = Defaults {
        device: Some(dir.path().join("absent")),
        ..Defaults::default()
    };
    saved.save(&path).unwrap();
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
    use std::io::{Cursor, Write};
    use zip::{write::SimpleFileOptions, ZipWriter};
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
    let mut defaults = Defaults {
        device: Some(device),
        output: Some(output.clone()),
        reader: Some("127.0.0.1:9".into()),
        ..Defaults::default()
    };
    defaults.save(&path).unwrap();
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
