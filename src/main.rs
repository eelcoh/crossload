use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use comfy_table::{presets::NOTHING, ColumnConstraint, ContentArrangement, Table};
use crossload::config::{self, required};
use crossload::kobo::{Book, Library, Source};

#[derive(Parser)]
#[command(version, about = "Import and prepare books for CrossPoint readers")]
struct Cli {
    /// Override the defaults file.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Private activation and ACSM recovery directory.
    #[arg(long, global = true)]
    state_dir: Option<PathBuf>,
    /// Preserve the original EPUB bytes instead of optimizing images for the X4.
    #[arg(long, global = true)]
    no_optimize: bool,
    /// Keep the local filename directly in the destination, without author folders.
    #[arg(long, global = true)]
    flat: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show, save or remove persistent defaults.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Browse local EPUB/ACSM files and Kobo books interactively.
    Tui {
        /// Include Kobo previews in the browser.
        #[arg(long)]
        show_previews: bool,
        #[arg(long)]
        device: Option<PathBuf>,
        /// Starting local directory (defaults to the current directory).
        #[arg(long)]
        browse: Option<PathBuf>,
        /// Local directory for imported books.
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        serial: Option<String>,
        #[command(flatten)]
        transfer: Transfer,
    },
    /// Save an optimized device copy locally without transferring it.
    Optimize {
        book: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Copy a validated EPUB to an existing directory on a mounted SD card.
    Copy {
        book: PathBuf,
        #[arg(long)]
        to: Option<PathBuf>,
    },
    /// Fulfill an ACSM file and import its EPUB using an existing activation.
    Import {
        acsm: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
        #[command(flatten)]
        transfer: Transfer,
    },
    /// Send a validated EPUB to CrossPoint over Wi-Fi and verify its contents.
    Send {
        book: PathBuf,
        /// Reader IP or HTTP URL shown in File Transfer mode.
        #[arg(long)]
        to: Option<String>,
        /// Preserve a verified incomplete prefix under a backup name, then resend.
        #[arg(long)]
        repair: bool,
        /// Existing directory on the reader's SD card.
        #[arg(long)]
        folder: Option<String>,
    },
    /// Import or check an Adobe/ByteBooks ADEPT activation.
    Adobe {
        #[command(subcommand)]
        command: AdobeCommand,
    },
    /// List or import downloaded books from a mounted Kobo.
    Kobo {
        #[command(subcommand)]
        command: KoboCommand,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    Show,
    Set {
        #[command(flatten)]
        values: config::Defaults,
    },
    Unset {
        key: String,
    },
}

#[derive(Subcommand)]
enum AdobeCommand {
    /// Import an ACSM Input/libgourou activation directory or export ZIP.
    Setup {
        #[arg(long)]
        from: PathBuf,
    },
    /// Check the installed activation without contacting a server.
    Status,
}

#[derive(Args)]
struct Device {
    /// Kobo mount directory containing .kobo/KoboReader.sqlite.
    #[arg(long)]
    device: Option<PathBuf>,
}

#[derive(Args)]
struct Transfer {
    /// Send the imported EPUB to this CrossPoint IP or HTTP URL.
    #[arg(long, conflicts_with = "copy_to", num_args = 0..=1, default_missing_value = "")]
    send_to: Option<String>,
    /// Copy the imported EPUB to an existing mounted-card directory.
    #[arg(long, conflicts_with = "send_to", num_args = 0..=1, default_missing_value = "__crossload_saved_copy_destination__")]
    copy_to: Option<PathBuf>,
    /// Existing directory on the reader's SD card (default: /).
    #[arg(long, requires = "send_to")]
    folder: Option<String>,
}

impl Transfer {
    fn resolve(mut self, defaults: &config::Defaults) -> Result<Self> {
        if self.send_to.as_deref() == Some("") {
            self.send_to = Some(required(
                None,
                defaults.reader.clone(),
                "--send-to/--reader",
            )?);
        }
        if self
            .copy_to
            .as_ref()
            .is_some_and(|p| p == std::path::Path::new("__crossload_saved_copy_destination__"))
        {
            self.copy_to = Some(required(None, defaults.copy_to.clone(), "--copy-to")?);
        }
        self.folder = self.folder.or_else(|| defaults.folder.clone());
        Ok(self)
    }

    fn reader(&self) -> Result<Option<crossload::crosspoint::Reader>> {
        if let Some(destination) = &self.copy_to {
            anyhow::ensure!(
                destination.is_dir(),
                "--copy-to requires an existing directory on the mounted card"
            );
        }
        self.send_to
            .as_deref()
            .map(|address| {
                crossload::crosspoint::Reader::new(address, self.folder.as_deref().unwrap_or("/"))
            })
            .transpose()
    }
}

#[derive(Subcommand)]
enum KoboCommand {
    /// Plan additive sync; --apply imports and transfers missing books.
    Sync {
        /// Skip this Kobo book ID (repeat for multiple books).
        #[arg(long)]
        exclude: Vec<String>,
        #[command(flatten)]
        device: Device,
        #[arg(long)]
        output: Option<PathBuf>,
        /// CrossPoint address (defaults to the saved reader).
        #[arg(long, conflicts_with = "copy_to")]
        to: Option<String>,
        /// Use a mounted card instead of Wi-Fi.
        #[arg(long, conflicts_with = "to")]
        copy_to: Option<PathBuf>,
        #[arg(long, conflicts_with = "copy_to")]
        folder: Option<String>,
        #[arg(long)]
        serial: Option<String>,
        /// Execute the plan. The default is a read-only dry run.
        #[arg(long)]
        apply: bool,
        #[arg(long, conflicts_with = "apply")]
        dry_run: bool,
        /// Preserve and repair verified incomplete Wi-Fi uploads.
        #[arg(long)]
        repair: bool,
        #[arg(long)]
        json: bool,
    },
    /// List Kobo store books, previews, and sideloaded EPUBs with their IDs.
    List {
        /// Include previews (hidden by default).
        #[arg(long)]
        show_previews: bool,
        #[command(flatten)]
        device: Device,
        /// Print structured JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Import one book as an EPUB. Existing files are never overwritten.
    Import {
        /// Exact ID from `crossload kobo list`.
        book_id: String,
        #[command(flatten)]
        device: Device,
        /// Local destination directory (created if necessary).
        #[arg(long)]
        output: Option<PathBuf>,
        /// Device serial, if it cannot be read from device.xml.
        #[arg(long)]
        serial: Option<String>,
        #[command(flatten)]
        transfer: Transfer,
    },
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let config_path = cli
        .config
        .clone()
        .map(Ok)
        .unwrap_or_else(config::default_path)?;
    let mut defaults = config::load(&config_path)?;
    let state = || -> Result<crossload::adobe::Store> {
        let path = cli
            .state_dir
            .clone()
            .map(Ok)
            .unwrap_or_else(crossload::adobe::default_state_dir)?;
        crossload::adobe::Store::open(&path)
    };
    match cli.command {
        Command::Config { command } => match command {
            ConfigCommand::Show => {
                println!("{}", serde_json::to_string_pretty(&defaults)?);
                eprintln!(
                    "Defaults: {}",
                    printable(&config_path.display().to_string())
                );
            }
            ConfigCommand::Set { values } => {
                defaults.merge(values);
                defaults.save(&config_path)?;
                println!("Saved {}", printable(&config_path.display().to_string()));
            }
            ConfigCommand::Unset { key } => {
                defaults.unset(&key)?;
                defaults.save(&config_path)?;
                println!("Removed {key}");
            }
        },
        Command::Tui {
            show_previews,
            device,
            browse,
            output,
            serial,
            transfer,
        } => {
            let transfer = transfer.resolve(&defaults)?;
            crossload::tui::run(crossload::tui::Options {
                show_previews,
                device: device.or(defaults.device.clone()),
                browse: browse
                    .or(defaults.browse.clone())
                    .unwrap_or_else(|| PathBuf::from("."))
                    .canonicalize()?,
                output: required(output, defaults.output.clone(), "--output")?,
                serial,
                state: cli.state_dir.clone(),
                send_to: transfer.send_to,
                copy_to: transfer.copy_to,
                folder: transfer.folder.unwrap_or_else(|| "/".to_owned()),
                optimize: !cli.no_optimize,
                organized: !cli.flat,
            })?;
        }
        Command::Optimize { book, output } => {
            let output = required(output, defaults.output.clone(), "--output")?;
            let prepared = prepare(&book, !cli.no_optimize, !cli.flat)?;
            std::fs::create_dir_all(&output)?;
            let result = crossload::copy::copy(&prepared.path, &output)?;
            println!(
                "Device copy: {}",
                printable(&result.path.display().to_string())
            );
        }
        Command::Copy { book, to } => copy(
            &book,
            &required(to, defaults.copy_to.clone(), "--to/--copy-to")?,
            !cli.no_optimize,
            !cli.flat,
        )?,
        Command::Import {
            acsm,
            output,
            transfer,
        } => {
            let output = required(output, defaults.output.clone(), "--output")?;
            let transfer = transfer.resolve(&defaults)?;
            let reader = transfer.reader()?;
            let path = state()?.import(&acsm, &output)?;
            println!("Imported {}", printable(&path.display().to_string()));
            if let Some(reader) = reader {
                send(&reader, &path, !cli.no_optimize, !cli.flat)?;
            }
            if let Some(destination) = transfer.copy_to {
                copy(&path, &destination, !cli.no_optimize, !cli.flat)?;
            }
        }
        Command::Send {
            book,
            to,
            folder,
            repair,
        } => {
            let to = required(to, defaults.reader.clone(), "--to/--reader")?;
            let folder = folder
                .or(defaults.folder.clone())
                .unwrap_or_else(|| "/".to_owned());
            send(
                &crossload::crosspoint::Reader::new(&to, &folder)?.repair_incomplete(repair),
                &book,
                !cli.no_optimize,
                !cli.flat,
            )?;
        }
        Command::Adobe { command } => {
            match command {
                AdobeCommand::Setup { from } => {
                    state()?.import_activation(&from)?;
                    println!("Activation imported and checked. No server was contacted.");
                }
                AdobeCommand::Status => {
                    if state()?.status()? {
                        println!("Activation installed and locally valid. Native libgourou backend ready.");
                    } else {
                        println!("No activation installed. Run crossload adobe setup --from <directory or ZIP>.");
                    }
                }
            }
        }
        Command::Kobo { command } => match command {
            KoboCommand::Sync {
                exclude,
                device,
                output,
                to,
                copy_to,
                folder,
                serial,
                apply,
                dry_run: _,
                repair,
                json,
            } => {
                let library = Library::open(&required(
                    device.device,
                    defaults.device.clone(),
                    "--device",
                )?)?;
                let output = required(output, defaults.output.clone(), "--output")?;
                let destination = if let Some(path) = copy_to {
                    crossload::sync::card(&path)?
                } else {
                    let address = required(
                        to,
                        defaults.reader.clone(),
                        "--to/--reader (or select --copy-to)",
                    )?;
                    crossload::inventory::Destination::Reader(crossload::crosspoint::Reader::new(
                        &address,
                        &folder
                            .or(defaults.folder.clone())
                            .unwrap_or_else(|| "/".into()),
                    )?)
                };
                let report = crossload::sync::run(
                    &library,
                    &destination,
                    &crossload::sync::Options {
                        output,
                        serial,
                        apply,
                        optimize: !cli.no_optimize,
                        organized: !cli.flat,
                        repair,
                        exclude,
                    },
                    |progress| eprintln!("{}", printable(progress)),
                )?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    println!("STATUS\tTITLE\tID\tDESTINATION\tDETAIL");
                    for row in &report.books {
                        let status = serde_json::to_value(&row.status)?;
                        println!(
                            "{}\t{}\t{}\t{}\t{}",
                            status.as_str().unwrap_or("unknown"),
                            printable(&row.title),
                            printable(&row.id),
                            printable(row.path.as_deref().unwrap_or("")),
                            printable(&row.detail)
                        );
                    }
                }
                eprintln!(
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
                if apply && matches!(destination, crossload::inventory::Destination::Card(_)) {
                    eprintln!("Safely eject the card before disconnecting it.");
                }
            }

            KoboCommand::List {
                device,
                json,
                show_previews,
            } => {
                let library = Library::open(&required(
                    device.device,
                    defaults.device.clone(),
                    "--device",
                )?)?;
                let books: Vec<_> = library
                    .books()?
                    .into_iter()
                    .filter(|book| show_previews || !book.preview)
                    .collect();
                if json {
                    println!("{}", serde_json::to_string_pretty(&books)?);
                } else if books.is_empty() {
                    eprintln!(
                        "No downloaded books to show. Use --show-previews to include previews."
                    );
                    print_books(&books);
                } else {
                    print_books(&books);
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
                let transfer = transfer.resolve(&defaults)?;
                let reader = transfer.reader()?;
                let library = Library::open(&required(
                    device.device,
                    defaults.device.clone(),
                    "--device",
                )?)?;
                let path = library.import(&book_id, &output, serial.as_deref())?;
                println!("Imported {}", printable(&path.display().to_string()));
                if let Some(reader) = reader {
                    send(&reader, &path, !cli.no_optimize, !cli.flat)?;
                }
                if let Some(destination) = transfer.copy_to {
                    copy(&path, &destination, !cli.no_optimize, !cli.flat)?;
                }
            }
        },
    }
    Ok(())
}

fn prepare(
    book: &std::path::Path,
    optimize: bool,
    organized: bool,
) -> Result<crossload::prepare::Prepared> {
    let prepared = crossload::prepare::prepare(book, optimize, organized)?;
    if optimize {
        eprintln!(
            "Device copy: {} → {} bytes; {} images optimized for X4 (480×800).",
            prepared.original_bytes, prepared.bytes, prepared.images
        );
    }
    Ok(prepared)
}

fn copy(
    book: &std::path::Path,
    destination: &std::path::Path,
    optimize: bool,
    organized: bool,
) -> Result<()> {
    anyhow::ensure!(
        destination.is_dir(),
        "Destination must be an existing mounted-card directory"
    );
    let prepared = prepare(book, optimize, organized)?;
    let directory = if organized {
        crossload::prepare::author_directory(destination, &prepared.author)?
    } else {
        destination.to_owned()
    };
    let copied = crossload::copy::copy(&prepared.path, &directory)?;
    println!(
        "{} {} ({} bytes; verified)",
        if copied.already_present {
            "Already on destination:"
        } else {
            "Copied"
        },
        printable(&copied.path.display().to_string()),
        copied.bytes
    );
    println!("Safely eject the card before disconnecting it.");
    Ok(())
}

fn send(
    reader: &crossload::crosspoint::Reader,
    book: &std::path::Path,
    optimize: bool,
    organized: bool,
) -> Result<()> {
    use anyhow::Context;
    eprintln!("Sending EPUB to CrossPoint; checking the reader's copy afterward…");
    let prepared = prepare(book, optimize, organized)?;
    let author_reader;
    let reader = if organized {
        author_reader = reader.for_author(&prepared.author)?;
        &author_reader
    } else {
        reader
    };
    let sent = reader.send(&prepared.path).with_context(|| {
        format!(
            "Could not complete Wi-Fi transfer. Local EPUB remains at {}; retry with crossload send",
            book.display()
        )
    })?;
    if let Some(backup) = &sent.backup {
        println!("Incomplete original preserved at {}", printable(backup));
    }
    println!(
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

fn print_books(books: &[Book]) {
    let mut table = Table::new();
    table
        .load_style(NOTHING)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(["TITLE", "AUTHOR", "SOURCE", "TYPE", "KEYS", "ID"]);
    let interactive = std::io::stdout().is_terminal();
    if !interactive {
        println!("TITLE\tAUTHOR\tSOURCE\tTYPE\tKEYS\tID");
    }
    for book in books {
        let row = [
            printable(&book.title),
            printable(&book.author),
            match book.source {
                Source::KoboStore => "Kobo store",
                Source::Sideloaded => "Sideloaded",
            }
            .to_owned(),
            if book.preview { "Preview" } else { "Book" }.to_owned(),
            if book.encrypted { "Kobo" } else { "—" }.to_owned(),
            printable(&book.id),
        ];
        if interactive {
            table.add_row(row);
        } else {
            println!("{}", row.join("\t"));
        }
    }
    if !interactive {
        return;
    }
    // Keep IDs intact for copying into the import command; wrap descriptive
    // columns instead. Type and key labels also stay on a single line.
    for index in [2, 3, 4, 5] {
        if let Some(column) = table.column_mut(index) {
            column.set_constraint(ColumnConstraint::ContentWidth);
        }
    }
    let previews = books.iter().filter(|book| book.preview).count();
    println!("{table}");
    println!(
        "\n{} entries · {} books · {} previews",
        books.len(),
        books.len() - previews,
        previews
    );
    println!("Keys: — = none recorded in the Kobo database.");
}

fn printable(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {}", printable(&format!("{error:#}")));
            std::process::ExitCode::FAILURE
        }
    }
}
