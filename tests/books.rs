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
    assert_eq!(updates, 3);
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
