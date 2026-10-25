// Wiring: which folders, devices and endpoints crossload gets, and with which scope. This file is
// the review surface for what the app can do; everything it does goes through `crossload::init`
// (the terminal interface) or `crossload::cli::init` (the command line), with what is opened here.
// On Linux the program is then locked to what was opened (`lock_cli`, `run_tui`): the operating
// system refuses the rest.

mod lock;

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use ambit_caps_dir::{OneFile, RelPath};
use ambit_caps_dir_native::NativeDir;
use ambit_caps_net::{Http, Origin, Scheme, Scope};
use ambit_caps_net_native::{HttpOptions, NativeHttp};
use ambit_caps_ws_native::{NativeWebSocket, WsOptions};
use ambit_cli::Args;
use ambit_lockdown::{Grant, Grants, Lockdown, Locked};
use anyhow::{Context, Result};
use crossload::cli::{Done, Own, Request};
use crossload::crosspoint::{Address, Link};
use crossload_adobe_native::NativeAdept;
use crossload_kobo_native::NativeKobo;

/// The largest book crossload handles; also the reader's whole-file body limit.
const MAX_BOOK: u64 = 128 * 1024 * 1024;
/// CrossPoint's upload WebSocket.
const WEBSOCKET_PORT: u16 = 81;

/// The reader over Wi-Fi: its HTTP port, and its upload WebSocket. A reader given by IP address
/// is anything on the local network on those ports; one given by name is that name exactly (a
/// local-network grant only covers IP addresses, since a name could resolve anywhere).
fn reader_scope(address: Option<&Address>) -> (Scope, Scope) {
    match address {
        None => (Scope::default(), Scope::default()),
        Some(address) if address.host().parse::<IpAddr>().is_ok() => (
            Scope::new([Origin::lan(Scheme::Http, address.port())]),
            Scope::new([Origin::lan(Scheme::Ws, WEBSOCKET_PORT)]),
        ),
        Some(address) => {
            let host = address.host();
            let exact = |scheme: &str, port: u16| {
                Scope::parse([format!("{scheme}://{host}:{port}").as_str()]).unwrap_or_default()
            };
            (exact("http", address.port()), exact("ws", WEBSOCKET_PORT))
        }
    }
}

/// The reader's small requests: status, listings, folders, renames.
fn reader_control(scope: Scope) -> NativeHttp {
    NativeHttp::new(
        scope,
        HttpOptions {
            timeout: Duration::from_secs(15),
            max_body: 4 * 1024 * 1024,
        },
    )
}

/// Whole files from the reader: inventory, and the read-back that verifies an upload.
fn reader_files(scope: Scope) -> NativeHttp {
    NativeHttp::new(
        scope,
        HttpOptions {
            timeout: Duration::from_secs(600),
            max_body: MAX_BOOK,
        },
    )
}

/// The reader's upload protocol: 4 KiB frames out, short text acknowledgements back.
fn reader_upload(scope: Scope) -> NativeWebSocket {
    NativeWebSocket::new(
        scope,
        WsOptions {
            connect_timeout: Duration::from_secs(5),
            io_timeout: Duration::from_secs(30),
            max_message: 64 * 1024,
        },
    )
}

/// OPDS catalogues (command line only): the catalogue the user named, the saved one, and the
/// Palace Bookshelf, each with its subdomains (catalogues hand books out from a CDN beside
/// them). A redirect anywhere else is refused.
fn catalogues(urls: &[&str]) -> NativeHttp {
    let mut origins = Vec::new();
    for url in urls {
        let Ok(uri) = url.parse::<ambit_caps_net::http::Uri>() else {
            continue;
        };
        let (Some(scheme), Some(host)) = (uri.scheme_str(), uri.host()) else {
            continue;
        };
        let port = uri.port_u16().map(|p| format!(":{p}")).unwrap_or_default();
        origins.push(format!("{scheme}://{host}{port}"));
        // The registrable part, roughly: the last two labels.
        let labels: Vec<&str> = host.split('.').collect();
        if labels.len() >= 2 && host.parse::<IpAddr>().is_err() {
            let domain = labels[labels.len() - 2..].join(".");
            origins.push(format!("{scheme}://*.{domain}{port}"));
            origins.push(format!("{scheme}://{domain}{port}"));
        }
    }
    let scope = Scope::parse(origins.iter().map(String::as_str)).unwrap_or_default();
    NativeHttp::new(
        scope,
        HttpOptions {
            timeout: Duration::from_secs(300),
            max_body: MAX_BOOK,
        },
    )
}

/// A folder this program reads and adds to. It must exist.
fn folder(path: &Path) -> Result<NativeDir> {
    NativeDir::open(path).with_context(|| format!("Cannot open {}", path.display()))
}

/// One of crossload's own folders (settings, state, cache), created if missing. Private to the
/// user, as the files in it were.
fn own_folder(path: &Path) -> Result<NativeDir> {
    std::fs::create_dir_all(path).with_context(|| format!("Cannot create {}", path.display()))?;
    folder(path)
}

/// Where things live, from the environment and the arguments.
struct Paths {
    home: Option<PathBuf>,
    config: PathBuf,
    state: PathBuf,
    cache: PathBuf,
    adept: PathBuf,
}

impl Paths {
    fn new(config: Option<PathBuf>, adept: Option<PathBuf>) -> Result<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let xdg = |name: &str| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        };
        let mac = cfg!(target_os = "macos");
        let need_home = || {
            home.clone()
                .context("HOME is unset; use --config and --state-dir")
        };
        let config = match config {
            Some(path) => path,
            None => xdg("XDG_CONFIG_HOME")
                .map_or_else(|| need_home().map(|h| h.join(".config")), Ok)?
                .join("crossload/config.json"),
        };
        // A record is not a cache: it is not disposable, so it does not live with the things
        // that are.
        let state = if mac {
            need_home()?.join("Library/Application Support/crossload")
        } else {
            xdg("XDG_STATE_HOME")
                .map_or_else(|| need_home().map(|h| h.join(".local/state")), Ok)?
                .join("crossload")
        };
        let cache = if mac {
            need_home()?.join("Library/Caches/crossload")
        } else {
            xdg("XDG_CACHE_HOME")
                .map_or_else(|| need_home().map(|h| h.join(".cache")), Ok)?
                .join("crossload")
        };
        let adept = match adept {
            Some(path) => path,
            None => crossload_adobe_native::default_state_dir(
                &need_home()?,
                xdg("XDG_DATA_HOME").as_deref(),
            ),
        };
        Ok(Self {
            home,
            config,
            state,
            cache,
            adept,
        })
    }

    /// The settings file: its folder, opened to replace the file in it, and its name.
    fn settings(&self) -> Result<(NativeDir, String)> {
        let parent = self
            .config
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let name = self
            .config
            .file_name()
            .and_then(|n| n.to_str())
            .context("The settings file needs a plain file name")?;
        Ok((own_folder(parent)?, name.to_owned()))
    }

    /// The settings file for the command line. Its folder is made when settings are first saved,
    /// so a command that only reads (or `--help`) leaves nothing behind.
    fn settings_store(&self) -> Result<crossload::config::Store> {
        let parent = self
            .config
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let name = self
            .config
            .file_name()
            .and_then(|n| n.to_str())
            .context("The settings file needs a plain file name")?;
        Ok(crossload::config::Store {
            dir: Arc::new(NativeDir::creating(parent)),
            file: RelPath::new([name])?,
            display: self.config.display().to_string(),
            home: self.home.as_ref().map(|h| h.display().to_string()),
        })
    }

    /// The command line's own: settings (read now), the record of copies and the cache (their
    /// folders made on the first write), and the activation.
    fn own(&self) -> Result<Own> {
        let settings = self.settings_store()?;
        Ok(Own {
            defaults: crossload::config::load(&settings)?,
            settings,
            history: Some(crossload::history::Record {
                dir: Arc::new(NativeDir::creating(&self.state)),
                file: RelPath::new([crossload::history::FILE])?,
            }),
            cache: Some(crossload::cache::Store {
                dir: Arc::new(NativeDir::creating(&self.cache)),
                file: RelPath::new([crossload::cache::FILE])?,
            }),
            adept: Arc::new(NativeAdept::new(&self.adept)),
        })
    }
}

/// Mounted Kobos, at the conventional mount locations.
fn detect_kobos() -> Vec<String> {
    let mut roots = vec![
        PathBuf::from("/Volumes"),
        PathBuf::from("/mnt"),
        PathBuf::from("/media"),
    ];
    // Linux desktop mounts normally sit one level below the user directory.
    for parent in ["/run/media", "/media"] {
        roots.extend(
            std::fs::read_dir(parent)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok().map(|e| e.path())),
        );
    }
    detect_in(&roots)
}

/// Kobos in these folders or one level below them. Never recurses into a device's books.
fn detect_in(roots: &[PathBuf]) -> Vec<String> {
    let mut devices = vec![];
    for root in roots {
        let candidates = std::iter::once(root.clone()).chain(
            std::fs::read_dir(root)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok().map(|e| e.path())),
        );
        for candidate in candidates {
            if candidate.join(".kobo/KoboReader.sqlite").is_file() {
                let candidate = candidate.canonicalize().unwrap_or(candidate);
                let shown = candidate.display().to_string();
                if !devices.contains(&shown) {
                    devices.push(shown);
                }
            }
        }
    }
    devices.sort();
    devices
}

/// What the terminal interface is started with: the command line's choices, made into paths.
struct Tui {
    paths: Paths,
    browse: PathBuf,
    output: PathBuf,
    device: Option<PathBuf>,
    send_to: Option<String>,
    copy_to: Option<PathBuf>,
    folder: String,
    first_run: bool,
    defaults: crossload::config::Defaults,
    settings: crossload::books::Settings,
    /// The settings the command line gave, by option, for the settings panel.
    given: [Option<&'static str>; 6],
}

/// Books imported from a Kobo must not land back on it. Checked through symlinks, on the part
/// of the path that exists, before anything is created. A Kobo that isn't mounted can't be
/// written into.
fn outside_device(output: &Path, device: &Path) -> Result<()> {
    let Ok(device) = device.canonicalize() else {
        return Ok(());
    };
    anyhow::ensure!(
        !output
            .components()
            .any(|c| c == std::path::Component::ParentDir),
        "Output path must not contain .."
    );
    let output = absolute(output);
    let ancestor = output
        .ancestors()
        .find(|p| p.exists())
        .context("No existing output parent")?;
    anyhow::ensure!(
        !ancestor.canonicalize()?.starts_with(&device),
        "Choose a local output directory outside the Kobo"
    );
    Ok(())
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Runs the terminal interface until it quits. When the user saved new settings, it starts
/// again with what `restart` makes of them: the folders and devices they name, opened afresh.
///
/// Locked before it first starts, to what it opened then and the places devices appear. A lock
/// can't be widened, so settings that need more start the program again (its supervisor, outside
/// the lock, does), with the same arguments: it opens and locks afresh from them and what was
/// saved. The terminal interface has exited by then, so the terminal is restored. Only if that
/// can't be done does it end, with the message returned here: starting it again opens them.
fn run_tui(
    mut tui: Tui,
    restart: impl Fn(crossload::config::Defaults) -> Result<Tui>,
) -> Result<Option<String>> {
    use std::io::IsTerminal;
    anyhow::ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "The TUI needs an interactive terminal; use the CLI commands for redirected input/output"
    );
    let mut held = None;
    loop {
        let (app, grants) = tui_app(&tui)?;
        match &held {
            None => {
                // The import folder is made now, if it isn't there, so the lock covers it.
                let output = absolute(&tui.output);
                let lock = if output.is_dir() {
                    Lockdown::new()
                } else {
                    Lockdown::new().make(&output)?
                };
                let lock = lock.allow(&grants);
                let (asked, holds) = (lock.grants().to_vec(), lock::held(lock.grants()));
                let locked = lock.apply()?;
                if let Some(warning) = locked.warning() {
                    eprintln!("crossload: {warning}");
                }
                // Where nothing is locked (macOS), new settings are opened as before.
                held = Some((!matches!(locked, Locked::Not(_))).then_some((asked, holds)));
            }
            Some(None) => {}
            Some(Some((asked, held))) => {
                // What the lock asked for, and didn't get because it wasn't there, isn't new.
                let wanted: Vec<Grant> =
                    grants.into_iter().filter(|g| !asked.contains(g)).collect();
                let missing = lock::uncovered(held, &wanted);
                if !missing.is_empty() {
                    // Returns only if it can't restart.
                    let error = ambit_lockdown::restart();
                    if ambit_lockdown::can_restart() {
                        eprintln!("crossload: {error}");
                    }
                    let needs: Vec<String> = missing.iter().map(lock::describe).collect();
                    return Ok(Some(format!(
                        "Settings saved. They need {}, which this run of crossload can't reach. \
                         Start crossload again to use them.",
                        needs.join(", ")
                    )));
                }
            }
        }
        let app = ambit_tui_native::run(app, ambit_tui_native::Options::default())?;
        match app.restart() {
            Some(saved) => tui = restart(saved)?,
            None => return Ok(None),
        }
    }
}

/// The terminal interface, and what it reaches: for the lock, and to check new settings against
/// it.
fn tui_app(tui: &Tui) -> Result<(crossload::Crossload, Vec<Grant>)> {
    let browse = absolute(&tui.browse);
    let output = absolute(&tui.output);
    let unavailable = vec![];
    if let Some(device) = &tui.device {
        outside_device(&output, device)?;
    }
    // The import folder is the one place crossload creates for itself, when the first book is
    // added to it.
    let import_exists = output.is_dir();
    let opened = crossload::tui::Opened {
        books: browse.is_dir(),
        import: import_exists,
        kobo: tui
            .device
            .as_ref()
            .map(|d| d.join(".kobo/KoboReader.sqlite").is_file()),
        card: tui.copy_to.as_ref().map(|c| c.is_dir()),
    };
    // The books folder is read and added to (archived ACSM requests). Opened on each use, so a
    // folder that isn't there (yet) still starts the interface and can be fixed in the settings.
    let books = NativeDir::mount(&browse);
    // The import folder is the one place crossload creates for itself, on the first book added.
    let import = NativeDir::creating(&output);
    let address = tui
        .send_to
        .as_deref()
        .and_then(|address| Address::parse(address, &tui.folder).ok());
    let (http, ws) = reader_scope(address.as_ref());
    let paths = &tui.paths;
    let (settings_dir, settings_file) = paths.settings()?;
    let options = crossload::tui::Options {
        config: paths.config.display().to_string(),
        home: paths.home.as_ref().map(|h| h.display().to_string()),
        first_run: tui.first_run,
        browse: browse.display().to_string(),
        output: output.display().to_string(),
        device: tui
            .device
            .as_ref()
            .map(|d| absolute(d).display().to_string()),
        send_to: tui.send_to.clone(),
        copy_to: tui
            .copy_to
            .as_ref()
            .map(|c| absolute(c).display().to_string()),
        folder: tui.folder.clone(),
        defaults: tui.defaults.clone(),
        kobos: detect_kobos(),
        opened,
        given: tui.given,
        color: std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty()),
    };
    let kobo = tui
        .device
        .as_ref()
        .map(|device| (NativeDir::mount(device), NativeKobo::new(device)));
    let card = tui.copy_to.as_ref().map(NativeDir::mount);
    let (control, files, upload) = (
        reader_control(http.clone()),
        reader_files(http),
        reader_upload(ws),
    );
    let adept = NativeAdept::new(&paths.adept);
    let (state, cache) = (own_folder(&paths.state).ok(), own_folder(&paths.cache).ok());

    // What it reaches. The folders it names, also before they are there (a card or Kobo not
    // plugged in yet, the import folder): a lock covers what's there when it's applied, so those
    // tell a restart whether the lock still covers them. The places devices appear while it runs.
    // And the temporary folder a Kobo's database is read through, so a Kobo chosen later can be.
    let mut grants = vec![
        Grant::Terminal,
        Grant::ReadWrite(browse.clone()),
        Grant::ReadWrite(output.clone()),
        Grant::ReadWrite(std::env::temp_dir()),
    ];
    grants.extend(lock::media());
    for path in tui.device.iter().chain(&tui.copy_to) {
        grants.push(Grant::ReadWrite(absolute(path)));
    }
    if let Some((dir, db)) = &kobo {
        grants.extend(dir.grants());
        grants.extend(db.grants());
    }
    for caps in [
        &books as &dyn Grants,
        &import,
        &card,
        &control,
        &files,
        &upload,
        &adept,
        &settings_dir,
        &state,
        &cache,
    ] {
        grants.extend(caps.grants());
    }

    let app = crossload::init(
        ambit_tui_native::StdClock::new(),
        ambit_tui_native::OsRandom,
        books,
        import,
        kobo,
        card,
        control,
        files,
        upload,
        adept,
        Some(settings_dir),
        state,
        cache,
        crossload::Config {
            options,
            settings: tui.settings.clone(),
            reader: tui.send_to.clone().map(|a| (a, tui.folder.clone())),
            websocket_port: WEBSOCKET_PORT,
            settings_file,
            unavailable,
        },
    );
    Ok((app, grants))
}

impl Tui {
    /// Where the command line's choices are. The books folder is where the program was started,
    /// unless given; the import folder is the books folder, unless given.
    fn new(paths: Paths, asked: crossload::cli::Tui) -> Result<Self> {
        let browse = match &asked.browse {
            Some(browse) => PathBuf::from(browse),
            None => std::env::current_dir()?,
        };
        Ok(Self {
            paths,
            output: asked
                .output
                .as_ref()
                .map_or_else(|| browse.clone(), PathBuf::from),
            browse,
            device: asked.device.as_ref().map(PathBuf::from),
            send_to: asked.send_to.clone(),
            copy_to: asked.copy_to.as_ref().map(PathBuf::from),
            folder: asked.folder.clone(),
            first_run: asked.first_run,
            given: asked.given(),
            defaults: asked.defaults,
            settings: asked.settings,
        })
    }
}

// The command line. What each kind of name on it opens as; the app opens only names the user
// typed, and the defaults saved for that kind.

/// The values given for `option` on the command line, as `--option value` or `--option=value`,
/// before any `--`. The settings and the activation are opened before the app runs, and the
/// output opener needs the Kobos, so the wiring reads these itself; the app declares them too.
fn typed(args: &Args, option: &str) -> Vec<String> {
    let mut values = vec![];
    let mut list = args.list().iter().skip(1);
    while let Some(arg) = list.next() {
        if arg == "--" {
            break;
        } else if arg == option {
            values.extend(list.next().cloned());
        } else if let Some(value) = arg
            .strip_prefix(option)
            .and_then(|rest| rest.strip_prefix('='))
        {
            values.push(value.to_owned());
        }
    }
    values
}

fn parent_of(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// A book to copy, send or optimize: that one file, read. The folder it is in stays out of reach.
fn book_file(value: &str) -> Result<OneFile, String> {
    let path = Path::new(value);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("Book filename must be UTF-8")?;
    let dir = folder(parent_of(path)).map_err(|e| format!("{e:#}"))?;
    OneFile::new(dir, name).map_err(|e| e.to_string())
}

/// An ACSM file: its folder, read and added to, because a fulfilled request is archived next to
/// it under a new name. Nothing in that folder is changed or removed.
fn acsm_request(value: &str) -> Result<Request, String> {
    let path = Path::new(value);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("ACSM filename must be UTF-8")?;
    let parent = parent_of(path);
    Ok(Request {
        folder: Arc::new(folder(parent).map_err(|e| format!("{e:#}"))?),
        name: RelPath::new([name]).map_err(|e| e.to_string())?,
        shown: parent.display().to_string(),
    })
}

/// `--browse`: the books folder, read and added to (archived ACSM requests). Opened on each use:
/// it may come and go.
fn books_folder(value: &str) -> Result<crossload::books::Folder, String> {
    Ok(crossload::books::Folder {
        display: absolute(Path::new(value)).display().to_string(),
        dir: Arc::new(NativeDir::mount(value)),
    })
}

/// `--output`: a folder books are added to, made on the first write. Refused inside any of
/// `kobos`, so books imported from a Kobo never land on it.
fn output_folder(
    kobos: Vec<PathBuf>,
) -> impl Fn(&str) -> Result<crossload::books::Folder, String> + Send + Sync + 'static {
    move |value| {
        for kobo in &kobos {
            outside_device(Path::new(value), kobo).map_err(|e| format!("{e:#}"))?;
        }
        Ok(crossload::books::Folder {
            display: absolute(Path::new(value)).display().to_string(),
            dir: Arc::new(NativeDir::creating(value)),
        })
    }
}

/// `--copy-to`, `copy --to`: the reader's mounted card, read and added to. Opened on each use;
/// the app asks whether it is there before copying. (Its type also allows removing, which the
/// terminal interface offers; no command removes anything.)
fn card(value: &str) -> Result<crossload::inventory::Card, String> {
    Ok(crossload::inventory::Card {
        display: absolute(Path::new(value)).display().to_string(),
        dir: Arc::new(NativeDir::mount(value)),
    })
}

/// `--device`: a mounted Kobo. Its mount, opened on each use (read; copies to the Kobo are
/// added), and its database, read.
fn kobo(value: &str) -> Result<crossload::kobo::Device, String> {
    Ok(crossload::kobo::Device {
        display: absolute(Path::new(value)).display().to_string(),
        dir: Arc::new(NativeDir::mount(value)),
        db: Arc::new(NativeKobo::new(value)),
    })
}

/// `--to`, `--send-to`: the reader at that address over Wi-Fi, scoped as `reader_scope` says.
fn reader(value: &str) -> Result<Link, String> {
    let address = Address::parse(value, "/").map_err(|e| format!("{e:#}"))?;
    let (http, ws) = reader_scope(Some(&address));
    Ok(Link {
        control: Arc::new(reader_control(http.clone())),
        files: Arc::new(reader_files(http)),
        upload: Arc::new(reader_upload(ws)),
        clock: Arc::new(ambit_cli_native::StdClock::new()),
        random: Arc::new(ambit_cli_native::OsRandom),
        websocket_port: WEBSOCKET_PORT,
    })
}

/// A catalogue's address, or a book's in one: HTTP to it, the saved catalogue and the Palace
/// Bookshelf, as `catalogues` scopes them.
fn catalogue(
    saved: Option<String>,
) -> impl Fn(&str) -> Result<Arc<dyn Http>, String> + Send + Sync + 'static {
    move |value| {
        let mut urls = vec![value, crossload::opds::BOOKSHELF];
        urls.extend(saved.as_deref());
        Ok(Arc::new(catalogues(&urls)) as Arc<dyn Http>)
    }
}

/// `adobe setup --from`: an exported activation folder or ZIP, read once.
fn activation(value: &str) -> Result<crossload_adobe::Activation, String> {
    crossload_adobe_native::read_activation(Path::new(value)).map_err(|e| e.0)
}

/// The command line's lock: what it may open, as each kind's opener opens it. Files, ACSM
/// requests and catalogues as the app's `Named` would open them (every word typed, since a
/// positional name could be any of them); the books folder, output, card, Kobo and reader only
/// as typed after their options or saved, since only those are opened as one. Crossload's own
/// folders and the activation. The folders the command writes to are made first. Not for the
/// terminal interface.
fn lock_cli(
    args: &Args,
    paths: Option<&Paths>,
    saved: &crossload::config::Defaults,
    kobos: &[PathBuf],
) -> Result<()> {
    let writes = crossload::cli::writes(args.list(), saved);
    if writes.tui {
        // Locked by `run_tui`, to what it opens: a lock can only narrow.
        return Ok(());
    }
    let given = |options: &[&str], saved: &[&Option<String>]| -> Vec<String> {
        options
            .iter()
            .flat_map(|option| typed(args, option))
            .chain(saved.iter().filter_map(|s| (*s).clone()))
            .collect()
    };
    let mut lock = Lockdown::new();

    lock = lock.allow(&args.named(book_file).open_all());
    // An ACSM's folder is opened for a file only.
    lock = lock.allow(
        &args
            .named(|value| match Path::new(value).is_file() {
                true => folder(parent_of(Path::new(value))).map_err(|e| e.to_string()),
                false => Err("not a file".into()),
            })
            .open_all(),
    );
    let saved_catalog = saved.catalog.clone();
    lock = lock.allow(
        &args
            .named(move |value| {
                let mut urls = vec![value, crossload::opds::BOOKSHELF];
                urls.extend(saved_catalog.as_deref());
                Ok(catalogues(&urls))
            })
            .also(saved.catalog.clone())
            .also([crossload::opds::BOOKSHELF])
            .open_all(),
    );
    for value in given(&["--from"], &[]) {
        lock = lock.allow(&Grant::Read(PathBuf::from(value)));
    }

    let mut browse = given(&["--browse"], &[&saved.browse]);
    browse.push(".".into());
    for value in browse {
        lock = lock.allow(&NativeDir::mount(value));
    }
    let output = output_folder(kobos.to_vec());
    for value in given(&["--output"], &[&saved.output]) {
        if output(&value).is_ok() {
            lock = lock.allow(&NativeDir::creating(value));
        }
    }
    for value in given(&["--copy-to", "--to"], &[&saved.copy_to]) {
        lock = lock.allow(&NativeDir::mount(value));
    }
    for value in given(&["--device"], &[&saved.device]) {
        lock = lock
            .allow(&NativeDir::mount(&value))
            .allow(&NativeKobo::new(&value));
    }
    for value in given(&["--to", "--send-to"], &[&saved.reader]) {
        if let Ok(address) = Address::parse(&value, "/") {
            let (http, ws) = reader_scope(Some(&address));
            lock = lock.allow(&reader_control(http)).allow(&reader_upload(ws));
        }
    }

    if let Some(paths) = paths {
        let parent = paths
            .config
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        for (dir, write) in [
            (parent, writes.settings),
            (&paths.state, writes.history),
            (&paths.cache, writes.cache),
        ] {
            lock = match write {
                true => lock.make(dir)?,
                false => lock.allow(&NativeDir::creating(dir)),
            };
        }
        if writes.activation {
            // Private, as the activation makes it.
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&paths.adept)
                .with_context(|| format!("Cannot create {}", paths.adept.display()))?;
        }
        lock = lock.allow(&NativeAdept::new(&paths.adept));
    }
    // Made only where the app would accept it: never inside a Kobo.
    if let Some(value) = writes.output.filter(|value| output(value).is_ok()) {
        lock = lock.make(value)?;
    }
    lock.apply_or_warn("crossload")?;
    Ok(())
}

fn main() -> ExitCode {
    // Before anything is opened: on Linux the program runs under a supervisor that can start it
    // again, locked afresh, when new settings need more than its lock holds.
    ambit_lockdown::restartable();
    let args = ambit_cli_native::args();
    let config = typed(&args, "--config").pop().map(PathBuf::from);
    let state_dir = typed(&args, "--state-dir").pop().map(PathBuf::from);
    let paths = Paths::new(config.clone(), state_dir.clone());
    let own = match &paths {
        Ok(paths) => paths.own().map_err(|e| format!("{e:#}")),
        Err(e) => Err(format!("{e:#}")),
    };
    let saved = own
        .as_ref()
        .map(|own| own.defaults.clone())
        .unwrap_or_default();
    // Saved defaults count as named, each for its own kind only. The books folder is where the
    // program was started, unless given; the Bookshelf is the catalogue unless one is given.
    let kobos: Vec<PathBuf> = typed(&args, "--device")
        .into_iter()
        .chain(saved.device.clone())
        .map(PathBuf::from)
        .collect();
    if let Err(error) = lock_cli(&args, paths.as_ref().ok(), &saved, &kobos) {
        eprintln!(
            "Error: {}",
            crossload::cli::printable(&format!("{error:#}"))
        );
        return ExitCode::FAILURE;
    }
    let app = crossload::cli::init(
        ambit_cli_native::StdClock::new(),
        args.named(book_file),
        args.named(acsm_request),
        args.named(books_folder)
            .also(saved.browse.clone())
            .also(["."]),
        args.named(output_folder(kobos)).also(saved.output.clone()),
        args.named(card).also(saved.copy_to.clone()),
        args.named(kobo).also(saved.device.clone()),
        args.named(reader).also(saved.reader.clone()),
        args.named(catalogue(saved.catalog.clone()))
            .also(saved.catalog.clone())
            .also([crossload::opds::BOOKSHELF]),
        args.named(activation),
        own,
    );
    match ambit_cli_native::run(app, &args) {
        Done::Exit(exit) => ExitCode::from(exit.0),
        Done::Tui(asked) => {
            let result = paths
                .and_then(|paths| Tui::new(paths, asked.clone()))
                .and_then(|tui| {
                    run_tui(tui, |saved| {
                        Tui::new(
                            Paths::new(config.clone(), state_dir.clone())?,
                            asked.restart(saved)?,
                        )
                    })
                });
            match result {
                Ok(None) => ExitCode::SUCCESS,
                Ok(Some(message)) => {
                    eprintln!("{message}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!(
                        "Error: {}",
                        crossload::cli::printable(&format!("{error:#}"))
                    );
                    ExitCode::FAILURE
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_kobo_mounts_without_guessing_from_volume_names() {
        let temp = tempfile::tempdir().unwrap();
        let mount = temp.path().join("My reader");
        std::fs::create_dir_all(mount.join(".kobo")).unwrap();
        std::fs::write(mount.join(".kobo/KoboReader.sqlite"), b"fixture").unwrap();
        std::fs::create_dir(temp.path().join("KOBOeReader")).unwrap();
        assert_eq!(
            detect_in(&[temp.path().into(), mount.clone()]),
            vec![mount.canonicalize().unwrap().display().to_string()]
        );
    }

    #[test]
    fn a_reader_by_ip_is_the_local_network_and_by_name_exactly_that_name() {
        let by_ip = Address::parse("192.168.1.7", "/").unwrap();
        let (http, ws) = reader_scope(Some(&by_ip));
        assert_eq!(http, Scope::new([Origin::lan(Scheme::Http, 80)]));
        assert_eq!(ws, Scope::new([Origin::lan(Scheme::Ws, 81)]));
        let by_name = Address::parse("crosspoint.local", "/").unwrap();
        let (http, ws) = reader_scope(Some(&by_name));
        assert_eq!(http, Scope::parse(["http://crosspoint.local:80"]).unwrap());
        assert_eq!(ws, Scope::parse(["ws://crosspoint.local:81"]).unwrap());
        assert_eq!(reader_scope(None), (Scope::default(), Scope::default()));
    }
}
