use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::process::Command;

use aes::cipher::{
    block_padding::{NoPadding, Pkcs7},
    BlockEncryptMut, KeyInit,
};
use base64::Engine;
use crossload::{
    epub,
    kobo::{Library, Source},
};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

const ID: &str = "test-volume-001";
const SERIAL: &str = "N1234567890";
const USER: &str = "11111111-2222-3333-4444-555555555555";
const CHAPTER: &[u8] = b"<?xml version=\"1.0\"?><html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>Test</title></head><body><p>A synthetic test book.</p></body></html>";
const FONTS: &[u8] = b"<encryption xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\" xmlns:enc=\"http://www.w3.org/2001/04/xmlenc#\"><enc:EncryptedData><enc:EncryptionMethod Algorithm=\"http://www.idpf.org/2008/embedding\"/><enc:CipherData><enc:CipherReference URI=\"OPS/font.otf\"/></enc:CipherData></enc:EncryptedData></encryption>";

struct Device {
    dir: TempDir,
    // Keep open for WAL tests, so SQLite doesn't checkpoint on close.
    db: Connection,
}

impl Device {
    fn new(encrypted: bool, wal: bool, fonts: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".kobo/kepub")).unwrap();
        fs::create_dir(dir.path().join(".adobe-digital-editions")).unwrap();
        fs::write(
            dir.path().join(".adobe-digital-editions/device.xml"),
            format!("<deviceInfo><deviceSerial>{SERIAL}</deviceSerial></deviceInfo>"),
        )
        .unwrap();
        let db = Connection::open(dir.path().join(".kobo/KoboReader.sqlite")).unwrap();
        if wal {
            db.pragma_update(None, "journal_mode", "WAL").unwrap();
        }
        db.execute_batch(
            "CREATE TABLE content (ContentID TEXT PRIMARY KEY, Title TEXT, Attribution TEXT);
            CREATE TABLE content_keys (volumeid TEXT, elementid TEXT, elementkey TEXT);
            CREATE TABLE user (UserID TEXT);",
        )
        .unwrap();
        db.execute("INSERT INTO user VALUES (?1)", [USER]).unwrap();
        db.execute(
            "INSERT INTO content VALUES (?1, 'Test / book', 'Test Author')",
            [ID],
        )
        .unwrap();
        // A cloud-only book and a chapter must not be listed as downloadable books.
        db.execute("INSERT INTO content VALUES ('cloud-only', 'Cloud', '')", [])
            .unwrap();
        db.execute(
            "INSERT INTO content VALUES ('chapter#1', 'Chapter', '')",
            [],
        )
        .unwrap();
        let chapter = if encrypted {
            // Generate independently of Flamberge's fixture helpers and key API.
            let device_hash = format!("{:x}", Sha256::digest(format!("88b3a2e13{SERIAL}")));
            let user_hash = Sha256::digest(format!("{device_hash}{USER}"));
            let user_key: [u8; 16] = user_hash[16..].try_into().unwrap();
            let page_key = [0x73; 16];
            let wrapped = ecb::Encryptor::<aes::Aes128>::new(&user_key.into())
                .encrypt_padded_vec_mut::<NoPadding>(&page_key);
            db.execute(
                "INSERT INTO content_keys VALUES (?1, 'OPS/chapter.xhtml', ?2)",
                [
                    ID,
                    &base64::engine::general_purpose::STANDARD.encode(wrapped),
                ],
            )
            .unwrap();
            ecb::Encryptor::<aes::Aes128>::new(&page_key.into())
                .encrypt_padded_vec_mut::<Pkcs7>(CHAPTER)
        } else {
            CHAPTER.to_vec()
        };
        fs::write(
            dir.path().join(".kobo/kepub").join(ID),
            book_zip(&chapter, fonts),
        )
        .unwrap();
        Self { dir, db }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }
    fn book(&self) -> std::path::PathBuf {
        self.path().join(".kobo/kepub").join(ID)
    }
}

fn book_zip(chapter: &[u8], fonts: bool) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let entries: &[(&str, &[u8])] = &[
        ("mimetype", b"application/epub+zip"),
        ("META-INF/container.xml", b"<container xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\"><rootfiles><rootfile full-path=\"OPS/book.opf\" media-type=\"application/oebps-package+xml\"/></rootfiles></container>"),
        ("OPS/book.opf", b"<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\"><metadata/><manifest><item id=\"chapter\" href=\"chapter.xhtml\" media-type=\"application/xhtml+xml\"/></manifest><spine><itemref idref=\"chapter\"/></spine></package>"),
        ("OPS/chapter.xhtml", chapter),
    ];
    for (name, data) in entries {
        zip.start_file(
            *name,
            SimpleFileOptions::default().compression_method(if *name == "mimetype" {
                CompressionMethod::Stored
            } else {
                CompressionMethod::Deflated
            }),
        )
        .unwrap();
        zip.write_all(data).unwrap();
    }
    if fonts {
        zip.start_file("META-INF/encryption.xml", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(FONTS).unwrap();
        zip.start_file("OPS/font.otf", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"obfuscated font bytes").unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn member(data: &[u8], name: &str) -> Vec<u8> {
    let mut zip = ZipArchive::new(Cursor::new(data)).unwrap();
    let mut bytes = Vec::new();
    zip.by_name(name).unwrap().read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn imports_encrypted_book_and_preserves_source() {
    let device = Device::new(true, false, false);
    let source = fs::read(device.book()).unwrap();
    let database = fs::read(device.path().join(".kobo/KoboReader.sqlite")).unwrap();
    let output = tempfile::tempdir().unwrap();
    let library = Library::open(device.path()).unwrap();
    let books = library.books().unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(books[0].id, ID);
    assert!(books[0].encrypted);
    let path = library.import(ID, output.path(), None).unwrap();
    assert_eq!(
        member(&fs::read(path).unwrap(), "OPS/chapter.xhtml"),
        CHAPTER
    );
    assert_eq!(fs::read(device.book()).unwrap(), source);
    assert_eq!(
        fs::read(device.path().join(".kobo/KoboReader.sqlite")).unwrap(),
        database
    );
}

#[test]
fn snapshot_includes_uncheckpointed_wal_data() {
    let device = Device::new(true, true, false);
    let wal_path = device.path().join(".kobo/KoboReader.sqlite-wal");
    let wal = fs::read(&wal_path).unwrap();
    assert!(!wal.is_empty());
    let library = Library::open(device.path()).unwrap();
    assert_eq!(library.books().unwrap().len(), 1);
    let output = tempfile::tempdir().unwrap();
    library.import(ID, output.path(), None).unwrap();
    assert_eq!(fs::read(wal_path).unwrap(), wal);
}

#[test]
fn wrong_serial_produces_no_output() {
    let device = Device::new(true, false, false);
    let output = tempfile::tempdir().unwrap();
    let library = Library::open(device.path()).unwrap();
    assert!(library
        .import(ID, output.path(), Some("WRONG-SERIAL"))
        .is_err());
    assert_eq!(fs::read_dir(output.path()).unwrap().count(), 0);
}

#[test]
fn serial_override_works_without_device_xml() {
    let device = Device::new(true, false, false);
    fs::remove_file(device.path().join(".adobe-digital-editions/device.xml")).unwrap();
    let library = Library::open(device.path()).unwrap();
    let output = tempfile::tempdir().unwrap();
    assert!(library.import(ID, output.path(), None).is_err());
    library.import(ID, output.path(), Some(SERIAL)).unwrap();
}

#[test]
fn plain_epub_is_copied_exactly_and_existing_files_survive() {
    let device = Device::new(false, false, true);
    let library = Library::open(device.path()).unwrap();
    let output = tempfile::tempdir().unwrap();
    let path = library.import(ID, output.path(), None).unwrap();
    let data = fs::read(&path).unwrap();
    assert_eq!(data, fs::read(device.book()).unwrap());
    fs::write(&path, b"existing content").unwrap();
    assert!(library.import(ID, output.path(), None).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"existing content");
    assert_eq!(fs::read_dir(output.path()).unwrap().count(), 1);
}

#[test]
fn encrypted_import_preserves_obfuscated_fonts() {
    let device = Device::new(true, false, true);
    let output = tempfile::tempdir().unwrap();
    let path = Library::open(device.path())
        .unwrap()
        .import(ID, output.path(), None)
        .unwrap();
    let data = fs::read(path).unwrap();
    assert_eq!(member(&data, "META-INF/encryption.xml"), FONTS);
    assert_eq!(member(&data, "OPS/font.otf"), b"obfuscated font bytes");
}

#[test]
fn incomplete_download_is_rejected() {
    let device = Device::new(true, false, false);
    device
        .db
        .execute("UPDATE content_keys SET elementid='OPS/missing.xhtml'", [])
        .unwrap();
    let output = tempfile::tempdir().unwrap();
    let error = Library::open(device.path())
        .unwrap()
        .import(ID, output.path(), None)
        .unwrap_err();
    assert!(error.to_string().contains("missing encrypted member"));
    assert_eq!(fs::read_dir(output.path()).unwrap().count(), 0);
}

#[test]
fn output_cannot_be_inside_device() {
    let device = Device::new(false, false, false);
    let output = device.path().join("new/output");
    assert!(Library::open(device.path())
        .unwrap()
        .import(ID, &output, None)
        .is_err());
    assert!(!device.path().join("new").exists());
}

#[test]
fn unicode_title_fits_filesystem_limits() {
    let device = Device::new(false, false, false);
    device
        .db
        .execute(
            "UPDATE content SET Title=?1 WHERE ContentID=?2",
            [&"📖".repeat(100), ID],
        )
        .unwrap();
    let output = tempfile::tempdir().unwrap();
    let path = Library::open(device.path())
        .unwrap()
        .import(ID, output.path(), None)
        .unwrap();
    assert!(path.file_name().unwrap().len() < 255);
}

#[cfg(unix)]
#[test]
fn symlinked_book_outside_device_is_not_listed() {
    let device = Device::new(false, false, false);
    let external = tempfile::NamedTempFile::new().unwrap();
    fs::remove_file(device.book()).unwrap();
    std::os::unix::fs::symlink(external.path(), device.book()).unwrap();
    assert!(Library::open(device.path())
        .unwrap()
        .books()
        .unwrap()
        .is_empty());
}

#[test]
fn cli_lists_json_imports_and_reports_errors() {
    let device = Device::new(true, false, false);
    let output = Command::new(env!("CARGO_BIN_EXE_xteink"))
        .args(["kobo", "list", "--json", "--device"])
        .arg(device.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let books: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(books[0]["id"], ID);
    let target = tempfile::tempdir().unwrap();
    let imported = Command::new(env!("CARGO_BIN_EXE_xteink"))
        .args(["kobo", "import", ID, "--device"])
        .arg(device.path())
        .arg("--output")
        .arg(target.path())
        .output()
        .unwrap();
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    assert_eq!(fs::read_dir(target.path()).unwrap().count(), 1);
    let missing = Command::new(env!("CARGO_BIN_EXE_xteink"))
        .args(["kobo", "import", "missing-id", "--device"])
        .arg(device.path())
        .arg("--output")
        .arg(target.path())
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("not downloaded"));
}

#[test]
fn malformed_epub_never_reaches_output() {
    let device = Device::new(false, false, false);
    fs::write(device.book(), book_zip(b"not XHTML", false)).unwrap();
    let output = tempfile::tempdir().unwrap();
    assert!(Library::open(device.path())
        .unwrap()
        .import(ID, output.path(), None)
        .is_err());
    assert_eq!(fs::read_dir(output.path()).unwrap().count(), 0);
    assert!(epub::inspect(b"not a ZIP").is_err());
}

#[test]
fn selects_the_requested_volume_in_a_multi_book_database() {
    let device = Device::new(true, false, false);
    device
        .db
        .execute(
            "INSERT INTO content VALUES ('other-volume', 'Other book', '')",
            [],
        )
        .unwrap();
    device
        .db
        .execute(
            "INSERT INTO content_keys VALUES ('other-volume', 'OPS/chapter.xhtml', 'invalid-key')",
            [],
        )
        .unwrap();
    fs::copy(
        device.book(),
        device.path().join(".kobo/kepub/other-volume"),
    )
    .unwrap();
    let library = Library::open(device.path()).unwrap();
    assert_eq!(library.books().unwrap().len(), 2);
    let output = tempfile::tempdir().unwrap();
    let path = library.import(ID, output.path(), None).unwrap();
    assert_eq!(
        member(&fs::read(path).unwrap(), "OPS/chapter.xhtml"),
        CHAPTER
    );
}

#[test]
fn adobe_metadata_is_reported_as_unsupported() {
    let device = Device::new(false, false, false);
    let mut zip = ZipWriter::new_append(Cursor::new(fs::read(device.book()).unwrap())).unwrap();
    zip.start_file("META-INF/rights.xml", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"<rights/>").unwrap();
    fs::write(device.book(), zip.finish().unwrap().into_inner()).unwrap();
    let output = tempfile::tempdir().unwrap();
    let error = Library::open(device.path())
        .unwrap()
        .import(ID, output.path(), None)
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("Adobe import is not implemented"));
    assert_eq!(fs::read_dir(output.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn read_only_database_can_be_imported() {
    use std::os::unix::fs::PermissionsExt;
    let device = Device::new(true, false, false);
    let database = device.path().join(".kobo/KoboReader.sqlite");
    fs::set_permissions(&database, fs::Permissions::from_mode(0o444)).unwrap();
    let output = tempfile::tempdir().unwrap();
    Library::open(device.path())
        .unwrap()
        .import(ID, output.path(), None)
        .unwrap();
    assert_eq!(
        fs::metadata(database).unwrap().permissions().mode() & 0o777,
        0o444
    );
}

#[cfg(unix)]
#[test]
fn symlinked_output_cannot_write_into_device() {
    let device = Device::new(false, false, false);
    let output = tempfile::tempdir().unwrap();
    let link = output.path().join("link");
    std::os::unix::fs::symlink(device.path(), &link).unwrap();
    assert!(Library::open(device.path())
        .unwrap()
        .import(ID, &link.join("new-books"), None)
        .is_err());
    assert!(!device.path().join("new-books").exists());
}

fn mark_preview(device: &Device) {
    device
        .db
        .execute_batch(
            "ALTER TABLE content ADD COLUMN Accessibility INTEGER;
        ALTER TABLE content ADD COLUMN IsDownloaded TEXT;",
        )
        .unwrap();
    device
        .db
        .execute(
            "UPDATE content SET Accessibility=6, IsDownloaded='true' WHERE ContentID=?1",
            [ID],
        )
        .unwrap();
}

#[test]
fn downloaded_preview_with_kobo_placeholder_gets_actionable_error() {
    let device = Device::new(false, false, false);
    mark_preview(&device);
    // Reproduce the reported layout using synthetic text: nested OPF points to
    // a relative kobo-locked.html, but the blank placeholder is at ZIP root.
    let original = fs::read(device.book()).unwrap();
    let mut source = ZipArchive::new(Cursor::new(original)).unwrap();
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..source.len() {
        let mut file = source.by_index(i).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        if file.name() == "OPS/book.opf" {
            bytes = String::from_utf8(bytes).unwrap()
                .replace("</manifest>", "<item id=\"kobo-locked.html\" href=\"kobo-locked.html\" media-type=\"application/xhtml+xml\"/></manifest>")
                .replace("</spine>", "<itemref idref=\"kobo-locked.html\"/></spine>").into_bytes();
        }
        writer
            .start_file(file.name(), SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&bytes).unwrap();
    }
    writer
        .start_file("kobo-locked.html", SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"<html><head/><body/></html>").unwrap();
    let preview = writer.finish().unwrap().into_inner();
    assert!(format!("{:#}", epub::validate(&preview).unwrap_err()).contains("OPS/kobo-locked.html"));
    fs::write(device.book(), &preview).unwrap();

    let library = Library::open(device.path()).unwrap();
    assert!(library.books().unwrap()[0].preview);
    let output = tempfile::tempdir().unwrap();
    let error = library.import(ID, output.path(), None).unwrap_err();
    assert!(error.to_string().contains("preview, not the full book"));
    assert!(error.to_string().contains("Download the full edition"));
    assert_eq!(fs::read_dir(output.path()).unwrap().count(), 0);
    assert_eq!(fs::read(device.book()).unwrap(), preview);
}

#[test]
fn cli_labels_previews_in_table_and_json() {
    let device = Device::new(false, false, false);
    mark_preview(&device);
    for json in [false, true] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_xteink"));
        cmd.args(["kobo", "list", "--show-previews", "--device"])
            .arg(device.path());
        if json {
            cmd.arg("--json");
        }
        let output = cmd.output().unwrap();
        assert!(output.status.success());
        if json {
            let books: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(books[0]["preview"], true);
        } else {
            let text = String::from_utf8(output.stdout).unwrap();
            for header in ["TITLE", "AUTHOR", "SOURCE", "TYPE", "KEYS", "ID"] {
                assert!(text.contains(header));
            }
            assert!(text.contains("Preview"));
            assert!(text.contains(ID));
        }
    }
    for json in [false, true] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_crossload"));
        cmd.args(["kobo", "list", "--device"]).arg(device.path());
        if json {
            cmd.arg("--json");
        }
        let output = cmd.output().unwrap();
        assert!(output.status.success());
        assert!(!String::from_utf8_lossy(&output.stdout).contains(ID));
        if json {
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
                serde_json::json!([])
            );
        } else {
            assert!(String::from_utf8_lossy(&output.stderr).contains("--show-previews"));
        }
    }
    let output_dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_xteink"))
        .args(["kobo", "import", ID, "--device"])
        .arg(device.path())
        .arg("--output")
        .arg(output_dir.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("preview, not the full book"));
}

#[test]
fn accessibility_accepts_text_and_null_without_misclassifying_full_books() {
    let device = Device::new(false, false, false);
    device
        .db
        .execute("ALTER TABLE content ADD COLUMN Accessibility TEXT", [])
        .unwrap();
    for (value, preview) in [(Some("6"), true), (Some("1"), false), (None, false)] {
        device
            .db
            .execute(
                "UPDATE content SET Accessibility=?1 WHERE ContentID=?2",
                rusqlite::params![value, ID],
            )
            .unwrap();
        let library = Library::open(device.path()).unwrap();
        assert_eq!(library.books().unwrap()[0].preview, preview);
        let output = tempfile::tempdir().unwrap();
        assert_eq!(library.import(ID, output.path(), None).is_err(), preview);
    }
}

fn sideload(device: &Device, relative: &str) -> std::path::PathBuf {
    let data = book_zip(CHAPTER, false);
    let mut source = ZipArchive::new(Cursor::new(data)).unwrap();
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..source.len() {
        let mut file = source.by_index(i).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        if file.name() == "OPS/book.opf" {
            bytes = String::from_utf8(bytes).unwrap().replace("<metadata/>",
                "<metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title>Test / book</dc:title><dc:creator>Émile</dc:creator><dc:creator>Another Author</dc:creator></metadata>").into_bytes();
        }
        writer
            .start_file(file.name(), SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&bytes).unwrap();
    }
    let path = device.path().join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, writer.finish().unwrap().into_inner()).unwrap();
    path
}

#[test]
fn lists_sideloaded_metadata_and_extensionless_epubs_but_skips_other_files() {
    let device = Device::new(false, false, false);
    for path in [
        "Author/book.epub",
        "Other/collection.kepub.epub",
        "Unknown/temporary_book",
    ] {
        sideload(&device, path);
    }
    sideload(&device, ".hidden/system.epub");
    sideload(&device, "fonts/not-a-book.epub");
    fs::write(
        device.path().join("Unknown/weather.csv"),
        b"day,temp\nMonday,20",
    )
    .unwrap();
    let books = Library::open(device.path()).unwrap().books().unwrap();
    assert_eq!(books.len(), 4);
    let local: Vec<_> = books
        .iter()
        .filter(|b| b.source == Source::Sideloaded)
        .collect();
    assert_eq!(local.len(), 3);
    for book in &local {
        assert_eq!(book.title, "Test / book");
        assert_eq!(book.author, "Émile, Another Author");
        assert!(book.id.starts_with("file:"));
        assert!(book.path.is_some());
    }
    assert_eq!(
        local
            .iter()
            .map(|b| &b.id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
}

#[test]
fn highlights_and_notes_come_off_the_device_and_dog_ears_stay_behind() {
    let device = Device::new(false, false, false);
    let db = Connection::open(device.path().join(".kobo/KoboReader.sqlite")).unwrap();
    db.execute_batch(
        "CREATE TABLE Bookmark (VolumeID TEXT, Text TEXT, Annotation TEXT,
         DateCreated TEXT, ChapterProgress REAL, Hidden TEXT, Type TEXT);",
    )
    .unwrap();
    let rows: [(&str, &str, &str, &str, f64, &str, &str); 4] = [
        (
            ID,
            "  a passage worth keeping  ",
            "",
            "2025-09-02T11:12:54.000",
            0.216,
            "false",
            "highlight",
        ),
        (
            ID,
            "",
            "a thought with no passage",
            "2025-09-03T08:00:00.000",
            0.5,
            "false",
            "note",
        ),
        // A dog-ear marks a page and says nothing, and a hidden mark was
        // taken back; neither has anything to carry off the device.
        (
            ID,
            "",
            "",
            "2025-09-04T08:00:00.000",
            0.9,
            "false",
            "dogear",
        ),
        (
            ID,
            "deleted highlight",
            "",
            "2025-09-05T08:00:00.000",
            0.1,
            "true",
            "highlight",
        ),
    ];
    for row in rows {
        db.execute(
            "INSERT INTO Bookmark VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![row.0, row.1, row.2, row.3, row.4, row.5, row.6],
        )
        .unwrap();
    }
    drop(db);

    let notes = Library::open(device.path()).unwrap().annotations().unwrap();
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert_eq!(notes[0].text, "a passage worth keeping");
    assert_eq!(notes[0].title, "Test / book");
    assert_eq!(notes[0].author, "Test Author");
    assert_eq!(notes[1].note, "a thought with no passage");

    let marks: Vec<_> = notes.iter().collect();
    let markdown = crossload::kobo::as_markdown("Test / book", "Test Author", &marks);
    assert!(
        markdown.starts_with("# Test / book\n\n*Test Author*\n"),
        "{markdown}"
    );
    // The book's words are quoted; the reader's are not, so which is which
    // survives the trip.
    assert!(markdown.contains("> a passage worth keeping"), "{markdown}");
    assert!(
        markdown.contains("\na thought with no passage\n"),
        "{markdown}"
    );
    assert!(markdown.contains("— 22% in, 2025-09-02"), "{markdown}");

    // A device without the table at all is a device with no marks on it.
    let bare = Device::new(false, false, false);
    assert!(Library::open(bare.path())
        .unwrap()
        .annotations()
        .unwrap()
        .is_empty());
}

#[test]
fn sideloaded_pdfs_are_listed_and_imported_untouched() {
    let device = Device::new(false, false, false);
    let paper = b"%PDF-1.7\n1 0 obj<</Type/Catalog>>endobj\ntrailer\n%%EOF\n".to_vec();
    fs::create_dir_all(device.path().join("Papers")).unwrap();
    fs::write(device.path().join("Papers/Some Paper.pdf"), &paper).unwrap();
    // A file whose name promises nothing we carry is still skipped.
    fs::write(device.path().join("Papers/notes.txt"), b"nothing").unwrap();

    let library = Library::open(device.path()).unwrap();
    let books = library.books().unwrap();
    let paper_book = books.iter().find(|b| b.title == "Some Paper").unwrap();
    assert_eq!(paper_book.format, crossload::format::Format::Pdf);
    assert_eq!(paper_book.author, "");
    assert!(!paper_book.encrypted);
    assert_eq!(books.iter().filter(|b| b.title == "notes").count(), 0);

    // Import carries the bytes over and names the copy by its own format.
    let output = tempfile::tempdir().unwrap();
    let imported = library.import(&paper_book.id, output.path(), None).unwrap();
    assert_eq!(imported.extension().unwrap(), "pdf");
    assert_eq!(fs::read(&imported).unwrap(), paper);
}

#[test]
fn imports_sideloaded_copies_without_keys_and_keeps_same_title_store_preview_separate() {
    let device = Device::new(false, false, false);
    mark_preview(&device);
    let first = sideload(&device, "Author/book.kepub.epub");
    sideload(&device, "Unknown/extensionless");
    let library = Library::open(device.path()).unwrap();
    let books = library.books().unwrap();
    assert_eq!(books.len(), 3);
    assert!(books.iter().find(|b| b.id == ID).unwrap().preview);
    let output = tempfile::tempdir().unwrap();
    for book in books.iter().filter(|b| b.source == Source::Sideloaded) {
        let path = library.import(&book.id, output.path(), None).unwrap();
        assert_eq!(fs::read(path).unwrap(), fs::read(&first).unwrap());
        assert!(!book.preview);
    }
    assert_eq!(fs::read_dir(output.path()).unwrap().count(), 2);
    assert!(library.import(ID, output.path(), None).is_err());
}

#[test]
fn sideloaded_ids_survive_mount_location_changes_and_missing_store_directory() {
    let first = Device::new(false, false, false);
    let second = Device::new(false, false, false);
    for device in [&first, &second] {
        sideload(device, "Author/book.epub");
    }
    fs::remove_file(second.book()).unwrap();
    fs::remove_dir(second.path().join(".kobo/kepub")).unwrap();
    let first_books = Library::open(first.path()).unwrap().books().unwrap();
    let second_library = Library::open(second.path()).unwrap();
    let second_books = second_library.books().unwrap();
    assert_eq!(second_books.len(), 1);
    assert_eq!(
        first_books
            .iter()
            .find(|b| b.source == Source::Sideloaded)
            .unwrap()
            .id,
        second_books[0].id
    );
    let output = tempfile::tempdir().unwrap();
    second_library
        .import(&second_books[0].id, output.path(), None)
        .unwrap();
}

#[cfg(unix)]
#[test]
fn sideloaded_scan_ignores_symlink_cycles_and_external_files() {
    let device = Device::new(false, false, false);
    let other = Device::new(false, false, false);
    let external = sideload(&other, "Author/book.epub");
    std::os::unix::fs::symlink(device.path(), device.path().join("cycle")).unwrap();
    std::os::unix::fs::symlink(external, device.path().join("outside.epub")).unwrap();
    assert_eq!(
        Library::open(device.path()).unwrap().books().unwrap().len(),
        1
    );
}

#[test]
fn cli_lists_source_and_imports_sideloaded_id() {
    let device = Device::new(false, false, false);
    sideload(&device, "Unknown/without-extension");
    let listed = Command::new(env!("CARGO_BIN_EXE_xteink"))
        .args(["kobo", "list", "--json", "--device"])
        .arg(device.path())
        .output()
        .unwrap();
    assert!(listed.status.success());
    let books: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    let local = books
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["source"] == "sideloaded")
        .unwrap();
    assert_eq!(local["path"], "Unknown/without-extension");
    let output = tempfile::tempdir().unwrap();
    let imported = Command::new(env!("CARGO_BIN_EXE_xteink"))
        .args(["kobo", "import", local["id"].as_str().unwrap(), "--device"])
        .arg(device.path())
        .arg("--output")
        .arg(output.path())
        .output()
        .unwrap();
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    assert_eq!(fs::read_dir(output.path()).unwrap().count(), 1);
}

fn with_store_identifier(data: &[u8]) -> Vec<u8> {
    let mut source = ZipArchive::new(Cursor::new(data)).unwrap();
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..source.len() {
        let mut file = source.by_index(i).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        if file.name() == "OPS/book.opf" {
            bytes = String::from_utf8(bytes).unwrap().replace("<metadata/>", &format!(
                "<metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:identifier>urn:uuid:{ID}</dc:identifier></metadata>"
            )).into_bytes();
        }
        writer
            .start_file(file.name(), SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn extensionless_encrypted_store_copy_uses_the_original_volume_keys() {
    let device = Device::new(true, false, false);
    let encrypted = with_store_identifier(&fs::read(device.book()).unwrap());
    fs::write(device.book(), &encrypted).unwrap();
    fs::create_dir(device.path().join("Unknown")).unwrap();
    let copy = device.path().join("Unknown/view_device_book");
    fs::write(&copy, &encrypted).unwrap();
    let library = Library::open(device.path()).unwrap();
    let books = library.books().unwrap();
    let book = books
        .iter()
        .find(|b| b.source == Source::Sideloaded)
        .unwrap();
    assert!(book.encrypted);
    let output = tempfile::tempdir().unwrap();
    let path = library.import(&book.id, output.path(), None).unwrap();
    assert_eq!(
        member(&fs::read(path).unwrap(), "OPS/chapter.xhtml"),
        CHAPTER
    );
    assert_eq!(fs::read(copy).unwrap(), encrypted);
}

#[test]
fn decrypted_copy_with_same_identifier_is_not_decrypted_again() {
    let device = Device::new(true, false, false);
    fs::write(
        device.book(),
        with_store_identifier(&fs::read(device.book()).unwrap()),
    )
    .unwrap();
    let plain = with_store_identifier(&book_zip(CHAPTER, false));
    fs::write(device.path().join("already-readable.epub"), &plain).unwrap();
    let library = Library::open(device.path()).unwrap();
    let books = library.books().unwrap();
    let book = books
        .iter()
        .find(|b| b.source == Source::Sideloaded)
        .unwrap();
    assert!(!book.encrypted);
    let output = tempfile::tempdir().unwrap();
    let path = library.import(&book.id, output.path(), None).unwrap();
    assert_eq!(fs::read(path).unwrap(), plain);
}
