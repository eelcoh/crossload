//! The CrossPoint transfer protocol against a simulated reader: uploads are staged, verified by
//! reading them back, and only then named; nothing on the reader is overwritten.
use ambit_caps_net::{HttpErrorKind, Origin, Scheme, Scope, WebSocket};
use ambit_caps_sim::{SimHttp, SimWebSocket};
use crossload::crosspoint::{Address, Reader};
use crossload::inventory::{self, Destination, Inventory};
use spec::books;
use spec::reader::IP;
use spec::world::World;
use std::io::{Cursor, Write};

const NAME: &str = "Één boek [test].epub";

fn book() -> Vec<u8> {
    books::epub("Wi-Fi test", "Author")
}

#[test]
fn websocket_upload_verifies_staging_before_publication() {
    let world = World::new();
    let bytes = book();
    let sent = world.wifi("/").send(NAME, &bytes).unwrap();
    assert!(!sent.already_present);
    assert_eq!(sent.bytes, bytes.len());
    assert_eq!(sent.path, format!("/{NAME}"));
    // Only the final name is left, with exactly these bytes.
    assert_eq!(world.reader.files().len(), 1);
    assert_eq!(world.reader.get(&format!("/{NAME}")).unwrap(), bytes);
    // The staging copy was read back before it was renamed into place.
    let log = world.reader.log();
    let download = log
        .iter()
        .position(|l| l.starts_with("GET /download?path=%2Fxteink-"))
        .expect("read back");
    let rename = log
        .iter()
        .position(|l| l == "POST /rename")
        .expect("renamed");
    assert!(download < rename);
    // 4 KiB frames, as CrossPoint's own page sends.
    assert!(world.reader.largest_frame() <= 4096);
}

#[test]
fn identical_existing_file_is_verified_without_upload() {
    let world = World::new();
    let bytes = book();
    world.reader.file(&format!("/{NAME}"), bytes.clone());
    assert!(world.wifi("/").send(NAME, &bytes).unwrap().already_present);
    assert!(world.upload.sent().is_empty());
}

#[test]
fn collision_with_different_bytes_is_not_overwritten() {
    let world = World::new();
    let bytes = book();
    let mut wrong = bytes.clone();
    wrong[0] ^= 1;
    world.reader.file(&format!("/{NAME}"), wrong.clone());
    let error = world.wifi("/").send(NAME, &bytes).unwrap_err();
    assert!(format!("{error:#}")
        .to_lowercase()
        .contains("nothing was overwritten"));
    assert_eq!(world.reader.get(&format!("/{NAME}")).unwrap(), wrong);
    assert!(world.upload.sent().is_empty());
}

#[test]
fn corrupt_readback_reports_failure_and_publishes_nothing() {
    let world = World::new();
    world.reader.corrupt_readback();
    let error = world.wifi("/").send(NAME, &book()).unwrap_err();
    assert!(
        error.to_string().contains("verification failed"),
        "{error:#}"
    );
    assert!(world.reader.get(&format!("/{NAME}")).is_none());
}

#[test]
fn firmware_upload_failure_is_not_retried() {
    let world = World::new();
    world.reader.refuse_uploads("Write failed - disk full?");
    let error = world.wifi("/").send(NAME, &book()).unwrap_err();
    assert!(format!("{error:#}").contains("Write failed"), "{error:#}");
    let starts = world
        .upload
        .sent()
        .iter()
        .filter(|(_, m)| matches!(m, ambit_caps_net::WsMessage::Text(t) if t.starts_with("START:")))
        .count();
    assert_eq!(starts, 1);
    assert!(world.reader.files().is_empty());
}

#[test]
fn missing_folder_rejected_before_upload() {
    let world = World::new();
    let error = world.wifi("/Books").send(NAME, &book()).unwrap_err();
    assert!(error.to_string().contains("does not exist"));
    assert!(world.upload.sent().is_empty());
}

#[test]
fn nested_folder_queries_are_encoded() {
    let world = World::new();
    world.reader.file(&format!("/My Books/{NAME}"), vec![]);
    let error = world.wifi("/My Books/").send(NAME, &book()).unwrap_err();
    assert!(format!("{error:#}")
        .to_lowercase()
        .contains("nothing was overwritten"));
    assert!(world
        .reader
        .log()
        .contains(&"GET /api/files?path=%2FMy+Books".to_string()));
}

#[test]
fn invalid_input_and_unsafe_destinations_fail_before_network() {
    for address in [
        "",
        "file:///tmp",
        "https://reader",
        "http://user:secret@reader",
        "http://reader/upload",
        "http://reader?x=1",
        "http://reader/#x",
    ] {
        assert!(Address::parse(address, "/").is_err(), "{address}");
    }
    for folder in [
        "Books",
        "/..",
        "/.crosspoint",
        "/Books/../Other",
        "/Books//Other",
        "/fonts\\bad",
        "/XTCache",
    ] {
        assert!(Address::parse(IP, folder).is_err(), "{folder}");
    }
    let world = World::new();
    assert!(world
        .wifi("/")
        .send("bad.epub", b"not an EPUB")
        .unwrap_err()
        .to_string()
        .contains("validation"));
    assert!(world.wifi("/").send("../bad\n.epub", &book()).is_err());
    assert!(world.http.sent().is_empty());
}

#[test]
fn wrong_server_response_is_rejected() {
    let world = World::new();
    world.reader.status_body("<html>login</html>");
    assert!(world.wifi("/").send(NAME, &book()).is_err());
    assert!(world.upload.sent().is_empty());
}

#[test]
fn a_reader_outside_the_scope_is_refused_and_nothing_is_sent() {
    // Granted: the local network on port 80, and a name only when given exactly.
    let world = World::new();
    for address in [
        "8.8.8.8",
        "127.0.0.1",
        "reader.example.com",
        "192.168.1.50:8080",
    ] {
        let reader = Reader::new(Address::parse(address, "/").unwrap(), world.link());
        let error = reader.status().unwrap_err();
        assert!(
            format!("{error:#}").contains("outside what this session may reach"),
            "{address}: {error:#}"
        );
    }
    assert!(world.http.sent().is_empty());
    // A reader given by name gets exactly that name, and nothing else on the network.
    let named = SimHttp::new(Scope::parse(["http://crosspoint.local:80"]).unwrap());
    let ws = SimWebSocket::new(Scope::parse(["ws://crosspoint.local:81"]).unwrap());
    assert_eq!(
        ws.connect("ws://192.168.1.50:81/").err().unwrap().kind,
        HttpErrorKind::Denied
    );
    let mut link = world.link();
    link.control = std::sync::Arc::new(named.clone());
    let reader = Reader::new(Address::parse("crosspoint.local", "/").unwrap(), link);
    // Nothing answers in the sim, but the request was allowed out.
    assert!(reader.status().is_err());
    assert_eq!(named.sent().len(), 1);
    // A local-network grant never covers a name.
    let lan = Scope::new([Origin::lan(Scheme::Http, 80)]);
    assert!(lan
        .check(
            &ambit_caps_net::http::Request::get("http://crosspoint.local/")
                .body(vec![])
                .unwrap()
        )
        .is_err());
}

#[test]
fn repair_preserves_verified_prefix_then_sends_fresh_copy() {
    let world = World::new();
    let bytes = book();
    world
        .reader
        .file(&format!("/{NAME}"), bytes[..100].to_vec());
    let sent = world
        .wifi("/")
        .repair_incomplete(true)
        .send(NAME, &bytes)
        .unwrap();
    assert!(!sent.already_present);
    let backup = sent.backup.unwrap();
    assert!(backup.contains(".incomplete-100"));
    assert_eq!(world.reader.get(&backup).unwrap(), bytes[..100].to_vec());
    assert_eq!(world.reader.get(&format!("/{NAME}")).unwrap(), bytes);
}

#[test]
fn repair_refuses_unrelated_shorter_file() {
    let world = World::new();
    world.reader.file(&format!("/{NAME}"), b"nope".to_vec());
    let error = world
        .wifi("/")
        .repair_incomplete(true)
        .send(NAME, &book())
        .unwrap_err();
    assert!(error.to_string().contains("repair refused"));
    assert_eq!(world.reader.get(&format!("/{NAME}")).unwrap(), b"nope");
}

#[test]
fn websocket_handles_multiple_acknowledgement_windows() {
    let world = World::new();
    let mut zip = zip::ZipWriter::new_append(Cursor::new(book())).unwrap();
    zip.start_file(
        "large.bin",
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )
    .unwrap();
    zip.write_all(&vec![0x42; 150000]).unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    let sent = world.wifi("/").send(NAME, &bytes).unwrap();
    assert_eq!(sent.bytes, bytes.len());
    assert_eq!(world.reader.get(&format!("/{NAME}")).unwrap(), bytes);
}

#[test]
fn author_folder_is_created_and_existing_case_is_reused() {
    let world = World::new();
    let reader = world.wifi("/").for_author("Mick Herron").unwrap();
    assert_eq!(reader.folder(), "/Mick Herron");
    assert!(world.reader.log().contains(&"POST /mkdir".to_string()));

    let world = World::new();
    world.reader.dir("/MICK HERRON");
    let reader = world.wifi("/").for_author("Mick Herron").unwrap();
    assert_eq!(reader.folder(), "/MICK HERRON");
    assert!(!world.reader.log().contains(&"POST /mkdir".to_string()));
}

#[test]
fn author_folder_collision_is_rejected() {
    let world = World::new();
    world.reader.file("/Mick Herron", b"abc".to_vec());
    assert!(world.wifi("/").for_author("Mick Herron").is_err());
}

#[test]
fn author_folder_transfer_stages_verifies_and_publishes_inside_folder() {
    let world = World::new();
    let bytes = book();
    let sent = world
        .wifi("/")
        .for_author("Mick Herron")
        .unwrap()
        .send(NAME, &bytes)
        .unwrap();
    assert_eq!(sent.path, format!("/Mick Herron/{NAME}"));
    assert_eq!(
        world.reader.files().keys().collect::<Vec<_>>(),
        [&sent.path]
    );
}

#[test]
fn a_book_prepared_for_another_screen_says_so() {
    let world = World::new();
    world.reader.device("X3");
    let sent = world
        .wifi("/")
        .prepared_for(Some(crossload::profile::X4))
        .send(NAME, &book())
        .unwrap();
    let note = sent.note.unwrap();
    assert!(note.contains("X4 screen"), "{note}");
    assert!(note.contains("Set profile to X3"), "{note}");
}

#[test]
fn a_reader_with_the_same_screen_takes_the_copy_without_a_word() {
    let world = World::new();
    world.reader.status_body(
        r#"{"version":"2.0.0-rc+cb2a4c7","firmwareFamily":"crosspoint","device":"xteink_x4_pro"}"#,
    );
    let sent = world
        .wifi("/")
        .prepared_for(Some(crossload::profile::X4))
        .send(NAME, &book())
        .unwrap();
    assert_eq!(sent.note, None);
}

#[test]
fn crosspoint_2_is_recognized_by_its_firmware_family() {
    let world = World::new();
    // As an X4 Pro on 2.0.0-rc answers, trimmed.
    world.reader.status_body(
        r#"{"version":"2.0.0-rc+cb2a4c7","firmwareFamily":"crosspoint","device":"xteink_x4_pro"}"#,
    );
    let status = world.wifi("/").status().unwrap();
    assert_eq!(status.device, "xteink_x4_pro");
    world
        .reader
        .status_body(r#"{"version":"2.0.0","firmwareFamily":"other","device":"xteink_x4_pro"}"#);
    assert!(world.wifi("/").status().is_err());
}

#[test]
fn inventory_recurses_and_fingerprints_renamed_books_without_writes() {
    let world = World::new();
    let bytes = book();
    world
        .reader
        .file("/Author/renamed.epub", bytes.clone())
        .dir("/XTCache");
    let destination = Destination::Reader(world.wifi("/"));
    let snapshot = Inventory::scan(&destination, |_| {}).unwrap();
    assert_eq!(snapshot.entries.len(), 2);
    assert_eq!(
        snapshot
            .matching(&[&inventory::identity(&bytes)])
            .unwrap()
            .file
            .path,
        "/Author/renamed.epub"
    );
    assert!(world.reader.log().iter().all(|l| l.starts_with("GET ")));
}

#[test]
fn incomplete_inventory_download_is_an_error() {
    let world = World::new();
    world.reader.file("/book.epub", book());
    world.reader.corrupt_readback();
    // A download shorter than the listing promised is a changed file.
    let destination = Destination::Reader(world.wifi("/"));
    let file = inventory::FileEntry {
        path: "/book.epub".into(),
        size: 100_000,
        directory: false,
    };
    assert!(destination.read(&file).is_err());
}

#[test]
fn a_copy_over_wifi_says_each_phase_and_the_bytes_so_far() {
    let mut world = World::new();
    world.places.card = false;
    world.places.wifi = true;
    world.places.kobo = false;
    // Big enough for three acknowledgement windows.
    let mut zip = zip::ZipWriter::new_append(Cursor::new(books::epub("Big", "Writer"))).unwrap();
    zip.start_file(
        "large.bin",
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )
    .unwrap();
    zip.write_all(&vec![0x42; 150000]).unwrap();
    world
        .books
        .file("big.epub", zip.finish().unwrap().into_inner());
    let library = world.library();
    let book = library.scan().books.remove(0);
    let said = std::sync::Mutex::new(Vec::<String>::new());
    crossload::books::transfer(&library, &book, crossload::Place::CrossPoint, &|text| {
        said.lock().unwrap().push(text.to_owned())
    })
    .unwrap();
    let said = said.into_inner().unwrap();
    let position = |start: &str| {
        said.iter()
            .position(|s| s.starts_with(start))
            .unwrap_or_else(|| panic!("no {start:?} in {said:?}"))
    };
    let phases = [
        "Reading Local copy",
        "Preparing device copy",
        "Copying to CrossPoint",
        "Uploading to the reader… 0 B of",
        "Verifying the reader's copy",
    ];
    let order: Vec<usize> = phases.iter().map(|p| position(p)).collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{said:?}");
    // Bytes so far, after each acknowledged window.
    let uploads: Vec<&String> = said
        .iter()
        .filter(|s| s.starts_with("Uploading to the reader…"))
        .collect();
    assert!(uploads.len() >= 4, "{said:?}");
    assert!(uploads[1].contains("64.0 KiB of"), "{said:?}");
}
