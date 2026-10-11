//! Changing what a book says about itself.
//!
//! Only the package document is touched: the title, the author, and the series
//! it belongs to. Every other file is copied across byte for byte, which is
//! what lets `inventory::identity` treat an edited book and the copies of it
//! already on a device as one book rather than two.
//!
//! Nothing here writes to a reader or a Kobo. A book is corrected where its
//! original lives, and an ordinary copy carries the correction outward.
use anyhow::{ensure, Context, Result};
use std::io::{Cursor, Read, Write};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

/// What a reader may change. A field left as None keeps whatever the book
/// already says; `Some("")` clears it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Edit {
    pub title: Option<String>,
    pub author: Option<String>,
    pub series: Option<String>,
    pub series_index: Option<String>,
}
impl Edit {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// Remove every element of a kind, wherever it sits, by splicing out the text
/// between its opening and closing tags. The document is not reparsed, so
/// everything this does not name survives exactly as written.
fn without(document: &str, opening: &[&str], closing: &str) -> String {
    let mut out = String::with_capacity(document.len());
    let mut rest = document;
    loop {
        let found = opening
            .iter()
            .filter_map(|open| rest.find(open).map(|at| (at, *open)))
            .min_by_key(|(at, _)| *at);
        let Some((at, open)) = found else { break };
        // A tag only matches at a boundary: <dc:title> must not match inside
        // <dc:titleSomethingElse>.
        let after = &rest[at + open.len()..];
        if !after.starts_with('>') && !after.starts_with(char::is_whitespace) {
            out.push_str(&rest[..at + open.len()]);
            rest = after;
            continue;
        }
        let Some(end) = rest[at..].find('>').map(|e| at + e + 1) else {
            break;
        };
        // An empty element closes itself and has no closing tag to find.
        if rest[at..end].ends_with("/>") {
            out.push_str(&rest[..at]);
            rest = &rest[end..];
            continue;
        }
        let Some(close) = rest[end..].find(closing).map(|c| end + c + closing.len()) else {
            break;
        };
        out.push_str(&rest[..at]);
        rest = &rest[close..];
    }
    out.push_str(rest);
    out
}

/// Apply an edit to a package document, leaving everything it does not name.
fn edited_package(package: &str, edit: &Edit) -> Result<String> {
    let at = package
        .find("</metadata>")
        .context("the package document has no metadata to change")?;
    let (head, tail) = package.split_at(at);
    let mut head = head.to_owned();
    if edit.title.is_some() {
        head = without(&head, &["<dc:title", "<title"], "</dc:title>");
        head = without(&head, &["<title"], "</title>");
    }
    if edit.author.is_some() {
        head = without(&head, &["<dc:creator", "<creator"], "</dc:creator>");
        head = without(&head, &["<creator"], "</creator>");
    }
    if edit.series.is_some() || edit.series_index.is_some() {
        // Both spellings of a series, so the two cannot disagree afterwards.
        head = without(&head, &["<meta name=\"calibre:series\""], "</meta>");
        head = without(&head, &["<meta name=\"calibre:series_index\""], "</meta>");
        head = without(
            &head,
            &["<meta property=\"belongs-to-collection\""],
            "</meta>",
        );
        head = without(&head, &["<meta refines=\"#crossload-series\""], "</meta>");
    }
    let mut added = String::new();
    if let Some(title) = edit.title.as_ref().filter(|t| !t.trim().is_empty()) {
        added.push_str(&format!("<dc:title>{}</dc:title>", escaped(title.trim())));
    }
    if let Some(author) = edit.author.as_ref().filter(|a| !a.trim().is_empty()) {
        added.push_str(&format!(
            "<dc:creator>{}</dc:creator>",
            escaped(author.trim())
        ));
    }
    if let Some(series) = edit.series.as_ref().filter(|s| !s.trim().is_empty()) {
        let series = escaped(series.trim());
        // Written both ways: Calibre's pair is what most tools read, and the
        // EPUB 3 collection is what the standard says.
        added.push_str(&format!(
            "<meta name=\"calibre:series\" content=\"{series}\"/>"
        ));
        added.push_str(&format!(
            "<meta property=\"belongs-to-collection\" id=\"crossload-series\">{series}</meta>"
        ));
        if let Some(index) = edit.series_index.as_ref().filter(|i| !i.trim().is_empty()) {
            let index = index.trim();
            ensure!(
                index.parse::<f32>().is_ok(),
                "a position in a series has to be a number, not {index}"
            );
            let index = escaped(index);
            added.push_str(&format!(
                "<meta name=\"calibre:series_index\" content=\"{index}\"/>"
            ));
            added.push_str(&format!(
                "<meta refines=\"#crossload-series\" property=\"group-position\">{index}</meta>"
            ));
        }
    }
    Ok(format!("{head}{added}{tail}"))
}

/// Rewrite a book with new metadata, changing nothing else about it.
pub fn apply(data: &[u8], edit: &Edit) -> Result<Vec<u8>> {
    ensure!(!edit.is_empty(), "nothing to change");
    let mut archive = ZipArchive::new(Cursor::new(data)).context("Book is not a readable EPUB")?;
    crate::epub::font_metadata(data)
        .context("This book is still encrypted; its metadata was not changed")?;
    let package = {
        let mut text = String::new();
        archive
            .by_name("META-INF/container.xml")
            .context("Book has no container")?
            .read_to_string(&mut text)?;
        let container = roxmltree::Document::parse_with_options(
            &text,
            roxmltree::ParsingOptions {
                allow_dtd: true,
                ..Default::default()
            },
        )?;
        container
            .descendants()
            .find(|node| node.is_element() && node.tag_name().name() == "rootfile")
            .and_then(|node| node.attribute("full-path"))
            .context("Book names no package document")?
            .to_owned()
    };
    let mut out = ZipWriter::new(Cursor::new(Vec::new()));
    out.start_file(
        "mimetype",
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
    )?;
    out.write_all(b"application/epub+zip")?;
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut rewrote = false;
    for index in 0..archive.len() {
        let mut member = archive.by_index(index)?;
        let name = member.name().to_owned();
        if name == "mimetype" {
            continue;
        }
        if member.is_dir() {
            out.add_directory(name, deflated)?;
            continue;
        }
        let mut bytes = Vec::new();
        member.read_to_end(&mut bytes)?;
        if name == package {
            let text = std::str::from_utf8(&bytes)
                .context("the package document is not text this can change")?;
            let changed = edited_package(text, edit)?;
            out.start_file(&name, deflated)?;
            out.write_all(changed.as_bytes())?;
            rewrote = true;
            continue;
        }
        out.start_file(&name, deflated)?;
        out.write_all(&bytes)?;
    }
    ensure!(
        rewrote,
        "the package document named by this book is missing"
    );
    let data = out.finish()?.into_inner();
    crate::epub::validate(&data)
        .context("the changed book failed validation; nothing was written")?;
    // What was asked for has to be what the book now says, or the change is
    // refused rather than left half applied.
    let after = crate::epub::metadata_from(&data)?.context("the changed book lost its metadata")?;
    if let Some(title) = edit.title.as_ref().filter(|t| !t.trim().is_empty()) {
        ensure!(
            after.title.as_deref() == Some(title.trim()),
            "the title did not take; nothing was written"
        );
    }
    if let Some(series) = edit.series.as_ref().filter(|s| !s.trim().is_empty()) {
        ensure!(
            after.series.as_deref() == Some(series.trim()),
            "the series did not take; nothing was written"
        );
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The package document of a book, which is deflated inside it.
    fn package_of(data: &[u8]) -> String {
        let mut archive = ZipArchive::new(Cursor::new(data)).unwrap();
        let mut text = String::new();
        archive
            .by_name("b.opf")
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    }

    fn book(metadata: &str) -> Vec<u8> {
        let opf = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
             <package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" \
             unique-identifier=\"i\"><metadata \
             xmlns:dc=\"http://purl.org/dc/elements/1.1/\">{metadata}</metadata>\
             <manifest><item id=\"c\" href=\"c.xhtml\" \
             media-type=\"application/xhtml+xml\"/></manifest>\
             <spine><itemref idref=\"c\"/></spine></package>"
        );
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (name, body) in [
            ("mimetype", "application/epub+zip".to_owned()),
            (
                "META-INF/container.xml",
                "<container version=\"1.0\" \
                 xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\"><rootfiles>\
                 <rootfile full-path=\"b.opf\" \
                 media-type=\"application/oebps-package+xml\"/></rootfiles></container>"
                    .to_owned(),
            ),
            ("b.opf", opf),
            (
                "c.xhtml",
                "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>c</title></head>\
                 <body><p>The text of it.</p></body></html>"
                    .to_owned(),
            ),
        ] {
            zip.start_file::<_, ()>(name, stored).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn a_correction_changes_what_is_asked_for_and_nothing_else() {
        let before = book(
            "<dc:identifier id=\"i\">urn:uuid:1</dc:identifier>\
             <dc:title>Calibans strijd</dc:title>\
             <dc:creator>James Corey</dc:creator><dc:language>nl</dc:language>",
        );
        let after = apply(
            &before,
            &Edit {
                author: Some("James S. A. Corey".into()),
                series: Some("The Expanse".into()),
                series_index: Some("2".into()),
                ..Edit::default()
            },
        )
        .unwrap();

        let read = crate::epub::metadata_from(&after).unwrap().unwrap();
        assert_eq!(read.author, "James S. A. Corey");
        assert_eq!(read.series.as_deref(), Some("The Expanse"));
        assert_eq!(read.series_index, Some(2.0));
        // What was not named is left alone.
        assert_eq!(read.title.as_deref(), Some("Calibans strijd"));
        assert!(read.identifiers.iter().any(|i| i == "urn:uuid:1"));

        // And the book is the same book: only its metadata moved, so a copy of
        // it already on a reader still matches.
        let a = crate::inventory::identity(&before);
        let b = crate::inventory::identity(&after);
        assert_ne!(a.sha256, b.sha256);
        assert_eq!(a.resources, b.resources);
        assert!(a.matches(&b));
    }

    #[test]
    fn a_series_written_twice_is_read_back_the_same_either_way() {
        // Calibre's pair and the EPUB 3 collection both go in, so whichever a
        // reader looks for, it finds the same answer.
        let after = apply(
            &book("<dc:title>A</dc:title><dc:creator>B</dc:creator>"),
            &Edit {
                series: Some("Dune".into()),
                series_index: Some("1.5".into()),
                ..Edit::default()
            },
        )
        .unwrap();
        let text = package_of(&after);
        assert!(text.contains("calibre:series\" content=\"Dune\""), "{text}");
        assert!(text.contains("belongs-to-collection"), "{text}");
        assert!(text.contains("group-position\">1.5"), "{text}");
        assert_eq!(
            crate::epub::metadata_from(&after)
                .unwrap()
                .unwrap()
                .series_index,
            Some(1.5)
        );

        // Correcting it again replaces rather than accumulates.
        let twice = apply(
            &after,
            &Edit {
                series: Some("Dune Chronicles".into()),
                series_index: Some("2".into()),
                ..Edit::default()
            },
        )
        .unwrap();
        let text = package_of(&twice);
        assert_eq!(text.matches("calibre:series\"").count(), 1, "{text}");
        assert_eq!(text.matches("belongs-to-collection").count(), 1, "{text}");
        assert!(!text.contains(">Dune<"), "{text}");
    }

    #[test]
    fn a_position_that_is_not_a_number_is_refused() {
        let refused = apply(
            &book("<dc:title>A</dc:title>"),
            &Edit {
                series: Some("Dune".into()),
                series_index: Some("second".into()),
                ..Edit::default()
            },
        )
        .unwrap_err();
        assert!(
            format!("{refused:#}").contains("has to be a number"),
            "{refused:#}"
        );
        assert!(apply(&book("<dc:title>A</dc:title>"), &Edit::default()).is_err());
    }
}
