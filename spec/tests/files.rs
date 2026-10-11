//! Copies, device copies and the identity cache: what lands where, and what never does.
use std::io::{Cursor, Write};
use std::sync::Arc;

use ambit_caps_dir::{Dir, RelPath};
use ambit_caps_dir_native::NativeDir;
use ambit_caps_sim::{SimClock, SimDir};
use crossload::cache::{self, Index, Source, Variant};
use crossload::{copy, prepare, profile::X4};
use spec::books::{epub, member};
use std::time::{Duration, UNIX_EPOCH};
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};

fn clock() -> Arc<SimClock> {
    Arc::new(SimClock::at(
        UNIX_EPOCH + Duration::from_secs(1_760_000_000),
    ))
}

fn root() -> RelPath {
    RelPath::root()
}

#[test]
fn copy_preserves_source_and_skips_identical_destination() {
    let card = SimDir::new();
    let data = epub("Copy test", "Writer");
    let result = copy::copy(&data, "Book.epub", &card, &root()).unwrap();
    assert!(!result.already_present);
    assert_eq!(result.bytes, data.len());
    assert_eq!(card.read("Book.epub").unwrap(), data);
    assert!(
        copy::copy(&data, "Book.epub", &card, &root())
            .unwrap()
            .already_present
    );
    assert_eq!(card.list(&root()).unwrap().len(), 1);
}

#[test]
fn a_missing_folder_is_not_created() {
    let card = SimDir::new();
    let data = epub("Copy test", "Writer");
    let missing = RelPath::parse("not-mounted/Books").unwrap();
    assert!(copy::copy(&data, "Book.epub", &card, &missing).is_err());
    assert!(card.list(&root()).unwrap().is_empty());
}

#[test]
fn different_and_case_colliding_files_are_preserved() {
    let card = SimDir::new();
    card.file("book.EPUB", b"existing book".to_vec());
    let error = copy::copy(&epub("Copy test", "Writer"), "Book.epub", &card, &root()).unwrap_err();
    assert!(format!("{error:#}").contains("nothing was overwritten"));
    assert_eq!(card.read("book.EPUB").unwrap(), b"existing book");
    assert_eq!(card.list(&root()).unwrap().len(), 1);
}

#[test]
fn a_full_card_gets_nothing_rather_than_half_a_book() {
    let card = SimDir::new();
    let data = epub("Copy test", "Writer");
    card.space(data.len() as u64 - 1);
    let error = copy::copy(&data, "Book.epub", &card, &root()).unwrap_err();
    assert!(
        format!("{error:#}").contains("Not enough free space"),
        "{error:#}"
    );
    assert!(card.list(&root()).unwrap().is_empty());
    card.space(data.len() as u64);
    copy::copy(&data, "Book.epub", &card, &root()).unwrap();
}

#[test]
fn malformed_book_does_not_reach_card() {
    let card = SimDir::new();
    assert!(copy::copy(b"broken ZIP", "Book.epub", &card, &root()).is_err());
    assert!(copy::copy(&epub("a", "b"), "../Book.epub", &card, &root()).is_err());
    assert!(copy::copy(&epub("a", "b"), "Book.exe", &card, &root()).is_err());
    assert!(card.list(&root()).unwrap().is_empty());
}

#[test]
fn a_symlink_on_the_card_is_neither_followed_nor_replaced() {
    let tmp = tempfile::tempdir().unwrap();
    let card = tmp.path().join("card");
    std::fs::create_dir(&card).unwrap();
    let outside = tmp.path().join("outside");
    std::fs::write(&outside, b"untouched").unwrap();
    std::os::unix::fs::symlink(&outside, card.join("Book.epub")).unwrap();
    let dir = NativeDir::open(&card).unwrap();
    assert!(copy::copy(&epub("a", "b"), "Book.epub", &dir, &root()).is_err());
    assert_eq!(std::fs::read(outside).unwrap(), b"untouched");
    // Nor can an author folder lead off the card.
    std::os::unix::fs::symlink(tmp.path(), card.join("Escape")).unwrap();
    assert!(prepare::author_directory(&dir, &root(), "Escape").is_err());
}

fn with_photo() -> Vec<u8> {
    let mut jpeg = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(1600, 2400)
        .write_to(&mut jpeg, image::ImageFormat::Jpeg)
        .unwrap();
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in [
        ("mimetype", b"application/epub+zip".to_vec()),
        ("META-INF/container.xml", b"<container><rootfiles><rootfile full-path='book.opf'/></rootfiles></container>".to_vec()),
        ("book.opf", b"<package xmlns:dc='http://purl.org/dc/elements/1.1/'><metadata><dc:title>A Book</dc:title><dc:creator>Writer</dc:creator></metadata><manifest><item id='c' href='chapter.xhtml' media-type='application/xhtml+xml'/><item id='i' href='cover.jpg' media-type='image/jpeg'/></manifest><spine><itemref idref='c'/></spine></package>".to_vec()),
        ("chapter.xhtml", b"<html><head/><body><p id='p'>Preserve every word.</p><img src='cover.jpg'/><a href='#p'>Back</a></body></html>".to_vec()),
        ("cover.jpg", jpeg.into_inner()),
    ] {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(&bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[test]
fn optimization_preserves_original_text_links_and_is_deterministic() {
    let original = with_photo();
    let result = prepare::prepare(original.clone(), "input.epub", Some(X4), true).unwrap();
    assert_eq!(result.author, "Writer");
    assert_eq!(result.name, "A Book.epub");
    assert_eq!(result.images, 1);
    assert!(result.bytes < result.original_bytes);
    for name in ["chapter.xhtml", "book.opf", "META-INF/container.xml"] {
        assert_eq!(member(&result.data, name), member(&original, name));
    }
    let image = image::load_from_memory(&member(&result.data, "cover.jpg")).unwrap();
    assert!(image.width() <= 480 && image.height() <= 800);
    let second = prepare::prepare(original.clone(), "input.epub", Some(X4), true).unwrap();
    assert_eq!(second.data, result.data);
    let again = prepare::prepare(result.data.clone(), "A Book.epub", Some(X4), true).unwrap();
    assert_eq!(again.images, 0);
    assert_eq!(again.data, result.data);
    let mut zip = ZipArchive::new(Cursor::new(result.data)).unwrap();
    let first = zip.by_index(0).unwrap();
    assert_eq!(first.name(), "mimetype");
    assert_eq!(first.compression(), zip::CompressionMethod::Stored);
}

#[test]
fn opting_out_preserves_exact_bytes_and_filename() {
    let original = with_photo();
    let result = prepare::prepare(original.clone(), "input.epub", None, false).unwrap();
    assert_eq!(result.name, "input.epub");
    assert_eq!(result.data, original);
}

#[test]
fn directory_names_are_bounded_and_cannot_escape_the_card() {
    assert_eq!(
        prepare::component("../../evil/path", "Unknown"),
        "_.._evil_path"
    );
    assert_eq!(prepare::component("...", "Unknown"), "Unknown");
    assert!(prepare::component(&"é".repeat(200), "Unknown").len() <= 120);
    let card = SimDir::new();
    assert!(
        prepare::author_directory(&card, &RelPath::parse("missing").unwrap(), "Author").is_err()
    );
    let author = prepare::author_directory(&card, &root(), "Author").unwrap();
    assert_eq!(
        prepare::author_directory(&card, &root(), "AUTHOR").unwrap(),
        author
    );
    assert!(prepare::author_directory(&card, &root(), "a/b").is_err());
}

fn source(sha: &str) -> Source {
    Source {
        title: "Book".into(),
        author: "Author".into(),
        format: crossload::format::Format::Epub,
        verdict: None,
        series: None,
        series_index: None,
        size: 42,
        sha: sha.into(),
        resources: None,
        optimized: false,
        locked: false,
        variants: vec![(sha.into(), None)],
    }
}

fn store(dir: &SimDir) -> Option<cache::Store> {
    Some(cache::Store {
        dir: Arc::new(dir.clone()),
        file: RelPath::new([cache::FILE]).unwrap(),
    })
}

#[test]
fn stored_identities_survive_reopening_and_bad_files_never_fail() {
    let dir = SimDir::new();
    let index = Index::open(store(&dir), clock());
    assert!(index.source("missing").is_none());
    index.put_source("key".into(), source("abc"));
    index.put_variant(
        "variant".into(),
        Variant {
            sha: "def".into(),
            resources: Some("res".into()),
        },
    );
    index.save();
    let reopened = Index::open(store(&dir), clock());
    assert_eq!(reopened.source("key").unwrap().sha, "abc");
    assert_eq!(reopened.variant("variant").unwrap().sha, "def");
    // Corrupt content and a version from another release start empty rather
    // than failing, because a scan must never depend on the cache.
    dir.file(cache::FILE, b"not json at all".to_vec());
    assert!(Index::open(store(&dir), clock()).source("key").is_none());
    dir.file(
        cache::FILE,
        br#"{"version":999,"sources":{},"variants":{}}"#.to_vec(),
    );
    assert!(Index::open(store(&dir), clock()).source("key").is_none());
    // A folder that cannot be written to is silent, not an error.
    let full = SimDir::new();
    full.space(0);
    let index = Index::open(store(&full), clock());
    index.put_source("key".into(), source("abc"));
    index.save();
    assert!(full.read(cache::FILE).is_none());
    // No folder at all keeps nothing between scans.
    let index = Index::open(None, clock());
    index.put_source("key".into(), source("abc"));
    index.save();
}

#[test]
fn a_copy_made_for_one_screen_is_never_served_as_one_made_for_another() {
    use crossload::profile::Profile;
    const OTHER: Profile = Profile {
        device: "Other",
        width: 960,
        height: 540,
        marker: "other-images-v1:960x540:gray:jpeg85",
    };
    let sha = "abc123";
    let x4 = cache::variant_key(sha, Some(X4), true);
    assert_ne!(cache::variant_key(sha, Some(OTHER), true), x4);
    assert_ne!(cache::variant_key(sha, None, true), x4);
    assert_eq!(cache::variant_key(sha, Some(X4), true), x4);
    let source = cache::source_key("CrossPoint", "/b.epub", "1.0.0", Some(X4), true);
    assert_ne!(
        cache::source_key("CrossPoint", "/b.epub", "1.0.0", Some(OTHER), true),
        source
    );
}

#[test]
fn keys_separate_scopes_sources_fingerprints_and_options() {
    let key = cache::source_key("Local", "/book.epub", "5.1.0", Some(X4), true);
    assert_eq!(
        cache::source_key("Local", "/book.epub", "5.1.0", Some(X4), true),
        key
    );
    assert_ne!(
        cache::source_key("Kobo", "/book.epub", "5.1.0", Some(X4), true),
        key
    );
    assert_ne!(
        cache::source_key("Local", "/other.epub", "5.1.0", Some(X4), true),
        key
    );
    assert_ne!(
        cache::source_key("Local", "/book.epub", "6.1.0", Some(X4), true),
        key
    );
    assert_ne!(
        cache::source_key("Local", "/book.epub", "5.1.0", None, true),
        key
    );
    assert_ne!(
        cache::source_key("Local", "/book.epub", "5.1.0", Some(X4), false),
        key
    );
}

/// A local file's identity is reused while its size and modification time stay
/// the same. SimDir has no modification times, so this one is on a real folder.
#[test]
fn stored_identities_are_reused_until_a_file_changes_and_never_outlive_it() {
    let tmp = tempfile::tempdir().unwrap();
    let books = tmp.path().join("books");
    std::fs::create_dir(&books).unwrap();
    let path = books.join("book.epub");
    std::fs::write(
        &path,
        spec::books::plain("text", zip::CompressionMethod::Stored),
    )
    .unwrap();
    let world = spec::world::World::new();
    let mut library = world.library();
    library.books.dir = Arc::new(NativeDir::open(&books).unwrap());
    library.kobo = None;
    library.reader = None;
    let first = library.scan();
    assert_eq!(first.books.len(), 1);
    assert_eq!(library.scan().books, first.books);
    // Same size and a restored timestamp: deliberately invisible, which is
    // what makes a refresh cheap.
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    std::fs::write(
        &path,
        spec::books::plain("txet", zip::CompressionMethod::Stored),
    )
    .unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert_eq!(library.scan().books, first.books);
    // A newer timestamp is a miss, and the new contents take over.
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified + Duration::from_secs(2))
        .unwrap();
    let rescanned = library.scan();
    assert_ne!(
        rescanned.books[0].copies[0].sha,
        first.books[0].copies[0].sha
    );
    // A cached book whose file is gone must not survive as an offline history.
    std::fs::remove_file(&path).unwrap();
    assert!(library.scan().books.is_empty());
    // The fingerprint itself: size and modification time.
    let dir = NativeDir::open(tmp.path()).unwrap();
    std::fs::write(tmp.path().join("f"), b"bytes").unwrap();
    let f = RelPath::parse("f").unwrap();
    let one = cache::file_fingerprint(&dir.metadata(&f).unwrap()).unwrap();
    std::fs::write(tmp.path().join("f"), b"longer bytes").unwrap();
    assert_ne!(
        cache::file_fingerprint(&dir.metadata(&f).unwrap()).unwrap(),
        one
    );
    assert!(cache::file_fingerprint(
        &SimDir::new()
            .file("g", b"x".to_vec())
            .metadata(&RelPath::parse("g").unwrap())
            .unwrap()
    )
    .is_none());
}
