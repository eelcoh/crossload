//! Kobo sync onto a reader's card: a dry run writes nothing, and applying it twice changes nothing
//! the second time.
use crossload::books::Folder;
use crossload::inventory::{self, Destination};
use crossload::sync::{self, Options, Status};
use crossload_kobo::StoreBook;
use spec::books::plain as epub;
use spec::world::{World, IMPORT};
use std::sync::Arc;
use zip::CompressionMethod;

fn add(world: &World, id: &str, bytes: &[u8], preview: bool) {
    world.kobo.book(StoreBook {
        id: id.into(),
        title: "Book".into(),
        author: "Author".into(),
        encrypted: false,
        preview,
    });
    world
        .kobo_dir
        .file(&format!(".kobo/kepub/{id}"), bytes.to_vec());
}

fn options(world: &World, apply: bool) -> Options {
    Options {
        output: Folder {
            dir: Arc::new(world.import.clone()),
            display: IMPORT.into(),
        },
        serial: None,
        apply,
        optimize: Some(crossload::profile::X4),
        organized: true,
        repair: false,
        exclude: vec![],
    }
}

fn run(world: &World, apply: bool) -> sync::Report {
    sync::run(
        &world.device(),
        &world.card_destination(),
        &options(world, apply),
        |_| {},
    )
    .unwrap()
}

fn card_files(world: &World) -> usize {
    use ambit_caps_dir::Dir;
    world
        .card
        .list(&ambit_caps_dir::RelPath::root())
        .unwrap()
        .len()
}

#[test]
fn dry_run_has_no_published_writes_and_apply_is_repeatable() {
    let world = World::new();
    let bytes = epub("text", CompressionMethod::Stored);
    add(&world, "one", &bytes, false);
    add(&world, "duplicate", &bytes, false);
    add(&world, "preview", &bytes, true);
    let report = run(&world, false);
    assert_eq!(report.previews_skipped, 1);
    assert_eq!(report.books[0].status, Status::Transfer);
    assert_eq!(report.books[1].status, Status::Duplicate);
    assert!(world.import.read("Book [").is_none());
    assert_eq!(card_files(&world), 0);
    let first = run(&world, true);
    assert_eq!(first.failures(), 0);
    assert!(first.books.iter().any(|b| b.status == Status::Sent));
    assert_eq!(world.card.read("Author/Book.epub").unwrap(), bytes);
    let second = run(&world, true);
    assert!(second.books.iter().all(|b| b.status == Status::Present));
    use ambit_caps_dir::Dir;
    assert_eq!(
        world
            .import
            .list(&ambit_caps_dir::RelPath::root())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn renamed_repacked_book_is_recognized_but_same_title_different_text_is_not() {
    let world = World::new();
    let original = epub("original", CompressionMethod::Stored);
    add(&world, "one", &original, false);
    let repacked = epub("original", CompressionMethod::Deflated);
    assert_ne!(original, repacked);
    assert!(inventory::identity(&original).matches(&inventory::identity(&repacked)));
    world.card.file("old-name.epub", repacked);
    assert_eq!(run(&world, false).books[0].status, Status::Present);
    world.card.file(
        "old-name.epub",
        epub("different edition", CompressionMethod::Stored),
    );
    assert_eq!(run(&world, false).books[0].status, Status::Transfer);
}

#[test]
fn collision_preserves_both_books_and_local_recovery_checks_bytes() {
    let world = World::new();
    let bytes = epub("one", CompressionMethod::Stored);
    add(&world, "one", &bytes, false);
    world.card.file("Author/Book.epub", b"unrelated".to_vec());
    assert_eq!(run(&world, true).failures(), 1);
    assert_eq!(world.card.read("Author/Book.epub").unwrap(), b"unrelated");
    let device = world.device();
    let (name, data) = device.import("one", None).unwrap();
    let imported = crossload::kobo::publish(&world.import, &name, &data, true).unwrap();
    assert_eq!(
        crossload::kobo::publish(&world.import, &name, &data, true).unwrap(),
        imported
    );
    world
        .import
        .file(&imported.to_string(), b"other local file".to_vec());
    assert!(crossload::kobo::publish(&world.import, &name, &data, true).is_err());
    assert_eq!(
        world.import.read(&imported.to_string()).unwrap(),
        b"other local file"
    );
}

#[test]
fn distinct_books_with_same_destination_are_reported_without_overwrite() {
    let world = World::new();
    add(
        &world,
        "one",
        &epub("one", CompressionMethod::Stored),
        false,
    );
    add(
        &world,
        "two",
        &epub("two", CompressionMethod::Stored),
        false,
    );
    let plan = run(&world, false);
    assert_eq!(plan.books[0].status, Status::Transfer);
    assert_eq!(plan.books[1].status, Status::Failed);
    let applied = run(&world, true);
    assert_eq!(applied.books[0].status, Status::Sent);
    assert_eq!(applied.books[1].status, Status::Failed);
}

#[test]
fn an_unreachable_reader_never_means_an_empty_one() {
    let world = World::new();
    add(
        &world,
        "one",
        &epub("one", CompressionMethod::Stored),
        false,
    );
    // Nothing answers at this address: an error, not a reader without books.
    let unavailable = Destination::Reader(crossload::crosspoint::Reader::new(
        crossload::crosspoint::Address::parse("192.168.1.99", "/").unwrap(),
        world.link(),
    ));
    assert!(sync::run(
        &world.device(),
        &unavailable,
        &options(&world, true),
        |_| {}
    )
    .is_err());
    assert_eq!(card_files(&world), 0);
}

#[test]
fn sync_over_wifi_uploads_through_the_reader_protocol() {
    let world = World::new();
    let bytes = epub("one", CompressionMethod::Stored);
    add(&world, "one", &bytes, false);
    let reader = Destination::Reader(world.wifi("/"));
    let report = sync::run(&world.device(), &reader, &options(&world, true), |_| {}).unwrap();
    assert_eq!(report.failures(), 0, "{:?}", report.books);
    assert_eq!(report.books[0].status, Status::Sent);
    assert!(world.reader.get("/Author/Book.epub").is_some());
}

#[test]
fn explicit_exclusion_keeps_existing_variant_and_syncs_remaining_books() {
    let world = World::new();
    add(
        &world,
        "store",
        &epub("store variant", CompressionMethod::Stored),
        false,
    );
    let existing = epub("calibre variant", CompressionMethod::Stored);
    add(&world, "sideload", &existing, false);
    world.card.file("Author/Book.epub", existing.clone());
    let report = run(&world, false);
    let failure = report
        .books
        .iter()
        .find(|b| b.status == Status::Failed)
        .unwrap();
    assert!(
        failure.detail.contains("--exclude store"),
        "{}",
        failure.detail
    );
    assert!(!failure.detail.contains("--repair"));
    let mut opts = options(&world, true);
    opts.exclude = vec!["store".into()];
    let report = sync::run(&world.device(), &world.card_destination(), &opts, |_| {}).unwrap();
    assert_eq!(report.excluded, 1);
    assert_eq!(report.failures(), 0);
    assert_eq!(report.books.len(), 1);
    assert_eq!(report.books[0].status, Status::Present);
    assert_eq!(world.card.read("Author/Book.epub").unwrap(), existing);
    opts.exclude = vec!["typo".into()];
    assert!(sync::run(&world.device(), &world.card_destination(), &opts, |_| {}).is_err());
}
