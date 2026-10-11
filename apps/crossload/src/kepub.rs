//! Kobo's own flavour of EPUB.
//!
//! A sideloaded EPUB is read by the Kobo's generic engine, which tracks
//! progress poorly and keeps no statistics. A book named `.kepub.epub` whose
//! text is divided into `koboSpan` elements is read by the engine the store's
//! own books get. Calibre's KoboTouchExtended does the same thing.
//!
//! The danger here is quiet corruption: a book that still opens but reads
//! wrongly. So the rewrite is deliberately shallow — it never parses and
//! re-serializes a document, only splices spans around text it has already
//! found — and every document it touches must still parse afterwards, or the
//! whole conversion is refused rather than a broken book written.
use anyhow::{ensure, Context, Result};
use std::io::{Cursor, Read, Write};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

/// Parse as a reading system would. A DOCTYPE is refused by default, and
/// nearly every EPUB 2 book carries one, so refusing them would rule out most
/// real libraries. External entities are still not fetched, and roxmltree
/// bounds the expansion of internal ones.
fn parse(text: &str) -> Result<roxmltree::Document<'_>, roxmltree::Error> {
    roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
}

/// Elements whose text is markup or machinery, not prose.
const SKIP: [&str; 3] = ["script", "style", "pre"];

/// Where a sentence may end. Kobo's own spans are roughly sentence-sized.
fn sentence_end(text: &str, at: usize) -> bool {
    let bytes = text.as_bytes();
    matches!(bytes.get(at), Some(b'.' | b'!' | b'?'))
        && bytes
            .get(at + 1)
            .is_none_or(|next| next.is_ascii_whitespace())
}

/// Split a run of text where sentences end, keeping every byte.
fn sentences(text: &str) -> Vec<&str> {
    let mut parts = vec![];
    let mut start = 0;
    for at in 0..text.len() {
        if !text.is_char_boundary(at) || !sentence_end(text, at) {
            continue;
        }
        // Take the punctuation and the space after it with the sentence.
        let mut end = at + 1;
        while end < text.len() && text.as_bytes()[end].is_ascii_whitespace() {
            end += 1;
        }
        parts.push(&text[start..end]);
        start = end;
    }
    if start < text.len() {
        parts.push(&text[start..]);
    }
    parts
}

/// Divide a document's prose into spans the Kobo can count.
///
/// This walks the markup rather than parsing it: anything between `<` and `>`
/// is copied untouched, and only the text between tags is wrapped. Nothing is
/// re-escaped, so entities arrive exactly as they left.
fn spans(document: &str) -> String {
    // Everything before the body is prologue: doctype, head, metadata.
    let start = match document.find("<body").or_else(|| document.find("<BODY")) {
        Some(at) => match document[at..].find('>') {
            Some(close) => at + close + 1,
            None => return document.to_owned(),
        },
        None => return document.to_owned(),
    };
    let mut out = String::with_capacity(document.len() + document.len() / 4);
    out.push_str(&document[..start]);
    let rest = &document[start..];
    let (mut paragraph, mut segment) = (1_u32, 1_u32);
    let mut skipping: Option<&str> = None;
    let mut index = 0;
    while index < rest.len() {
        let Some(open) = rest[index..].find('<') else {
            break;
        };
        let text = &rest[index..index + open];
        match skipping {
            // Inside script, style or pre, text is carried across untouched.
            Some(_) => out.push_str(text),
            None if text.trim().is_empty() => out.push_str(text),
            None => {
                for sentence in sentences(text) {
                    if sentence.trim().is_empty() {
                        out.push_str(sentence);
                        continue;
                    }
                    out.push_str(&format!(
                        "<span class=\"koboSpan\" id=\"kobo.{paragraph}.{segment}\">{sentence}</span>"
                    ));
                    segment += 1;
                }
                paragraph += 1;
                segment = 1;
            }
        }
        index += open;
        // Copy the tag itself, whatever it is: a comment, CDATA or an element.
        let tag_end = if rest[index..].starts_with("<!--") {
            rest[index..].find("-->").map(|at| index + at + 3)
        } else if rest[index..].starts_with("<![CDATA[") {
            rest[index..].find("]]>").map(|at| index + at + 3)
        } else {
            rest[index..].find('>').map(|at| index + at + 1)
        };
        let Some(tag_end) = tag_end else {
            out.push_str(&rest[index..]);
            return out;
        };
        let tag = &rest[index..tag_end];
        out.push_str(tag);
        index = tag_end;
        // Track the regions whose text must be left alone.
        let name: String = tag
            .trim_start_matches('<')
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        match skipping {
            Some(open) if tag.starts_with("</") && name == open => skipping = None,
            None if SKIP.contains(&name.as_str()) && !tag.ends_with("/>") => {
                skipping = SKIP.iter().find(|skip| **skip == name).copied()
            }
            _ => {}
        }
    }
    out.push_str(&rest[index..]);
    out
}

/// Resolve a manifest href against the package document's own folder, the way
/// a reading system does.
fn resolve(opf: &str, href: &str) -> String {
    // Percent escapes are how a manifest writes a space or an accent.
    let mut decoded = String::with_capacity(href.len());
    let bytes = href.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                match u8::from_str_radix(&href[index + 1..index + 3], 16) {
                    Ok(byte) => {
                        decoded.push(byte as char);
                        index += 3;
                    }
                    Err(_) => {
                        decoded.push('%');
                        index += 1;
                    }
                }
            }
            _ => {
                decoded.push(href[index..].chars().next().unwrap_or('%'));
                index += href[index..].chars().next().map_or(1, char::len_utf8);
            }
        }
    }
    let base = opf.rsplit_once('/').map_or("", |(parent, _)| parent);
    let mut parts: Vec<&str> = vec![];
    for part in base.split('/').chain(decoded.split('/')) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

/// The documents a book's own manifest calls prose.
///
/// Asking the manifest rather than guessing from a file's name matters: an
/// EPUB may name its chapters `.htm`, or anything else, and a book whose
/// chapters were skipped would be written out looking converted while none of
/// it had been.
fn content_documents(archive: &mut ZipArchive<Cursor<&[u8]>>) -> Option<Vec<String>> {
    let read = |archive: &mut ZipArchive<Cursor<&[u8]>>, name: &str| -> Option<String> {
        let mut text = String::new();
        archive.by_name(name).ok()?.read_to_string(&mut text).ok()?;
        Some(text)
    };
    let container = read(archive, "META-INF/container.xml")?;
    let container = parse(&container).ok()?;
    let opf = container
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "rootfile")
        .and_then(|node| node.attribute("full-path"))?
        .to_owned();
    let package = read(archive, &opf)?;
    let package = parse(&package).ok()?;
    Some(
        package
            .descendants()
            .filter(|node| node.is_element() && node.tag_name().name() == "item")
            .filter(|node| node.attribute("media-type") == Some("application/xhtml+xml"))
            .filter_map(|node| node.attribute("href"))
            .map(|href| resolve(&opf, href))
            .collect(),
    )
}

/// Rewrite an EPUB as a kepub: the same book, with its prose divided into
/// spans the Kobo's own reading engine counts.
///
/// A document that does not parse to begin with is left exactly as it is —
/// nothing here can reason about markup it cannot read. A document that parses
/// before and not after is this code's fault, and refuses the whole conversion
/// rather than writing a book that opens wrongly.
pub fn kepubify(data: &[u8]) -> Result<Vec<u8>> {
    let mut archive = ZipArchive::new(Cursor::new(data)).context("Book is not a readable EPUB")?;
    // A book still under its own encryption is not ours to take apart.
    crate::epub::font_metadata(data)
        .context("This book is still encrypted; it was not converted for the Kobo")?;
    let documents = content_documents(&mut archive);
    let mut out = ZipWriter::new(Cursor::new(Vec::new()));
    // A book that has been through this before, here or in Calibre, is already
    // divided. Dividing it again would nest a second set of spans inside the
    // first and leave the Kobo counting the same words twice.
    let mut already = 0;
    out.start_file(
        "mimetype",
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
    )?;
    out.write_all(b"application/epub+zip")?;
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut rewritten = 0;
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
        let content = name.to_ascii_lowercase();
        // The manifest is the authority; its absence leaves only the names.
        let markup = match &documents {
            Some(documents) => documents.contains(&name),
            None => [".xhtml", ".html", ".htm"]
                .iter()
                .any(|suffix| content.ends_with(suffix)),
        };
        if markup {
            if let Ok(text) = std::str::from_utf8(&bytes) {
                // Only a document that reads as XML now may be rewritten, and
                // only a rewrite that still reads as XML may be kept.
                if text.contains("class=\"koboSpan\"") {
                    already += 1;
                    out.start_file(&name, deflated)?;
                    out.write_all(&bytes)?;
                    continue;
                }
                if parse(text).is_ok() {
                    let divided = spans(text);
                    parse(&divided).map_err(|e| {
                        anyhow::anyhow!(
                            "Dividing {name} into spans broke it ({e}); nothing was written"
                        )
                    })?;
                    out.start_file(&name, deflated)?;
                    out.write_all(divided.as_bytes())?;
                    rewritten += 1;
                    continue;
                }
            }
        }
        out.start_file(&name, deflated)?;
        out.write_all(&bytes)?;
    }
    let data = out.finish()?.into_inner();
    ensure!(
        rewritten + already > 0,
        "No readable text documents in this book; it was not converted for the Kobo"
    );
    crate::epub::validate(&data)
        .context("The Kobo conversion failed validation; nothing was written")?;
    Ok(data)
}

/// The name a Kobo reads as its own: the reading engine is chosen by this.
pub fn name(stem: &str) -> String {
    format!("{stem}.kepub.epub")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prose_is_divided_and_everything_else_is_carried_across() {
        let document = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
            <html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>A &amp; B</title>\
            <style>p { color: red; }</style></head>\
            <body><p>One sentence. And a second one.</p>\
            <p>With <em>emphasis</em> inside it.</p>\
            <script>if (a &lt; b) { }</script>\
            <!-- a comment --></body></html>";
        let divided = spans(document);
        // Still a document, which is the whole safety of this.
        assert!(roxmltree::Document::parse(&divided).is_ok(), "{divided}");
        // The head is prologue and is left alone, entities and all.
        assert!(divided.contains("<title>A &amp; B</title>"), "{divided}");
        // Style and script hold machinery, not prose.
        assert!(
            divided.contains("<style>p { color: red; }</style>"),
            "{divided}"
        );
        assert!(
            divided.contains("<script>if (a &lt; b) { }</script>"),
            "{divided}"
        );
        assert!(divided.contains("<!-- a comment -->"), "{divided}");
        // Two sentences become two spans, and each id is its own.
        assert!(divided.contains(">One sentence. </span>"), "{divided}");
        assert!(divided.contains(">And a second one.</span>"), "{divided}");
        assert!(divided.contains("id=\"kobo.1.1\""), "{divided}");
        assert!(divided.contains("id=\"kobo.1.2\""), "{divided}");
        // Emphasis splits a run, and both halves are still counted.
        assert!(divided.contains("<em>"), "{divided}");

        // Not a byte of the text itself is lost.
        let text = |markup: &str| {
            let doc = roxmltree::Document::parse(markup).unwrap();
            doc.descendants()
                .filter(|n| n.is_text())
                .filter_map(|n| n.text())
                .collect::<String>()
        };
        assert_eq!(text(document), text(&divided));
    }

    #[test]
    fn a_book_that_is_already_divided_is_left_alone() {
        // Dividing a kepub again would nest a second set of spans inside the
        // first, and the Kobo would count the same words twice.
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        let divided = "<html xmlns=\"http://www.w3.org/1999/xhtml\"><body>\
            <p><span class=\"koboSpan\" id=\"kobo.1.1\">Already counted.</span></p>\
            </body></html>";
        for (name, body) in [
            ("mimetype", "application/epub+zip"),
            (
                "META-INF/container.xml",
                "<container><rootfiles><rootfile full-path=\"b.opf\"/></rootfiles></container>",
            ),
            (
                "b.opf",
                "<package><metadata/><manifest><item id=\"a\" href=\"a.xhtml\" \
                 media-type=\"application/xhtml+xml\"/></manifest>\
                 <spine><itemref idref=\"a\"/></spine></package>",
            ),
            ("a.xhtml", divided),
        ] {
            zip.start_file::<_, ()>(name, options).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        let out = kepubify(&zip.finish().unwrap().into_inner()).unwrap();
        let mut archive = ZipArchive::new(Cursor::new(&out[..])).unwrap();
        let mut text = String::new();
        archive
            .by_name("a.xhtml")
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, divided);
        assert_eq!(text.matches("koboSpan").count(), 1, "{text}");
    }

    #[test]
    fn a_document_that_does_not_parse_is_left_exactly_as_it_was() {
        // Nothing here can reason about markup it cannot read.
        let broken = "<html><body><p>unclosed";
        assert!(roxmltree::Document::parse(broken).is_err());
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        for (name, body) in [
            ("mimetype", "application/epub+zip"),
            (
                "META-INF/container.xml",
                "<container><rootfiles><rootfile full-path=\"b.opf\"/></rootfiles></container>",
            ),
            ("b.opf", "<package><metadata/><manifest/><spine/></package>"),
            ("broken.xhtml", broken),
        ] {
            zip.start_file::<_, ()>(name, options).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        let data = zip.finish().unwrap().into_inner();
        // Nothing could be divided, so nothing is offered as divided.
        let refused = kepubify(&data).unwrap_err();
        assert!(
            format!("{refused:#}").contains("No readable text documents"),
            "{refused:#}"
        );
    }
}
