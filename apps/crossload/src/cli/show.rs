//! Tables and listings. On a terminal they line up to its width; anywhere else each row is one
//! line of tab-separated fields, for the next program.

use ambit_cli::Io;
use comfy_table::{presets::NOTHING, ColumnConstraint, ContentArrangement, Table};

use super::printable;
use crate::books::{Book, Place, Snapshot};
use crate::kobo::Source;

/// One table, laid out on the terminal or written as tabs.
pub(super) struct Rows {
    table: Option<Table>,
}

impl Rows {
    /// The header goes out at once in a pipe, and above the rows on a terminal.
    pub fn new(io: &mut Io<'_>, header: &[&str]) -> Self {
        let rows = Self::quiet(io, header);
        if rows.table.is_none() {
            let _ = writeln!(io.out, "{}", header.join("\t"));
        }
        rows
    }

    /// The table only, before any row is added: no output yet.
    pub fn quiet(io: &Io<'_>, header: &[&str]) -> Self {
        let table = io.terminal.map(|terminal| {
            let mut table = Table::new();
            table
                .load_style(NOTHING)
                .set_content_arrangement(ContentArrangement::Dynamic)
                .set_width(terminal.width)
                .set_header(header.iter().copied());
            table
        });
        Self { table }
    }

    pub fn on_terminal(&self) -> bool {
        self.table.is_some()
    }

    pub fn row<const N: usize>(&mut self, io: &mut Io<'_>, row: [String; N]) {
        match &mut self.table {
            Some(table) => {
                table.add_row(row);
            }
            None => {
                let _ = writeln!(io.out, "{}", row.join("\t"));
            }
        }
    }

    /// These columns keep their content on one line; the others wrap.
    pub fn whole(&mut self, columns: &[usize]) {
        if let Some(table) = &mut self.table {
            for &index in columns {
                if let Some(column) = table.column_mut(index) {
                    column.set_constraint(ColumnConstraint::ContentWidth);
                }
            }
        }
    }

    /// Writes the table, on a terminal; in a pipe every row is already out.
    pub fn finish(self, io: &mut Io<'_>) -> bool {
        match self.table {
            Some(table) => {
                let _ = writeln!(io.out, "{table}");
                true
            }
            None => false,
        }
    }
}

pub(super) fn books(io: &mut Io<'_>, books: &[crate::kobo::Book]) {
    let mut rows = Rows::new(io, &["TITLE", "AUTHOR", "SOURCE", "TYPE", "KEYS", "ID"]);
    for book in books {
        rows.row(
            io,
            [
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
            ],
        );
    }
    // Keep IDs intact for copying into the import command; wrap descriptive
    // columns instead. Type and key labels also stay on a single line.
    rows.whole(&[2, 3, 4, 5]);
    if !rows.finish(io) {
        return;
    }
    let previews = books.iter().filter(|book| book.preview).count();
    let _ = writeln!(
        io.out,
        "\n{} entries · {} books · {} previews",
        books.len(),
        books.len() - previews,
        previews
    );
    let _ = writeln!(io.out, "Keys: — = none recorded in the Kobo database.");
}

/// Locations report themselves on stderr, so a redirected catalog stays data.
pub(super) fn locations(io: &mut Io<'_>, snapshot: &Snapshot) {
    for (place, status) in &snapshot.status {
        let _ = writeln!(io.err, "{}: {}", place.label(), printable(status));
    }
    if !snapshot.acsm.is_empty() {
        let _ = writeln!(
            io.err,
            "{} local ACSM file(s) await fulfillment; use `crossload import`.",
            snapshot.acsm.len()
        );
    }
}

fn places(book: &Book) -> String {
    [Place::Local, Place::Kobo, Place::CrossPoint]
        .iter()
        .filter(|place| book.has(**place))
        .map(|place| place.label())
        .collect::<Vec<_>>()
        .join(",")
}

fn source_label(book: &Book) -> String {
    book.preferred()
        .map(|copy| {
            format!(
                "{} ({})",
                copy.place.label(),
                if copy.sha.is_empty() {
                    "unreadable"
                } else if copy.optimized {
                    "device copy"
                } else {
                    "original"
                }
            )
        })
        .unwrap_or_else(|| "none".into())
}

pub(super) fn catalog(io: &mut Io<'_>, snapshot: &Snapshot) {
    let mut rows = Rows::new(io, &["TITLE", "AUTHOR", "WHERE", "SOURCE", "BYTES"]);
    for book in &snapshot.books {
        rows.row(
            io,
            [
                printable(&book.title),
                printable(&book.author),
                places(book),
                source_label(book),
                book.preferred()
                    .map(|c| c.size)
                    .unwrap_or_default()
                    .to_string(),
            ],
        );
    }
    rows.whole(&[2, 3, 4]);
    if rows.finish(io) {
        let _ = writeln!(io.out, "\n{} books", snapshot.books.len());
    }
}

pub(super) type Planned<'a> = (&'a &'a Book, Place, Result<String, String>);

pub(super) fn plan(io: &mut Io<'_>, planned: &[Planned<'_>], to: Place, apply: bool) {
    if planned.is_empty() {
        let _ = writeln!(
            io.out,
            "Nothing to copy: every book is already at {}.",
            to.label()
        );
        return;
    }
    let mut rows = Rows::new(io, &["TITLE", "AUTHOR", "FROM", "RESULT"]);
    for (book, source, outcome) in planned {
        rows.row(
            io,
            [
                printable(&book.title),
                printable(&book.author),
                source.label().to_owned(),
                match outcome {
                    Ok(_) if !apply => "planned".to_owned(),
                    Ok(message) => printable(message),
                    Err(e) => format!("failed: {}", printable(e)),
                },
            ],
        );
    }
    rows.finish(io);
    let failures = planned.iter().filter(|(_, _, r)| r.is_err()).count();
    let _ = writeln!(
        io.out,
        "\n{} book(s) {} to {}{}",
        planned.len() - failures,
        if apply { "copied" } else { "would be copied" },
        to.label(),
        if failures > 0 {
            format!("; {failures} failed")
        } else {
            String::new()
        }
    );
    if !apply {
        let _ = writeln!(io.out, "This was a dry run. Add --apply to copy them.");
    }
}

/// Print a feed, following rel=next while pages remain.
///
/// Entries this program cannot deliver are hidden by default rather than
/// dropped silently: the count is always reported, so a catalogue of Readium
/// LCP looks empty for a stated reason.
pub(super) fn feed(
    io: &mut Io<'_>,
    http: &dyn ambit_caps_net::Http,
    start: &str,
    all: bool,
    pages: u32,
) -> anyhow::Result<()> {
    use crate::opds;
    let header = ["TITLE", "AUTHOR", "OFFER", "ADDRESS"];
    let mut next = Some(start.to_owned());
    let mut page = 0;
    let mut hidden = 0usize;
    let mut shown = 0usize;
    // Nothing is printed until a feed has actually answered, so a wrong
    // address does not leave a header standing above an error.
    let mut rows = Rows::quiet(io, &header);
    while let (Some(url), true) = (next.take(), page < pages.max(1)) {
        let feed = opds::fetch(http, &url)?;
        if page == 0 {
            if !feed.title.is_empty() {
                let _ = writeln!(io.err, "{}", printable(&feed.title));
            }
            if !rows.on_terminal() {
                let _ = writeln!(io.out, "{}", header.join("\t"));
            }
        }
        for entry in feed.books() {
            let Some(offer) = entry.best() else { continue };
            let delivery = offer.delivery();
            if !delivery.available() && !all {
                hidden += 1;
                continue;
            }
            shown += 1;
            rows.row(
                io,
                [
                    printable(&entry.title),
                    printable(&entry.author()),
                    format!("{} \u{2014} {}", offer.kind.label(), delivery.label()),
                    if delivery.available() {
                        offer.href.clone()
                    } else {
                        "\u{2014}".to_owned()
                    },
                ],
            );
        }
        for entry in feed.navigation() {
            rows.row(
                io,
                [
                    printable(&entry.title),
                    String::new(),
                    "section".to_owned(),
                    entry.subsection.clone().unwrap_or_default(),
                ],
            );
        }
        next = feed.next;
        page += 1;
    }
    rows.whole(&[3]);
    rows.finish(io);
    if hidden > 0 {
        let _ = writeln!(
            io.err,
            "{hidden} more this cannot deliver, hidden; --all lists them with the reason"
        );
    }
    if shown == 0 && hidden == 0 {
        let _ = writeln!(io.err, "Nothing here.");
    }
    if next.is_some() {
        let _ = writeln!(io.err, "More follows; --pages asks for it.");
    }
    Ok(())
}
