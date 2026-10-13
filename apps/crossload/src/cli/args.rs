//! The command line's shape, with clap's builder (its derive's generated code switches off lint
//! groups that the app rules forbid), and what it parses into.
//!
//! Written to read as the derive did, so help, errors and suggestions are unchanged: each
//! argument's id is its field name, its value name that name in capitals, and its help the first
//! paragraph of what was its doc comment, without the closing full stop.
use clap::builder::{NonEmptyStringValueParser, PossibleValue, PossibleValuesParser};
use clap::{value_parser, Arg, ArgAction, ArgMatches, Command};

pub(super) struct Cli {
    pub no_optimize: bool,
    pub flat: bool,
    pub command: Cmd,
}

pub(super) enum Cmd {
    Config(ConfigCommand),
    Tui {
        show_previews: bool,
        device: Option<String>,
        browse: Option<String>,
        output: Option<String>,
        serial: Option<String>,
        transfer: Transfer,
    },
    History {
        count: usize,
        failed: bool,
        json: bool,
    },
    Books {
        library: LibraryArgs,
        json: bool,
    },
    Sync {
        from: Location,
        to: Location,
        library: LibraryArgs,
        apply: bool,
        json: bool,
    },
    Optimize {
        book: String,
        output: Option<String>,
    },
    Copy {
        book: String,
        to: Option<String>,
    },
    Import {
        acsm: String,
        output: Option<String>,
        transfer: Transfer,
    },
    Send {
        book: String,
        to: Option<String>,
        repair: bool,
        folder: Option<String>,
    },
    Catalog(CatalogCommand),
    Adobe(AdobeCommand),
    Kobo(KoboCommand),
}

pub(super) enum ConfigCommand {
    Show,
    Set { values: crate::config::Defaults },
    Unset { key: String },
}

pub(super) enum CatalogCommand {
    Browse {
        url: Option<String>,
        all: bool,
        pages: u32,
    },
    Search {
        query: String,
        url: Option<String>,
        all: bool,
        pages: u32,
    },
    Get {
        acquisition: String,
        output: Option<String>,
        transfer: Transfer,
    },
}

pub(super) enum AdobeCommand {
    Setup { from: String },
    Status,
}

pub(super) enum KoboCommand {
    Sync {
        exclude: Vec<String>,
        device: Option<String>,
        output: Option<String>,
        to: Option<String>,
        copy_to: Option<String>,
        folder: Option<String>,
        serial: Option<String>,
        apply: bool,
        repair: bool,
        json: bool,
    },
    Notes {
        device: Option<String>,
        output: Option<String>,
        json: bool,
    },
    Shelves {
        device: Option<String>,
        json: bool,
    },
    List {
        show_previews: bool,
        shelf: Option<String>,
        device: Option<String>,
        json: bool,
    },
    Import {
        book_id: String,
        device: Option<String>,
        output: Option<String>,
        serial: Option<String>,
        transfer: Transfer,
    },
}

/// Where an imported book goes next.
pub(super) struct Transfer {
    pub send_to: Option<String>,
    pub copy_to: Option<String>,
    pub folder: Option<String>,
}

/// What `--copy-to` without a value stands for: the saved card.
pub(super) const SAVED_COPY_DESTINATION: &str = "__crossload_saved_copy_destination__";

/// The locations a catalog command reads, shared by books and sync.
pub(super) struct LibraryArgs {
    pub show_previews: bool,
    pub device: Option<String>,
    pub browse: Option<String>,
    pub output: Option<String>,
    pub serial: Option<String>,
    pub transfer: Transfer,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Location {
    Local,
    Kobo,
    /// A reader running CrossPoint, whichever device it is. The old name still works, as it does
    /// for the compatibility command.
    CrossPoint,
}

impl Location {
    pub fn place(self) -> crate::books::Place {
        match self {
            Self::Local => crate::books::Place::Local,
            Self::Kobo => crate::books::Place::Kobo,
            Self::CrossPoint => crate::books::Place::CrossPoint,
        }
    }
}

/// `--long <NAME>`: any text.
fn text(id: &'static str, long: &'static str, name: &'static str) -> Arg {
    Arg::new(id)
        .long(long)
        .value_name(name)
        .value_parser(value_parser!(String))
        .action(ArgAction::Set)
}

/// `--long <NAME>`: a path. An empty one is refused, as clap refuses an empty path.
fn path(id: &'static str, long: &'static str, name: &'static str) -> Arg {
    text(id, long, name).value_parser(NonEmptyStringValueParser::new())
}

fn flag(id: &'static str, long: &'static str) -> Arg {
    Arg::new(id).long(long).action(ArgAction::SetTrue)
}

/// A required argument by position.
fn positional(id: &'static str, name: &'static str) -> Arg {
    Arg::new(id)
        .value_name(name)
        .required(true)
        .value_parser(value_parser!(String))
        .action(ArgAction::Set)
}

fn location(id: &'static str, name: &'static str, help: &'static str) -> Arg {
    Arg::new(id)
        .long(id)
        .value_name(name)
        .required(true)
        .help(help)
        .value_parser(PossibleValuesParser::new([
            PossibleValue::new("local"),
            PossibleValue::new("kobo"),
            PossibleValue::new("crosspoint")
                .help(
                    "A reader running CrossPoint, whichever device it is. The old name still \
                     works, as it does for the compatibility command",
                )
                .alias("xteink"),
        ]))
        .action(ArgAction::Set)
}

fn device() -> Arg {
    path("device", "device", "DEVICE")
        .help("Kobo mount directory containing .kobo/KoboReader.sqlite")
}

fn transfer() -> [Arg; 3] {
    [
        text("send_to", "send-to", "SEND_TO")
            .help("Send the imported EPUB to this CrossPoint IP or HTTP URL")
            .conflicts_with("copy_to")
            .num_args(0..=1)
            .default_missing_value(""),
        path("copy_to", "copy-to", "COPY_TO")
            .help("Copy the imported EPUB to an existing mounted-card directory")
            .conflicts_with("send_to")
            .num_args(0..=1)
            .default_missing_value(SAVED_COPY_DESTINATION),
        text("folder", "folder", "FOLDER")
            .help("Existing directory on the reader's SD card (default: /)")
            .requires("send_to"),
    ]
}

fn library() -> Vec<Arg> {
    let mut args = vec![
        flag("show_previews", "show-previews").help("Include Kobo previews"),
        path("device", "device", "DEVICE"),
        path("browse", "browse", "BROWSE")
            .help("Local directory to scan (defaults to the saved browse directory)"),
        path("output", "output", "OUTPUT").help("Local directory for imported books"),
        text("serial", "serial", "SERIAL"),
    ];
    args.extend(transfer());
    args
}

fn json(help: &'static str) -> Arg {
    flag("json", "json").help(help)
}

fn dry_run() -> Arg {
    flag("dry_run", "dry-run").conflicts_with("apply")
}

fn apply() -> Arg {
    flag("apply", "apply").help("Execute the plan. The default is a read-only dry run")
}

/// A command with commands under it: one of them is required, and without one it shows help.
fn group(name: &'static str) -> Command {
    Command::new(name)
        .subcommand_required(true)
        .arg_required_else_help(true)
}

pub(super) fn command() -> Command {
    group("crossload-cli")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Import and prepare books for CrossPoint readers")
        // Read by the wiring, which opens the settings and the activation before the app runs;
        // declared here so that help shows them and the parser accepts them.
        .arg(
            path("config", "config", "CONFIG")
                .global(true)
                .help("Override the defaults file"),
        )
        .arg(
            path("state_dir", "state-dir", "STATE_DIR")
                .global(true)
                .help("Private activation and ACSM recovery directory"),
        )
        .arg(flag("no_optimize", "no-optimize").global(true).help(
            "Preserve the original EPUB bytes instead of optimizing images for the X4. PDF and \
             CBZ are never rewritten either way",
        ))
        .arg(
            flag("flat", "flat").global(true).help(
                "Keep the local filename directly in the destination, without author folders",
            ),
        )
        .subcommand(
            group("config")
                .about("Show, save or remove persistent defaults")
                .subcommand(Command::new("show"))
                .subcommand(
                    Command::new("set")
                        .about("Crossload's own settings, as the command line sets them")
                        .args(defaults()),
                )
                .subcommand(Command::new("unset").arg(positional("key", "KEY"))),
        )
        .subcommand(
            Command::new("tui")
                .about("Browse local books, ACSM requests and Kobo books interactively")
                .arg(
                    flag("show_previews", "show-previews")
                        .help("Include Kobo previews in the browser"),
                )
                .arg(path("device", "device", "DEVICE"))
                .arg(
                    path("browse", "browse", "BROWSE")
                        .help("Starting local directory (defaults to the current directory)"),
                )
                .arg(path("output", "output", "OUTPUT").help("Local directory for imported books"))
                .arg(text("serial", "serial", "SERIAL"))
                .args(transfer()),
        )
        .subcommand(
            Command::new("history")
                .about("Show what was copied, when, and whether it arrived")
                .arg(
                    Arg::new("count")
                        .long("count")
                        .short('n')
                        .value_name("COUNT")
                        .default_value("20")
                        .value_parser(value_parser!(usize))
                        .action(ArgAction::Set)
                        .help("How many of the most recent copies to show"),
                )
                .arg(flag("failed", "failed").help("Show only the copies that failed"))
                .arg(json("Print structured JSON instead of a table")),
        )
        .subcommand(
            Command::new("books")
                .about("List the unified Local, Kobo and CrossPoint catalog")
                .args(library())
                .arg(json("Print structured JSON instead of a table")),
        )
        .subcommand(
            Command::new("sync")
                .about("Plan copies of every book missing from one location; --apply performs them")
                .arg(location(
                    "from",
                    "FROM",
                    "Location to copy from: local, kobo or xteink",
                ))
                .arg(location(
                    "to",
                    "TO",
                    "Location to copy to: local, kobo or xteink",
                ))
                .args(library())
                .arg(apply())
                .arg(dry_run())
                .arg(flag("json", "json")),
        )
        .subcommand(
            Command::new("optimize")
                .about("Save an optimized device copy locally without transferring it")
                .arg(positional("book", "BOOK").value_parser(NonEmptyStringValueParser::new()))
                .arg(path("output", "output", "OUTPUT")),
        )
        .subcommand(
            Command::new("copy")
                .about("Copy a validated book to an existing directory on a mounted SD card")
                .arg(positional("book", "BOOK").value_parser(NonEmptyStringValueParser::new()))
                .arg(path("to", "to", "TO")),
        )
        .subcommand(
            Command::new("import")
                .about("Fulfill an ACSM file and import its EPUB using an existing activation")
                .arg(positional("acsm", "ACSM").value_parser(NonEmptyStringValueParser::new()))
                .arg(path("output", "output", "OUTPUT"))
                .args(transfer()),
        )
        .subcommand(
            Command::new("send")
                .about("Send a validated book to CrossPoint over Wi-Fi and verify its contents")
                .arg(positional("book", "BOOK").value_parser(NonEmptyStringValueParser::new()))
                .arg(
                    text("to", "to", "TO")
                        .help("Reader IP or HTTP URL shown in File Transfer mode"),
                )
                .arg(
                    flag("repair", "repair").help(
                        "Preserve a verified incomplete prefix under a backup name, then resend",
                    ),
                )
                .arg(
                    text("folder", "folder", "FOLDER")
                        .help("Existing directory on the reader's SD card"),
                ),
        )
        .subcommand(
            group("catalog")
                .about("Browse an OPDS catalogue and download from it")
                .subcommand(
                    Command::new("browse")
                        .about(
                            "List what a catalogue offers. Defaults to the DPLA Palace Bookshelf, \
                             which needs no library card",
                        )
                        .arg(
                            positional("url", "URL")
                                .required(false)
                                .help("Feed address, or the saved catalogue"),
                        )
                        .arg(
                            flag("all", "all").help(
                                "Also list entries this program cannot deliver, with the reason",
                            ),
                        )
                        .arg(pages().help("Follow rel=next this many times")),
                )
                .subcommand(
                    Command::new("search")
                        .about("Search a catalogue by title, author or subject")
                        .arg(positional("query", "QUERY"))
                        .arg(text("url", "url", "URL"))
                        .arg(flag("all", "all"))
                        .arg(pages()),
                )
                .subcommand(
                    Command::new("get")
                        .about("Download one book, using an address that browse or search printed")
                        .arg(positional("acquisition", "ACQUISITION"))
                        .arg(
                            path("output", "output", "OUTPUT")
                                .help("Local directory for imported books"),
                        )
                        .args(transfer()),
                ),
        )
        .subcommand(
            group("adobe")
                .about("Import or check an Adobe/ByteBooks ADEPT activation")
                .subcommand(
                    Command::new("setup")
                        .about("Import an ACSM Input/libgourou activation directory or export ZIP")
                        .arg(path("from", "from", "FROM").required(true)),
                )
                .subcommand(
                    Command::new("status")
                        .about("Check the installed activation without contacting a server"),
                ),
        )
        .subcommand(
            group("kobo")
                .about("List or import downloaded books from a mounted Kobo")
                .subcommand(
                    Command::new("sync")
                        .about("Plan additive sync; --apply imports and transfers missing books")
                        .arg(
                            text("exclude", "exclude", "EXCLUDE")
                                .action(ArgAction::Append)
                                .help("Skip this Kobo book ID (repeat for multiple books)"),
                        )
                        .arg(device())
                        .arg(path("output", "output", "OUTPUT"))
                        .arg(
                            text("to", "to", "TO")
                                .help("CrossPoint address (defaults to the saved reader)")
                                .conflicts_with("copy_to"),
                        )
                        .arg(
                            path("copy_to", "copy-to", "COPY_TO")
                                .help("Use a mounted card instead of Wi-Fi")
                                .conflicts_with("to"),
                        )
                        .arg(text("folder", "folder", "FOLDER").conflicts_with("copy_to"))
                        .arg(text("serial", "serial", "SERIAL"))
                        .arg(apply())
                        .arg(dry_run())
                        .arg(
                            flag("repair", "repair")
                                .help("Preserve and repair verified incomplete Wi-Fi uploads"),
                        )
                        .arg(flag("json", "json")),
                )
                .subcommand(
                    Command::new("notes")
                        .about("Print the highlights and notes made on the Kobo, as Markdown")
                        .arg(device())
                        .arg(
                            path("output", "output", "OUTPUT")
                                .help("Write one file per book here instead of printing them"),
                        )
                        .arg(json("Print structured JSON instead of Markdown")),
                )
                .subcommand(
                    Command::new("shelves")
                        .about("List the collections made on the Kobo and the books on them")
                        .arg(device())
                        .arg(json("Print structured JSON instead of a table")),
                )
                .subcommand(
                    Command::new("list")
                        .about(
                            "List Kobo store books, previews, and sideloaded EPUBs with their IDs",
                        )
                        .arg(
                            flag("show_previews", "show-previews")
                                .help("Include previews (hidden by default)"),
                        )
                        .arg(text("shelf", "shelf", "SHELF").help(
                            "Only books on this collection, as `crossload kobo shelves` names it",
                        ))
                        .arg(device())
                        .arg(json("Print structured JSON instead of a table")),
                )
                .subcommand(
                    Command::new("import")
                        .about("Import one book as an EPUB. Existing files are never overwritten")
                        .arg(
                            positional("book_id", "BOOK_ID")
                                .help("Exact ID from `crossload kobo list`"),
                        )
                        .arg(device())
                        .arg(
                            path("output", "output", "OUTPUT")
                                .help("Local destination directory (created if necessary)"),
                        )
                        .arg(
                            text("serial", "serial", "SERIAL")
                                .help("Device serial, if it cannot be read from device.xml"),
                        )
                        .args(transfer()),
                ),
        )
}

fn pages() -> Arg {
    Arg::new("pages")
        .long("pages")
        .value_name("PAGES")
        .default_value("1")
        .value_parser(value_parser!(u32))
        .action(ArgAction::Set)
}

/// Crossload's own settings, as `config set` takes them.
fn defaults() -> [Arg; 9] {
    [
        path("device", "device", "DEVICE"),
        path("output", "output", "OUTPUT"),
        text("reader", "reader", "READER")
            .help("CrossPoint address used by send or an explicit --send-to"),
        path("copy_to", "copy-to", "COPY_TO"),
        path("browse", "browse", "BROWSE"),
        text("folder", "folder", "FOLDER"),
        Arg::new("kepub")
            .long("kepub")
            .value_name("KEPUB")
            .value_parser(value_parser!(bool))
            .action(ArgAction::Set)
            .help("Send books to the Kobo as kepubs, which its own reading engine counts"),
        text("profile", "profile", "PROFILE")
            .help("The reader model to prepare device copies for, as CrossPoint names it"),
        text("catalog", "catalog", "CATALOG")
            .help("OPDS catalogue browsed by default, instead of the Palace Bookshelf"),
    ]
}

fn one(m: &ArgMatches, id: &str) -> Option<String> {
    m.get_one::<String>(id).cloned()
}

fn given(m: &ArgMatches, id: &str) -> String {
    one(m, id).unwrap_or_default()
}

fn transfer_of(m: &ArgMatches) -> Transfer {
    Transfer {
        send_to: one(m, "send_to"),
        copy_to: one(m, "copy_to"),
        folder: one(m, "folder"),
    }
}

fn library_of(m: &ArgMatches) -> LibraryArgs {
    LibraryArgs {
        show_previews: m.get_flag("show_previews"),
        device: one(m, "device"),
        browse: one(m, "browse"),
        output: one(m, "output"),
        serial: one(m, "serial"),
        transfer: transfer_of(m),
    }
}

fn location_of(m: &ArgMatches, id: &str) -> Location {
    match m.get_one::<String>(id).map(String::as_str) {
        Some("local") => Location::Local,
        Some("kobo") => Location::Kobo,
        _ => Location::CrossPoint,
    }
}

fn pages_of(m: &ArgMatches) -> u32 {
    m.get_one::<u32>("pages").copied().unwrap_or(1)
}

/// What clap parsed, as the commands take it. Only called on matches of [`command`], whose
/// required subcommands and arguments are there.
pub(super) fn parse(m: &ArgMatches) -> Cli {
    let Some((name, sub)) = m.subcommand() else {
        unreachable!("a command is required")
    };
    let nested = || sub.subcommand().expect("a command is required");
    let command = match name {
        "config" => Cmd::Config(match nested() {
            ("show", _) => ConfigCommand::Show,
            ("set", m) => ConfigCommand::Set {
                values: crate::config::Defaults {
                    device: one(m, "device"),
                    output: one(m, "output"),
                    reader: one(m, "reader"),
                    copy_to: one(m, "copy_to"),
                    browse: one(m, "browse"),
                    folder: one(m, "folder"),
                    kepub: m.get_one::<bool>("kepub").copied(),
                    profile: one(m, "profile"),
                    catalog: one(m, "catalog"),
                },
            },
            (_, m) => ConfigCommand::Unset {
                key: given(m, "key"),
            },
        }),
        "tui" => Cmd::Tui {
            show_previews: sub.get_flag("show_previews"),
            device: one(sub, "device"),
            browse: one(sub, "browse"),
            output: one(sub, "output"),
            serial: one(sub, "serial"),
            transfer: transfer_of(sub),
        },
        "history" => Cmd::History {
            count: sub.get_one::<usize>("count").copied().unwrap_or(20),
            failed: sub.get_flag("failed"),
            json: sub.get_flag("json"),
        },
        "books" => Cmd::Books {
            library: library_of(sub),
            json: sub.get_flag("json"),
        },
        "sync" => Cmd::Sync {
            from: location_of(sub, "from"),
            to: location_of(sub, "to"),
            library: library_of(sub),
            apply: sub.get_flag("apply"),
            json: sub.get_flag("json"),
        },
        "optimize" => Cmd::Optimize {
            book: given(sub, "book"),
            output: one(sub, "output"),
        },
        "copy" => Cmd::Copy {
            book: given(sub, "book"),
            to: one(sub, "to"),
        },
        "import" => Cmd::Import {
            acsm: given(sub, "acsm"),
            output: one(sub, "output"),
            transfer: transfer_of(sub),
        },
        "send" => Cmd::Send {
            book: given(sub, "book"),
            to: one(sub, "to"),
            repair: sub.get_flag("repair"),
            folder: one(sub, "folder"),
        },
        "catalog" => Cmd::Catalog(match nested() {
            ("browse", m) => CatalogCommand::Browse {
                url: one(m, "url"),
                all: m.get_flag("all"),
                pages: pages_of(m),
            },
            ("search", m) => CatalogCommand::Search {
                query: given(m, "query"),
                url: one(m, "url"),
                all: m.get_flag("all"),
                pages: pages_of(m),
            },
            (_, m) => CatalogCommand::Get {
                acquisition: given(m, "acquisition"),
                output: one(m, "output"),
                transfer: transfer_of(m),
            },
        }),
        "adobe" => Cmd::Adobe(match nested() {
            ("setup", m) => AdobeCommand::Setup {
                from: given(m, "from"),
            },
            _ => AdobeCommand::Status,
        }),
        _ => Cmd::Kobo(match nested() {
            ("sync", m) => KoboCommand::Sync {
                exclude: m
                    .get_many::<String>("exclude")
                    .into_iter()
                    .flatten()
                    .cloned()
                    .collect(),
                device: one(m, "device"),
                output: one(m, "output"),
                to: one(m, "to"),
                copy_to: one(m, "copy_to"),
                folder: one(m, "folder"),
                serial: one(m, "serial"),
                apply: m.get_flag("apply"),
                repair: m.get_flag("repair"),
                json: m.get_flag("json"),
            },
            ("notes", m) => KoboCommand::Notes {
                device: one(m, "device"),
                output: one(m, "output"),
                json: m.get_flag("json"),
            },
            ("shelves", m) => KoboCommand::Shelves {
                device: one(m, "device"),
                json: m.get_flag("json"),
            },
            ("list", m) => KoboCommand::List {
                show_previews: m.get_flag("show_previews"),
                shelf: one(m, "shelf"),
                device: one(m, "device"),
                json: m.get_flag("json"),
            },
            (_, m) => KoboCommand::Import {
                book_id: given(m, "book_id"),
                device: one(m, "device"),
                output: one(m, "output"),
                serial: one(m, "serial"),
                transfer: transfer_of(m),
            },
        }),
    };
    Cli {
        no_optimize: m.get_flag("no_optimize"),
        flat: m.get_flag("flat"),
        command,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_command_line_is_well_formed() {
        super::command().debug_assert();
    }
}
