//! The terminal interface end to end, on the sim terminal with sim capabilities: what the user
//! sees, and what lands where. Tasks run inline, in the order asked.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ambit_tui::{Key, KeyCode};
use crossload_kobo::StoreBook;
use spec::books::{epub, plain};
use spec::world::{World, BOOKS};
use zip::CompressionMethod;

fn status(screen: &str) -> String {
    screen
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .to_string()
}

#[test]
fn the_library_is_scanned_at_start_and_shows_every_place() {
    let world = World::new();
    world.books.file("dune.epub", epub("Dune", "Frank Herbert"));
    world
        .card
        .file("Frank Herbert/Dune.epub", epub("Dune", "Frank Herbert"));
    world
        .kobo_dir
        .file("Piranesi.epub", epub("Piranesi", "Susanna Clarke"));
    let sim = world.start();
    let screen = sim.screen();
    assert!(screen.contains("Dune"), "{screen}");
    assert!(screen.contains("Piranesi"), "{screen}");
    assert!(screen.contains("Library ready"), "{screen}");
    // Scanning only reads; it only ever writes the identity cache.
    assert!(world.import.read("Dune.epub").is_none());
    assert!(world.cache.read(crossload::cache::FILE).is_some());
}

#[test]
fn a_book_is_copied_to_the_reader_over_wifi_and_verified() {
    let mut world = World::new();
    world.places.card = false;
    world.places.wifi = true;
    world.places.kobo = false;
    world.books.file("dune.epub", epub("Dune", "Frank Herbert"));
    let mut sim = world.start();
    assert!(sim.screen().contains("Dune"));
    sim.key(KeyCode::Enter).key('3');
    let screen = sim.screen();
    assert!(
        screen.contains("Copied to /Frank Herbert/Dune.epub (verified)"),
        "{screen}"
    );
    let sent = world.reader.get("/Frank Herbert/Dune.epub").unwrap();
    assert_eq!(
        crossload::epub::metadata_from(&sent)
            .unwrap()
            .unwrap()
            .title
            .as_deref(),
        Some("Dune")
    );
    // The copy is remembered.
    let history = String::from_utf8(world.state.read(crossload::history::FILE).unwrap()).unwrap();
    assert!(history.contains("\"title\":\"Dune\""), "{history}");
    assert!(history.contains("\"ok\":true"), "{history}");
}

#[test]
fn several_books_go_to_the_card_one_after_the_other() {
    let world = World::new();
    for (title, author) in [("A", "One"), ("B", "Two"), ("C", "Three")] {
        world
            .books
            .file(&format!("{title}.epub"), epub(title, author));
    }
    let mut sim = world.start();
    sim.key('a').key(KeyCode::Enter).key('3');
    let screen = sim.screen();
    assert!(
        screen.contains("Copied 3 of 3 books to CrossPoint"),
        "{screen}"
    );
    for (title, author) in [("A", "One"), ("B", "Two"), ("C", "Three")] {
        assert!(world.card.read(&format!("{author}/{title}.epub")).is_some());
    }
    let history = String::from_utf8(world.state.read(crossload::history::FILE).unwrap()).unwrap();
    let order: Vec<_> = history
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()["title"].clone())
        .collect();
    assert_eq!(order, ["A", "B", "C"]);
}

#[test]
fn a_full_card_gets_nothing_and_says_so() {
    let world = World::new();
    world.books.file("dune.epub", epub("Dune", "Frank Herbert"));
    world.card.space(10);
    let mut sim = world.start();
    sim.key(KeyCode::Enter);
    let screen = sim.screen();
    assert!(screen.contains("10 B free"), "{screen}");
    sim.key('3');
    let screen = sim.screen();
    assert!(screen.contains("Not enough free space"), "{screen}");
    assert!(world.card.read("Frank Herbert/Dune.epub").is_none());
}

#[test]
fn only_the_card_copy_can_be_deleted_and_only_after_a_yes() {
    let world = World::new();
    world
        .books
        .file("dune.epub", plain("text", CompressionMethod::Stored));
    world
        .card
        .file("Author/Book.epub", plain("text", CompressionMethod::Stored));
    let mut sim = world.start();
    sim.key('d');
    // The dialog says which copies may go before anything is chosen.
    assert!(sim.screen().contains("add only"), "{}", sim.screen());
    // The local copy is refused: this program only adds to local folders.
    sim.key('2');
    assert!(
        status(&sim.screen()).contains("only adds books to Local"),
        "{}",
        sim.screen()
    );
    sim.key('1');
    assert!(
        world.card.read("Author/Book.epub").is_some(),
        "nothing before the yes"
    );
    sim.key('y');
    let screen = sim.screen();
    assert!(
        screen.contains("Deleted /Author/Book.epub from the reader's card"),
        "{screen}"
    );
    assert!(world.card.read("Author/Book.epub").is_none());
    assert!(world.books.read("dune.epub").is_some());
}

#[test]
fn an_acsm_is_fulfilled_into_the_import_folder_and_archived_beside_itself() {
    let world = World::new();
    let acsm = b"<fulfillmentToken xmlns='http://ns.adobe.com/adept'/>".to_vec();
    world.books.file("loan.acsm", acsm.clone());
    world
        .adept
        .activated()
        .book(&acsm, &epub("Borrowed", "Lender"));
    let mut sim = world.start();
    assert!(sim.screen().contains("loan.acsm"));
    sim.key(KeyCode::End).key(KeyCode::Enter).key('1');
    let screen = sim.screen();
    assert!(screen.contains("Imported"), "{screen}");
    assert_eq!(world.adept.fulfilled(), std::slice::from_ref(&acsm));
    use ambit_caps_dir::Dir;
    let imported = world.import.list(&ambit_caps_dir::RelPath::root()).unwrap();
    assert_eq!(imported.len(), 1);
    assert!(
        imported[0].name.starts_with("Borrowed ["),
        "{}",
        imported[0].name
    );
    // The request stays, and a copy in archive/ marks it spent.
    assert_eq!(world.books.read("loan.acsm").unwrap(), acsm);
    assert_eq!(world.books.read("archive/loan.acsm").unwrap(), acsm);
    sim.key('r');
    assert!(!sim.screen().contains("loan.acsm"), "{}", sim.screen());
}

#[test]
fn saving_settings_writes_them_and_starts_again_with_them() {
    let world = World::new();
    let relaunch = Arc::new(AtomicBool::new(false));
    let mut sim = world.start_with(world.options(), relaunch.clone());
    sim.key(',');
    assert!(sim.screen().contains("Books folder"), "{}", sim.screen());
    sim.key(Key::ctrl('s'));
    assert!(sim.quit(), "{}", sim.screen());
    assert!(relaunch.load(Ordering::Relaxed));
    let saved: serde_json::Value =
        serde_json::from_slice(&world.settings_dir.read("config.json").unwrap()).unwrap();
    assert_eq!(saved["browse"], BOOKS);
}

#[test]
fn quitting_waits_for_a_running_copy() {
    let world = World::new();
    world.books.file("dune.epub", epub("Dune", "Frank Herbert"));
    let mut sim = world.start();
    sim.key('q');
    assert!(sim.quit());
    let world = World::new();
    world.kobo.book(StoreBook {
        id: "x".into(),
        title: "X".into(),
        author: "Y".into(),
        encrypted: false,
        preview: true,
    });
    let mut sim = world.start();
    sim.key(Key::ctrl('c'));
    assert!(sim.quit());
}

#[test]
fn nothing_is_sent_to_a_reader_that_is_not_configured() {
    let world = World::new();
    world.books.file("dune.epub", epub("Dune", "Frank Herbert"));
    let mut sim = world.start();
    sim.key('r');
    assert!(world.http.sent().is_empty());
    assert!(world.upload.sent().is_empty());
}

#[test]
fn the_reader_connection_test_asks_for_its_status_and_never_uploads() {
    let mut world = World::new();
    world.places.card = false;
    world.places.wifi = true;
    let mut sim = world.start();
    sim.key(',');
    // Row 4: the reader's address, typed in.
    for _ in 0..3 {
        sim.key(KeyCode::Down);
    }
    sim.key('e')
        .key(Key::ctrl('u'))
        .keys(spec::reader::IP)
        .key(KeyCode::Enter);
    // Row 7: test the connection.
    for _ in 0..2 {
        sim.key(KeyCode::Down);
    }
    sim.key(KeyCode::Enter);
    let screen = sim.screen();
    assert!(
        screen.contains("Connected to X4 at 192.168.1.50"),
        "{screen}"
    );
    assert!(world.reader.log().iter().any(|l| l == "GET /api/status"));
    assert!(world.upload.sent().is_empty());
    // A reader that is not CrossPoint is not taken for one.
    world
        .reader
        .status_body(r#"{"version":"1.6.0","device":"unknown"}"#);
    sim.key(KeyCode::Enter);
    assert!(
        sim.screen()
            .contains("Unrecognized CrossPoint reader status"),
        "{}",
        sim.screen()
    );
}
