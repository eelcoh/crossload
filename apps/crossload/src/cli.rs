//! The command line, on ambit's cli platform.
//!
//! Everything it may open comes in through [`init`], as a [`Named`] per kind: it opens what the
//! user typed (or saved as the default for that kind), as the wiring decides, and nothing else.
//! A book that a command makes and then sends on is read back through the folder it was made in,
//! never by a path. Output goes through [`Io`]: results to `out`, messages and progress to `err`.
mod args;
mod show;

use std::cell::RefCell;
use std::sync::Arc;

use ambit_caps::Clock;
use ambit_caps_dir::{Dir, DirErrorKind, DirWrite, OneFile, RelPath};
use ambit_caps_net::Http;
use ambit_cli::{App, Exit, Io, Named};
use anyhow::{Context, Result};
use crossload_adobe::{Activation, Adept};

use crate::books::{Folder, Library};
use crate::config::{self, required};
use crate::crosspoint::{Address, Link, Reader};
use crate::inventory::{Card, Destination};
use args::{
    AdobeCommand, CatalogCommand, Cmd, ConfigCommand, KoboCommand, LibraryArgs, Transfer,
    SAVED_COPY_DESTINATION,
};

/// What a run ends with: an exit code, or the terminal interface to start.
// One per run, so the size of the interface's settings is not worth a box.
#[allow(clippy::large_enum_variant)]
pub enum Done {
    Exit(Exit),
    Tui(Tui),
}

/// The terminal interface as the command line asked for it. Places not given are `None`: the
/// wiring knows where the program was started, which is where the books folder then is; the
/// import folder is the books folder unless given.
#[derive(Clone)]
pub struct Tui {
    pub defaults: config::Defaults,
    /// Neither the command line nor the settings say where imports go.
    pub first_run: bool,
    pub browse: Option<String>,
    pub output: Option<String>,
    pub device: Option<String>,
    pub send_to: Option<String>,
    pub copy_to: Option<String>,
    pub folder: String,
    pub settings: crate::books::Settings,
    optimize: bool,
}

impl Tui {
    #[allow(clippy::too_many_arguments)]
    fn new(
        defaults: config::Defaults,
        transfer: Transfer,
        device: Option<String>,
        browse: Option<String>,
        output: Option<String>,
        show_previews: bool,
        serial: Option<String>,
        optimize: bool,
        organized: bool,
    ) -> Result<Self> {
        let explicit_reader = transfer.send_to.is_some();
        Ok(Self {
            first_run: output.is_none() && defaults.output.is_none(),
            browse: browse.or(defaults.browse.clone()),
            output: output.or(defaults.output.clone()),
            device: device.or(defaults.device.clone()),
            send_to: transfer.send_to.or(defaults.reader.clone()),
            copy_to: transfer.copy_to.or_else(|| {
                if explicit_reader {
                    None
                } else {
                    defaults.copy_to.clone()
                }
            }),
            folder: transfer
                .folder
                .or(defaults.folder.clone())
                .unwrap_or_else(|| "/".to_owned()),
            settings: crate::books::Settings {
                show_previews,
                serial,
                optimize: optimize.then(|| defaults.screen()).transpose()?,
                organized,
                kepub: defaults.kepub.unwrap_or(false),
            },
            optimize,
            defaults,
        })
    }

    /// After a save, the saved settings are what count: the arguments that chose places give way
    /// to them; the rest are kept.
    pub fn restart(&self, saved: config::Defaults) -> Result<Self> {
        Self::new(
            saved,
            Transfer {
                send_to: None,
                copy_to: None,
                folder: None,
            },
            None,
            None,
            None,
            self.settings.show_previews,
            self.settings.serial.clone(),
            self.optimize,
            self.settings.organized,
        )
    }
}

/// An ACSM file named on the command line: the folder it's in, to read it and, once fulfilled,
/// to archive it beside it (add-only); its name there; and the folder as the user knows it.
pub struct Request {
    pub folder: Arc<dyn DirWrite>,
    pub name: RelPath,
    pub shown: String,
}

/// Crossload's own: the settings file (and what it says), the record of copies, the cache, and
/// ACSM fulfilment with the installed activation. The wiring reads the settings, since the
/// defaults in them count as named.
pub struct Own {
    pub settings: config::Store,
    pub defaults: config::Defaults,
    pub history: Option<crate::history::Record>,
    pub cache: Option<crate::cache::Store>,
    pub adept: Arc<dyn Adept>,
}

/// The capabilities, one `Named` per kind of name:
/// - `files`: a book named to copy, send or optimize: that one file, to read.
/// - `requests`: an ACSM file: its folder, to read it and add the archived copy.
/// - `books`: `--browse`, a folder to read (and add archived requests to).
/// - `outputs`: `--output`, a folder to add imported and prepared books to.
/// - `cards`: `--copy-to` and `copy --to`, a mounted card.
/// - `kobos`: `--device`, a mounted Kobo: its mount and its database.
/// - `readers`: `--to` and `--send-to`, the reader over Wi-Fi at that address.
/// - `catalogues`: an OPDS catalogue's address, or a book's address in one.
/// - `activations`: `adobe setup --from`, an exported activation, read.
/// - `own`: crossload's own files and its activation, or why they could not be had (then every
///   command fails with that, once the command line has been read).
#[allow(clippy::too_many_arguments)]
pub fn init(
    clock: impl Clock,
    files: Named<OneFile>,
    requests: Named<Request>,
    books: Named<Folder>,
    outputs: Named<Folder>,
    cards: Named<Card>,
    kobos: Named<crate::kobo::Device>,
    readers: Named<Link>,
    catalogues: Named<Arc<dyn Http>>,
    activations: Named<Activation>,
    own: Result<Own, String>,
) -> impl App<Done = Done> {
    Cli {
        clock: Arc::new(clock),
        files,
        requests,
        books,
        outputs,
        cards,
        kobos,
        readers,
        catalogues,
        activations,
        own,
    }
}

/// What a command line will write, for the wiring: on Linux it locks the program before the
/// command runs, and a lock covers only folders that are there, so it makes these first. Nothing
/// for a command line that asks for help or doesn't parse. `defaults` are the saved settings.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Writes {
    /// The terminal interface: the wiring opens and locks it itself.
    pub tui: bool,
    /// The settings file's folder.
    pub settings: bool,
    /// The record of copies.
    pub history: bool,
    pub cache: bool,
    /// The activation's state directory: setting it up, checking it (which makes it, as it always
    /// has) and fulfilling an ACSM.
    pub activation: bool,
    /// The folder books are added to, as typed or saved.
    pub output: Option<String>,
}

pub fn writes(args: &[String], defaults: &config::Defaults) -> Writes {
    let Ok(matches) = args::command().try_get_matches_from(args) else {
        return Writes::default();
    };
    let output = |typed: Option<String>| typed.or(defaults.output.clone());
    let mut writes = Writes::default();
    match args::parse(&matches).command {
        Cmd::Tui { .. } => writes.tui = true,
        Cmd::Config(ConfigCommand::Set { .. } | ConfigCommand::Unset { .. }) => {
            writes.settings = true
        }
        // Listing the library keeps what it learnt about each book.
        Cmd::Books { .. } => writes.cache = true,
        // Copying to the computer, or converting a PDF on the way to the reader, adds to the
        // import folder; every copy is recorded.
        Cmd::Sync { library, apply, .. } => {
            writes.cache = true;
            if apply {
                writes.history = true;
                writes.output = output(library.output);
            }
        }
        Cmd::Optimize { output: typed, .. }
        | Cmd::Catalog(CatalogCommand::Get { output: typed, .. })
        | Cmd::Kobo(KoboCommand::Import { output: typed, .. }) => writes.output = output(typed),
        Cmd::Import { output: typed, .. } => {
            writes.output = output(typed);
            writes.activation = true;
        }
        Cmd::Adobe(_) => writes.activation = true,
        Cmd::Kobo(KoboCommand::Sync {
            output: typed,
            apply: true,
            ..
        }) => writes.output = output(typed),
        // Notes go to a folder only when one is typed (the saved one isn't used), and not as JSON.
        Cmd::Kobo(KoboCommand::Notes {
            output: typed,
            json: false,
            ..
        }) => writes.output = typed,
        Cmd::Config(ConfigCommand::Show)
        | Cmd::History { .. }
        | Cmd::Copy { .. }
        | Cmd::Send { .. }
        | Cmd::Catalog(_)
        | Cmd::Kobo(_) => {}
    }
    writes
}

struct Cli {
    clock: Arc<dyn Clock>,
    files: Named<OneFile>,
    requests: Named<Request>,
    books: Named<Folder>,
    outputs: Named<Folder>,
    cards: Named<Card>,
    kobos: Named<crate::kobo::Device>,
    readers: Named<Link>,
    catalogues: Named<Arc<dyn Http>>,
    activations: Named<Activation>,
    own: Result<Own, String>,
}

impl App for Cli {
    type Done = Done;

    fn run(self, args: &[String], io: &mut Io<'_>) -> Done {
        let matches = match args::command().try_get_matches_from(args) {
            Ok(matches) => matches,
            // Help and --version are answers, to stdout; a wrong command line is an error.
            Err(e) if e.use_stderr() => {
                let _ = write!(io.err, "{}", e.render());
                return Done::Exit(Exit::USAGE);
            }
            Err(e) => {
                let _ = write!(io.out, "{}", e.render());
                return Done::Exit(Exit::SUCCESS);
            }
        };
        match self.command(args::parse(&matches), io) {
            Ok(Some(tui)) => Done::Tui(tui),
            Ok(None) => Done::Exit(Exit::SUCCESS),
            Err(error) => {
                let _ = writeln!(io.err, "Error: {}", printable(&format!("{error:#}")));
                Done::Exit(Exit::FAILURE)
            }
        }
    }
}

/// Control characters from a book's metadata or a file name become spaces, so nothing printed
/// can move the cursor or change the terminal.
pub fn printable(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Writes a line, as `println!` would. A closed pipe is not worth failing the command over:
/// nothing is reading any more.
macro_rules! say {
    ($to:expr, $($arg:tt)*) => {{
        let _ = writeln!($to, $($arg)*);
    }};
}

/// `base/path`, as `Path::join` shows it.
fn join(base: &str, path: &str) -> String {
    if base.is_empty() {
        path.to_owned()
    } else if base.ends_with('/') {
        format!("{base}{path}")
    } else {
        format!("{base}/{path}")
    }
}

/// The last part of a path as typed, as `Path::file_name` has it: `None` for a folder's `..`, a
/// bare `/` or `.`.
fn file_name(path: &str) -> Option<&str> {
    let last = path
        .split('/')
        .rfind(|part| !part.is_empty() && *part != ".")?;
    (last != "..").then_some(last)
}

/// Whether a folder from the command line is there now, as a folder.
fn present(dir: &dyn Dir) -> bool {
    dir.metadata(&RelPath::root()).is_ok_and(|meta| meta.is_dir)
}

/// A name the user gave, opened. The opener's own message, if it couldn't be.
fn opened<T>(named: &Named<T>, value: &str) -> Result<T> {
    named.open(value).map_err(|e| anyhow::anyhow!("{e}"))
}

/// Makes `folder` (named `path`) now, through the capability, if it isn't there.
fn make(folder: Folder, path: &str) -> Result<Folder> {
    match folder.dir.create_dir(&RelPath::root()) {
        Ok(()) => Ok(folder),
        Err(e) if e.kind == DirErrorKind::Exists => Ok(folder),
        Err(e) => Err(anyhow::Error::new(e).context(format!("Cannot create {path}"))),
    }
}

/// The book a command made, from the folder it made it in: its name and its bytes.
fn made(folder: &Folder, path: &RelPath) -> Result<(String, Vec<u8>)> {
    let data = crate::files::book(&*folder.dir, path)
        .with_context(|| format!("Cannot read {}", join(&folder.display, &path.to_string())))?;
    let name = path.parts().last().cloned().unwrap_or_default();
    Ok((name, data))
}

fn screen(
    defaults: &config::Defaults,
    no_optimize: bool,
) -> Result<Option<crate::profile::Profile>> {
    (!no_optimize).then(|| defaults.screen()).transpose()
}

impl Cli {
    fn own(&self) -> Result<&Own> {
        self.own.as_ref().map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// A folder to add books to, named with `--output` (or saved).
    fn output(&self, path: &str) -> Result<Folder> {
        opened(&self.outputs, path)
    }

    /// A folder to add books to, made now if it isn't there: for the commands that write into it
    /// straight away, which check it is there first.
    fn made_output(&self, path: &str) -> Result<Folder> {
        make(self.output(path)?, path)
    }

    /// A mounted card. Whether it is there is asked when it matters, with that command's words.
    fn card(&self, path: &str) -> Result<Card> {
        opened(&self.cards, path)
    }

    /// The reader over Wi-Fi at `address`, in `folder`.
    fn reader(&self, address: &str, folder: &str) -> Result<Reader> {
        let parsed = Address::parse(address, folder)?;
        Ok(Reader::new(parsed, opened(&self.readers, address)?))
    }

    /// A Kobo that must be there now.
    fn kobo(&self, path: &str) -> Result<crate::kobo::Device> {
        let device = opened(&self.kobos, path)?;
        anyhow::ensure!(
            present(&*device.dir),
            "Cannot open Kobo at {path}; pass the Kobo mount directory with --device"
        );
        Ok(device)
    }

    /// A book named on the command line: its file name, and its bytes.
    fn read_book(&self, path: &str) -> Result<(String, Vec<u8>)> {
        let file = opened(&self.files, path)?;
        let data = crate::files::book(&file, &file.path())
            .with_context(|| format!("Cannot read {path}"))?;
        Ok((file.name().to_owned(), data))
    }

    fn resolve(&self, mut transfer: Transfer, defaults: &config::Defaults) -> Result<Transfer> {
        if transfer.send_to.as_deref() == Some("") {
            transfer.send_to = Some(required(
                None,
                defaults.reader.clone(),
                "--send-to/--reader",
            )?);
        }
        if transfer.copy_to.as_deref() == Some(SAVED_COPY_DESTINATION) {
            transfer.copy_to = Some(required(None, defaults.copy_to.clone(), "--copy-to")?);
        }
        transfer.folder = transfer.folder.or_else(|| defaults.folder.clone());
        Ok(transfer)
    }

    /// The reader an imported book is sent to, if any; a card to copy it to must be there.
    fn destination_reader(&self, transfer: &Transfer) -> Result<Option<Reader>> {
        if let Some(destination) = &transfer.copy_to {
            anyhow::ensure!(
                present(&*self.card(destination)?.dir),
                "--copy-to requires an existing directory on the mounted card"
            );
        }
        transfer
            .send_to
            .as_deref()
            .map(|address| self.reader(address, transfer.folder.as_deref().unwrap_or("/")))
            .transpose()
    }

    fn library(
        &self,
        args: LibraryArgs,
        defaults: &config::Defaults,
        optimize: bool,
        organized: bool,
    ) -> Result<Library> {
        let own = self.own()?;
        let transfer = self.resolve(args.transfer, defaults)?;
        let explicit_reader = transfer.send_to.is_some();
        let local = args
            .browse
            .or(defaults.browse.clone())
            .unwrap_or_else(|| ".".to_owned());
        let output = required(args.output, defaults.output.clone(), "--output")?;
        let kobo = args.device.or(defaults.device.clone());
        let reader_address = transfer.send_to.or(defaults.reader.clone());
        let card = transfer.copy_to.or_else(|| {
            if explicit_reader {
                None
            } else {
                defaults.copy_to.clone()
            }
        });
        let reader_folder = transfer
            .folder
            .or(defaults.folder.clone())
            .unwrap_or_else(|| "/".to_owned());
        let settings = crate::books::Settings {
            show_previews: args.show_previews,
            serial: args.serial,
            optimize: optimize.then(|| defaults.screen()).transpose()?,
            organized,
            kepub: defaults.kepub.unwrap_or(false),
        };
        let mut unavailable = vec![];
        let reader = match (card, reader_address) {
            (Some(card), _) => Some(Destination::Card(self.card(&card)?)),
            (None, Some(address)) => match self.reader(&address, &reader_folder) {
                Ok(reader) => Some(Destination::Reader(reader)),
                Err(e) => {
                    unavailable.push((crate::Place::CrossPoint, format!("{e:#}")));
                    None
                }
            },
            (None, None) => None,
        };
        Ok(Library {
            settings,
            books: opened(&self.books, &local)?,
            import: self.output(&output)?,
            kobo: kobo.map(|path| opened(&self.kobos, &path)).transpose()?,
            reader,
            adept: own.adept.clone(),
            cache: own.cache.clone(),
            history: own.history.clone(),
            clock: self.clock.clone(),
            unavailable,
        })
    }

    /// Runs one command: `Some` when it is the terminal interface, for the wiring to start.
    fn command(&self, cli: args::Cli, io: &mut Io<'_>) -> Result<Option<Tui>> {
        let own = self.own()?;
        let store = &own.settings;
        let mut defaults = own.defaults.clone();
        let optimize = !cli.no_optimize;
        let organized = !cli.flat;
        match cli.command {
            Cmd::Config(command) => match command {
                ConfigCommand::Show => {
                    say!(io.out, "{}", serde_json::to_string_pretty(&defaults)?);
                    say!(io.err, "Defaults: {}", printable(&store.display));
                }
                ConfigCommand::Set { values } => {
                    defaults.merge(values);
                    defaults.save(store)?;
                    say!(io.out, "Saved {}", printable(&store.display));
                }
                ConfigCommand::Unset { key } => {
                    defaults.unset(&key)?;
                    defaults.save(store)?;
                    say!(io.out, "Removed {key}");
                }
            },
            Cmd::Tui {
                show_previews,
                device,
                browse,
                output,
                serial,
                transfer,
            } => {
                let transfer = self.resolve(transfer, &defaults)?;
                return Tui::new(
                    defaults,
                    transfer,
                    device,
                    browse,
                    output,
                    show_previews,
                    serial,
                    optimize,
                    organized,
                )
                .map(Some);
            }
            Cmd::History {
                count,
                failed,
                json,
            } => {
                // Ask for more than wanted when filtering, so a run of successes
                // does not hide the failures behind them.
                let asked = if failed {
                    count.saturating_mul(50)
                } else {
                    count
                };
                let mut entries = crate::history::read(own.history.as_ref(), asked.max(count));
                if failed {
                    entries.retain(|entry| !entry.ok);
                }
                let entries = &entries[entries.len().saturating_sub(count)..];
                if json {
                    say!(io.out, "{}", serde_json::to_string_pretty(entries)?);
                } else if entries.is_empty() {
                    say!(
                        io.out,
                        "Nothing copied yet, or the record has not been kept."
                    );
                } else {
                    let mut rows =
                        show::Rows::new(io, &["WHEN", "TITLE", "AUTHOR", "FROM", "TO", "RESULT"]);
                    for entry in entries {
                        rows.row(
                            io,
                            [
                                crate::history::stamp(entry.at),
                                printable(&entry.title),
                                printable(&entry.author),
                                entry.from.label().to_owned(),
                                entry.to.label().to_owned(),
                                if entry.ok {
                                    "copied".to_owned()
                                } else {
                                    format!("failed: {}", printable(&entry.detail))
                                },
                            ],
                        );
                    }
                    rows.finish(io);
                }
            }
            Cmd::Books { library, json } => {
                let library = self.library(library, &defaults, optimize, organized)?;
                let snapshot = library.scan();
                show::locations(io, &snapshot);
                if json {
                    say!(io.out, "{}", serde_json::to_string_pretty(&snapshot.books)?);
                } else {
                    show::catalog(io, &snapshot);
                }
            }
            Cmd::Sync {
                from,
                to,
                library,
                apply,
                json,
            } => {
                anyhow::ensure!(from != to, "--from and --to must differ");
                let library = self.library(library, &defaults, optimize, organized)?;
                let (from, to) = (from.place(), to.place());
                let snapshot = library.scan();
                show::locations(io, &snapshot);
                for place in [from, to] {
                    anyhow::ensure!(
                        snapshot.ready(place),
                        "{} is unavailable, so nothing can be planned for it",
                        place.label()
                    );
                }
                // Unreadable copies never hashed, so they cannot be a source.
                let planned: Vec<_> = snapshot
                    .books
                    .iter()
                    .filter(|book| book.has(from) && !book.has(to))
                    .filter(|book| book.preferred().is_some_and(|c| !c.sha.is_empty()))
                    .collect();
                let mut done = Vec::new();
                for (index, book) in planned.iter().enumerate() {
                    let source = book.preferred().map(|c| c.place).unwrap_or(from);
                    let outcome = if apply {
                        // Live progress goes to stderr, so stdout stays the result.
                        say!(
                            io.err,
                            "[{}/{}] {} — copying to {}",
                            index + 1,
                            planned.len(),
                            printable(&book.title),
                            to.label()
                        );
                        let err = RefCell::new(&mut *io.err);
                        crate::books::transfer(&library, book, to, &|text| {
                            if text.starts_with("Note:") {
                                say!(err.borrow_mut(), "{}", printable(text));
                            }
                        })
                        .map_err(|e| format!("{e:#}"))
                    } else {
                        Ok(String::new())
                    };
                    if let Err(e) = &outcome {
                        say!(io.err, "      failed: {}", printable(e));
                    }
                    done.push((book, source, outcome));
                }
                if json {
                    let rows: Vec<_> = done
                        .iter()
                        .map(|(book, source, outcome)| {
                            serde_json::json!({
                                "title": book.title,
                                "author": book.author,
                                "from": source.label(),
                                "to": to.label(),
                                "applied": apply,
                                "result": outcome.as_ref().ok(),
                                "error": outcome.as_ref().err(),
                            })
                        })
                        .collect();
                    say!(io.out, "{}", serde_json::to_string_pretty(&rows)?);
                } else {
                    show::plan(io, &done, to, apply);
                }
                let failures = done.iter().filter(|(_, _, r)| r.is_err()).count();
                anyhow::ensure!(failures == 0, "{failures} of {} copies failed", done.len());
            }
            Cmd::Optimize { book, output } => {
                // Optimizing means rebuilding an EPUB. Handing back an untouched
                // PDF under the name "device copy" would be a misleading success.
                anyhow::ensure!(
                    file_name(&book)
                        .and_then(crate::format::Format::of)
                        .is_none_or(|format| format.rewritten()),
                    "Only EPUB is optimized for the reader; copy a PDF or CBZ as it is with crossload copy"
                );
                let output =
                    self.made_output(&required(output, defaults.output.clone(), "--output")?)?;
                let (name, data) = self.read_book(&book)?;
                let prepared = prepare(
                    io,
                    name,
                    data,
                    screen(&defaults, cli.no_optimize)?,
                    organized,
                )?;
                let result = crate::copy::copy(
                    &prepared.data,
                    &prepared.name,
                    &*output.dir,
                    &RelPath::root(),
                )?;
                say!(
                    io.out,
                    "Device copy: {}",
                    printable(&format!("{}/{}", output.display, result.path))
                );
            }
            Cmd::Copy { book, to } => {
                let destination = required(to, defaults.copy_to.clone(), "--to/--copy-to")?;
                let card = self.copy_destination(&destination)?;
                let (name, data) = self.read_book(&book)?;
                copy(
                    io,
                    name,
                    data,
                    &card,
                    &destination,
                    screen(&defaults, cli.no_optimize)?,
                    organized,
                )?;
            }
            Cmd::Import {
                acsm,
                output,
                transfer,
            } => {
                let output = required(output, defaults.output.clone(), "--output")?;
                let transfer = self.resolve(transfer, &defaults)?;
                let reader = self.destination_reader(&transfer)?;
                let output = self.made_output(&output)?;
                let book = self.import_acsm(io, &acsm, &output)?;
                self.pass_on(
                    io,
                    &output,
                    &book,
                    reader,
                    transfer,
                    &defaults,
                    cli.no_optimize,
                    organized,
                )?;
            }
            Cmd::Send {
                book,
                to,
                folder,
                repair,
            } => {
                let to = required(to, defaults.reader.clone(), "--to/--reader")?;
                let folder = folder
                    .or(defaults.folder.clone())
                    .unwrap_or_else(|| "/".to_owned());
                let reader = self.reader(&to, &folder)?.repair_incomplete(repair);
                send(
                    io,
                    &reader,
                    || self.read_book(&book),
                    &book,
                    screen(&defaults, cli.no_optimize)?,
                    organized,
                )?;
            }
            Cmd::Catalog(command) => {
                self.catalog(io, command, &defaults, cli.no_optimize, organized)?
            }
            Cmd::Adobe(command) => match command {
                AdobeCommand::Setup { from } => {
                    let activation = opened(&self.activations, &from)?;
                    own.adept
                        .import_activation(&activation)
                        .map_err(|e| anyhow::anyhow!(e.0))?;
                    say!(
                        io.out,
                        "Activation imported and checked. No server was contacted."
                    );
                }
                AdobeCommand::Status => {
                    if own.adept.status().map_err(|e| anyhow::anyhow!(e.0))? {
                        say!(
                            io.out,
                            "Activation installed and locally valid. Native libgourou backend ready."
                        );
                    } else {
                        say!(
                            io.out,
                            "No activation installed. Run crossload adobe setup --from <directory or ZIP>."
                        );
                    }
                }
            },
            Cmd::Kobo(command) => {
                self.kobo_command(io, command, &defaults, cli.no_optimize, cli.flat)?
            }
        }
        Ok(None)
    }

    /// The card `copy` copies to, which must be there.
    fn copy_destination(&self, destination: &str) -> Result<Card> {
        let card = self.card(destination)?;
        anyhow::ensure!(
            present(&*card.dir),
            "Destination must be an existing mounted-card directory"
        );
        Ok(card)
    }

    /// Sends or copies a book a command just made in `output`, as `--send-to` or `--copy-to`
    /// asked.
    #[allow(clippy::too_many_arguments)]
    fn pass_on(
        &self,
        io: &mut Io<'_>,
        output: &Folder,
        book: &RelPath,
        reader: Option<Reader>,
        transfer: Transfer,
        defaults: &config::Defaults,
        no_optimize: bool,
        organized: bool,
    ) -> Result<()> {
        let shown = join(&output.display, &book.to_string());
        if let Some(reader) = reader {
            let optimize = screen(defaults, no_optimize)?;
            send(
                io,
                &reader,
                || made(output, book),
                &shown,
                optimize,
                organized,
            )?;
        }
        if let Some(destination) = transfer.copy_to {
            let card = self.copy_destination(&destination)?;
            let (name, data) = made(output, book)?;
            copy(
                io,
                name,
                data,
                &card,
                &destination,
                screen(defaults, no_optimize)?,
                organized,
            )?;
        }
        Ok(())
    }

    fn catalog(
        &self,
        io: &mut Io<'_>,
        command: CatalogCommand,
        defaults: &config::Defaults,
        no_optimize: bool,
        organized: bool,
    ) -> Result<()> {
        match command {
            CatalogCommand::Browse { url, all, pages } => {
                let start = url
                    .or_else(|| defaults.catalog.clone())
                    .unwrap_or_else(|| crate::opds::BOOKSHELF.to_owned());
                let http = opened(&self.catalogues, &start)?;
                show::feed(io, &*http, &start, all, pages)?;
            }
            CatalogCommand::Search {
                query,
                url,
                all,
                pages,
            } => {
                let start = url
                    .or_else(|| defaults.catalog.clone())
                    .unwrap_or_else(|| crate::opds::BOOKSHELF.to_owned());
                let http = opened(&self.catalogues, &start)?;
                let feed = crate::opds::fetch(&*http, &start)?;
                let description = feed.search.with_context(|| {
                    format!("{} does not offer a search", printable(&feed.title))
                })?;
                show::feed(
                    io,
                    &*http,
                    &crate::opds::search(&*http, &description, &query)?,
                    all,
                    pages,
                )?;
            }
            CatalogCommand::Get {
                acquisition,
                output,
                transfer,
            } => {
                let output =
                    self.made_output(&required(output, defaults.output.clone(), "--output")?)?;
                let transfer = self.resolve(transfer, defaults)?;
                let reader = self.destination_reader(&transfer)?;
                let http = opened(&self.catalogues, &acquisition)?;
                let data = crate::opds::download(
                    &*http,
                    &acquisition,
                    crate::epub::MAX_BOOK_BYTES as usize,
                )?;
                // What arrived decides what it is. A catalogue's declared type
                // is a promise, and the bytes are the fact.
                let format = crate::format::Format::ALL
                    .into_iter()
                    .find(|&format| crate::format::validate(format, &data).is_ok())
                    .context(
                        "The catalogue returned something that is not an EPUB, PDF or CBZ. \
                         A borrow link needs a library card, which this does not yet carry",
                    )?;
                let book = crate::opds::publish(&data, format, &*output.dir)?;
                say!(
                    io.out,
                    "Downloaded {}",
                    printable(&join(&output.display, &book.to_string()))
                );
                self.pass_on(
                    io,
                    &output,
                    &book,
                    reader,
                    transfer,
                    defaults,
                    no_optimize,
                    organized,
                )?;
            }
        }
        Ok(())
    }

    fn import_acsm(&self, io: &mut Io<'_>, acsm: &str, output: &Folder) -> Result<RelPath> {
        let request = opened(&self.requests, acsm)?;
        let data = crate::files::read(&*request.folder, &request.name, 8 << 20)?;
        let book = crate::acsm::import(&*self.own()?.adept, output, &data)?;
        say!(
            io.out,
            "Imported {}",
            printable(&join(&output.display, &book.to_string()))
        );
        // The fulfillment is done; archiving the spent request is a courtesy.
        match crate::acsm::archive(&*request.folder, &request.name, &data) {
            Ok(archived) => say!(
                io.out,
                "Archived {}",
                printable(&join(&request.shown, &archived.to_string()))
            ),
            Err(e) => say!(
                io.err,
                "Book imported; the ACSM stays pending: {}",
                printable(&format!("{e:#}"))
            ),
        }
        Ok(book)
    }

    fn kobo_command(
        &self,
        io: &mut Io<'_>,
        command: KoboCommand,
        defaults: &config::Defaults,
        no_optimize: bool,
        flat: bool,
    ) -> Result<()> {
        let optimize = || screen(defaults, no_optimize);
        match command {
            KoboCommand::Sync {
                exclude,
                device,
                output,
                to,
                copy_to,
                folder: reader_folder,
                serial,
                apply,
                repair,
                json,
            } => {
                let library = self.kobo(&required(device, defaults.device.clone(), "--device")?)?;
                let output = required(output, defaults.output.clone(), "--output")?;
                let destination = if let Some(path) = copy_to {
                    let card = self.card(&path)?;
                    anyhow::ensure!(
                        present(&*card.dir),
                        "Select an existing mounted-card directory"
                    );
                    Destination::Card(card)
                } else {
                    let address = required(
                        to,
                        defaults.reader.clone(),
                        "--to/--reader (or select --copy-to)",
                    )?;
                    Destination::Reader(
                        self.reader(
                            &address,
                            &reader_folder
                                .or(defaults.folder.clone())
                                .unwrap_or_else(|| "/".into()),
                        )?,
                    )
                };
                let options = crate::sync::Options {
                    output: self.output(&output)?,
                    serial,
                    apply,
                    optimize: optimize()?,
                    organized: !flat,
                    repair,
                    exclude,
                };
                let err = &mut *io.err;
                let report = crate::sync::run(&library, &destination, &options, |progress| {
                    say!(err, "{}", printable(progress))
                })?;
                if json {
                    say!(io.out, "{}", serde_json::to_string_pretty(&report)?);
                } else {
                    say!(io.out, "STATUS\tTITLE\tID\tDESTINATION\tDETAIL");
                    for row in &report.books {
                        let status = serde_json::to_value(&row.status)?;
                        say!(
                            io.out,
                            "{}\t{}\t{}\t{}\t{}",
                            status.as_str().unwrap_or("unknown"),
                            printable(&row.title),
                            printable(&row.id),
                            printable(row.path.as_deref().unwrap_or("")),
                            printable(&row.detail)
                        );
                    }
                }
                say!(
                    io.err,
                    "{}: {} books; {} previews skipped; {} excluded; {} failures.{}",
                    if apply { "Sync" } else { "Dry run" },
                    report.books.len(),
                    report.previews_skipped,
                    report.excluded,
                    report.failures(),
                    if apply {
                        ""
                    } else {
                        " Use --apply to transfer missing books."
                    }
                );
                anyhow::ensure!(
                    report.failures() == 0,
                    "{} Resolve the reported issues and rerun.",
                    if apply {
                        "Sync has failures; successful copies are retained."
                    } else {
                        "Dry run has conflicts or failures; no books were transferred."
                    }
                );
                if apply && matches!(destination, Destination::Card(_)) {
                    say!(io.err, "Safely eject the card before disconnecting it.");
                }
            }
            KoboCommand::Shelves { device, json } => {
                let library = self.kobo(&required(device, defaults.device.clone(), "--device")?)?;
                let shelves = library.shelves()?;
                if json {
                    say!(io.out, "{}", serde_json::to_string_pretty(&shelves)?);
                } else if shelves.is_empty() {
                    say!(io.err, "No collections on this Kobo.");
                } else {
                    let mut rows = show::Rows::new(io, &["COLLECTION", "TITLE", "AUTHOR"]);
                    for shelf in &shelves {
                        if shelf.books.is_empty() {
                            rows.row(
                                io,
                                [printable(&shelf.name), "(empty)".to_owned(), String::new()],
                            );
                        }
                        for (index, book) in shelf.books.iter().enumerate() {
                            // The collection is named once and then implied,
                            // so a long shelf reads as one block.
                            rows.row(
                                io,
                                [
                                    if index == 0 {
                                        printable(&shelf.name)
                                    } else {
                                        String::new()
                                    },
                                    printable(&book.title),
                                    printable(&book.author),
                                ],
                            );
                        }
                    }
                    rows.finish(io);
                    say!(
                        io.err,
                        "{} collections, {} books.",
                        shelves.len(),
                        shelves.iter().map(|s| s.books.len()).sum::<usize>()
                    );
                }
            }
            KoboCommand::Notes {
                device,
                output,
                json,
            } => {
                let library = self.kobo(&required(device, defaults.device.clone(), "--device")?)?;
                let notes = library.annotations()?;
                if json {
                    say!(io.out, "{}", serde_json::to_string_pretty(&notes)?);
                } else if notes.is_empty() {
                    say!(io.err, "No highlights or notes on this Kobo.");
                } else {
                    // Grouped by book, in the order the query returned them.
                    let mut books: Vec<(&str, &str)> = vec![];
                    for note in &notes {
                        if !books.contains(&note.book()) {
                            books.push(note.book());
                        }
                    }
                    let directory = output.as_deref().map(|o| self.made_output(o)).transpose()?;
                    for (title, author) in books {
                        let marks: Vec<_> = notes
                            .iter()
                            .filter(|note| note.book() == (title, author))
                            .collect();
                        let text = crate::kobo::as_markdown(title, author, &marks);
                        match (&directory, &output) {
                            (Some(dir), Some(shown)) => {
                                let name = crate::prepare::component(title, "Untitled");
                                let path = join(shown, &format!("{name}.md"));
                                // A reader's own notes are not ours to overwrite either.
                                dir.dir
                                    .create_new(&RelPath::new([format!("{name}.md")])?, text.as_bytes())
                                    .map_err(|e| {
                                        anyhow::anyhow!(
                                            "Cannot write {path}: {e}; existing files are never overwritten"
                                        )
                                    })?;
                                say!(io.err, "{}", printable(&path));
                            }
                            _ => say!(io.out, "{text}"),
                        }
                    }
                }
            }
            KoboCommand::List {
                device,
                json,
                shelf,
                show_previews,
            } => {
                let library = self.kobo(&required(device, defaults.device.clone(), "--device")?)?;
                // A named collection that is not there is a mistake worth
                // saying, rather than an empty listing that looks like a Kobo
                // with nothing on it.
                let on_shelf = match &shelf {
                    None => None,
                    Some(wanted) => {
                        let shelves = library.shelves()?;
                        let found = shelves
                            .iter()
                            .find(|s| s.name.eq_ignore_ascii_case(wanted))
                            .with_context(|| {
                                format!(
                                    "This Kobo has no collection called {wanted}. It has: {}",
                                    match shelves.len() {
                                        0 => "none".to_owned(),
                                        _ => shelves
                                            .iter()
                                            .map(|s| s.name.as_str())
                                            .collect::<Vec<_>>()
                                            .join(", "),
                                    }
                                )
                            })?;
                        Some(
                            found
                                .books
                                .iter()
                                .map(|book| book.id.clone())
                                .collect::<std::collections::HashSet<_>>(),
                        )
                    }
                };
                let books: Vec<_> = library
                    .books()?
                    .into_iter()
                    .filter(|book| show_previews || !book.preview)
                    .filter(|book| on_shelf.as_ref().is_none_or(|ids| ids.contains(&book.id)))
                    .collect();
                if json {
                    say!(io.out, "{}", serde_json::to_string_pretty(&books)?);
                } else {
                    if books.is_empty() {
                        say!(
                            io.err,
                            "No downloaded books to show. Use --show-previews to include previews."
                        );
                    }
                    show::books(io, &books);
                }
            }
            KoboCommand::Import {
                book_id,
                device,
                output,
                serial,
                transfer,
            } => {
                let output = required(output, defaults.output.clone(), "--output")?;
                let transfer = self.resolve(transfer, defaults)?;
                let reader = self.destination_reader(&transfer)?;
                let device_path = required(device, defaults.device.clone(), "--device")?;
                let library = self.kobo(&device_path)?;
                // The opener refuses a folder inside a Kobo named on the command line or saved.
                let folder = self.output(&output)?;
                let (name, data) = library.import(&book_id, serial.as_deref())?;
                let folder = make(folder, &output)?;
                let book = crate::kobo::publish(&*folder.dir, &name, &data, false)?;
                say!(
                    io.out,
                    "Imported {}",
                    printable(&join(&folder.display, &book.to_string()))
                );
                self.pass_on(
                    io,
                    &folder,
                    &book,
                    reader,
                    transfer,
                    defaults,
                    no_optimize,
                    !flat,
                )?;
            }
        }
        Ok(())
    }
}

fn prepare(
    io: &mut Io<'_>,
    name: String,
    data: Vec<u8>,
    optimize: Option<crate::profile::Profile>,
    organized: bool,
) -> Result<crate::prepare::Prepared> {
    let prepared = crate::prepare::prepare(data, &name, optimize, organized)?;
    if let Some(screen) = optimize {
        say!(
            io.err,
            "Device copy: {} → {} bytes; {} images optimized for {} ({}×{}).",
            prepared.original_bytes,
            prepared.bytes,
            prepared.images,
            screen.device,
            screen.width,
            screen.height
        );
    }
    Ok(prepared)
}

/// Copies a book onto a card; `destination` is the card as the user named it.
fn copy(
    io: &mut Io<'_>,
    name: String,
    data: Vec<u8>,
    card: &Card,
    destination: &str,
    optimize: Option<crate::profile::Profile>,
    organized: bool,
) -> Result<()> {
    let prepared = prepare(io, name, data, optimize, organized)?;
    let directory = if organized {
        crate::prepare::author_directory(&*card.dir, &RelPath::root(), &prepared.author)?
    } else {
        RelPath::root()
    };
    let copied = crate::copy::copy(&prepared.data, &prepared.name, &*card.dir, &directory)?;
    say!(
        io.out,
        "{} {} ({} bytes; verified)",
        if copied.already_present {
            "Already on destination:"
        } else {
            "Copied"
        },
        printable(&join(destination, &copied.path.to_string())),
        copied.bytes
    );
    say!(io.out, "Safely eject the card before disconnecting it.");
    Ok(())
}

/// Sends a book, read when it's time, to the reader; `shown` is where the local copy is, for the
/// message on failure.
fn send(
    io: &mut Io<'_>,
    reader: &Reader,
    book: impl FnOnce() -> Result<(String, Vec<u8>)>,
    shown: &str,
    optimize: Option<crate::profile::Profile>,
    organized: bool,
) -> Result<()> {
    say!(
        io.err,
        "Sending EPUB to CrossPoint; checking the reader's copy afterward…"
    );
    let (name, data) = book()?;
    let prepared = prepare(io, name, data, optimize, organized)?;
    let reader = reader.clone().prepared_for(optimize);
    let reader = if organized {
        reader.for_author(&prepared.author)?
    } else {
        reader
    };
    let sent = reader
        .send(&prepared.name, &prepared.data)
        .with_context(|| {
            format!(
                "Could not complete Wi-Fi transfer. Local EPUB remains at {shown}; retry with crossload send"
            )
        })?;
    if let Some(note) = &sent.note {
        say!(io.err, "{}", printable(note));
    }
    if let Some(backup) = &sent.backup {
        say!(
            io.out,
            "Incomplete original preserved at {}",
            printable(backup)
        );
    }
    say!(
        io.out,
        "{} {} ({} bytes; SHA-256 verified)",
        if sent.already_present {
            "Already on reader:"
        } else {
            "Sent"
        },
        printable(&sent.path),
        sent.bytes
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_joins_read_as_paths_do() {
        for (path, name) in [
            ("a/b.epub", Some("b.epub")),
            ("b.epub", Some("b.epub")),
            ("a/b.epub/", Some("b.epub")),
            ("a/./b.epub/.", Some("b.epub")),
            ("/", None),
            (".", None),
            ("a/..", None),
            ("", None),
        ] {
            assert_eq!(file_name(path), name, "{path}");
        }
        assert_eq!(join("card", "A/b.epub"), "card/A/b.epub");
        assert_eq!(join("card/", "b.epub"), "card/b.epub");
        assert_eq!(join("", "b.epub"), "b.epub");
    }
}
