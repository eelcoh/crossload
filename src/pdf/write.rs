//! Writing rebuilt prose out as an EPUB.
//!
//! The book is one document with an anchor at every heading, rather than a file
//! per chapter: a PDF's headings are a guess, and a guess that splits a book
//! into files is harder to read past than one that only fills a table of
//! contents.
use super::layout::Block;
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::io::Write;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            // A control character is not text; a tab and a newline are spaces
            // in a paragraph anyway.
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// The book's title: what the PDF calls itself, or its first heading.
pub(super) fn title(named: Option<String>, blocks: &[Block]) -> String {
    named
        .map(|title| title.trim().to_owned())
        .filter(|title| !title.is_empty())
        .or_else(|| {
            blocks.iter().find_map(|block| match block {
                Block::Heading(text) if !text.trim().is_empty() => Some(text.trim().to_owned()),
                _ => None,
            })
        })
        .unwrap_or_else(|| "Untitled".to_owned())
}

pub(super) fn epub(title: &str, blocks: &[Block]) -> Result<Vec<u8>> {
    let mut body = String::new();
    let mut contents = String::new();
    let mut headings = 0;
    for block in blocks {
        match block {
            Block::Heading(text) => {
                headings += 1;
                let id = format!("h{headings}");
                body.push_str(&format!("<h2 id=\"{id}\">{}</h2>\n", escaped(text.trim())));
                contents.push_str(&format!(
                    "<li><a href=\"book.xhtml#{id}\">{}</a></li>\n",
                    escaped(text.trim())
                ));
            }
            Block::Paragraph(text) if !text.trim().is_empty() => {
                body.push_str(&format!("<p>{}</p>\n", escaped(text.trim())));
            }
            Block::Paragraph(_) => {}
        }
    }
    if contents.is_empty() {
        contents.push_str("<li><a href=\"book.xhtml\">Text</a></li>\n");
    }
    // A stable identifier: the same PDF converted twice is the same book, and
    // nothing here should depend on the clock.
    let identifier = format!("{:x}", Sha256::digest(body.as_bytes()));
    let title = escaped(title);
    let page = |inner: &str| {
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
             <html xmlns=\"http://www.w3.org/1999/xhtml\" \
             xmlns:epub=\"http://www.idpf.org/2007/ops\" lang=\"en\" xml:lang=\"en\">\n\
             <head><title>{title}</title><meta charset=\"utf-8\"/></head>\n\
             <body>\n{inner}</body>\n</html>\n"
        )
    };
    let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    // The mimetype must be first and stored, or a reader may not recognize it.
    zip.start_file(
        "mimetype",
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
    )?;
    zip.write_all(b"application/epub+zip")?;
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut file = |name: &str, content: &str| -> Result<()> {
        zip.start_file(name, deflated)?;
        zip.write_all(content.as_bytes())?;
        Ok(())
    };
    file(
        "META-INF/container.xml",
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\n\
         <rootfiles><rootfile full-path=\"OEBPS/content.opf\" \
         media-type=\"application/oebps-package+xml\"/></rootfiles>\n</container>\n",
    )?;
    file(
        "OEBPS/content.opf",
        &format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
             <package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" \
             unique-identifier=\"id\">\n\
             <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n\
             <dc:identifier id=\"id\">urn:sha256:{identifier}</dc:identifier>\n\
             <dc:title>{title}</dc:title>\n<dc:language>en</dc:language>\n\
             <meta property=\"dcterms:modified\">1970-01-01T00:00:00Z</meta>\n\
             <meta name=\"crossload:source\" content=\"converted from PDF\"/>\n\
             </metadata>\n<manifest>\n\
             <item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" \
             properties=\"nav\"/>\n\
             <item id=\"book\" href=\"book.xhtml\" media-type=\"application/xhtml+xml\"/>\n\
             </manifest>\n<spine><itemref idref=\"book\"/></spine>\n</package>\n"
        ),
    )?;
    file(
        "OEBPS/nav.xhtml",
        &page(&format!(
            "<nav epub:type=\"toc\" id=\"toc\"><h1>Contents</h1>\n<ol>\n{contents}</ol>\n</nav>\n"
        )),
    )?;
    file("OEBPS/book.xhtml", &page(&body))?;
    Ok(zip.finish()?.into_inner())
}
