use crossload::books::{self, Options, Place};
use std::{
    fs,
    io::{Cursor, Write},
};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
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

fn options(root: &std::path::Path) -> Options {
    let local = root.join("local");
    fs::create_dir(&local).unwrap();
    let card = root.join("card");
    fs::create_dir(&card).unwrap();
    Options {
        show_previews: false,
        local,
        output: root.join("output"),
        kobo: Some(root.join("kobo")),
        reader: None,
        card: Some(card),
        folder: "/".into(),
        serial: None,
        optimize: true,
        organized: true,
        cache: Some(root.join("index.json")),
    }
}
fn kobo(root: &std::path::Path) {
    fs::create_dir_all(root.join(".kobo/kepub")).unwrap();
    let db = rusqlite::Connection::open(root.join(".kobo/KoboReader.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE content (ContentID TEXT, Title TEXT, Attribution TEXT, Accessibility INTEGER); CREATE TABLE content_keys (volumeid TEXT, elementid TEXT, elementkey TEXT); CREATE TABLE user (UserID TEXT);").unwrap();
}
#[test]
fn disconnected_kobo_does_not_hide_local_or_reader_books_and_matches_are_conservative() {
    let tmp = tempfile::tempdir().unwrap();
    let o = options(tmp.path());
    fs::write(
        o.local.join("original.epub"),
        epub("text", CompressionMethod::Stored),
    )
    .unwrap();
    fs::write(
        o.card.as_ref().unwrap().join("renamed.epub"),
        epub("text", CompressionMethod::Deflated),
    )
    .unwrap();
    fs::write(
        o.card.as_ref().unwrap().join("other.epub"),
        epub("different edition", CompressionMethod::Stored),
    )
    .unwrap();
    let mut updates = 0;
    let snapshot = books::scan(&o, |_| updates += 1);
    // Every location publishes its own result, partial catalogs arrive before.
    assert!(updates >= 3);
    assert!(!snapshot.ready(Place::Kobo));
    assert!(snapshot.ready(Place::Local));
    assert!(snapshot.ready(Place::Xteink));
    assert_eq!(snapshot.books.len(), 2);
    let shared = snapshot.books.iter().find(|b| b.has(Place::Local)).unwrap();
    assert!(shared.has(Place::Xteink));
    assert_eq!(shared.preferred().unwrap().place, Place::Local);
    assert!(!o.output.exists());
}
#[test]
fn transfers_use_originals_and_sideload_kobo_without_overwriting() {
    let tmp = tempfile::tempdir().unwrap();
    let o = options(tmp.path());
    let root = o.kobo.as_ref().unwrap();
    kobo(root);
    let original = epub("text", CompressionMethod::Stored);
    fs::write(o.local.join("source.epub"), &original).unwrap();
    let snapshot = books::scan(&o, |_| {});
    let book = &snapshot.books[0];
    books::transfer(&o, book, Place::Kobo, &|_| {}).unwrap();
    assert_eq!(fs::read(root.join("Author/Book.epub")).unwrap(), original);
    let snapshot = books::scan(&o, |_| {});
    assert_eq!(snapshot.books.len(), 1);
    assert!(snapshot.books[0].has(Place::Kobo));
    let mut from_kobo = snapshot.books[0].clone();
    from_kobo.copies.retain(|c| c.place == Place::Kobo);
    books::transfer(&o, &from_kobo, Place::Local, &|_| {}).unwrap();
    assert_eq!(fs::read(o.output.join("Book.epub")).unwrap(), original);
    fs::write(o.output.join("Book.epub"), b"existing").unwrap();
    assert!(books::transfer(&o, &from_kobo, Place::Local, &|_| {}).is_err());
    assert_eq!(fs::read(o.output.join("Book.epub")).unwrap(), b"existing");
    fs::write(o.local.join("source.epub"), b"changed").unwrap();
    assert!(books::transfer(&o, book, Place::Xteink, &|_| {}).is_err());
}
#[test]
fn reader_only_books_can_be_recovered_to_local_or_kobo() {
    let tmp = tempfile::tempdir().unwrap();
    let o = options(tmp.path());
    kobo(o.kobo.as_ref().unwrap());
    let bytes = epub("reader only", CompressionMethod::Stored);
    fs::write(o.card.as_ref().unwrap().join("only.epub"), &bytes).unwrap();
    let snapshot = books::scan(&o, |_| {});
    let book = &snapshot.books[0];
    assert_eq!(book.preferred().unwrap().place, Place::Xteink);
    assert!(books::transfer(&o, book, Place::Local, &|_| {})
        .unwrap()
        .contains("device copy"));
    assert_eq!(fs::read(o.output.join("Book.epub")).unwrap(), bytes);
    books::transfer(&o, book, Place::Kobo, &|_| {}).unwrap();
    assert_eq!(
        fs::read(o.kobo.as_ref().unwrap().join("Author/Book.epub")).unwrap(),
        bytes
    );
}

#[test]
fn optimized_reader_copy_groups_with_original_and_original_is_used_for_kobo() {
    let tmp = tempfile::tempdir().unwrap();
    let o = options(tmp.path());
    kobo(o.kobo.as_ref().unwrap());
    let source = o.local.join("original.epub");
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let base = epub("with image", CompressionMethod::Stored);
    let mut input = zip::ZipArchive::new(Cursor::new(base)).unwrap();
    for i in 0..input.len() {
        let mut member = input.by_index(i).unwrap();
        zip.start_file(member.name(), SimpleFileOptions::default())
            .unwrap();
        std::io::copy(&mut member, &mut zip).unwrap();
    }
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        900,
        1200,
        image::Rgb([200, 30, 70]),
    ))
    .write_to(&mut png, image::ImageFormat::Png)
    .unwrap();
    zip.start_file("cover.png", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(png.get_ref()).unwrap();
    fs::write(&source, zip.finish().unwrap().into_inner()).unwrap();
    let prepared = crossload::prepare::prepare(&source, true, true).unwrap();
    assert_ne!(
        fs::read(&source).unwrap(),
        fs::read(&prepared.path).unwrap()
    );
    fs::copy(
        &prepared.path,
        o.card.as_ref().unwrap().join("renamed.epub"),
    )
    .unwrap();
    let snapshot = books::scan(&o, |_| {});
    assert_eq!(snapshot.books.len(), 1);
    let book = &snapshot.books[0];
    assert!(book.has(Place::Xteink));
    assert_eq!(book.preferred().unwrap().place, Place::Local);
    books::transfer(&o, book, Place::Kobo, &|_| {}).unwrap();
    assert_eq!(
        fs::read(o.kobo.as_ref().unwrap().join("Author/Book.epub")).unwrap(),
        fs::read(&source).unwrap()
    );
    // If the local file is itself a recovered device copy, prefer Kobo's original.
    fs::copy(&prepared.path, &source).unwrap();
    let snapshot = books::scan(&o, |_| {});
    assert_eq!(snapshot.books.len(), 1);
    assert_eq!(snapshot.books[0].preferred().unwrap().place, Place::Kobo);
    books::transfer(&o, &snapshot.books[0], Place::Local, &|_| {}).unwrap();
    assert_eq!(
        fs::read(o.output.join("Book.epub")).unwrap(),
        fs::read(o.kobo.as_ref().unwrap().join("Author/Book.epub")).unwrap()
    );
}

#[test]
fn disconnected_card_and_browse_keep_output_books_and_unreadable_rows_visible() {
    let tmp = tempfile::tempdir().unwrap();
    let mut o = options(tmp.path());
    fs::create_dir(&o.output).unwrap();
    fs::write(
        o.output.join("good.epub"),
        epub("good", CompressionMethod::Stored),
    )
    .unwrap();
    fs::write(o.output.join("broken.epub"), b"bad").unwrap();
    o.local = tmp.path().join("missing");
    o.card = Some(tmp.path().join("unplugged"));
    let snapshot = books::scan(&o, |_| {});
    assert_eq!(snapshot.books.len(), 2);
    assert!(snapshot.ready(Place::Local));
    assert!(!snapshot.ready(Place::Xteink));
    let broken = snapshot
        .books
        .iter()
        .find(|b| b.title == "broken.epub")
        .unwrap();
    assert!(books::transfer(&o, broken, Place::Kobo, &|_| {}).is_err());
    assert!(!o.card.unwrap().exists());
}

#[test]
fn stored_identities_are_reused_until_a_file_changes_and_never_outlive_it() {
    let tmp = tempfile::tempdir().unwrap();
    let o = options(tmp.path());
    let path = o.local.join("book.epub");
    fs::write(&path, epub("text", CompressionMethod::Stored)).unwrap();
    let first = books::scan(&o, |_| {});
    assert_eq!(first.books.len(), 1);
    // An unchanged library produces exactly the same catalog from the cache.
    let second = books::scan(&o, |_| {});
    assert_eq!(second.books, first.books);
    // Rewriting the bytes while restoring size and timestamp is deliberately
    // invisible: trusting (size, mtime) is what makes a refresh cheap.
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    fs::write(&path, epub("txet", CompressionMethod::Stored)).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert_eq!(books::scan(&o, |_| {}).books, first.books);
    // A newer timestamp is a miss, and the new contents take over.
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified + std::time::Duration::from_secs(2))
        .unwrap();
    let rescanned = books::scan(&o, |_| {});
    assert_ne!(
        rescanned.books[0].copies[0].sha,
        first.books[0].copies[0].sha
    );
    // A cached book whose file is gone must not survive as an offline history.
    fs::remove_file(&path).unwrap();
    assert!(books::scan(&o, |_| {}).books.is_empty());
}

#[test]
fn an_unchanged_kobo_book_is_not_imported_again() {
    let tmp = tempfile::tempdir().unwrap();
    let o = options(tmp.path());
    let root = o.kobo.as_ref().unwrap();
    kobo(root);
    let sideloaded = root.join("Author/Book.epub");
    fs::create_dir_all(sideloaded.parent().unwrap()).unwrap();
    fs::write(&sideloaded, epub("text", CompressionMethod::Stored)).unwrap();
    let first = books::scan(&o, |_| {});
    assert_eq!(first.books.len(), 1);
    assert!(first.books[0].has(Place::Kobo));
    // Replacing the contents while restoring size and timestamp proves the
    // second scan never read the device file: the identity is unchanged.
    let modified = fs::metadata(&sideloaded).unwrap().modified().unwrap();
    fs::write(&sideloaded, epub("txet", CompressionMethod::Stored)).unwrap();
    fs::File::options()
        .write(true)
        .open(&sideloaded)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert_eq!(books::scan(&o, |_| {}).books, first.books);
    // A newer timestamp is a miss, and a removed book never lingers.
    fs::File::options()
        .write(true)
        .open(&sideloaded)
        .unwrap()
        .set_modified(modified + std::time::Duration::from_secs(2))
        .unwrap();
    let rescanned = books::scan(&o, |_| {});
    assert_ne!(
        rescanned.books[0].copies[0].sha,
        first.books[0].copies[0].sha
    );
    fs::remove_file(&sideloaded).unwrap();
    assert!(books::scan(&o, |_| {}).books.is_empty());
}

#[test]
fn books_are_published_before_their_location_finishes() {
    let tmp = tempfile::tempdir().unwrap();
    let o = options(tmp.path());
    for i in 0..3 {
        fs::write(
            o.local.join(format!("book{i}.epub")),
            epub(&format!("text {i}"), CompressionMethod::Stored),
        )
        .unwrap();
    }
    let mut seen = Vec::new();
    let snapshot = books::scan(&o, |s| {
        let local = s.status.iter().find(|(p, _)| *p == Place::Local).unwrap();
        seen.push((s.books.len(), local.1.clone()));
    });
    assert_eq!(snapshot.books.len(), 3);
    let (count, status) = seen
        .iter()
        .find(|(count, _)| (1..3).contains(count))
        .expect("a partial catalog before Local finished");
    assert!(status.starts_with("Checking"), "{status}");
    assert_eq!(
        *status,
        format!(
            "Checking ({count} book{})",
            if *count == 1 { "" } else { "s" }
        )
    );
    assert!(seen.last().unwrap().1.starts_with("Ready (3 books"));
}

#[test]
fn a_fulfilled_request_in_archive_is_no_longer_pending() {
    let tmp = tempfile::tempdir().unwrap();
    let o = options(tmp.path());
    fs::write(o.local.join("waiting.acsm"), b"<request/>").unwrap();
    let archive = o.local.join("archive");
    fs::create_dir_all(&archive).unwrap();
    fs::write(archive.join("spent.acsm"), b"<request/>").unwrap();
    let snapshot = books::scan(&o, |_| {});
    assert_eq!(snapshot.acsm.len(), 1);
    assert!(snapshot.acsm[0].ends_with("waiting.acsm"));
}

#[test]
fn removing_a_copy_keeps_the_book_and_refuses_the_last_one() {
    let tmp = tempfile::tempdir().unwrap();
    let o = options(tmp.path());
    let original = epub("text", CompressionMethod::Stored);
    let here = o.local.join("keep.epub");
    let duplicate = o.local.join("duplicate.epub");
    fs::write(&here, &original).unwrap();
    fs::write(&duplicate, epub("text", CompressionMethod::Deflated)).unwrap();
    let snapshot = books::scan(&o, |_| {});
    // Repacked bytes are the same book, so both files are copies of one book.
    assert_eq!(snapshot.books.len(), 1);
    let book = &snapshot.books[0];
    assert_eq!(book.copies.len(), 2);
    let removed = books::remove(
        &o,
        book,
        Place::Local,
        &duplicate.to_string_lossy(),
        &|_| {},
    )
    .unwrap();
    assert!(removed.contains("Deleted"), "{removed}");
    assert!(!duplicate.exists());
    assert_eq!(
        fs::read(&here).unwrap(),
        original,
        "the other copy is untouched"
    );
    // What remains is the only copy, and the only copy is never removed.
    let snapshot = books::scan(&o, |_| {});
    let book = &snapshot.books[0];
    assert_eq!(book.copies.len(), 1);
    let refused = books::remove(&o, book, Place::Local, &here.to_string_lossy(), &|_| {})
        .unwrap_err()
        .to_string();
    assert!(refused.contains("only copy"), "{refused}");
    assert!(here.exists());
}

#[test]
fn a_changed_or_unknown_copy_is_never_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let o = options(tmp.path());
    let first = o.local.join("one.epub");
    let second = o.local.join("two.epub");
    fs::write(&first, epub("text", CompressionMethod::Stored)).unwrap();
    fs::write(&second, epub("text", CompressionMethod::Deflated)).unwrap();
    let book = books::scan(&o, |_| {}).books.remove(0);
    // Replaced contents are not the copy that was found.
    fs::write(
        &second,
        epub("different text entirely", CompressionMethod::Stored),
    )
    .unwrap();
    let refused = books::remove(&o, &book, Place::Local, &second.to_string_lossy(), &|_| {})
        .unwrap_err()
        .to_string();
    assert!(refused.contains("changed since discovery"), "{refused}");
    assert!(second.exists());
    // A path that is not one of this book's copies is refused outright.
    let unknown = books::remove(&o, &book, Place::Local, "/elsewhere.epub", &|_| {})
        .unwrap_err()
        .to_string();
    assert!(unknown.contains("no longer in the library"), "{unknown}");
}

#[test]
fn a_reader_copy_is_removed_from_a_card_but_never_over_wifi() {
    let tmp = tempfile::tempdir().unwrap();
    let mut o = options(tmp.path());
    let local = o.local.join("book.epub");
    fs::write(&local, epub("text", CompressionMethod::Stored)).unwrap();
    let card = o.card.clone().unwrap();
    fs::write(
        card.join("copy.epub"),
        epub("text", CompressionMethod::Stored),
    )
    .unwrap();
    let book = books::scan(&o, |_| {}).books.remove(0);
    assert_eq!(book.copies.len(), 2);
    let on_card = book
        .copies
        .iter()
        .find(|c| c.place == Place::Xteink)
        .unwrap()
        .path
        .clone();
    // Over Wi-Fi the protocol cannot delete, and nothing is attempted.
    let wifi = books::Options {
        card: None,
        reader: Some("127.0.0.1:1".into()),
        ..o.clone()
    };
    let refused = books::remove(&wifi, &book, Place::Xteink, &on_card, &|_| {})
        .unwrap_err()
        .to_string();
    assert!(refused.contains("Wi-Fi"), "{refused}");
    assert!(card.join("copy.epub").exists());
    // On a mounted card it is an ordinary file, and only that file goes.
    o.reader = None;
    let removed = books::remove(&o, &book, Place::Xteink, &on_card, &|_| {}).unwrap();
    assert!(removed.contains("card"), "{removed}");
    assert!(!card.join("copy.epub").exists());
    assert!(local.exists());
}
