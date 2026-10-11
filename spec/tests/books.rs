//! The library across local folders, a Kobo and a CrossPoint reader, on sim capabilities.
use std::io::{Cursor, Write};

use crossload::books::{self, Place, Scan};
use crossload::cache;
use crossload_kobo::StoreBook;
use spec::books::{cbz, epub, pdf, plain};
use spec::world::{World, BOOKS, CARD, IMPORT};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

fn scan(world: &World) -> books::Snapshot {
    world.library().scan()
}

#[test]
fn disconnected_kobo_does_not_hide_local_or_reader_books_and_matches_are_conservative() {
    let world = World::new();
    world.kobo.missing();
    world
        .books
        .file("original.epub", plain("text", CompressionMethod::Stored));
    world
        .card
        .file("renamed.epub", plain("text", CompressionMethod::Deflated));
    world.card.file(
        "other.epub",
        plain("different edition", CompressionMethod::Stored),
    );
    let snapshot = scan(&world);
    assert!(!snapshot.ready(Place::Kobo));
    assert!(snapshot.ready(Place::Local));
    assert!(snapshot.ready(Place::CrossPoint));
    assert_eq!(snapshot.books.len(), 2);
    let shared = snapshot.books.iter().find(|b| b.has(Place::Local)).unwrap();
    assert!(shared.has(Place::CrossPoint));
    assert_eq!(shared.preferred().unwrap().place, Place::Local);
    // Scanning only reads.
    assert!(world.import.read("Book.epub").is_none());
}

#[test]
fn transfers_use_originals_and_sideload_kobo_without_overwriting() {
    let world = World::new();
    let original = plain("text", CompressionMethod::Stored);
    world.books.file("source.epub", original.clone());
    let snapshot = scan(&world);
    let book = &snapshot.books[0];
    let library = world.library();
    books::transfer(&library, book, Place::Kobo, &|_| {}).unwrap();
    assert_eq!(world.kobo_dir.read("Author/Book.epub").unwrap(), original);
    let snapshot = scan(&world);
    assert_eq!(snapshot.books.len(), 1);
    assert!(snapshot.books[0].has(Place::Kobo));
    let mut from_kobo = snapshot.books[0].clone();
    from_kobo.copies.retain(|c| c.place == Place::Kobo);
    books::transfer(&library, &from_kobo, Place::Local, &|_| {}).unwrap();
    assert_eq!(world.import.read("Book.epub").unwrap(), original);
    // An existing file is never replaced.
    world.import.file("Book.epub", b"existing".to_vec());
    assert!(books::transfer(&library, &from_kobo, Place::Local, &|_| {}).is_err());
    assert_eq!(world.import.read("Book.epub").unwrap(), b"existing");
    // A source that changed since discovery is not copied.
    world.books.file("source.epub", b"changed".to_vec());
    assert!(books::transfer(&library, book, Place::CrossPoint, &|_| {}).is_err());
    assert!(world.card.read("Author/Book.epub").is_none());
}

#[test]
fn reader_only_books_can_be_recovered_to_local_or_kobo() {
    let world = World::new();
    let bytes = plain("reader only", CompressionMethod::Stored);
    world.card.file("only.epub", bytes.clone());
    let snapshot = scan(&world);
    let book = &snapshot.books[0];
    assert_eq!(book.preferred().unwrap().place, Place::CrossPoint);
    let library = world.library();
    assert!(books::transfer(&library, book, Place::Local, &|_| {})
        .unwrap()
        .contains("device copy"));
    assert_eq!(world.import.read("Book.epub").unwrap(), bytes);
    books::transfer(&library, book, Place::Kobo, &|_| {}).unwrap();
    assert_eq!(world.kobo_dir.read("Author/Book.epub").unwrap(), bytes);
}

fn with_cover() -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let base = plain("with image", CompressionMethod::Stored);
    let mut input = zip::ZipArchive::new(Cursor::new(base)).unwrap();
    for i in 0..input.len() {
        let mut member = input.by_index(i).unwrap();
        zip.start_file(member.name(), SimpleFileOptions::default())
            .unwrap();
        std::io::copy(&mut member, &mut zip).unwrap();
    }
    zip.start_file("cover.png", SimpleFileOptions::default())
        .unwrap();
    // A large flat image, so optimizing changes it.
    let mut png = vec![];
    let width = 900u32;
    let height = 1200u32;
    {
        // An uncompressed-ish PNG built by hand would be long; a JPEG-free
        // way is to let the app's own optimizer see a PNG it can decode.
        let raw = vec![0x80u8; (width * height * 3) as usize];
        let image = image_png(width, height, &raw);
        png.extend(image);
    }
    zip.write_all(&png).unwrap();
    zip.finish().unwrap().into_inner()
}

/// A minimal valid RGB PNG, stored (zlib level 0), so the test needs no image crate.
fn image_png(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
    fn crc(data: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    fn adler(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for &d in data {
            a = (a + d as u32) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend(data);
        out.extend(&body);
        out.extend(crc(&body).to_be_bytes());
    }
    let mut raw = vec![];
    for row in rgb.chunks((width * 3) as usize) {
        raw.push(0);
        raw.extend(row);
    }
    let mut z = vec![0x78, 0x01];
    for (i, block) in raw.chunks(65535).enumerate() {
        let last = (i + 1) * 65535 >= raw.len();
        z.push(u8::from(last));
        z.extend((block.len() as u16).to_le_bytes());
        z.extend((!(block.len() as u16)).to_le_bytes());
        z.extend(block);
    }
    z.extend(adler(&raw).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = width.to_be_bytes().to_vec();
    header.extend(height.to_be_bytes());
    header.extend([8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

#[test]
fn optimized_reader_copy_groups_with_original_and_original_is_used_for_kobo() {
    let world = World::new();
    let source = with_cover();
    world.books.file("original.epub", source.clone());
    let prepared = crossload::prepare::prepare(
        source.clone(),
        "original.epub",
        Some(crossload::profile::X4),
        true,
    )
    .unwrap();
    assert_ne!(source, prepared.data);
    world.card.file("renamed.epub", prepared.data.clone());
    let snapshot = scan(&world);
    assert_eq!(snapshot.books.len(), 1);
    let book = &snapshot.books[0];
    assert!(book.has(Place::CrossPoint));
    assert_eq!(book.preferred().unwrap().place, Place::Local);
    let library = world.library();
    books::transfer(&library, book, Place::Kobo, &|_| {}).unwrap();
    assert_eq!(world.kobo_dir.read("Author/Book.epub").unwrap(), source);
    // If the local file is itself a recovered device copy, prefer Kobo's original.
    world.books.file("original.epub", prepared.data.clone());
    let snapshot = scan(&world);
    assert_eq!(snapshot.books.len(), 1);
    assert_eq!(snapshot.books[0].preferred().unwrap().place, Place::Kobo);
    books::transfer(&library, &snapshot.books[0], Place::Local, &|_| {}).unwrap();
    assert_eq!(world.import.read("Book.epub").unwrap(), source);
}

#[test]
fn an_unplugged_card_keeps_import_books_and_unreadable_rows_visible() {
    let mut world = World::new();
    world
        .import
        .file("good.epub", plain("good", CompressionMethod::Stored));
    world.import.file("broken.epub", b"bad".to_vec());
    // An unplugged card: the folder is not there.
    world.card = ambit_caps_sim::SimDir::new();
    world.card.remove("x");
    let mut library = world.library();
    library.reader = Some(crossload::inventory::Destination::Card(
        crossload::inventory::Card {
            dir: std::sync::Arc::new(Unplugged),
            display: CARD.into(),
        },
    ));
    world.places.kobo = false;
    let snapshot = library.scan();
    assert_eq!(snapshot.books.len(), 2);
    assert!(snapshot.ready(Place::Local));
    assert!(!snapshot.ready(Place::CrossPoint));
    let broken = snapshot
        .books
        .iter()
        .find(|b| b.title == "broken.epub")
        .unwrap();
    assert!(broken.author.starts_with("Unreadable:"));
    assert!(books::transfer(&library, broken, Place::Kobo, &|_| {}).is_err());
}

/// A card that is not mounted: everything is `NotFound`, as `run-crossload`'s mount says.
struct Unplugged;
impl ambit_caps_dir::Dir for Unplugged {
    fn metadata(
        &self,
        p: &ambit_caps_dir::RelPath,
    ) -> Result<ambit_caps_dir::Meta, ambit_caps_dir::DirError> {
        Err(gone(p))
    }
    fn open(
        &self,
        p: &ambit_caps_dir::RelPath,
    ) -> Result<Box<dyn ambit_caps_dir::File>, ambit_caps_dir::DirError> {
        Err(gone(p))
    }
    fn list(
        &self,
        p: &ambit_caps_dir::RelPath,
    ) -> Result<Vec<ambit_caps_dir::Entry>, ambit_caps_dir::DirError> {
        Err(gone(p))
    }
}
impl ambit_caps_dir::DirWrite for Unplugged {
    fn create_dir(&self, p: &ambit_caps_dir::RelPath) -> Result<(), ambit_caps_dir::DirError> {
        Err(gone(p))
    }
    fn create_new(
        &self,
        p: &ambit_caps_dir::RelPath,
        _: &[u8],
    ) -> Result<(), ambit_caps_dir::DirError> {
        Err(gone(p))
    }
    fn available_space(
        &self,
        p: &ambit_caps_dir::RelPath,
    ) -> Result<u64, ambit_caps_dir::DirError> {
        Err(gone(p))
    }
}
impl ambit_caps_dir::DirModify for Unplugged {
    fn replace(
        &self,
        p: &ambit_caps_dir::RelPath,
        _: &[u8],
    ) -> Result<(), ambit_caps_dir::DirError> {
        Err(gone(p))
    }
    fn remove_file(&self, p: &ambit_caps_dir::RelPath) -> Result<(), ambit_caps_dir::DirError> {
        Err(gone(p))
    }
}
fn gone(p: &ambit_caps_dir::RelPath) -> ambit_caps_dir::DirError {
    ambit_caps_dir::DirError::new(
        ambit_caps_dir::DirErrorKind::NotFound,
        format!("{p}: not mounted"),
    )
}

#[test]
fn an_unchanged_reader_book_is_not_read_again() {
    // The reader's listing gives a size and nothing else, so that is the key.
    let mut world = World::new();
    world.places.card = false;
    world.places.wifi = true;
    world.places.kobo = false;
    world
        .reader
        .file("/book.epub", plain("text", CompressionMethod::Stored));
    let first = scan(&world);
    assert_eq!(first.books.len(), 1);
    let downloads = |world: &World| {
        world
            .reader
            .log()
            .iter()
            .filter(|l| l.starts_with("GET /download"))
            .count()
    };
    assert_eq!(downloads(&world), 1);
    assert_eq!(scan(&world).books, first.books);
    assert_eq!(downloads(&world), 1, "the cache answered");
    // A different size is a miss, and a removed book never lingers.
    world.reader.file(
        "/book.epub",
        plain("longer text", CompressionMethod::Stored),
    );
    let rescanned = scan(&world);
    assert_ne!(
        rescanned.books[0].copies[0].sha,
        first.books[0].copies[0].sha
    );
    assert!(world.cache.read(cache::FILE).is_some());
}

#[test]
fn books_are_counted_while_their_location_works() {
    let world = World::new();
    for i in 0..3 {
        world.books.file(
            &format!("book{i}.epub"),
            plain(&format!("text {i}"), CompressionMethod::Stored),
        );
    }
    let library = world.library();
    let index = cache::Index::open(None, library.clock.clone());
    let mut scan = Scan::new();
    let todo = scan.listed(Place::Local, library.list(Place::Local, &index).unwrap());
    assert_eq!(todo.len(), 3);
    let mut seen = vec![];
    for item in todo {
        scan.book(Place::Local, library.read(Place::Local, item, &index));
        let snapshot = scan.snapshot();
        let status = snapshot
            .status
            .iter()
            .find(|(p, _)| *p == Place::Local)
            .unwrap();
        seen.push(status.1.clone());
    }
    assert_eq!(seen[0], "Checking (1 book)");
    assert_eq!(seen[1], "Checking (2 books)");
    scan.finished(Place::Local, Ok(()));
    let snapshot = scan.snapshot();
    assert!(snapshot.ready(Place::Local));
    assert_eq!(snapshot.status[0].1, "Ready (3 books, 0 unreadable)");
}

#[test]
fn a_request_with_a_copy_in_archive_is_no_longer_pending() {
    let world = World::new();
    world.books.file("waiting.acsm", b"<request/>".to_vec());
    world.books.file("spent.acsm", b"<spent/>".to_vec());
    world.books.file("archive/spent.acsm", b"<spent/>".to_vec());
    world.books.file("archive/old.acsm", b"<old/>".to_vec());
    let snapshot = scan(&world);
    assert_eq!(snapshot.acsm, [format!("{BOOKS}/waiting.acsm")]);
}

#[test]
fn local_and_kobo_copies_are_never_removed_by_this_program() {
    let world = World::new();
    world
        .books
        .file("keep.epub", plain("text", CompressionMethod::Stored));
    world
        .books
        .file("duplicate.epub", plain("text", CompressionMethod::Deflated));
    world
        .kobo_dir
        .file("Author/Book.epub", plain("text", CompressionMethod::Stored));
    let snapshot = scan(&world);
    // Repacked bytes are the same book, so the files are copies of one book.
    assert_eq!(snapshot.books.len(), 1);
    let book = &snapshot.books[0];
    assert_eq!(book.copies.len(), 3);
    let library = world.library();
    for copy in book.copies.clone() {
        let refused = books::remove(&library, book, copy.place, &copy.path, &|_| {})
            .unwrap_err()
            .to_string();
        assert!(refused.contains("only adds"), "{refused}");
    }
    assert!(world.books.read("duplicate.epub").is_some());
    assert!(world.kobo_dir.read("Author/Book.epub").is_some());
}

#[test]
fn a_reader_copy_is_removed_from_a_card_but_never_over_wifi() {
    let mut world = World::new();
    world.places.kobo = false;
    world
        .books
        .file("book.epub", plain("text", CompressionMethod::Stored));
    world
        .card
        .file("copy.epub", plain("text", CompressionMethod::Stored));
    let book = scan(&world).books.remove(0);
    assert_eq!(book.copies.len(), 2);
    let on_card = book
        .copies
        .iter()
        .find(|c| c.place == Place::CrossPoint)
        .unwrap()
        .path
        .clone();
    // Over Wi-Fi the protocol cannot delete, and nothing is attempted.
    let mut wifi = world.clone();
    wifi.places.card = false;
    wifi.places.wifi = true;
    let refused = books::remove(&wifi.library(), &book, Place::CrossPoint, &on_card, &|_| {})
        .unwrap_err()
        .to_string();
    assert!(refused.contains("Wi-Fi"), "{refused}");
    assert!(world.card.read("copy.epub").is_some());
    // A copy that changed since discovery is not the copy that was found,
    // even at the same size.
    world
        .card
        .file("copy.epub", plain("texu", CompressionMethod::Stored));
    let refused = books::remove(
        &world.library(),
        &book,
        Place::CrossPoint,
        &on_card,
        &|_| {},
    )
    .unwrap_err()
    .to_string();
    assert!(refused.contains("changed since discovery"), "{refused}");
    world
        .card
        .file("copy.epub", plain("text", CompressionMethod::Stored));
    // A path that is not one of this book's copies is refused outright.
    let unknown = books::remove(
        &world.library(),
        &book,
        Place::CrossPoint,
        "/elsewhere.epub",
        &|_| {},
    )
    .unwrap_err()
    .to_string();
    assert!(unknown.contains("no longer in the library"), "{unknown}");
    // On a mounted card it is an ordinary file, and only that file goes.
    let removed = books::remove(
        &world.library(),
        &book,
        Place::CrossPoint,
        &on_card,
        &|_| {},
    )
    .unwrap();
    assert!(removed.contains("card"), "{removed}");
    assert!(world.card.read("copy.epub").is_none());
    assert!(world.books.read("book.epub").is_some());
    // What remains is the only copy, and the only copy is never removed.
    let only = scan(&world).books.remove(0);
    world
        .card
        .file("x.epub", plain("text", CompressionMethod::Stored));
    let alone = books::Book {
        copies: only.copies.clone(),
        ..only
    };
    let refused = books::remove(
        &world.library(),
        &alone,
        Place::Local,
        &alone.copies[0].path,
        &|_| {},
    )
    .unwrap_err()
    .to_string();
    assert!(refused.contains("only copy"), "{refused}");
}

#[test]
fn pdfs_and_comics_are_carried_whole_and_are_never_device_copies() {
    let world = World::new();
    let paper = pdf("a paper");
    let comic = cbz(&["001.jpg", "002.jpg"]);
    world.books.file("Some Paper.pdf", paper.clone());
    world.books.file("Vol 1.cbz", comic.clone());
    // A PDF on the reader is not listed by it, so Crossload does not list it either.
    world.card.file("Some Paper.pdf", paper.clone());
    // A format Crossload does not carry stays out of the library, and a file
    // that lies about its format is a warning rather than a book.
    world.books.file("Vol 2.cbr", comic.clone());
    world.books.file("Broken.pdf", b"not a PDF at all".to_vec());

    let snapshot = scan(&world);
    let titles: Vec<_> = snapshot.books.iter().map(|b| b.title.as_str()).collect();
    assert!(!titles.contains(&"Vol 2"), "{titles:?}");
    let paper_book = snapshot
        .books
        .iter()
        .find(|b| b.title == "Some Paper")
        .unwrap();
    assert!(paper_book.has(Place::Local) && !paper_book.has(Place::CrossPoint));
    assert_eq!(paper_book.copies.len(), 1);
    assert!(paper_book.copies.iter().all(|c| !c.optimized));
    assert_eq!(paper_book.author, "");
    let comic_book = snapshot.books.iter().find(|b| b.title == "Vol 1").unwrap();
    assert_eq!(comic_book.copies.len(), 1);
    let broken = snapshot
        .books
        .iter()
        .find(|b| b.title == "Broken.pdf")
        .unwrap();
    assert!(
        broken.author.starts_with("Unreadable:"),
        "{}",
        broken.author
    );

    let library = world.library();
    let refused = books::transfer(&library, comic_book, Place::CrossPoint, &|_| {}).unwrap_err();
    assert!(
        format!("{refused:#}").contains("only lists EPUB"),
        "{refused:#}"
    );
    assert!(world.card.read("Unknown author/Vol 1.cbz").is_none());
    books::transfer(&library, comic_book, Place::Kobo, &|_| {}).unwrap();
    assert_eq!(
        world.kobo_dir.read("Unknown author/Vol 1.cbz").unwrap(),
        comic
    );
}

#[test]
fn a_full_card_refuses_a_copy_and_room_is_said_where_it_can_be() {
    let world = World::new();
    let book = plain("text", CompressionMethod::Stored);
    world.books.file("book.epub", book.clone());
    world.card.space(100);
    world.kobo_dir.space(1 << 20);
    let library = world.library();
    assert_eq!(books::room(&library, Place::CrossPoint), Some(100));
    assert_eq!(books::room(&library, Place::Kobo), Some(1 << 20));
    let snapshot = scan(&world);
    let error =
        books::transfer(&library, &snapshot.books[0], Place::CrossPoint, &|_| {}).unwrap_err();
    assert!(
        format!("{error:#}").contains("Not enough free space"),
        "{error:#}"
    );
    assert!(world.card.read("Author/Book.epub").is_none());
    // A reader reached over Wi-Fi cannot say, and says so.
    let mut wifi = world.clone();
    wifi.places.card = false;
    wifi.places.wifi = true;
    assert_eq!(books::room(&wifi.library(), Place::CrossPoint), None);
}

#[test]
fn a_book_reaches_the_kobo_as_a_kepub_when_that_is_asked_for() {
    let mut world = World::new();
    world.settings.kepub = true;
    world.books.file(
        "original.epub",
        plain("a sentence. and another.", CompressionMethod::Stored),
    );
    let snapshot = scan(&world);
    let book = snapshot.books.iter().find(|b| b.has(Place::Local)).unwrap();
    books::transfer(&world.library(), book, Place::Kobo, &|_| {}).unwrap();
    use ambit_caps_dir::Dir;
    let sent = world
        .kobo_dir
        .list(&ambit_caps_dir::RelPath::parse("Author").unwrap())
        .unwrap();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].name.ends_with(".kepub.epub"), "{}", sent[0].name);
    let data = world
        .kobo_dir
        .read(&format!("Author/{}", sent[0].name))
        .unwrap();
    let text = String::from_utf8(spec::books::member(&data, "chapter.xhtml")).unwrap();
    assert!(text.contains("class=\"koboSpan\""), "{text}");
    assert!(text.contains(">a sentence. </span>"), "{text}");
}

#[test]
fn a_correction_is_added_beside_the_book_and_stays_the_same_book() {
    let world = World::new();
    let original = plain("text", CompressionMethod::Stored);
    world.books.file("book.epub", original.clone());
    world.card.file("book.epub", original.clone());
    let snapshot = scan(&world);
    assert_eq!(snapshot.books.len(), 1);
    let local = snapshot.books[0]
        .copies
        .iter()
        .find(|c| c.place == Place::Local)
        .unwrap()
        .path
        .clone();
    assert_eq!(local, format!("{BOOKS}/book.epub"));
    let message = books::correct(
        &world.library(),
        &local,
        &crossload::metadata::Edit {
            author: Some("James S. A. Corey".into()),
            series: Some("The Expanse".into()),
            series_index: Some("2".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        message.contains(&format!("{IMPORT}/book.epub")),
        "{message}"
    );
    // The original is left exactly as it was; the correction is a new file.
    assert_eq!(world.books.read("book.epub").unwrap(), original);
    let corrected = world.import.read("book.epub").unwrap();
    let metadata = crossload::epub::metadata_from(&corrected).unwrap().unwrap();
    assert_eq!(metadata.author, "James S. A. Corey");
    // Still one book: only what it says about itself moved.
    let snapshot = scan(&world);
    assert_eq!(snapshot.books.len(), 1, "{:?}", snapshot.books);
    assert!(snapshot.books[0].has(Place::Local) && snapshot.books[0].has(Place::CrossPoint));
    // Correcting again never overwrites the first correction.
    let again = books::correct(
        &world.library(),
        &format!("{IMPORT}/book.epub"),
        &Default::default(),
    );
    assert!(again.is_err() || world.import.read("book (corrected).epub").is_some());
}

#[test]
fn a_store_book_on_the_kobo_is_found_by_its_file_and_decrypted_with_its_keys() {
    let world = World::new();
    let plain_book = epub("Dune", "Frank Herbert");
    // The encrypted file on the device; the sim's keys give back the plain book.
    let mut encrypted = plain_book.clone();
    encrypted.extend(b"ciphertext");
    world
        .kobo_dir
        .file(".kobo/kepub/volume-1", plain_book.clone());
    world.kobo.book(StoreBook {
        id: "volume-1".into(),
        title: "Dune".into(),
        author: "Frank Herbert".into(),
        encrypted: true,
        preview: false,
    });
    world.kobo.book(StoreBook {
        id: "not-downloaded".into(),
        title: "Elsewhere".into(),
        author: "".into(),
        encrypted: false,
        preview: false,
    });
    world
        .kobo
        .keys("volume-1", &["chapter.xhtml"], "SERIAL1", &plain_book);
    world.kobo_dir.file(
        ".adobe-digital-editions/device.xml",
        b"<device><deviceSerial>SERIAL1</deviceSerial></device>".to_vec(),
    );
    let snapshot = scan(&world);
    assert_eq!(snapshot.books.len(), 1, "{:?}", snapshot.books);
    let book = &snapshot.books[0];
    assert_eq!(book.title, "Dune");
    let copy = &book.copies[0];
    assert_eq!(
        (copy.place, copy.path.as_str(), copy.locked),
        (Place::Kobo, "volume-1", true)
    );
    assert_eq!(world.kobo.decrypted(), ["volume-1"]);
}
