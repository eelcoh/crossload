// Wiring: which folders, devices and endpoints crossload gets, and with which scope. This file is
// the review surface for what the app can do; everything it does goes through `crossload::init`
// or, for the command line (`cli.rs`), through the app's functions with the capabilities built
// here.

mod cli;
mod mount;

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ambit_caps_dir::RelPath;
use ambit_caps_dir_native::NativeDir;
use ambit_caps_net::{Origin, Scheme, Scope};
use ambit_caps_net_native::{HttpOptions, NativeHttp, NativeWebSocket, WsOptions};
use anyhow::{Context, Result};
use crossload::crosspoint::Address;
use crossload_adobe_native::NativeAdept;
use crossload_kobo_native::NativeKobo;

use mount::Mount;

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

    fn settings_store(&self) -> Result<crossload::config::Store> {
        let (dir, file) = self.settings()?;
        Ok(crossload::config::Store {
            dir: Arc::new(dir),
            file: RelPath::new([file.as_str()])?,
            display: self.config.display().to_string(),
            home: self.home.as_ref().map(|h| h.display().to_string()),
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

/// What the terminal interface is started with.
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
}

/// Books imported from a Kobo must not land back on it. Checked through symlinks, on the part
/// of the path that exists, before anything is created.
fn outside_device(output: &Path, device: &Path) -> Result<()> {
    anyhow::ensure!(
        !output
            .components()
            .any(|c| c == std::path::Component::ParentDir),
        "Output path must not contain .."
    );
    let Ok(device) = device.canonicalize() else {
        // Not mounted: nothing can be written into it.
        return Ok(());
    };
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

/// Runs the terminal interface until it quits. When the user saved new settings, crossload starts
/// again as a new process with `restart`'s arguments, so the folders and devices come from the
/// saved settings. (A new process rather than a second `run`: ambit's native backend leaves its
/// input thread waiting on the terminal after `run` returns, and it would swallow the next key.)
fn run_tui(tui: Tui, restart: &[std::ffi::OsString]) -> Result<()> {
    use std::io::IsTerminal;
    anyhow::ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "The TUI needs an interactive terminal; use the CLI commands for redirected input/output"
    );
    let relaunch = Arc::new(AtomicBool::new(false));
    let app = tui_app(&tui, relaunch.clone())?;
    ambit_tui_native::run(app, ambit_tui_native::Options::default())?;
    if relaunch.load(Ordering::Relaxed) {
        use std::os::unix::process::CommandExt;
        let error = std::process::Command::new(std::env::current_exe()?)
            .args(restart)
            .exec();
        return Err(error).context("Cannot start crossload again with the saved settings");
    }
    Ok(())
}

fn tui_app(tui: &Tui, relaunch: Arc<AtomicBool>) -> Result<impl ambit_tui::App> {
    use crossload::Place;
    let browse = absolute(&tui.browse);
    let output = absolute(&tui.output);
    let mut unavailable = vec![];
    // The books folder is read and added to (archived ACSM requests).
    let books = folder(&browse);
    if let Some(device) = &tui.device {
        outside_device(&output, device)?;
    }
    // The import folder is the one place crossload creates for itself, when the first book is
    // added to it.
    let import_exists = output.is_dir();
    let opened = crossload::tui::Opened {
        books: books.is_ok(),
        import: import_exists,
        kobo: tui
            .device
            .as_ref()
            .map(|d| d.join(".kobo/KoboReader.sqlite").is_file()),
        card: tui.copy_to.as_ref().map(|c| c.is_dir()),
    };
    if let Err(e) = &books {
        unavailable.push((Place::Local, format!("{e:#}")));
    }
    // A missing books or import folder still starts the interface, so it can be fixed in the
    // settings: the folder is then the path, opened when used, and failing until it exists.
    let books: Arc<dyn ambit_caps_dir::DirWrite> = match books {
        Ok(dir) => Arc::new(dir),
        Err(_) => Arc::new(Mount::new(&browse)),
    };
    let import: Arc<dyn ambit_caps_dir::DirWrite> = Arc::new(Mount::creating(&output));
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
        color: std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty()),
    };
    Ok(crossload::init(
        ambit_tui_native::StdClock::new(),
        ambit_tui_native::OsRandom,
        books,
        import,
        tui.device
            .as_ref()
            .map(|device| (Mount::new(device), NativeKobo::new(device))),
        tui.copy_to.as_ref().map(Mount::new),
        reader_control(http.clone()),
        reader_files(http),
        reader_upload(ws),
        NativeAdept::new(&paths.adept),
        Some(settings_dir),
        own_folder(&paths.state).ok(),
        own_folder(&paths.cache).ok(),
        crossload::Config {
            options,
            settings: tui.settings.clone(),
            reader: tui.send_to.clone().map(|a| (a, tui.folder.clone())),
            websocket_port: WEBSOCKET_PORT,
            settings_file,
            unavailable,
            relaunch,
        },
    ))
}

fn main() -> std::process::ExitCode {
    match cli::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {}", cli::printable(&format!("{error:#}")));
            std::process::ExitCode::FAILURE
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
