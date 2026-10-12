//! Sim capabilities for every place crossload works with, scoped as `run-crossload` scopes them,
//! and the two ways the tests use them: the library functions directly, or the whole terminal
//! interface under `ambit-tui-sim`.
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use ambit_caps_dir::RelPath;
use ambit_caps_net::{Origin, Scheme, Scope};
use ambit_caps_sim::{SeededRandom, SimClock, SimDir, SimHttp, SimWebSocket};
use ambit_tui_sim::Sim;
use crossload::books::{Folder, Library, Settings};
use crossload::crosspoint::{Address, Link, Reader};
use crossload::inventory::{Card, Destination};
use crossload::kobo::Device;
use crossload_adobe::SimAdept;
use crossload_kobo::SimKobo;

use crate::reader::{FakeReader, IP};

/// Where each place is, as the user would know it.
pub const BOOKS: &str = "/home/reader/Books";
pub const IMPORT: &str = "/home/reader/Imported";
pub const KOBO: &str = "/run/media/reader/KOBOeReader";
pub const CARD: &str = "/run/media/reader/X4";

/// Which places a test has.
#[derive(Clone, Copy, Debug)]
pub struct Places {
    pub kobo: bool,
    pub card: bool,
    /// The reader over Wi-Fi, when there is no card.
    pub wifi: bool,
}

impl Default for Places {
    fn default() -> Self {
        Self {
            kobo: true,
            card: true,
            wifi: false,
        }
    }
}

/// The reader's scope, as the wiring grants it for a reader given by IP address.
pub fn reader_scope() -> (Scope, Scope) {
    (
        Scope::new([Origin::lan(Scheme::Http, 80)]),
        Scope::new([Origin::lan(Scheme::Ws, 81)]),
    )
}

#[derive(Clone)]
pub struct World {
    pub clock: SimClock,
    pub books: SimDir,
    pub import: SimDir,
    pub kobo_dir: SimDir,
    pub kobo: SimKobo,
    pub card: SimDir,
    pub http: SimHttp,
    pub upload: SimWebSocket,
    pub reader: FakeReader,
    pub adept: SimAdept,
    pub settings_dir: SimDir,
    pub state: SimDir,
    pub cache: SimDir,
    pub places: Places,
    pub settings: Settings,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    pub fn new() -> Self {
        let (http, ws) = reader_scope();
        let world = Self {
            clock: SimClock::at(UNIX_EPOCH + Duration::from_secs(1_760_000_000)),
            books: SimDir::new(),
            import: SimDir::new(),
            kobo_dir: SimDir::new(),
            kobo: SimKobo::new(),
            card: SimDir::new(),
            http: SimHttp::new(http),
            upload: SimWebSocket::new(ws),
            reader: FakeReader::new(),
            adept: SimAdept::new(),
            settings_dir: SimDir::new(),
            state: SimDir::new(),
            cache: SimDir::new(),
            places: Places::default(),
            settings: Settings {
                show_previews: false,
                serial: None,
                optimize: Some(crossload::profile::X4),
                organized: true,
                kepub: false,
            },
        };
        world.kobo_dir.dir(".kobo/kepub");
        world.reader.serve(&world.http, &world.upload);
        world
    }

    pub fn link(&self) -> Link {
        Link {
            control: Arc::new(self.http.clone()),
            files: Arc::new(self.http.clone()),
            upload: Arc::new(self.upload.clone()),
            clock: Arc::new(self.clock.clone()),
            random: Arc::new(SeededRandom::new(7)),
            websocket_port: 81,
        }
    }

    /// The reader over Wi-Fi, with its base folder.
    pub fn wifi(&self, folder: &str) -> Reader {
        Reader::new(Address::parse(IP, folder).unwrap(), self.link())
    }

    pub fn card_destination(&self) -> Destination {
        Destination::Card(Card {
            dir: Arc::new(self.card.clone()),
            display: CARD.into(),
        })
    }

    pub fn device(&self) -> Device {
        Device {
            dir: Arc::new(self.kobo_dir.clone()),
            db: Arc::new(self.kobo.clone()),
            display: KOBO.into(),
        }
    }

    /// The library over the sim places, as the interface would get it.
    pub fn library(&self) -> Library {
        Library {
            settings: self.settings.clone(),
            books: Folder {
                dir: Arc::new(self.books.clone()),
                display: BOOKS.into(),
            },
            import: Folder {
                dir: Arc::new(self.import.clone()),
                display: IMPORT.into(),
            },
            kobo: self.places.kobo.then(|| self.device()),
            reader: if self.places.card {
                Some(self.card_destination())
            } else if self.places.wifi {
                Some(Destination::Reader(self.wifi("/")))
            } else {
                None
            },
            adept: Arc::new(self.adept.clone()),
            cache: Some(crossload::cache::Store {
                dir: Arc::new(self.cache.clone()),
                file: RelPath::new([crossload::cache::FILE]).unwrap(),
            }),
            history: Some(crossload::history::Record {
                dir: Arc::new(self.state.clone()),
                file: RelPath::new([crossload::history::FILE]).unwrap(),
            }),
            clock: Arc::new(self.clock.clone()),
            unavailable: vec![],
        }
    }

    /// The interface's options, naming the places this world has.
    pub fn options(&self) -> crossload::tui::Options {
        crossload::tui::Options {
            config: "/home/reader/.config/crossload/config.json".into(),
            home: Some("/home/reader".into()),
            first_run: false,
            browse: BOOKS.into(),
            output: IMPORT.into(),
            device: self.places.kobo.then(|| KOBO.into()),
            send_to: (self.places.wifi && !self.places.card).then(|| IP.into()),
            copy_to: self.places.card.then(|| CARD.into()),
            folder: "/".into(),
            defaults: Default::default(),
            kobos: vec![],
            opened: crossload::tui::Opened {
                books: true,
                import: true,
                kobo: self.places.kobo.then_some(true),
                card: self.places.card.then_some(true),
            },
            color: true,
        }
    }

    /// The whole interface on the sim terminal, with these options.
    pub fn start_with(&self, options: crossload::tui::Options) -> Sim<crossload::Crossload> {
        let reader =
            (self.places.wifi && !self.places.card).then(|| (IP.to_string(), "/".to_string()));
        let app = crossload::init(
            self.clock.clone(),
            SeededRandom::new(7),
            self.books.clone(),
            self.import.clone(),
            self.places
                .kobo
                .then(|| (self.kobo_dir.clone(), self.kobo.clone())),
            self.places.card.then(|| self.card.clone()),
            self.http.clone(),
            self.http.clone(),
            self.upload.clone(),
            self.adept.clone(),
            Some(self.settings_dir.clone()),
            Some(self.state.clone()),
            Some(self.cache.clone()),
            crossload::Config {
                options,
                settings: self.settings.clone(),
                reader,
                websocket_port: 81,
                settings_file: "config.json".into(),
                unavailable: vec![],
            },
        );
        Sim::new(app, self.clock.clone(), 120, 30)
    }

    /// The whole interface on the sim terminal.
    pub fn start(&self) -> Sim<crossload::Crossload> {
        self.start_with(self.options())
    }
}
