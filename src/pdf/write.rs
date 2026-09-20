//! Writing rebuilt prose out as an EPUB.
//!
//! A heading starts a new file, so a reader can turn to a chapter and load only
//! that chapter. Because a PDF's headings are a guess, headings that arrive in
//! a run — a title page, a heading with its subtitle — stay together rather
//! than becoming a file each, and text before the first heading opens the book
//! in a file of its own.
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

/// One file of the book: what it is called in the table of contents, and the
/// markup inside it.
struct Chapter {
    title: String,
    body: String,
}

/// Cut the blocks into chapters at their headings.
fn chapters(blocks: &[Block]) -> Vec<Chapter> {
    let mut chapters: Vec<Chapter> = vec![];
    // Whether the chapter being filled has any prose yet. A heading that
    // follows another heading belongs with it, not to a file of its own.
    let mut prose = false;
    for block in blocks {
        match block {
            Block::Heading(text) if !text.trim().is_empty() => {
                let text = text.trim();
                if prose || chapters.is_empty() {
                    chapters.push(Chapter {
                        title: text.to_owned(),
                        body: String::new(),
                    });
                    prose = false;
                }
                let chapter = chapters.last_mut().expect("just pushed or non-empty");
                chapter
                    .body
                    .push_str(&format!("<h2>{}</h2>\n", escaped(text)));
            }
            Block::Paragraph(text) if !text.trim().is_empty() => {
                let chapter = match chapters.last_mut() {
                    Some(chapter) => chapter,
                    None => {
                        // Text before any heading still has to live somewhere.
                        chapters.push(Chapter {
                            title: "Beginning".to_owned(),
                            body: String::new(),
                        });
                        chapters.last_mut().expect("just pushed")
                    }
                };
                chapter
                    .body
                    .push_str(&format!("<p>{}</p>\n", escaped(text.trim())));
                prose = true;
            }
            _ => {}
        }
    }
    chapters
}

pub(super) fn epub(title: &str, author: &str, blocks: &[Block]) -> Result<Vec<u8>> {
    let chapters = chapters(blocks);
    let name = |index: usize| format!("chapter{}.xhtml", index + 1);
    let mut contents = String::new();
    let mut manifest = String::new();
    let mut spine = String::new();
    let mut body = String::new();
    for (index, chapter) in chapters.iter().enumerate() {
        contents.push_str(&format!(
            "<li><a href=\"{}\">{}</a></li>\n",
            name(index),
            escaped(&chapter.title)
        ));
        manifest.push_str(&format!(
            "<item id=\"c{}\" href=\"{}\" media-type=\"application/xhtml+xml\"/>\n",
            index + 1,
            name(index)
        ));
        spine.push_str(&format!("<itemref idref=\"c{}\"/>\n", index + 1));
        body.push_str(&chapter.body);
    }
    // A stable identifier: the same PDF converted twice is the same book, and
    // nothing here should depend on the clock.
    let identifier = format!("{:x}", Sha256::digest(body.as_bytes()));
    let title = escaped(title);
    // A PDF that names nobody gets no creator at all, rather than one invented
    // for it: the book is then filed by its title, which is honest.
    let creator = match author.trim() {
        "" => String::new(),
        author => format!("<dc:creator>{}</dc:creator>\n", escaped(author)),
    };
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
             <dc:title>{title}</dc:title>\n{creator}<dc:language>en</dc:language>\n\
             <meta property=\"dcterms:modified\">1970-01-01T00:00:00Z</meta>\n\
             <meta name=\"crossload:source\" content=\"converted from PDF\"/>\n\
             </metadata>\n<manifest>\n\
             <item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" \
             properties=\"nav\"/>\n\
             {manifest}</manifest>\n<spine>\n{spine}</spine>\n</package>\n"
        ),
    )?;
    file(
        "OEBPS/nav.xhtml",
        &page(&format!(
            "<nav epub:type=\"toc\" id=\"toc\"><h1>Contents</h1>\n<ol>\n{contents}</ol>\n</nav>\n"
        )),
    )?;
    for (index, chapter) in chapters.iter().enumerate() {
        file(&format!("OEBPS/{}", name(index)), &page(&chapter.body))?;
    }
    Ok(zip.finish()?.into_inner())
}
