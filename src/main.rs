use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use comfy_table::{presets::UTF8_HORIZONTAL_ONLY, ColumnConstraint, ContentArrangement, Table};
use xteink::kobo::{Book, Library, Source};

#[derive(Parser)]
#[command(version, about = "Import books for your Xteink")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List or import downloaded books from a mounted Kobo.
    Kobo {
        #[command(subcommand)]
        command: KoboCommand,
    },
}

#[derive(Args)]
struct Device {
    /// Kobo mount directory containing .kobo/KoboReader.sqlite.
    #[arg(long)]
    device: PathBuf,
}

#[derive(Subcommand)]
enum KoboCommand {
    /// List Kobo store books, previews, and sideloaded EPUBs with their IDs.
    List {
        #[command(flatten)]
        device: Device,
        /// Print structured JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Import one book as an EPUB. Existing files are never overwritten.
    Import {
        /// Exact ID from `xteink kobo list`.
        book_id: String,
        #[command(flatten)]
        device: Device,
        /// Local destination directory (created if necessary).
        #[arg(long)]
        output: PathBuf,
        /// Device serial, if it cannot be read from device.xml.
        #[arg(long)]
        serial: Option<String>,
    },
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Kobo { command } => match command {
            KoboCommand::List { device, json } => {
                let library = Library::open(&device.device)?;
                let books = library.books()?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&books)?);
                } else if books.is_empty() {
                    println!("No downloaded Kobo store books found. Download a book on the Kobo, then reconnect it.");
                } else {
                    print_books(&books);
                }
            }
            KoboCommand::Import {
                book_id,
                device,
                output,
                serial,
            } => {
                let library = Library::open(&device.device)?;
                let path = library.import(&book_id, &output, serial.as_deref())?;
                println!("Imported {}", printable(&path.display().to_string()));
            }
        },
    }
    Ok(())
}

fn print_books(books: &[Book]) {
    let mut table = Table::new();
    table
        .load_style(UTF8_HORIZONTAL_ONLY)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(["TITLE", "AUTHOR", "SOURCE", "TYPE", "KEYS", "ID"]);
    // Use terminal width interactively and a predictable width when redirected.
    if !std::io::stdout().is_terminal() {
        table.set_width(120);
    }
    for book in books {
        table.add_row([
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
        ]);
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
