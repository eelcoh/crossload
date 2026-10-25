//! The command line as a user sees it, on the cli sim: command lines in, output, exit codes and
//! folders out. Each place opens only by the names the wiring would give it; nothing reaches a
//! disk, a device or a network.
use std::io::{Cursor, Write};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use ambit_caps_dir::RelPath;
use ambit_caps_net::{Http, Scope};
use ambit_caps_sim::{SeededRandom, SimClock, SimDir, SimHttp};
use ambit_cli::{Args, Exit};
use ambit_cli_sim::{args, run, run_on_terminal, Run};
use crossload::books::Folder;
use crossload::cli::{Done, Own, Request};
use crossload::crosspoint::Link;
use crossload::inventory::Card;
use crossload_adobe::{Activation, SimAdept};
use crossload_kobo::{SimKobo, StoreBook};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

const HOME: &str = "/home/reader";
const SETTINGS: &str = "/home/reader/.config/crossload/config.json";

fn epub(title: &str, author: &str) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(vec![]));
    for (name, value) in [
        ("mimetype", "application/epub+zip".to_owned()),
        (
            "META-INF/container.xml",
            "<container><rootfiles><rootfile full-path='book.opf'/></rootfiles></container>".into(),
        ),
        (
            "book.opf",
            format!(
                "<package xmlns:dc='http://purl.org/dc/elements/1.1/'><metadata><dc:title>{title}</dc:title>\
                 <dc:creator>{author}</dc:creator></metadata><manifest><item id='c' href='chapter.xhtml' \
                 media-type='application/xhtml+xml'/></manifest><spine><itemref idref='c'/></spine></package>"
            ),
        ),
        (
            "chapter.xhtml",
            format!("<html><head/><body><p>{title}</p></body></html>"),
        ),
    ] {
        zip.start_file(
            name,
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .unwrap();
        zip.write_all(value.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// Every place, and how the wiring would name it: a short name as typed, and where it is.
struct World {
    books: SimDir,
    output: SimDir,
    card: SimDir,
    kobo_dir: SimDir,
    kobo: SimKobo,
    adept: SimAdept,
    settings: SimDir,
    state: SimDir,
    cache: SimDir,
    catalogue: SimHttp,
    reader: SimHttp,
    /// Crossload's own files can't be had (as when HOME is unset).
    homeless: bool,
}

/// An opener that knows these names, each as a place shown at `display`.
fn places<T: 'static>(
    known: &[(&str, &str)],
    make: impl Fn(&str) -> T + Send + Sync + 'static,
) -> impl Fn(&str) -> Result<T, String> + Send + Sync + 'static {
    let known: Vec<(String, String)> = known
        .iter()
        .map(|(name, display)| (name.to_string(), display.to_string()))
        .collect();
    move |value| {
        known
            .iter()
            .find(|(name, display)| name == value || display == value)
            .map(|(_, display)| make(display))
            .ok_or_else(|| format!("Cannot open {value}: not found"))
    }
}

impl World {
    fn new() -> Self {
        let world = Self {
            books: SimDir::new(),
            output: SimDir::new(),
            card: SimDir::new(),
            kobo_dir: SimDir::new(),
            kobo: SimKobo::new(),
            adept: SimAdept::new(),
            settings: SimDir::new(),
            state: SimDir::new(),
            cache: SimDir::new(),
            catalogue: SimHttp::new(Scope::parse(["https://books.example"]).unwrap()),
            reader: SimHttp::new(Scope::parse(["http://192.168.1.50:80"]).unwrap()),
            homeless: false,
        };
        world
            .books
            .file("dune.epub", epub("Dune", "Frank Herbert"))
            .file("piranesi.epub", epub("Piranesi", "Susanna Clarke"));
        world.kobo_dir.dir(".kobo/kepub");
        world
    }

    fn own(&self) -> Result<Own, String> {
        if self.homeless {
            return Err("HOME is unset; use --config and --state-dir".into());
        }
        let settings = crossload::config::Store {
            dir: Arc::new(self.settings.clone()),
            file: RelPath::new(["config.json"]).unwrap(),
            display: SETTINGS.into(),
            home: Some(HOME.into()),
        };
        Ok(Own {
            defaults: crossload::config::load(&settings).map_err(|e| format!("{e:#}"))?,
            settings,
            history: Some(crossload::history::Record {
                dir: Arc::new(self.state.clone()),
                file: RelPath::new([crossload::history::FILE]).unwrap(),
            }),
            cache: Some(crossload::cache::Store {
                dir: Arc::new(self.cache.clone()),
                file: RelPath::new([crossload::cache::FILE]).unwrap(),
            }),
            adept: Arc::new(self.adept.clone()),
        })
    }

    /// The app, with what the wiring would open for this command line.
    fn app(&self, line: &Args) -> impl ambit_cli::App<Done = Done> {
        let own = self.own();
        let saved = own.as_ref().map(|o| o.defaults.clone()).unwrap_or_default();
        let (books, output, card, kobo_dir, kobo) = (
            self.books.clone(),
            self.output.clone(),
            self.card.clone(),
            self.kobo_dir.clone(),
            self.kobo.clone(),
        );
        let reader = self.reader.clone();
        let catalogue = self.catalogue.clone();
        let requests = self.books.clone();
        crossload::cli::init(
            SimClock::at(UNIX_EPOCH + Duration::from_secs(1_791_892_800)),
            line.named(ambit_cli_sim::files(&[("books", &self.books)])),
            line.named(move |value: &str| {
                let name = value
                    .strip_prefix("books/")
                    .ok_or_else(|| format!("Cannot open {value}: not found"))?;
                Ok(Request {
                    folder: Arc::new(requests.clone()),
                    name: RelPath::new([name]).map_err(|e| e.to_string())?,
                    shown: "books".into(),
                })
            }),
            line.named(places(
                &[("books", "/home/reader/Books"), (".", HOME)],
                move |display| Folder {
                    dir: Arc::new(books.clone()),
                    display: display.into(),
                },
            ))
            .also(saved.browse.clone())
            .also(["."]),
            line.named(places(&[("out", "/home/reader/out")], move |display| {
                Folder {
                    dir: Arc::new(output.clone()),
                    display: display.into(),
                }
            }))
            .also(saved.output.clone()),
            line.named(places(
                &[("card", "/run/media/reader/X4")],
                move |display| Card {
                    dir: Arc::new(card.clone()),
                    display: display.into(),
                },
            ))
            .also(saved.copy_to.clone()),
            line.named(places(
                &[("kobo", "/run/media/reader/KOBOeReader")],
                move |display| crossload::kobo::Device {
                    dir: Arc::new(kobo_dir.clone()),
                    db: Arc::new(kobo.clone()),
                    display: display.into(),
                },
            ))
            .also(saved.device.clone()),
            line.named(move |_: &str| {
                Ok(Link {
                    control: Arc::new(reader.clone()),
                    files: Arc::new(reader.clone()),
                    upload: Arc::new(ambit_caps_sim::SimWebSocket::new(Scope::default())),
                    clock: Arc::new(SimClock::at(UNIX_EPOCH)),
                    random: Arc::new(SeededRandom::new(7)),
                    websocket_port: 81,
                })
            })
            .also(saved.reader.clone()),
            line.named(move |_: &str| Ok(Arc::new(catalogue.clone()) as Arc<dyn Http>))
                .also(saved.catalog.clone())
                .also([crossload::opds::BOOKSHELF]),
            line.named(|_: &str| {
                Ok(Activation {
                    device_xml: b"<device/>".to_vec(),
                    activation_xml: b"<activation/>".to_vec(),
                    devicesalt: vec![0; 16],
                })
            }),
            own,
        )
    }

    fn run(&self, line: &[&str]) -> Run<Done> {
        let line = args(line);
        run(self.app(&line), &line, "")
    }

    /// A command that ends with an exit code: the code, stdout and stderr.
    fn exit(&self, line: &[&str]) -> (Exit, String, String) {
        let run = self.run(line);
        match run.done {
            Done::Exit(exit) => (exit, run.out, run.err),
            Done::Tui(_) => panic!("{line:?} started the interface"),
        }
    }

    fn ok(&self, line: &[&str]) -> (String, String) {
        let (exit, out, err) = self.exit(line);
        assert_eq!(
            exit,
            Exit::SUCCESS,
            "{line:?}\nstdout: {out}\nstderr: {err}"
        );
        (out, err)
    }
}

#[test]
fn help_and_version_are_answers_and_a_wrong_command_line_is_a_usage_error() {
    let world = World::new();
    let (out, err) = world.ok(&["crossload", "--version"]);
    // The workspace's version, which this crate shares.
    let version = format!("crossload-cli {}\n", env!("CARGO_PKG_VERSION"));
    assert_eq!((out.as_str(), err.as_str()), (version.as_str(), ""));
    let (out, _) = world.ok(&["crossload", "kobo", "import", "--help"]);
    assert!(
        out.contains("Usage: crossload kobo import [OPTIONS] <BOOK_ID>"),
        "{out}"
    );
    assert!(out.contains("--state-dir <STATE_DIR>"), "{out}");
    let (exit, out, err) = world.exit(&["crossload", "boks"]);
    assert_eq!(exit, Exit::USAGE);
    assert_eq!(out, "");
    assert!(
        err.contains("tip: some similar subcommands exist") || err.contains("'books'"),
        "{err}"
    );
    let (exit, _, err) = world.exit(&["crossload"]);
    assert_eq!(exit, Exit::USAGE);
    assert!(
        err.contains("Usage: crossload [OPTIONS] <COMMAND>"),
        "{err}"
    );
}

#[test]
fn without_its_own_files_every_command_fails_after_the_command_line_is_read() {
    let mut world = World::new();
    world.homeless = true;
    world.ok(&["crossload", "--help"]);
    let (exit, out, err) = world.exit(&["crossload", "adobe", "status"]);
    assert_eq!(exit, Exit::FAILURE);
    assert_eq!(out, "");
    assert_eq!(err, "Error: HOME is unset; use --config and --state-dir\n");
}

#[test]
fn settings_are_saved_shown_and_removed() {
    let world = World::new();
    let (out, _) = world.ok(&["crossload", "config", "set", "--reader", "192.168.1.50"]);
    assert_eq!(out, format!("Saved {SETTINGS}\n"));
    world.ok(&["crossload", "config", "set", "--output", "~/out"]);
    let (out, err) = world.ok(&["crossload", "config", "show"]);
    let shown: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(shown["reader"], "192.168.1.50");
    assert_eq!(shown["output"], "/home/reader/out");
    assert_eq!(err, format!("Defaults: {SETTINGS}\n"));
    let (out, _) = world.ok(&["crossload", "config", "unset", "reader"]);
    assert_eq!(out, "Removed reader\n");
    let (exit, _, err) = world.exit(&["crossload", "config", "set", "--output", "relative"]);
    assert_eq!(exit, Exit::FAILURE);
    assert!(err.starts_with("Error: "), "{err}");
    let (out, _) = world.ok(&["crossload", "config", "show"]);
    assert!(!out.contains("\"reader\""), "{out}");
}

#[test]
fn books_lists_the_catalog_with_tabs_in_a_pipe_and_lined_up_on_a_terminal() {
    let world = World::new();
    let (out, err) = world.ok(&["crossload", "books", "--browse", "books", "--output", "out"]);
    assert_eq!(
        out,
        "TITLE\tAUTHOR\tWHERE\tSOURCE\tBYTES\n\
         Dune\tFrank Herbert\tLocal\tLocal (original)\t843\n\
         Piranesi\tSusanna Clarke\tLocal\tLocal (original)\t852\n"
    );
    assert!(
        err.starts_with("Local: Ready (2 books, 0 unreadable)\n"),
        "{err}"
    );
    let line = args(&["crossload", "books", "--browse", "books", "--output", "out"]);
    let run = run_on_terminal(world.app(&line), &line, "", 100);
    assert!(
        run.out
            .contains(" Dune      Frank Herbert   Local  Local (original)  843"),
        "{}",
        run.out
    );
    assert!(run.out.ends_with("\n2 books\n"), "{}", run.out);
}

#[test]
fn sync_plans_first_then_copies_to_the_card_and_the_record_shows_it() {
    let world = World::new();
    let line = [
        "crossload",
        "sync",
        "--from",
        "local",
        "--to",
        "crosspoint",
        "--browse",
        "books",
        "--output",
        "out",
        "--copy-to",
        "card",
    ];
    let (out, _) = world.ok(&line);
    assert!(
        out.contains("Dune\tFrank Herbert\tLocal\tplanned\n"),
        "{out}"
    );
    assert!(
        out.ends_with("This was a dry run. Add --apply to copy them.\n"),
        "{out}"
    );
    assert!(world.card.read("Frank Herbert/Dune.epub").is_none());

    let applied: Vec<&str> = line.iter().copied().chain(["--apply"]).collect();
    let (out, err) = world.ok(&applied);
    assert!(
        out.contains("Copied to /run/media/reader/X4/Frank Herbert/Dune.epub (verified)."),
        "{out}"
    );
    assert!(err.contains("[1/2] Dune — copying to CrossPoint"), "{err}");
    assert!(world.card.read("Frank Herbert/Dune.epub").is_some());

    let (out, _) = world.ok(&applied);
    assert_eq!(
        out,
        "Nothing to copy: every book is already at CrossPoint.\n"
    );
    let (out, _) = world.ok(&["crossload", "history"]);
    assert!(
        out.starts_with("WHEN\tTITLE\tAUTHOR\tFROM\tTO\tRESULT\n"),
        "{out}"
    );
    assert!(
        out.contains("\tDune\tFrank Herbert\tLocal\tCrossPoint\tcopied\n"),
        "{out}"
    );
}

#[test]
fn copy_puts_a_book_on_the_card_and_refuses_a_card_that_is_not_there() {
    let world = World::new();
    let (out, err) = world.ok(&["crossload", "copy", "books/dune.epub", "--to", "card"]);
    assert_eq!(
        out,
        "Copied card/Frank Herbert/Dune.epub (843 bytes; verified)\n\
         Safely eject the card before disconnecting it.\n"
    );
    assert_eq!(
        err,
        "Device copy: 843 → 843 bytes; 0 images optimized for X4 (480×800).\n"
    );
    assert_eq!(
        world.card.read("Frank Herbert/Dune.epub").unwrap(),
        epub("Dune", "Frank Herbert")
    );
    world.card.plugged(false);
    let (exit, out, err) =
        world.exit(&["crossload", "copy", "books/piranesi.epub", "--to", "card"]);
    assert_eq!(exit, Exit::FAILURE);
    assert_eq!(out, "");
    assert_eq!(
        err,
        "Error: Destination must be an existing mounted-card directory\n"
    );
}

#[test]
fn optimize_saves_a_device_copy_in_the_saved_output_folder() {
    let world = World::new();
    let (exit, _, err) = world.exit(&["crossload", "optimize", "books/dune.epub"]);
    assert_eq!(exit, Exit::FAILURE);
    assert!(err.contains("Missing --output"), "{err}");
    world.ok(&["crossload", "config", "set", "--output", "/home/reader/out"]);
    let (out, _) = world.ok(&["crossload", "optimize", "books/dune.epub"]);
    assert_eq!(out, "Device copy: /home/reader/out/Dune.epub\n");
    assert!(world.output.read("Dune.epub").is_some());
    let (exit, _, err) = world.exit(&[
        "crossload",
        "optimize",
        "books/paper.pdf",
        "--output",
        "out",
    ]);
    assert_eq!(exit, Exit::FAILURE);
    assert!(err.contains("Only EPUB is optimized"), "{err}");
}

#[test]
fn an_acsm_is_fulfilled_into_the_output_and_archived_beside_itself() {
    let world = World::new();
    let acsm = b"<fulfillmentToken xmlns=\"http://ns.adobe.com/adept\"/>";
    world.books.file("request.acsm", acsm.to_vec());
    world
        .adept
        .activated()
        .book(acsm, &epub("Gideon", "Tamsyn Muir"));
    let (out, _) = world.ok(&[
        "crossload",
        "import",
        "books/request.acsm",
        "--output",
        "out",
    ]);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert!(
        lines[0].starts_with("Imported /home/reader/out/Gideon"),
        "{out}"
    );
    assert_eq!(lines[1], "Archived books/archive/request.acsm");
    assert_eq!(world.adept.fulfilled(), vec![acsm.to_vec()]);
    assert_eq!(world.books.read("archive/request.acsm").unwrap(), acsm);
}

#[test]
fn the_activation_is_imported_and_then_reported() {
    let world = World::new();
    let (out, _) = world.ok(&["crossload", "adobe", "status"]);
    assert!(out.starts_with("No activation installed."), "{out}");
    let (out, _) = world.ok(&["crossload", "adobe", "setup", "--from", "export.zip"]);
    assert_eq!(
        out,
        "Activation imported and checked. No server was contacted.\n"
    );
    let (out, _) = world.ok(&["crossload", "adobe", "status"]);
    assert!(
        out.starts_with("Activation installed and locally valid."),
        "{out}"
    );
}

#[test]
fn a_kobo_lists_its_books_and_imports_one() {
    let world = World::new();
    world.kobo.book(StoreBook {
        id: "volume-1".into(),
        title: "Dune".into(),
        author: "Frank Herbert".into(),
        encrypted: false,
        preview: false,
    });
    world
        .kobo_dir
        .file(".kobo/kepub/volume-1", epub("Dune", "Frank Herbert"));
    let (out, _) = world.ok(&["crossload", "kobo", "list", "--device", "kobo"]);
    assert_eq!(
        out,
        "TITLE\tAUTHOR\tSOURCE\tTYPE\tKEYS\tID\nDune\tFrank Herbert\tKobo store\tBook\t—\tvolume-1\n"
    );
    let (out, _) = world.ok(&[
        "crossload",
        "kobo",
        "import",
        "volume-1",
        "--device",
        "kobo",
        "--output",
        "out",
    ]);
    assert!(out.starts_with("Imported /home/reader/out/"), "{out}");
    let (exit, _, err) = world.exit(&["crossload", "kobo", "list", "--device", "elsewhere"]);
    assert_eq!(exit, Exit::FAILURE);
    assert_eq!(err, "Error: Cannot open elsewhere: not found\n");
    world.kobo_dir.plugged(false);
    let (exit, _, err) = world.exit(&["crossload", "kobo", "list", "--device", "kobo"]);
    assert_eq!(exit, Exit::FAILURE);
    assert_eq!(
        err,
        "Error: Cannot open Kobo at kobo; pass the Kobo mount directory with --device\n"
    );
}

#[test]
fn a_catalogue_lists_what_it_offers_and_says_what_it_hid() {
    let world = World::new();
    world.catalogue.respond(
        "GET",
        "https://books.example/feed",
        200,
        r#"<feed xmlns="http://www.w3.org/2005/Atom">
             <title>Example Books</title>
             <entry><title>Dune</title><id>1</id><author><name>Frank Herbert</name></author>
               <link href="/dune.epub" rel="http://opds-spec.org/acquisition/open-access"
                     type="application/epub+zip"/></entry>
             <entry><title>Locked</title><id>2</id>
               <link href="/locked" rel="http://opds-spec.org/acquisition/borrow"
                     type="application/vnd.readium.lcp.license.v1.0+json"/></entry>
           </feed>"#,
    );
    let (out, err) = world.ok(&[
        "crossload",
        "catalog",
        "browse",
        "https://books.example/feed",
    ]);
    assert!(
        out.starts_with("TITLE\tAUTHOR\tOFFER\tADDRESS\nDune\tFrank Herbert\t"),
        "{out}"
    );
    assert!(out.contains("https://books.example/dune.epub"), "{out}");
    assert!(err.starts_with("Example Books\n"), "{err}");
    assert!(err.contains("1 more this cannot deliver, hidden"), "{err}");
}

#[test]
fn a_reader_that_does_not_answer_fails_the_send_and_prints_no_result() {
    let world = World::new();
    let (exit, out, err) = world.exit(&[
        "crossload",
        "send",
        "books/dune.epub",
        "--to",
        "192.168.1.50",
    ]);
    assert_eq!(exit, Exit::FAILURE);
    assert_eq!(out, "");
    assert!(
        err.starts_with("Sending EPUB to CrossPoint; checking the reader's copy afterward…\n"),
        "{err}"
    );
    assert!(
        err.ends_with("Error: CrossPoint request failed: /api/status: http Network: sim: nothing answers GET http://192.168.1.50/api/status\n"),
        "{err}"
    );
}

/// A save starts the interface again with the saved settings filling in what the command line
/// didn't give. What it gave holds: a locked program starts again with the same arguments.
#[test]
fn tui_hands_back_what_to_start_and_a_save_keeps_what_the_command_line_gave() {
    let world = World::new();
    let run = world.run(&[
        "crossload",
        "--no-optimize",
        "tui",
        "--browse",
        "books",
        "--copy-to",
        "card",
        "--serial",
        "N1",
    ]);
    let Done::Tui(tui) = run.done else {
        panic!("no interface: {}", run.err)
    };
    assert_eq!(tui.browse.as_deref(), Some("books"));
    assert_eq!(tui.copy_to.as_deref(), Some("card"));
    assert_eq!(tui.output, None);
    assert!(tui.first_run);
    assert_eq!(tui.folder, "/");
    assert!(tui.settings.optimize.is_none());
    assert_eq!(
        tui.given(),
        [Some("--browse"), None, None, None, Some("--copy-to"), None]
    );
    let saved = crossload::config::Defaults {
        browse: Some("/home/reader/Books".into()),
        output: Some("/home/reader/out".into()),
        folder: Some("/books".into()),
        ..Default::default()
    };
    let again = tui.restart(saved).unwrap();
    assert_eq!(again.browse.as_deref(), Some("books"));
    assert_eq!(again.output.as_deref(), Some("/home/reader/out"));
    assert_eq!(again.copy_to.as_deref(), Some("card"));
    assert_eq!(again.folder, "/books");
    assert_eq!(again.given(), tui.given());
    assert_eq!(again.settings.serial.as_deref(), Some("N1"));
    assert!(again.settings.optimize.is_none());
    assert!(!again.first_run);
}

/// What each command line writes, for the wiring to make before it locks the program: nothing
/// for help, a mistake, or a command that only reads.
#[test]
fn a_command_line_says_what_it_will_write() {
    use crossload::cli::{writes, Writes};
    let saved = crossload::config::Defaults {
        output: Some("/saved".into()),
        ..Default::default()
    };
    let of = |line: &str| {
        let mut list = vec!["crossload".to_owned()];
        list.extend(line.split_whitespace().map(str::to_owned));
        writes(&list, &saved)
    };
    let nothing = Writes::default();
    for line in [
        "--help",
        "copy --help",
        "nonsense",
        "config show",
        "history",
        "copy book.epub --to /card",
        "send book.epub --to 192.168.1.7",
        "catalog browse",
        "kobo list",
        "kobo shelves",
        "kobo sync",
        "kobo notes --output /notes --json",
    ] {
        assert_eq!(of(line), nothing, "{line}");
    }
    assert!(of("tui").tui);
    assert!(of("config set --kepub true").settings);
    assert!(of("config unset kepub").settings);
    assert_eq!(
        of("books"),
        Writes {
            cache: true,
            ..Default::default()
        }
    );
    assert_eq!(
        of("sync --from local --to crosspoint --apply"),
        Writes {
            cache: true,
            history: true,
            output: Some("/saved".into()),
            ..Default::default()
        }
    );
    assert_eq!(
        of("import book.acsm --output /typed"),
        Writes {
            activation: true,
            output: Some("/typed".into()),
            ..Default::default()
        }
    );
    assert!(of("adobe status").activation);
    for line in [
        "optimize book.epub",
        "catalog get https://example.org/book.epub",
        "kobo import id",
        "kobo sync --apply",
    ] {
        assert_eq!(of(line).output.as_deref(), Some("/saved"), "{line}");
    }
    assert_eq!(of("kobo notes").output, None);
    assert_eq!(
        of("kobo notes --output /notes").output.as_deref(),
        Some("/notes")
    );
}
