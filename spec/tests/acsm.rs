//! ACSM requests: fulfilled through the `Adept` capability, imported without replacing anything,
//! and archived beside themselves in a folder that only gains files.
use ambit_caps_dir::{Dir, RelPath};
use ambit_caps_sim::SimDir;
use crossload::acsm;
use spec::books::epub;
use spec::world::{World, BOOKS, IMPORT};

const ACSM: &[u8] = b"<fulfillmentToken xmlns='http://ns.adobe.com/adept'/>";

#[test]
fn a_spent_request_is_copied_into_archive_and_never_overwrites_one_there() {
    let requests = SimDir::new();
    requests.file("book.acsm", ACSM.to_vec());
    let first = acsm::archive(&requests, &RelPath::parse("book.acsm").unwrap(), ACSM).unwrap();
    assert_eq!(first.to_string(), "archive/book.acsm");
    // The folder only gains files: the request stays where it was.
    assert_eq!(requests.read("book.acsm").unwrap(), ACSM);
    assert_eq!(requests.read("archive/book.acsm").unwrap(), ACSM);
    // Archiving the same request again is the same archive.
    let again = acsm::archive(&requests, &RelPath::parse("book.acsm").unwrap(), ACSM).unwrap();
    assert_eq!(again, first);
    // A different request of the same name is kept as well, never replacing it.
    let second = acsm::archive(
        &requests,
        &RelPath::parse("book.acsm").unwrap(),
        b"a different request",
    )
    .unwrap();
    assert_eq!(second.to_string(), "archive/book-1.acsm");
    assert_eq!(requests.read("archive/book.acsm").unwrap(), ACSM);
    assert_eq!(
        requests
            .list(&RelPath::parse("archive").unwrap())
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn archiving_reports_failure_without_touching_the_request() {
    let requests = SimDir::new();
    requests.file("book.acsm", ACSM.to_vec());
    // A file where the archive folder would go.
    requests.file("archive", b"not a directory".to_vec());
    assert!(acsm::archive(&requests, &RelPath::parse("book.acsm").unwrap(), ACSM).is_err());
    assert_eq!(requests.read("book.acsm").unwrap(), ACSM);
    assert_eq!(requests.read("archive").unwrap(), b"not a directory");
}

#[test]
fn a_request_is_fulfilled_once_into_the_import_folder() {
    let world = World::new();
    world.books.file("loans/book.acsm", ACSM.to_vec());
    world
        .adept
        .activated()
        .book(ACSM, &epub("Synthetic / book", "Writer"));
    let library = world.library();
    let message = acsm::fulfil(&library, &format!("{BOOKS}/loans/book.acsm"), &|_| {}).unwrap();
    assert!(
        message.contains(&format!("{IMPORT}/Synthetic _ book [")),
        "{message}"
    );
    assert!(message.contains("loans/archive/book.acsm"), "{message}");
    let imported = world.import.list(&RelPath::root()).unwrap();
    assert_eq!(imported.len(), 1);
    // A second import of the same request never replaces the book.
    let error = acsm::import(&world.adept, &library.import, ACSM).unwrap_err();
    assert!(
        format!("{error:#}").contains("never overwritten"),
        "{error:#}"
    );
    assert_eq!(world.import.list(&RelPath::root()).unwrap().len(), 1);
}

#[test]
fn a_fulfilment_that_is_not_a_book_publishes_nothing() {
    let world = World::new();
    world.books.file("book.acsm", ACSM.to_vec());
    world
        .adept
        .activated()
        .book(ACSM, b"<html>not a book</html>");
    let library = world.library();
    assert!(acsm::fulfil(&library, &format!("{BOOKS}/book.acsm"), &|_| {}).is_err());
    assert!(world.import.list(&RelPath::root()).unwrap().is_empty());
    assert!(
        world.books.read("archive/book.acsm").is_none(),
        "an uncertain outcome stays pending"
    );
    // Without an activation, nothing is asked of anyone.
    let world = World::new();
    world.books.file("book.acsm", ACSM.to_vec());
    assert!(acsm::fulfil(&world.library(), &format!("{BOOKS}/book.acsm"), &|_| {}).is_err());
    assert!(world.adept.fulfilled().is_empty());
}
