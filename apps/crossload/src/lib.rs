//! Crossload: one library across this computer, a Kobo and a CrossPoint reader.
//!
//! All authority comes in through [`init`], one capability per kind: the folders it may read and
//! add to, the reader's card (the one place it may delete from), the Kobo's mount and database,
//! the reader over Wi-Fi, ACSM fulfilment, its own settings, state and cache folders, the clock
//! and randomness. The library functions below take the same capabilities, so the command line
//! (in the wiring, until ambit has a cli platform) runs the same code with native ones.
#![forbid(
    unsafe_code,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::print_stdout,
    clippy::print_stderr
)]

pub mod acsm;
pub mod books;
pub mod cache;
pub mod config;
pub mod copy;
pub mod crosspoint;
pub mod epub;
pub mod files;
pub mod format;
pub mod history;
pub mod inventory;
pub mod kepub;
pub mod kobo;
pub mod metadata;
pub mod opds;
pub mod pdf;
pub mod prepare;
pub mod profile;
pub mod sync;
pub mod tui;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use ambit_caps::{Clock, Random};
use ambit_caps_dir::{DirModify, DirWrite, RelPath};
use ambit_caps_net::{Http, WebSocket};
use ambit_tui::App;
use crossload_adobe::Adept;
use crossload_kobo::Kobo;

pub use books::Place;

/// Settings from the command line and the settings file. Authority is not in here: that's
/// `init`'s other parameters.
pub struct Config {
    /// What the interface shows about each place.
    pub options: tui::Options,
    pub settings: books::Settings,
    /// The reader's address and its base folder, when one is configured.
    pub reader: Option<(String, String)>,
    /// The CrossPoint upload port.
    pub websocket_port: u16,
    /// The settings file's name in the settings folder.
    pub settings_file: String,
    /// Places that were asked for but could not be opened, and why.
    pub unavailable: Vec<(Place, String)>,
    /// Set when the user saved new settings: the wiring starts the app again with what they
    /// name.
    pub relaunch: Arc<AtomicBool>,
}

/// The capabilities, one per kind of authority:
/// - `books`, `import`: the books folder and the import folder. Read, and only ever added to.
/// - `kobo`: a mounted Kobo: its mount (read, added to) and its database (read).
/// - `card`: a mounted reader card: read, added to, and the one place a book may be deleted.
/// - `reader`, `reader_files`, `upload`: the reader over Wi-Fi: small requests, whole files,
///   and its upload WebSocket.
/// - `adept`: ACSM fulfilment with the installed Adobe/ByteBooks activation.
/// - `settings`, `state`, `cache`: crossload's own folders for its settings file, the record of
///   copies, and the identities of books already read.
/// - `clock`: timestamps and deadlines; `random`: names for uploads in progress.
#[allow(clippy::too_many_arguments)]
pub fn init(
    clock: impl Clock,
    random: impl Random,
    books: impl DirWrite,
    import: impl DirWrite,
    kobo: Option<(impl DirWrite, impl Kobo)>,
    card: Option<impl DirModify>,
    reader: impl Http,
    reader_files: impl Http,
    upload: impl WebSocket,
    adept: impl Adept,
    settings: Option<impl DirModify>,
    state: Option<impl DirModify>,
    cache: Option<impl DirModify>,
    config: Config,
) -> impl App {
    let clock: Arc<dyn Clock> = Arc::new(clock);
    let options = config.options;
    let link = crosspoint::Link {
        control: Arc::new(reader),
        files: Arc::new(reader_files),
        upload: Arc::new(upload),
        clock: clock.clone(),
        random: Arc::new(random),
        websocket_port: config.websocket_port,
    };
    let mut unavailable = config.unavailable;
    let destination = match (card, &config.reader) {
        (Some(card), _) => Some(inventory::Destination::Card(inventory::Card {
            dir: Arc::new(card),
            display: options.copy_to.clone().unwrap_or_default(),
        })),
        (None, Some((address, folder))) => match crosspoint::Address::parse(address, folder) {
            Ok(address) => Some(inventory::Destination::Reader(crosspoint::Reader::new(
                address,
                link.clone(),
            ))),
            Err(e) => {
                unavailable.push((Place::CrossPoint, format!("{e:#}")));
                None
            }
        },
        (None, None) => None,
    };
    let store = settings.and_then(|dir| {
        Some(config::Store {
            dir: Arc::new(dir),
            file: RelPath::parse(&config.settings_file).ok()?,
            display: options.config.clone(),
            home: options.home.clone(),
        })
    });
    let library = books::Library {
        settings: config.settings,
        books: books::Folder {
            dir: Arc::new(books),
            display: options.browse.clone(),
        },
        import: books::Folder {
            dir: Arc::new(import),
            display: options.output.clone(),
        },
        kobo: kobo.map(|(dir, db)| kobo::Device {
            dir: Arc::new(dir),
            db: Arc::new(db),
            display: options.device.clone().unwrap_or_default(),
        }),
        reader: destination,
        adept: Arc::new(adept),
        cache: cache.map(|dir| cache::Store {
            dir: Arc::new(dir),
            file: RelPath::new([cache::FILE]).expect("a plain name"),
        }),
        history: state.map(|dir| history::Record {
            dir: Arc::new(dir),
            file: RelPath::new([history::FILE]).expect("a plain name"),
        }),
        clock,
        unavailable,
    };
    tui::Shell::new(options, library, store, link, config.relaunch)
}
